//! RFC-0015 resource and asset identity (folded into the frozen contract).
//!
//! Resources are immutable-or-versioned artifacts (mesh, texture, material,
//! SDF, terrain, navigation map, shader, snapshot). They are content-addressed
//! and reference-stable independent of physical storage location.

use crate::Hash256;

/// Stable identity of a resource, independent of storage location.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ResourceId(pub u128);

/// Explicit residency of a resource's authoritative bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Residency {
    Cpu = 0,
    Gpu = 1,
    LocalCache = 2,
    Remote = 3,
}

/// Content-addressed resource descriptor. Identical content hashes deduplicate
/// across nodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceDescriptor {
    pub id: ResourceId,
    /// SHA-256 of the canonical resource bytes.
    pub content_hash: Hash256,
    /// Versioned schema the resource bytes conform to.
    pub schema_hash: Hash256,
    pub residency: Residency,
}

impl ResourceDescriptor {
    /// True when two descriptors reference the same content regardless of id.
    pub fn content_equals(self, other: &Self) -> bool {
        self.content_hash == other.content_hash && self.schema_hash == other.schema_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_identity_is_content_addressed_and_stable() {
        let a = ResourceDescriptor {
            id: ResourceId(1),
            content_hash: Hash256([9; 32]),
            schema_hash: Hash256([8; 32]),
            residency: Residency::Cpu,
        };
        let b = ResourceDescriptor {
            id: ResourceId(2),
            content_hash: Hash256([9; 32]),
            schema_hash: Hash256([8; 32]),
            residency: Residency::Remote,
        };
        // Same content, different id and residency -> still the same resource.
        assert!(a.content_equals(&b));
    }
}
