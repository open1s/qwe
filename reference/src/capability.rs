//! Bounded capability token verification per RFC-0033.
//!
//! A token binds issuer, subject, world, region, component selectors, access
//! mask, expiry (world version and wall-clock time), and nonce; keyed-digest
//! signature verification precedes any use of the claims. Narrowing is
//! expressible by minting a token with a subset of access; amplification or
//! delegation is impossible because the signature covers every claim field.

use pwe_api::{
    Access, Capability, CapabilityClaims, CapabilityVerifier, ComponentTypeId, Error, Hash256,
    RegionId, Result, Status, TransactionOptions, WorldId, WorldVersion,
};

use crate::sha256;
use crate::wire::{Reader, Writer};

/// Canonical token format version.
pub const CAPABILITY_TOKEN_MAJOR: u16 = 1;
/// Hard bound on a single token's wire size.
pub const MAX_CAPABILITY_TOKEN_BYTES: usize = 4096;
/// Hard bound on the number of component selectors a token may carry.
pub const MAX_CAPABILITY_SELECTORS: u32 = 64;
/// No wall-clock expiry.
pub const NO_TIME_EXPIRY: u64 = 0;

const SIGNATURE_LEN: usize = 32;

fn error(status: Status, detail: u32, offset: usize) -> Error {
    Error {
        status,
        detail,
        byte_offset: offset as u64,
    }
}

/// Claims plus the scope fields that the frozen `pwe_api::CapabilityClaims`
/// cannot carry. Verification is performed over this full set; only the
/// API-visible projection crosses the stable boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenClaims {
    pub claims: CapabilityClaims,
    /// Component/resource selectors this token is scoped to.
    pub selectors: Vec<ComponentTypeId>,
    /// Wall-clock expiry in nanoseconds since epoch; `NO_TIME_EXPIRY` if none.
    pub expires_at_time_ns: u64,
}

/// Canonical little-endian body every token signature covers.
fn encode_body(claims: &TokenClaims) -> Result<Vec<u8>> {
    let mut out = Writer::new();
    out.u16(CAPABILITY_TOKEN_MAJOR)?;
    out.bytes_raw(&claims.claims.issuer.0)?;
    out.bytes_raw(&claims.claims.subject.0)?;
    out.bytes_raw(&claims.claims.world.0.to_le_bytes())?;
    out.u64(claims.claims.region.0)?;
    out.u32(access_mask(claims.claims.access))?;
    let count = u32::try_from(claims.selectors.len()).map_err(|_| error(Status::Limit, 0, 0))?;
    if count > MAX_CAPABILITY_SELECTORS {
        return Err(error(Status::Limit, 1, 0));
    }
    out.u32(count)?;
    for selector in &claims.selectors {
        out.bytes_raw(&selector.0)?;
    }
    out.u64(claims.claims.expires_at_version.0)?;
    out.u64(claims.expires_at_time_ns)?;
    out.bytes_raw(&claims.claims.nonce)?;
    Ok(out.finish())
}

fn access_mask(access: Access) -> u32 {
    let mut mask = 0;
    for (bit, kind) in [
        (1, Access::READ),
        (2, Access::WRITE),
        (4, Access::CREATE),
        (8, Access::DESTROY),
        (16, Access::EMIT),
        (32, Access::IO),
    ] {
        if access.contains(kind) {
            mask |= bit;
        }
    }
    mask
}

fn access_from_mask(mask: u32) -> Result<Access> {
    if mask & !63 != 0 {
        // Unknown access bits are never silently dropped: reject outright.
        return Err(error(Status::Capability, 2, 0));
    }
    let mut access: Option<Access> = None;
    for (bit, kind) in [
        (1, Access::READ),
        (2, Access::WRITE),
        (4, Access::CREATE),
        (8, Access::DESTROY),
        (16, Access::EMIT),
        (32, Access::IO),
    ] {
        if mask & bit != 0 {
            access = Some(match access {
                None => kind,
                Some(existing) => existing.union(kind),
            });
        }
    }
    access.ok_or(error(Status::Capability, 3, 0))
}

