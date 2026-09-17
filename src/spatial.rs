//! RFC-0014 spatial and interest-management model (folded into the frozen
//! contract).
//!
//! Spatial indexing is shared infrastructure for physics, rendering, sensors,
//! AI, and networking. Interest management NEVER alters authoritative state
//! merely because an entity is outside a consumer's interest set.

use crate::{ComponentTypeId, EntityId};

/// A spatial index backend. Backend selection is runtime/compiler policy; the
/// semantic contract (queries) is identical.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SpatialBackend {
    Bvh = 0,
    Octree = 1,
    Grid = 2,
    SpatialHash = 3,
    RTree = 4,
}

/// An axis-aligned bounding region for spatial queries, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A unified spatial/semantic query. Any of the filters may be left unbounded.
#[derive(Clone, Copy, Debug, Default)]
pub struct InterestQuery {
    /// AABB to test; `None` means unbounded in space.
    pub region: Option<Aabb>,
    /// Semantic class id (stable hash); `None` means any class.
    pub class: Option<u64>,
    /// Required component types, if any.
    pub components: Option<&'static [ComponentTypeId]>,
    /// Maximum distance filter (e.g. LiDAR range); `None` = unbounded.
    pub max_distance: Option<f32>,
    /// Priority floor; only entities at or above this priority are returned.
    pub min_priority: Option<u8>,
}

/// Result of an interest query: the entities that fall inside the declared
/// interest set. This is a read-only projection; it never mutates state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterestResult {
    pub entity: EntityId,
    pub priority: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interest_query_is_read_only_and_defaults_unbounded() {
        let q = InterestQuery::default();
        assert!(q.region.is_none());
        assert!(q.class.is_none());
        assert!(q.max_distance.is_none());
    }
    #[test]
    fn aabb_is_structural_equality() {
        let a = Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        let b = Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        assert_eq!(a, b);
    }
}
