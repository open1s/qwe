//! RFC-0035 artifact identity and RFC-0027 interpreter-backed compiled cache.
//!
//! A compiled artifact has an identity equal to the canonical SHA-256 over a
//! bounded tuple of non-negotiable members. The loader fails closed on any
//! mismatch (ABI_MISMATCH, never reinterpretation). The cache is keyed by full
//! identity bytes; publication and invalidation are atomic. Until a real JIT
//! backend exists, an artifact delegates execution semantics to the EIR
//! interpreter (RFC-0004: interpreter is the semantic oracle).

use crate::eir::{EirModule, WorldWrite};
use crate::sha256::digest;
use crate::wire::Writer;
use pwe_api::{Error, Hash256, Result, Status, WorldId, WorldVersion};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// Canonical identity of an executable artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactIdentity {
    pub kind: u32,
    pub format_major: u16,
    pub format_minor: u16,
    pub eir_hash: Hash256,
    pub domain_ir_hash: Hash256,
    pub schema_set_hash: Hash256,
    pub runtime_abi_major: u16,
    pub component_abi_major: u32,
    pub target_kind: u16,
    pub features: Vec<u32>,
    pub capability_manifest: Hash256,
}

impl ArtifactIdentity {
    /// Canonical identity bytes (RFC-0035 tuple, bounded, deterministic).
    pub fn bytes(&self) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.u32(self.kind)?;
        out.u16(self.format_major)?;
        out.u16(self.format_minor)?;
        out.bytes_raw(&self.eir_hash.0)?;
        out.bytes_raw(&self.domain_ir_hash.0)?;
        out.bytes_raw(&self.schema_set_hash.0)?;
        out.u16(self.runtime_abi_major)?;
        out.u32(self.component_abi_major)?;
        out.u16(self.target_kind)?;
        let mut features = self.features.clone();
        features.sort_unstable();
        out.u32(features.len() as u32)?;
        for feature in features {
            out.u32(feature)?;
        }
        out.bytes_raw(&self.capability_manifest.0)?;
        Ok(out.finish())
    }

    pub fn hash(&self) -> Result<Hash256> {
        Ok(digest(&self.bytes()?))
    }

    /// Every non-negotiable member must be equal; any mismatch is ABI_MISMATCH.
    pub fn requires(&self, other: &Self) -> Result<()> {
        let mut self_features = self.features.clone();
        self_features.sort_unstable();
        let mut other_features = other.features.clone();
        other_features.sort_unstable();
        if self.kind != other.kind
            || self.format_major != other.format_major
            || self.format_minor != other.format_minor
            || self.eir_hash != other.eir_hash
            || self.domain_ir_hash != other.domain_ir_hash
            || self.schema_set_hash != other.schema_set_hash
            || self.runtime_abi_major != other.runtime_abi_major
            || self.component_abi_major != other.component_abi_major
            || self.target_kind != other.target_kind
            || self_features != other_features
            || self.capability_manifest != other.capability_manifest
        {
            return Err(error(Status::AbiMismatch, 1));
        }
        Ok(())
    }
}

/// A compiled artifact. Until a JIT backend exists, execution delegates to the
/// EIR interpreter so cache hits and misses share semantics.
#[derive(Clone)]
pub struct CompiledArtifact {
    pub identity: ArtifactIdentity,
    pub eir: EirModule,
}

impl CompiledArtifact {
    pub fn execute(&self, world: WorldId, version: WorldVersion) -> Result<Vec<WorldWrite>> {
        self.eir.validate(true)?;
        self.eir.interpret(world, version)
    }
}

/// Atomic interpreter-backed compiled-artifact cache (RFC-0027).
#[derive(Default)]
pub struct ArtifactCache {
    entries: BTreeMap<Hash256, CompiledArtifact>,
}

impl ArtifactCache {
    pub fn publish(&mut self, artifact: CompiledArtifact) -> Result<()> {
        let key = artifact.identity.hash()?;
        // A key collision with a distinct identity must never reinterpret.
        if let Some(existing) = self.entries.get(&key) {
            existing.identity.requires(&artifact.identity)?;
        }
        self.entries.insert(key, artifact);
        Ok(())
    }

    pub fn get(&self, identity: &ArtifactIdentity) -> Option<&CompiledArtifact> {
        let key = identity.hash().ok()?;
        self.entries.get(&key)
    }

    /// Invalidates one entry atomically (removal is a single map op).
    pub fn invalidate(&mut self, identity: &ArtifactIdentity) -> Result<()> {
        let key = identity.hash()?;
        self.entries.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::{EirModule, Function};

    fn identity(eir_hash: Hash256) -> ArtifactIdentity {
        ArtifactIdentity {
            kind: 1,
            format_major: 2,
            format_minor: 0,
            eir_hash,
            domain_ir_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            runtime_abi_major: 2,
            component_abi_major: 2,
            target_kind: 0,
            features: vec![],
            capability_manifest: Hash256([0; 32]),
        }
    }

    fn module() -> EirModule {
        EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![],
            }],
        }
    }

    #[test]
    fn artifact_identity_hash_is_stable_and_order_insensitive_for_features() {
        let mut a = identity(Hash256([1; 32]));
        a.features = vec![5, 3];
        let mut b = identity(Hash256([1; 32]));
        b.features = vec![3, 5];
        assert_eq!(a.hash().unwrap(), b.hash().unwrap());
    }

    #[test]
    fn artifact_requires_fails_closed_on_abi_mismatch() {
        let a = identity(Hash256([1; 32]));
        let mut b = identity(Hash256([2; 32]));
        b.eir_hash = Hash256([9; 32]);
        assert_eq!(a.requires(&b).unwrap_err().status, Status::AbiMismatch);
    }

    #[test]
    fn artifact_requires_rejects_format_minor_and_feature_mismatch() {
        let a = identity(Hash256([1; 32]));
        let mut minor = identity(Hash256([1; 32]));
        minor.format_minor = 1;
        assert_eq!(minor.requires(&a).unwrap_err().status, Status::AbiMismatch);
        let mut feats = identity(Hash256([1; 32]));
        feats.features = vec![1, 2];
        assert_eq!(feats.requires(&a).unwrap_err().status, Status::AbiMismatch);
    }

    #[test]
    fn artifact_cache_publish_lookup_invalidate_is_atomic() {
        let id = identity(Hash256([7; 32]));
        let artifact = CompiledArtifact {
            identity: id.clone(),
            eir: module(),
        };
        let mut cache = ArtifactCache::default();
        cache.publish(artifact).unwrap();
        assert!(cache.get(&id).is_some());
        cache.invalidate(&id).unwrap();
        assert!(cache.get(&id).is_none());
    }

    #[test]
    fn artifact_cache_lookup_fails_closed_on_mismatched_identity() {
        let id = identity(Hash256([7; 32]));
        let artifact = CompiledArtifact {
            identity: id.clone(),
            eir: module(),
        };
        let mut cache = ArtifactCache::default();
        cache.publish(artifact).unwrap();
        // A loader must gate publish/execute on identity equality. A distinct
        // identity hashes to a distinct key, so no artifact is served.
        let mut mismatched = id.clone();
        mismatched.eir_hash = Hash256([9; 32]);
        assert!(cache.get(&mismatched).is_none());
    }
}
