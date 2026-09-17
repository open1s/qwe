//! First compilable public Rust contract for PWE v0.2.
//! Implementations live behind these traits; this crate owns no World storage.

#![no_std]

use core::fmt;

pub mod audit;
pub mod compat;
pub mod detail;
pub mod ffi;
pub mod limits;
pub mod resource;
pub mod spatial;
pub mod time;

pub const RUNTIME_ABI_MAJOR: u16 = 2;
pub const COMPONENT_ABI_MAJOR: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Hash256(pub [u8; 32]);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct EntityId(pub u128);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct WorldId(pub u128);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct ComponentTypeId(pub [u8; 16]);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct RegionId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct WorldVersion(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct OwnershipEpoch(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct TransferId(pub u128);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EntityRef {
    pub id: EntityId,
    pub generation: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Ownership {
    pub region: RegionId,
    pub epoch: OwnershipEpoch,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferProposal {
    pub id: TransferId,
    pub entity: EntityRef,
    pub source: Ownership,
    pub target: RegionId,
    pub base_version: WorldVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Status {
    Ok = 0,
    Invalid = 1,
    Limit = 2,
    SchemaHash = 3,
    SchemaUnsupported = 4,
    EirInvalid = 5,
    Capability = 6,
    HandleStale = 7,
    Conflict = 8,
    OwnershipStale = 9,
    AbiMismatch = 10,
    HashCollision = 11,
    Internal = 255,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub status: Status,
    pub detail: u32,
    pub byte_offset: u64,
}
pub type Result<T> = core::result::Result<T, Error>;
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PWE {:?} ({}) at {}",
            self.status, self.detail, self.byte_offset
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Access(u32);
impl Access {
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const CREATE: Self = Self(4);
    pub const DESTROY: Self = Self(8);
    pub const EMIT: Self = Self(16);
    pub const IO: Self = Self(32);
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability<'a> {
    pub bytes: &'a [u8],
}
/// Verified claims. Only a `CapabilityVerifier` may turn untrusted token bytes
/// into this value at an external boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityClaims {
    pub issuer: Hash256,
    pub subject: Hash256,
    pub world: WorldId,
    pub region: RegionId,
    pub access: Access,
    pub expires_at_version: WorldVersion,
    pub nonce: [u8; 16],
}
pub trait CapabilityVerifier {
    fn verify(&self, token: Capability<'_>) -> Result<CapabilityClaims>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentDescriptor {
    pub type_id: ComponentTypeId,
    pub schema_hash: Hash256,
    pub abi_major: u32,
    pub flags: u32,
    pub value_size: u32,
    pub value_align: u32,
}
impl ComponentDescriptor {
    /// RFC-0019 defined component field flags: REQUIRED|TRANSIENT|REPLICATED|READ_ONLY.
    pub const FLAG_MASK: u32 = 0x0f;
    pub const fn validate(self) -> Result<Self> {
        if self.abi_major != COMPONENT_ABI_MAJOR
            || self.value_align == 0
            || self.value_align > 64
            || !self.value_align.is_power_of_two()
            || (self.flags & !Self::FLAG_MASK) != 0
        {
            Err(Error {
                status: Status::AbiMismatch,
                detail: 0,
                byte_offset: 0,
            })
        } else {
            Ok(self)
        }
    }

    /// RFC-0031: validates the descriptor against its schema. Runs the base
    /// checks, then enforces that a fixed-size descriptor's `value_size` equals
    /// the schema size, and that the descriptor's `schema_hash` matches the
    /// schema it claims. Callers with schema context pass `schema_size` (the
    /// schema's fixed byte size) and `schema_hash`; for variable-size schemas
    /// pass `None` for `schema_size` to skip the size equality check.
    pub const fn validate_against(
        self,
        schema_size: Option<u32>,
        schema_hash: Hash256,
    ) -> Result<Self> {
        if self.abi_major != COMPONENT_ABI_MAJOR
            || self.value_align == 0
            || self.value_align > 64
            || !self.value_align.is_power_of_two()
            || (self.flags & !Self::FLAG_MASK) != 0
        {
            return Err(Error {
                status: Status::AbiMismatch,
                detail: 0,
                byte_offset: 0,
            });
        }
        if let Some(size) = schema_size {
            if self.value_size != size {
                return Err(Error {
                    status: Status::AbiMismatch,
                    detail: 1,
                    byte_offset: 0,
                });
            }
        }
        let mut i = 0;
        while i < 32 {
            if self.schema_hash.0[i] != schema_hash.0[i] {
                return Err(Error {
                    status: Status::AbiMismatch,
                    detail: 2,
                    byte_offset: 0,
                });
            }
            i += 1;
        }
        Ok(self)
    }
}
pub trait Component: Sized + 'static {
    const DESCRIPTOR: ComponentDescriptor;
    fn encode(&self, out: &mut [u8]) -> Result<usize>;
    fn decode(input: &[u8]) -> Result<Self>;
}

/// A scoped read view into a component value (RFC-0033). A view carries its
/// `handle` + `generation` (identity), the `access` granted, and the immutable
/// `world_version` it was taken against. It is valid only within its
/// transaction/schedule scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadView<'a> {
    pub handle: u32,
    pub generation: u32,
    pub access: Access,
    pub component: ComponentDescriptor,
    pub world_version: WorldVersion,
    pub bytes: &'a [u8],
}
/// A scoped write view into a component value (RFC-0033). Carries the same
/// identity/access/version tuple as [`ReadView`]; valid only within its
/// transaction/schedule scope.
#[derive(Debug)]
pub struct WriteView<'a> {
    pub handle: u32,
    pub generation: u32,
    pub access: Access,
    pub component: ComponentDescriptor,
    pub world_version: WorldVersion,
    pub bytes: &'a mut [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictPolicy {
    Reject,
    Merge,
    LastWriterByPriority,
    CommutativeMerge,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransactionOptions {
    pub base_version: WorldVersion,
    pub ownership: Ownership,
    pub policy: ConflictPolicy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Commit {
    pub world_version: WorldVersion,
}

pub trait Transaction {
    fn read<'a>(
        &'a self,
        entity: EntityRef,
        component: ComponentDescriptor,
    ) -> Result<ReadView<'a>>;
    fn write<'a>(
        &'a mut self,
        entity: EntityRef,
        component: ComponentDescriptor,
    ) -> Result<WriteView<'a>>;
    fn create(&mut self, entity: EntityRef) -> Result<()>;
    fn destroy(&mut self, entity: EntityRef) -> Result<()>;
    fn commit(self) -> Result<Commit>
    where
        Self: Sized;
    fn abort(self)
    where
        Self: Sized;
}
pub trait Runtime {
    type Txn: Transaction;
    fn abi_major(&self) -> u16 {
        RUNTIME_ABI_MAJOR
    }
    fn world_id(&self) -> WorldId;
    fn world_version(&self) -> WorldVersion;
    fn begin(
        &mut self,
        options: TransactionOptions,
        capability: Capability<'_>,
    ) -> Result<Self::Txn>;
    fn ownership(&self, entity: EntityRef) -> Result<Ownership>;
}

#[cfg(test)]
mod tests {
    use super::*;
    const D: ComponentDescriptor = ComponentDescriptor {
        type_id: ComponentTypeId([0; 16]),
        schema_hash: Hash256([0; 32]),
        abi_major: 2,
        flags: 0,
        value_size: 4,
        value_align: 4,
    };
    #[test]
    fn descriptor_contract() {
        assert!(D.validate().is_ok());
        assert!(ComponentDescriptor {
            value_align: 3,
            ..D
        }
        .validate()
        .is_err());
    }
    #[test]
    fn descriptor_rejects_unknown_required_flag_bits() {
        assert!(ComponentDescriptor { flags: 0x08, ..D }.validate().is_ok());
        // RFC-0031: an unknown required flag bit fails closed (mask is 0x0f).
        assert_eq!(
            ComponentDescriptor { flags: 0x10, ..D }
                .validate()
                .unwrap_err()
                .status,
            Status::AbiMismatch
        );
    }
}