/// Whether the mask grants at least one known access kind.
fn grants_any(access: Access) -> bool {
    access.contains(Access::READ)
        || access.contains(Access::WRITE)
        || access.contains(Access::CREATE)
        || access.contains(Access::DESTROY)
        || access.contains(Access::EMIT)
        || access.contains(Access::IO)
}

/// Reference token signature: SHA-256 over `key || canonical body`.
///
/// RFC deviation: the RFC requires signature verification but does not pin an
/// algorithm; the reference uses a keyed digest so production asymmetric
/// schemes can replace it without changing semantics.
fn signature_for(key: &Hash256, body: &[u8]) -> Hash256 {
    let mut message = Vec::with_capacity(32 + body.len());
    message.extend_from_slice(&key.0);
    message.extend_from_slice(body);
    sha256::digest(&message)
}

/// Reference verifier: parses bounded tokens and verifies signatures before
/// any claim is used.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceCapabilityVerifier {
    /// Key bound to the trusted issuer; only tokens signed with it verify.
    pub issuer_key: Hash256,
}

impl ReferenceCapabilityVerifier {
    /// Mints a signed token. Exists so conformance tests and issuers can
    /// produce canonical tokens; it is still bounded.
    pub fn mint(&self, claims: &TokenClaims) -> Result<Capability<'static>> {
        if !grants_any(claims.claims.access) {
            return Err(error(Status::Capability, 4, 0));
        }
        let mut body = encode_body(claims)?;
        let sig = signature_for(&self.issuer_key, &body);
        body.extend_from_slice(&sig.0);
        if body.len() > MAX_CAPABILITY_TOKEN_BYTES {
            return Err(error(Status::Limit, 2, body.len() - SIGNATURE_LEN));
        }
        Ok(Capability {
            bytes: Vec::leak(body),
        })
    }

    /// Parses and authenticates a token without evaluating time expiry.
    pub fn verify_token(&self, token: Capability<'_>) -> Result<TokenClaims> {
        if token.bytes.len() > MAX_CAPABILITY_TOKEN_BYTES {
            return Err(error(Status::Limit, 3, MAX_CAPABILITY_TOKEN_BYTES));
        }
        let mut input = Reader::new(token.bytes)?;
        if input.u16()? != CAPABILITY_TOKEN_MAJOR {
            return Err(error(Status::Capability, 5, 0));
        }
        let issuer = Hash256(input.fixed(32)?.try_into().expect("32-byte slice"));
        let subject = Hash256(input.fixed(32)?.try_into().expect("32-byte slice"));
        let mut world = [0u8; 16];
        world.copy_from_slice(input.fixed(16)?);
        let region = RegionId(input.u64()?);
        let access = access_from_mask(input.u32()?)?;
        let selector_count = input.u32()?;
        if selector_count > MAX_CAPABILITY_SELECTORS {
            return Err(error(Status::Limit, 4, input.offset()));
        }
        let mut selectors = Vec::new();
        selectors
            .try_reserve_exact(selector_count as usize)
            .map_err(|_| error(Status::Limit, 5, input.offset()))?;
        for _ in 0..selector_count {
            let mut id = [0u8; 16];
            id.copy_from_slice(input.fixed(16)?);
            selectors.push(ComponentTypeId(id));
        }
        let expires_at_version = WorldVersion(input.u64()?);
        let expires_at_time_ns = input.u64()?;
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(input.fixed(16)?);
        let body_end = input.offset();
        let signature = input.fixed(SIGNATURE_LEN)?;
        input.finish()?;
        let expected = signature_for(&self.issuer_key, &token.bytes[..body_end]);
        if signature != expected.0 {
            return Err(error(Status::Capability, 6, body_end));
        }
        Ok(TokenClaims {
            claims: CapabilityClaims {
                issuer,
                subject,
                world: WorldId(u128::from_le_bytes(world)),
                region,
                access,
                expires_at_version,
                nonce,
            },
            selectors,
            expires_at_time_ns,
        })
    }

    /// Verifies a token and additionally rejects wall-clock expiry.
    pub fn verify_at(&self, token: Capability<'_>, now_ns: u64) -> Result<TokenClaims> {
        let claims = self.verify_token(token)?;
        if claims.expires_at_time_ns != NO_TIME_EXPIRY && claims.expires_at_time_ns < now_ns {
            return Err(error(Status::Capability, 7, 0));
        }
        Ok(claims)
    }
}

