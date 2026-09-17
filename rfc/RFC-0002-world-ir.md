# RFC-0002: World IR

**Status:** Draft \| **Version:** 0.1.0

## 1. Purpose

WIR is the canonical representation of what exists in a physical world.

## 2. Core model

``` rust
pub struct WorldIr {
    pub version: Version,
    pub world_id: WorldId,
    pub entities: EntityTable,
    pub components: ComponentTable,
    pub resources: ResourceTable,
    pub spatial: SpatialModel,
    pub systems: SystemGraph,
    pub events: EventSchema,
    pub timelines: TimelineTable,
}
```

`EntityId` is a stable `u64`. Long-lived references use
`(WorldId, EntityId, Generation)`.

## 3. Standard components

Transform, Geometry, Material, Physics, Behavior, Sensor, Semantic,
Animation, Environment, Spatial, Network.

## 4. Geometry

Mesh, PointCloud, Voxel, SDF, Implicit, Gaussian, Neural.

## 5. Physics

Mass, inertia, velocity, acceleration, collider, material, constraint.
WIR describes physical properties, never a concrete solver.

## 6. Systems

A System declares reads/writes/resources. The scheduler derives
dependencies from these declarations.

## 7. Time

A world may contain multiple TimeDomains, e.g. physics 1000 Hz, sensor
100 Hz, AI 20 Hz, rendering 60 Hz.

## 8. Serialization

Development: JSON/YAML/TOML. Production: canonical binary encoding.
Schema hash must identify semantic layout.

## 9. Validation

Reject duplicate IDs, invalid component versions, invalid references,
impossible ownership, malformed transforms, and incompatible schema
hashes.

## 10. Invariant

WIR describes *what the world is*, not *how an algorithm executes it*.
