//! RFC-0028 immutable render frame.
//!
//! A renderer acquires a frame against a declared `WorldVersion`. Every read
//! the frame serves is taken from an immutable byte snapshot captured at
//! acquisition, so all passes and resources in one frame observe exactly one
//! consistent world version, regardless of later commits. `Present` is not
//! modelled here: it is external IO and feeds back through a later explicit
//! transaction.

use crate::ReferenceWorld;
use pwe_api::{
    ComponentDescriptor, ComponentTypeId, EntityRef, Error, Ownership, Result, Status, WorldVersion,
};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// Identifies one render frame per RFC-0028. `world_version` is the immutable
/// version every pass reads; `render_time_ns` and `sequence` disambiguate
/// presentation, not authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderFrameId {
    pub world_version: WorldVersion,
    pub render_time_ns: i64,
    pub sequence: u64,
}

impl RenderFrameId {
    pub fn new(world_version: WorldVersion, render_time_ns: i64, sequence: u64) -> Self {
        Self {
            world_version,
            render_time_ns,
            sequence,
        }
    }
}

/// One endpoint (source tick) of an interpolation, RFC-0028. Naming both
/// source ticks explicitly (rather than a single blend) is what lets a renderer
/// reproduce the same interpolated sample across passes and frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterpSource {
    pub world_version: WorldVersion,
    pub render_time_ns: i64,
}

/// A named two-tick interpolation between two source frames. `source_a` and
/// `source_b` are the two authoritative source ticks; `alpha` is the blend in
/// `[0,1]` from `a` to `b`.
#[derive(Clone, Copy, Debug)]
pub struct Interpolation {
    pub source_a: InterpSource,
    pub source_b: InterpSource,
    pub alpha: f64,
}

impl Interpolation {
    pub fn new(source_a: InterpSource, source_b: InterpSource, alpha: f64) -> Self {
        Self {
            source_a,
            source_b,
            alpha,
        }
    }
    /// Bounded linear blend: `a + (b - a) * alpha`. A renderer may clamp `alpha`
    /// to `[0,1]`; it never mutates authority.
    pub fn lerp(&self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.alpha
    }
}

/// A frozen, consistent read snapshot of one world version. Acquiring a frame
/// copies the component bytes of the current world, so the frame outlives and
/// ignores any subsequent commit.
#[derive(Clone, Debug)]
pub struct RenderFrame {
    id: RenderFrameId,
    entities: BTreeMap<EntityRef, Ownership>,
    components: BTreeMap<(EntityRef, ComponentTypeId), (ComponentDescriptor, Vec<u8>)>,
}

impl RenderFrame {
    /// Reads one component from the frame. Returns [`Status::HandleStale`] if
    /// the entity or component was not present at the acquired version.
    pub fn component_bytes(&self, entity: EntityRef, component: ComponentTypeId) -> Result<&[u8]> {
        self.components
            .get(&(entity, component))
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or(error(Status::HandleStale, 3))
    }

    /// Present: submit a frame to external output (a display). This is external
    /// IO — it never mutates world state. The submitted frame is identified so
    /// input/feedback can be reconciled against the exact frame presented.
    pub fn present(self, output: &mut dyn PresentSink) -> Result<PresentReceipt> {
        output.present(self)
    }

    /// Ownership as of the acquired version.
    pub fn ownership(&self, entity: EntityRef) -> Result<Ownership> {
        self.entities
            .get(&entity)
            .copied()
            .ok_or(error(Status::HandleStale, 1))
    }

    pub fn id(&self) -> RenderFrameId {
        self.id
    }
}

/// A presentation target: external output that consumes a frame and returns
/// input/feedback to be fed back through a later explicit transaction
/// (RFC-0028 "Present is external IO").
pub trait PresentSink {
    fn present(&mut self, frame: RenderFrame) -> Result<PresentReceipt>;
}

/// Proof that a frame was presented and the input it produced (queued for the
/// next explicit transaction — never applied here).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentReceipt {
    pub frame_id: RenderFrameId,
    /// Number of input events the consumer produced from this presentation.
    pub input_events: u64,
}