impl CapabilityVerifier for ReferenceCapabilityVerifier {
    fn verify(&self, token: Capability<'_>) -> Result<CapabilityClaims> {
        Ok(self.verify_token(token)?.claims)
    }
}

/// Gates transaction creation: world, region, version expiry, and non-empty
/// access are all rejected here, before any transaction or mutation exists.
pub(crate) fn enforce_claims(
    capability: &CapabilityClaims,
    options: &TransactionOptions,
    world: WorldId,
    version: WorldVersion,
) -> Result<()> {
    if capability.world != world {
        return Err(error(Status::Capability, 8, 0));
    }
    if capability.region != options.ownership.region {
        return Err(error(Status::Capability, 9, 0));
    }
    if capability.expires_at_version < version {
        return Err(error(Status::Capability, 10, 0));
    }
    if !grants_any(capability.access) {
        return Err(error(Status::Capability, 11, 0));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReferenceWorld;
    use pwe_api::{ConflictPolicy, EntityId, EntityRef, Ownership, OwnershipEpoch};

    const KEY: Hash256 = Hash256([7; 32]);

    fn claims(access: Access) -> TokenClaims {
        TokenClaims {
            claims: CapabilityClaims {
                issuer: Hash256([1; 32]),
                subject: Hash256([2; 32]),
                world: WorldId(1),
                region: RegionId(7),
                access,
                expires_at_version: WorldVersion(u64::MAX),
                nonce: [0; 16],
            },
            selectors: vec![ComponentTypeId([9; 16])],
            expires_at_time_ns: NO_TIME_EXPIRY,
        }
    }

    fn options(version: u64) -> TransactionOptions {
        TransactionOptions {
            base_version: WorldVersion(version),
            ownership: Ownership {
                region: RegionId(7),
                epoch: OwnershipEpoch(1),
            },
            policy: ConflictPolicy::Reject,
        }
    }

    fn entity(n: u128) -> EntityRef {
        EntityRef {
            id: EntityId(n),
            generation: 1,
        }
    }

    fn verifier() -> ReferenceCapabilityVerifier {
        ReferenceCapabilityVerifier { issuer_key: KEY }
    }

    #[test]
    fn capability_token_round_trips_and_verifies() {
        let verifier = verifier();
        let want = claims(
            Access::READ
                .union(Access::WRITE)
                .union(Access::EMIT)
                .union(Access::IO),
        );
        let token = verifier.mint(&want).unwrap();
        assert_eq!(verifier.verify_token(token).unwrap(), want);
        // Re-encoding is byte-identical and selectors survive intact.
        let again = verifier
            .mint(&verifier.verify_token(token).unwrap())
            .unwrap();
        assert_eq!(again.bytes, token.bytes);
    }

    #[test]
    fn capability_token_rejects_tampering_and_wrong_key() {
        let verifier = verifier();
        let token = verifier.mint(&claims(Access::READ)).unwrap();
        // Wrong key: signature mismatch.
        let other = ReferenceCapabilityVerifier {
            issuer_key: Hash256([8; 32]),
        };
        assert_eq!(
            other.verify_token(token).unwrap_err().status,
            Status::Capability
        );
        // Flipping any body byte invalidates the signature.
        let mut forged = token.bytes.to_vec();
        forged[40] ^= 1;
        assert_eq!(
            verifier
                .verify_token(Capability { bytes: &forged })
                .unwrap_err()
                .status,
            Status::Capability
        );
        // Truncation and trailing bytes also fail closed.
        assert!(verifier
            .verify_token(Capability {
                bytes: &token.bytes[..token.bytes.len() - 1]
            })
            .is_err());
        let mut padded = token.bytes.to_vec();
        padded.push(0);
        assert!(verifier
            .verify_token(Capability { bytes: &padded })
            .is_err());
    }

    #[test]
    fn capability_token_enforces_bounds_and_access() {
        let verifier = verifier();
        // Oversized token rejected before parsing.
        let huge = vec![0u8; MAX_CAPABILITY_TOKEN_BYTES + 1];
        assert_eq!(
            verifier
                .verify_token(Capability { bytes: &huge })
                .unwrap_err()
                .status,
            Status::Limit
        );
        // Too many selectors rejected without unbounded allocation.
        let mut many = claims(Access::READ);
        many.selectors = vec![ComponentTypeId([1; 16]); MAX_CAPABILITY_SELECTORS as usize + 1];
        assert_eq!(verifier.mint(&many).unwrap_err().status, Status::Limit);
        // Unknown access bits in the mask are rejected, not silently dropped.
        let token = verifier.mint(&claims(Access::READ)).unwrap();
        let mut unknown = token.bytes.to_vec();
        // Access mask sits after 2 + 32 + 32 + 16 + 8 bytes.
        unknown[90] = 0b0100_0000;
        assert_eq!(
            verifier
                .verify_token(Capability { bytes: &unknown })
                .unwrap_err()
                .status,
            Status::Capability
        );
        // Time expiry is enforced at verify time.
        let mut expired = claims(Access::READ);
        expired.expires_at_time_ns = 50;
        let token = verifier.mint(&expired).unwrap();
        assert_eq!(
            verifier.verify_at(token, 51).unwrap_err().status,
            Status::Capability
        );
        assert!(verifier.verify_at(token, 50).is_ok());
    }

    #[test]
    fn capability_failures_reject_before_transaction_creation() {
        let mut world = ReferenceWorld::new(WorldId(1));
        let access = Access::CREATE.union(Access::WRITE);
        // Region and world mismatches fail with PWE_E_CAPABILITY before any
        // transaction exists; state is untouched.
        for capability in [
            CapabilityClaims {
                region: RegionId(8),
                ..claims(access).claims
            },
            CapabilityClaims {
                world: WorldId(2),
                ..claims(access).claims
            },
        ] {
            let before = world.version();
            assert_eq!(
                world.begin(options(0), capability).unwrap_err().status,
                Status::Capability
            );
            assert_eq!(world.version(), before);
        }
        // Valid claims still create a transaction; the capability gate ran
        // first. A stale base version is a commit-time conflict (RFC-0023
        // VALIDATED), not a capability error at begin.
        let mut tx = world.begin(options(0), claims(access).claims).unwrap();
        tx.create(entity(1)).unwrap();
        tx.commit().unwrap();
        assert_eq!(world.version(), WorldVersion(1));
        let mut stale = world.begin(options(0), claims(access).claims).unwrap();
        stale.create(entity(2)).unwrap();
        assert_eq!(stale.commit().unwrap_err().status, Status::Conflict);
        // A token whose version expiry has passed is rejected after the
        // commit advanced the world to version 1, before transaction creation.
        let verifier = verifier();
        let mut soon_expired = claims(access);
        soon_expired.claims.expires_at_version = WorldVersion(0);
        let token = verifier.mint(&soon_expired).unwrap();
        assert_eq!(
            world
                .begin(options(1), verifier.verify(token).unwrap())
                .unwrap_err()
                .status,
            Status::Capability
        );
    }
}
