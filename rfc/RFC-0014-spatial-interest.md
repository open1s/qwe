# RFC-0014: Spatial and Interest Management

**Status:** Draft \| **Version:** 0.1.0

## 1. Principle

Spatial indexing is shared infrastructure for physics, rendering,
sensors, AI and networking.

## 2. Abstraction

Spatial queries operate over entity bounds, semantic tags, layers and
time validity.

## 3. Backends

BVH, Octree, Grid, SpatialHash, RTree. Backend selection is
runtime/compiler policy.

## 4. Interest

An InterestQuery may filter by: - spatial region - semantic class -
component set - visibility - distance - temporal freshness - priority

## 5. LOD

LOD is a policy over representation and computation, not an independent
world.

## 6. Unified query

The same spatial/semantic query framework may feed renderer culling,
physics broad phase, LiDAR sampling, AI perception and network
replication.

## 7. Acceptance

Interest management must never alter authoritative state merely because
an entity is outside a consumer's interest set.