impl ReferenceWorld {
    /// Acquires an immutable frame for the world's current version. All reads
    /// from the returned frame are served from bytes copied here, so the frame
    /// observes exactly one `WorldVersion` and never a torn or later state.
    pub fn acquire_frame(&self, render_time_ns: i64, sequence: u64) -> RenderFrame {
        RenderFrame {
            id: RenderFrameId::new(self.version, render_time_ns, sequence),
            entities: self.entities.clone(),
            components: self
                .components
                .iter()
                .map(|(key, state)| (*key, (state.descriptor, state.bytes.clone())))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::{
        Access, CapabilityClaims, ComponentDescriptor, ConflictPolicy, EntityId, Hash256,
        Ownership, OwnershipEpoch, RegionId, TransactionOptions, WorldId,
    };

    const POSITION: ComponentDescriptor = ComponentDescriptor {
        type_id: ComponentTypeId([9; 16]),
        schema_hash: Hash256([3; 32]),
        abi_major: 2,
        flags: 0,
        value_size: 4,
        value_align: 4,
    };

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
    fn capability(access: Access) -> CapabilityClaims {
        CapabilityClaims {
            issuer: Hash256([1; 32]),
            subject: Hash256([2; 32]),
            world: WorldId(1),
            region: RegionId(7),
            access,
            expires_at_version: WorldVersion(u64::MAX),
            nonce: [0; 16],
        }
    }
    fn entity(n: u128) -> EntityRef {
        EntityRef {
            id: EntityId(n),
            generation: 1,
        }
    }

    fn world_with_renderable() -> ReferenceWorld {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world
            .begin(options(0), capability(Access::CREATE.union(Access::WRITE)))
            .unwrap();
        tx.create(entity(1)).unwrap();
        tx.put_component(entity(1), POSITION, &[1, 2, 3, 4])
            .unwrap();
        tx.commit().unwrap();
        world
    }

    #[test]
    fn render_frame_reads_exactly_one_consistent_version() {
        let world = world_with_renderable();
        // Frames at the same version are identical.
        let a = world.acquire_frame(100, 0);
        let b = world.acquire_frame(100, 0);
        assert_eq!(a.id().world_version, WorldVersion(1));
        assert_eq!(a.id(), b.id());
        assert_eq!(
            a.component_bytes(entity(1), POSITION.type_id).unwrap(),
            &[1, 2, 3, 4]
        );
        assert_eq!(a.ownership(entity(1)).unwrap().region, RegionId(7));
    }

    #[test]
    fn render_frame_is_immutable_across_later_commits() {
        let mut world = world_with_renderable();
        let frame = world.acquire_frame(0, 0);
        // Advance the world by overwriting the component after acquisition.
        let mut tx = world.begin(options(1), capability(Access::WRITE)).unwrap();
        tx.put_component(entity(1), POSITION, &[9, 9, 9, 9])
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(world.version(), WorldVersion(2));
        // The frame still reads the bytes and version it acquired.
        assert_eq!(frame.id().world_version, WorldVersion(1));
        assert_eq!(
            frame.component_bytes(entity(1), POSITION.type_id).unwrap(),
            &[1, 2, 3, 4]
        );
    }

    #[test]
    fn render_frame_missing_slot_fails_closed() {
        let world = world_with_renderable();
        let frame = world.acquire_frame(0, 0);
        assert_eq!(
            frame
                .component_bytes(entity(2), POSITION.type_id)
                .unwrap_err()
                .status,
            Status::HandleStale
        );
    }

    #[test]
    fn interpolation_names_both_source_ticks_and_lerps() {
        let a = InterpSource {
            world_version: WorldVersion(10),
            render_time_ns: 0,
        };
        let b = InterpSource {
            world_version: WorldVersion(11),
            render_time_ns: 1_000_000,
        };
        let interp = Interpolation::new(a, b, 0.25);
        // Both source ticks are carried so a pass can reproduce the sample.
        assert_eq!(interp.source_a, a);
        assert_eq!(interp.source_b, b);
        assert_eq!(interp.lerp(0.0, 100.0), 25.0);
        assert_eq!(interp.lerp(100.0, 0.0), 75.0);
    }

    #[test]
    fn present_is_external_io_that_does_not_mutate_world() {
        struct Sink;
        impl PresentSink for Sink {
            fn present(&mut self, frame: RenderFrame) -> Result<PresentReceipt> {
                Ok(PresentReceipt {
                    frame_id: frame.id(),
                    input_events: 1,
                })
            }
        }
        let world = world_with_renderable();
        let before = world.version();
        let frame = world.acquire_frame(0, 0);
        let receipt = frame.present(&mut Sink).unwrap();
        assert_eq!(receipt.frame_id.world_version, WorldVersion(1));
        assert_eq!(receipt.input_events, 1);
        // Present is external IO: authoritative state is untouched.
        assert_eq!(world.version(), before);
    }
}
