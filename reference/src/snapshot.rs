//! Deterministic snapshot slice for RFC-0025. Restore validates before replacement.

use crate::wire::{Reader, Writer};
use crate::{ComponentState, ReferenceWorld, ResourceRef};
use pwe_api::resource::ResourceId;
use pwe_api::{
    ComponentDescriptor, ComponentTypeId, EntityId, EntityRef, Error, Hash256, Ownership,
    OwnershipEpoch, RegionId, Result, Status, WorldId, WorldVersion,
};
use std::collections::BTreeMap;

const MAGIC: &[u8; 8] = b"PWESNP2\0";
fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}
fn u128_bytes(value: u128) -> [u8; 16] {
    value.to_le_bytes()
}
fn parse_u128(bytes: &[u8]) -> u128 {
    u128::from_le_bytes(bytes.try_into().unwrap())
}

impl ReferenceWorld {
    pub fn snapshot(&self, schema_set_hash: Hash256) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.bytes_raw(MAGIC)?;
        out.u16(2)?;
        out.u16(0)?;
        out.bytes_raw(&schema_set_hash.0)?;
        out.bytes_raw(&u128_bytes(self.id.0))?;
        out.u64(self.version.0)?;
        out.u64(self.sim_time.tick)?;
        out.u64(self.sim_time.nanos as u64)?;
        out.u32(self.resource_refs.len() as u32)?;
        for (id, reference) in &self.resource_refs {
            out.bytes_raw(&u128_bytes(id.0))?;
            out.bytes_raw(&reference.content_hash.0)?;
            out.bytes_raw(&reference.schema_hash.0)?;
        }
        out.u32(self.entities.len() as u32)?;
        for (entity, ownership) in &self.entities {
            out.bytes_raw(&u128_bytes(entity.id.0))?;
            out.u32(entity.generation)?;
            out.u64(ownership.region.0)?;
            out.u64(ownership.epoch.0)?;
        }
        out.u32(self.components.len() as u32)?;
        for ((entity, type_id), state) in &self.components {
            out.bytes_raw(&u128_bytes(entity.id.0))?;
            out.u32(entity.generation)?;
            out.bytes_raw(&type_id.0)?;
            out.bytes_raw(&state.descriptor.schema_hash.0)?;
            out.u32(state.descriptor.abi_major)?;
            out.u32(state.descriptor.flags)?;
            out.u32(state.descriptor.value_size)?;
            out.u32(state.descriptor.value_align)?;
            out.bytes(&state.bytes)?;
        }
        Ok(out.finish())
    }
    pub fn restore_snapshot(
        &mut self,
        expected_schema_set_hash: Hash256,
        bytes: &[u8],
    ) -> Result<()> {
        let mut input = Reader::new(bytes)?;
        if input.fixed(8)? != MAGIC || input.u16()? != 2 || input.u16()? != 0 {
            return Err(error(Status::Invalid, 1));
        }
        let schema = Hash256(input.fixed(32)?.try_into().unwrap());
        if schema != expected_schema_set_hash {
            return Err(error(Status::SchemaHash, 1));
        }
        let id = WorldId(parse_u128(input.fixed(16)?));
        let version = WorldVersion(input.u64()?);
        let sim_time = pwe_api::time::SimTime {
            tick: input.u64()?,
            nanos: input.u64()? as i64,
        };
        let resource_ref_count = input.u32()? as usize;
        let mut resource_refs = BTreeMap::new();
        for _ in 0..resource_ref_count {
            let rid = ResourceId(parse_u128(input.fixed(16)?));
            let reference = ResourceRef {
                content_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
                schema_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
            };
            if resource_refs.insert(rid, reference).is_some() {
                return Err(error(Status::Invalid, 4));
            }
        }
        let entity_count = input.u32()? as usize;
        let mut entities = BTreeMap::new();
        for _ in 0..entity_count {
            let entity = EntityRef {
                id: EntityId(parse_u128(input.fixed(16)?)),
                generation: input.u32()?,
            };
            let ownership = Ownership {
                region: RegionId(input.u64()?),
                epoch: OwnershipEpoch(input.u64()?),
            };
            if entity.generation == 0 || entities.insert(entity, ownership).is_some() {
                return Err(error(Status::Invalid, 2));
            }
        }
        let component_count = input.u32()? as usize;
        let mut components = BTreeMap::new();
        for _ in 0..component_count {
            let entity = EntityRef {
                id: EntityId(parse_u128(input.fixed(16)?)),
                generation: input.u32()?,
            };
            let type_id = ComponentTypeId(input.fixed(16)?.try_into().unwrap());
            let descriptor = ComponentDescriptor {
                type_id,
                schema_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
                abi_major: input.u32()?,
                flags: input.u32()?,
                value_size: input.u32()?,
                value_align: input.u32()?,
            };
            descriptor.validate()?;
            let value = input.bytes()?.to_vec();
            if value.len() != descriptor.value_size as usize
                || !entities.contains_key(&entity)
                || components
                    .insert(
                        (entity, type_id),
                        ComponentState {
                            descriptor,
                            bytes: value,
                        },
                    )
                    .is_some()
            {
                return Err(error(Status::Invalid, 3));
            }
        }
        input.finish()?;
        self.id = id;
        self.version = version;
        self.sim_time = sim_time;
        self.entities = entities;
        self.components = components;
        self.resource_refs = resource_refs;
        Ok(())
    }
}

const DELTA_MAGIC: &[u8; 8] = b"PWEDLT2\0";

/// RFC-0025 delta operation kinds, in a stable wire order.
mod op {
    pub const CREATE_ENTITY: u8 = 1;
    pub const DESTROY_ENTITY: u8 = 2;
    pub const ADD_COMPONENT: u8 = 3;
    pub const REMOVE_COMPONENT: u8 = 4;
    pub const REPLACE_COMPONENT: u8 = 5;
    pub const PATCH_COMPONENT: u8 = 6;
    pub const RESOURCE_UPDATE: u8 = 7;
    pub const OWNERSHIP_UPDATE: u8 = 8;
    pub const EVENT_BATCH: u8 = 9;
}

/// One deterministic delta operation. Payloads are canonical bytes; ops are
/// sorted by `sort_key` before encoding and must be ascending on decode.
#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp {
    CreateEntity {
        entity: EntityRef,
        ownership: Ownership,
    },
    DestroyEntity {
        entity: EntityRef,
        ownership: Ownership,
    },
    AddComponent {
        entity: EntityRef,
        ownership: Ownership,
        descriptor: ComponentDescriptor,
        bytes: Vec<u8>,
    },
    RemoveComponent {
        entity: EntityRef,
        ownership: Ownership,
        type_id: ComponentTypeId,
    },
    ReplaceComponent {
        entity: EntityRef,
        ownership: Ownership,
        descriptor: ComponentDescriptor,
        bytes: Vec<u8>,
    },
    /// Replaces `bytes` at `offset` in the current component value.
    PatchComponent {
        entity: EntityRef,
        ownership: Ownership,
        type_id: ComponentTypeId,
        offset: u32,
        bytes: Vec<u8>,
    },
    /// Adds or replaces a content-addressed resource reference in the world's
    /// resource store (RFC-0025 resource refs).
    ResourceUpdate {
        id: ResourceId,
        reference: ResourceRef,
    },
    OwnershipUpdate {
        entity: EntityRef,
        ownership: Ownership,
        new_ownership: Ownership,
    },
    /// Ordered event payload; deltas carry events, they never mutate state.
    EventBatch { domain: u32, bytes: Vec<u8> },
}

impl DeltaOp {
    /// Canonical sort key: (entity, kind, type, resource/domain, offset).
    /// Component ops sort under their entity; resource/event ops sort first.
    fn sort_key(&self) -> (EntityRef, u8, [u8; 32], u32) {
        const ZERO: EntityRef = EntityRef {
            id: EntityId(0),
            generation: 0,
        };
        fn widen(type_id: ComponentTypeId) -> [u8; 32] {
            let mut key = [0; 32];
            key[..16].copy_from_slice(&type_id.0);
            key
        }
        fn widen_u128(value: u128) -> [u8; 32] {
            let mut key = [0; 32];
            key[..16].copy_from_slice(&value.to_le_bytes());
            key
        }
        match self {
            DeltaOp::CreateEntity { entity, .. }
            | DeltaOp::DestroyEntity { entity, .. }
            | DeltaOp::OwnershipUpdate { entity, .. } => (*entity, self.kind(), [0; 32], 0),
            DeltaOp::AddComponent {
                entity, descriptor, ..
            }
            | DeltaOp::ReplaceComponent {
                entity, descriptor, ..
            } => (*entity, self.kind(), widen(descriptor.type_id), 0),
            DeltaOp::RemoveComponent {
                entity, type_id, ..
            } => (*entity, self.kind(), widen(*type_id), 0),
            DeltaOp::PatchComponent {
                entity,
                type_id,
                offset,
                ..
            } => (*entity, self.kind(), widen(*type_id), *offset),
            DeltaOp::ResourceUpdate { id, .. } => (ZERO, self.kind(), widen_u128(id.0), 0),
            DeltaOp::EventBatch { domain, .. } => {
                let mut key = [0; 32];
                key[..4].copy_from_slice(&domain.to_le_bytes());
                (ZERO, self.kind(), key, 0)
            }
        }
    }
    fn kind(&self) -> u8 {
        match self {
            DeltaOp::CreateEntity { .. } => op::CREATE_ENTITY,
            DeltaOp::DestroyEntity { .. } => op::DESTROY_ENTITY,
            DeltaOp::AddComponent { .. } => op::ADD_COMPONENT,
            DeltaOp::RemoveComponent { .. } => op::REMOVE_COMPONENT,
            DeltaOp::ReplaceComponent { .. } => op::REPLACE_COMPONENT,
            DeltaOp::PatchComponent { .. } => op::PATCH_COMPONENT,
            DeltaOp::ResourceUpdate { .. } => op::RESOURCE_UPDATE,
            DeltaOp::OwnershipUpdate { .. } => op::OWNERSHIP_UPDATE,
            DeltaOp::EventBatch { .. } => op::EVENT_BATCH,
        }
    }
    fn encode(&self, out: &mut Writer) -> Result<()> {
        fn entity(out: &mut Writer, entity: EntityRef) -> Result<()> {
            out.bytes_raw(&u128_bytes(entity.id.0))?;
            out.u32(entity.generation)
        }
        fn ownership(out: &mut Writer, ownership: Ownership) -> Result<()> {
            out.u64(ownership.region.0)?;
            out.u64(ownership.epoch.0)
        }
        fn descriptor(out: &mut Writer, descriptor: ComponentDescriptor) -> Result<()> {
            out.bytes_raw(&descriptor.type_id.0)?;
            out.bytes_raw(&descriptor.schema_hash.0)?;
            out.u32(descriptor.abi_major)?;
            out.u32(descriptor.flags)?;
            out.u32(descriptor.value_size)?;
            out.u32(descriptor.value_align)
        }
        out.u8(self.kind())?;
        match self {
            DeltaOp::CreateEntity {
                entity: e,
                ownership: o,
            }
            | DeltaOp::DestroyEntity {
                entity: e,
                ownership: o,
            } => {
                entity(out, *e)?;
                ownership(out, *o)
            }
            DeltaOp::AddComponent {
                entity: e,
                ownership: o,
                descriptor: d,
                bytes,
            }
            | DeltaOp::ReplaceComponent {
                entity: e,
                ownership: o,
                descriptor: d,
                bytes,
            } => {
                entity(out, *e)?;
                ownership(out, *o)?;
                descriptor(out, *d)?;
                out.bytes(bytes)
            }
            DeltaOp::RemoveComponent {
                entity: e,
                ownership: o,
                type_id,
            } => {
                entity(out, *e)?;
                ownership(out, *o)?;
                out.bytes_raw(&type_id.0)
            }
            DeltaOp::PatchComponent {
                entity: e,
                ownership: o,
                type_id,
                offset,
                bytes,
            } => {
                entity(out, *e)?;
                ownership(out, *o)?;
                out.bytes_raw(&type_id.0)?;
                out.u32(*offset)?;
                out.bytes(bytes)
            }
            DeltaOp::ResourceUpdate { id, reference } => {
                out.bytes_raw(&u128_bytes(id.0))?;
                out.bytes_raw(&reference.content_hash.0)?;
                out.bytes_raw(&reference.schema_hash.0)
            }
            DeltaOp::OwnershipUpdate {
                entity: e,
                ownership: o,
                new_ownership,
            } => {
                entity(out, *e)?;
                ownership(out, *o)?;
                ownership(out, *new_ownership)
            }
            DeltaOp::EventBatch { domain, bytes } => {
                out.u32(*domain)?;
                out.bytes(bytes)
            }
        }
    }
    fn decode(input: &mut Reader) -> Result<Self> {
        fn entity(input: &mut Reader) -> Result<EntityRef> {
            Ok(EntityRef {
                id: EntityId(parse_u128(input.fixed(16)?)),
                generation: input.u32()?,
            })
        }
        fn ownership(input: &mut Reader) -> Result<Ownership> {
            Ok(Ownership {
                region: RegionId(input.u64()?),
                epoch: OwnershipEpoch(input.u64()?),
            })
        }
        fn descriptor(input: &mut Reader) -> Result<ComponentDescriptor> {
            Ok(ComponentDescriptor {
                type_id: ComponentTypeId(input.fixed(16)?.try_into().unwrap()),
                schema_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
                abi_major: input.u32()?,
                flags: input.u32()?,
                value_size: input.u32()?,
                value_align: input.u32()?,
            })
        }
        Ok(match input.fixed(1)?[0] {
            op::CREATE_ENTITY => DeltaOp::CreateEntity {
                entity: entity(input)?,
                ownership: ownership(input)?,
            },
            op::DESTROY_ENTITY => DeltaOp::DestroyEntity {
                entity: entity(input)?,
                ownership: ownership(input)?,
            },
            op::ADD_COMPONENT => DeltaOp::AddComponent {
                entity: entity(input)?,
                ownership: ownership(input)?,
                descriptor: descriptor(input)?,
                bytes: input.bytes()?.to_vec(),
            },
            op::REMOVE_COMPONENT => DeltaOp::RemoveComponent {
                entity: entity(input)?,
                ownership: ownership(input)?,
                type_id: ComponentTypeId(input.fixed(16)?.try_into().unwrap()),
            },
            op::REPLACE_COMPONENT => DeltaOp::ReplaceComponent {
                entity: entity(input)?,
                ownership: ownership(input)?,
                descriptor: descriptor(input)?,
                bytes: input.bytes()?.to_vec(),
            },
            op::PATCH_COMPONENT => DeltaOp::PatchComponent {
                entity: entity(input)?,
                ownership: ownership(input)?,
                type_id: ComponentTypeId(input.fixed(16)?.try_into().unwrap()),
                offset: input.u32()?,
                bytes: input.bytes()?.to_vec(),
            },
            op::RESOURCE_UPDATE => DeltaOp::ResourceUpdate {
                id: ResourceId(parse_u128(input.fixed(16)?)),
                reference: ResourceRef {
                    content_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
                    schema_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
                },
            },
            op::OWNERSHIP_UPDATE => DeltaOp::OwnershipUpdate {
                entity: entity(input)?,
                ownership: ownership(input)?,
                new_ownership: ownership(input)?,
            },
            op::EVENT_BATCH => DeltaOp::EventBatch {
                domain: input.u32()?,
                bytes: input.bytes()?.to_vec(),
            },
            _ => return Err(error(Status::Invalid, 10)),
        })
    }
}

/// An RFC-0025 delta: one base version, one target version, sorted operations.
#[derive(Clone, Debug, PartialEq)]
pub struct StateDelta {
    pub schema_set_hash: Hash256,
    pub world: WorldId,
    pub base_version: WorldVersion,
    pub target_version: WorldVersion,
    pub operations: Vec<DeltaOp>,
}

impl StateDelta {
    /// Canonical encoding: operations are sorted by their canonical key.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut ops = self.operations.clone();
        ops.sort_by_key(DeltaOp::sort_key);
        let mut out = Writer::new();
        out.bytes_raw(DELTA_MAGIC)?;
        out.u16(2)?;
        out.u16(0)?;
        out.bytes_raw(&self.schema_set_hash.0)?;
        out.bytes_raw(&u128_bytes(self.world.0))?;
        out.u64(self.base_version.0)?;
        out.u64(self.target_version.0)?;
        out.u32(ops.len() as u32)?;
        for operation in &ops {
            operation.encode(&mut out)?;
        }
        Ok(out.finish())
    }
    /// Bounded decode; rejects malformed bytes and non-ascending op order.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        if input.fixed(8)? != DELTA_MAGIC || input.u16()? != 2 || input.u16()? != 0 {
            return Err(error(Status::Invalid, 11));
        }
        let delta = StateDelta {
            schema_set_hash: Hash256(input.fixed(32)?.try_into().unwrap()),
            world: WorldId(parse_u128(input.fixed(16)?)),
            base_version: WorldVersion(input.u64()?),
            target_version: WorldVersion(input.u64()?),
            operations: {
                let count = input.u32()? as usize;
                let mut operations = Vec::with_capacity(count.min(1024));
                let mut previous = None;
                for _ in 0..count {
                    let operation = DeltaOp::decode(&mut input)?;
                    let key = operation.sort_key();
                    if previous.is_some_and(|p| p >= key) {
                        return Err(error(Status::Invalid, 12));
                    }
                    previous = Some(key);
                    operations.push(operation);
                }
                operations
            },
        };
        input.finish()?;
        Ok(delta)
    }
}

impl ReferenceWorld {
    /// Applies an RFC-0025 delta atomically: world/schema/base are validated,
    /// every op is checked against ownership epochs, and state either reaches
    /// exactly the target version or is left completely untouched.
    pub fn apply_delta(&mut self, expected_schema_set_hash: Hash256, bytes: &[u8]) -> Result<()> {
        let delta = StateDelta::decode(bytes)?;
        if delta.schema_set_hash != expected_schema_set_hash {
            return Err(error(Status::SchemaHash, 2));
        }
        if delta.world != self.id {
            return Err(error(Status::Invalid, 13));
        }
        if delta.base_version != self.version {
            return Err(error(Status::Conflict, 8));
        }
        if delta.target_version.0 <= delta.base_version.0 {
            return Err(error(Status::Invalid, 14));
        }
        // Simulate on clones so any failure leaves authoritative state intact.
        let mut entities = self.entities.clone();
        let mut components = self.components.clone();
        let mut resource_refs = self.resource_refs.clone();
        for operation in &delta.operations {
            apply_op(
                &mut entities,
                &mut components,
                &mut resource_refs,
                operation,
            )?;
        }
        self.entities = entities;
        self.components = components;
        self.resource_refs = resource_refs;
        self.version = delta.target_version;
        Ok(())
    }
}

fn expect_ownership(
    entities: &BTreeMap<EntityRef, Ownership>,
    entity: EntityRef,
    ownership: Ownership,
) -> Result<()> {
    match entities.get(&entity) {
        Some(current) if *current == ownership => Ok(()),
        Some(_) => Err(error(Status::OwnershipStale, 5)),
        None => Err(error(Status::HandleStale, 6)),
    }
}
fn apply_op(
    entities: &mut BTreeMap<EntityRef, Ownership>,
    components: &mut BTreeMap<(EntityRef, ComponentTypeId), ComponentState>,
    resource_refs: &mut BTreeMap<ResourceId, ResourceRef>,
    operation: &DeltaOp,
) -> Result<()> {
    match operation {
        DeltaOp::CreateEntity { entity, ownership } => {
            if entity.generation == 0 || entities.contains_key(entity) {
                return Err(error(Status::Invalid, 15));
            }
            entities.insert(*entity, *ownership);
        }
        DeltaOp::DestroyEntity { entity, ownership } => {
            expect_ownership(entities, *entity, *ownership)?;
            entities.remove(entity);
            components.retain(|(component_entity, _), _| component_entity != entity);
        }
        DeltaOp::AddComponent {
            entity,
            ownership,
            descriptor,
            bytes,
        }
        | DeltaOp::ReplaceComponent {
            entity,
            ownership,
            descriptor,
            bytes,
        } => {
            descriptor.validate()?;
            if bytes.len() != descriptor.value_size as usize {
                return Err(error(Status::Invalid, 16));
            }
            expect_ownership(entities, *entity, *ownership)?;
            let exists = components.contains_key(&(*entity, descriptor.type_id));
            if matches!(operation, DeltaOp::AddComponent { .. }) == exists {
                return Err(error(Status::Invalid, 17));
            }
            components.insert(
                (*entity, descriptor.type_id),
                ComponentState {
                    descriptor: *descriptor,
                    bytes: bytes.clone(),
                },
            );
        }
        DeltaOp::RemoveComponent {
            entity,
            ownership,
            type_id,
        } => {
            expect_ownership(entities, *entity, *ownership)?;
            if components.remove(&(*entity, *type_id)).is_none() {
                return Err(error(Status::HandleStale, 7));
            }
        }
        DeltaOp::PatchComponent {
            entity,
            ownership,
            type_id,
            offset,
            bytes,
        } => {
            expect_ownership(entities, *entity, *ownership)?;
            let state = components
                .get_mut(&(*entity, *type_id))
                .ok_or(error(Status::HandleStale, 8))?;
            let end = (*offset as usize)
                .checked_add(bytes.len())
                .filter(|end| *end <= state.descriptor.value_size as usize)
                .ok_or(error(Status::Invalid, 18))?;
            state.bytes[*offset as usize..end].copy_from_slice(bytes);
        }
        DeltaOp::ResourceUpdate { id, reference } => {
            resource_refs.insert(*id, *reference);
        }
        DeltaOp::EventBatch { .. } => {}
        DeltaOp::OwnershipUpdate {
            entity,
            ownership,
            new_ownership,
        } => {
            expect_ownership(entities, *entity, *ownership)?;
            entities.insert(*entity, *new_ownership);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::{Access, CapabilityClaims, ConflictPolicy};
    fn claim() -> CapabilityClaims {
        CapabilityClaims {
            issuer: Hash256([1; 32]),
            subject: Hash256([2; 32]),
            world: WorldId(1),
            region: RegionId(7),
            access: Access::CREATE,
            expires_at_version: WorldVersion(u64::MAX),
            nonce: [0; 16],
        }
    }
    fn options() -> pwe_api::TransactionOptions {
        pwe_api::TransactionOptions {
            base_version: WorldVersion(0),
            ownership: Ownership {
                region: RegionId(7),
                epoch: OwnershipEpoch(1),
            },
            policy: ConflictPolicy::Reject,
        }
    }
    #[test]
    fn restore_is_atomic_and_deterministic() {
        let mut source = ReferenceWorld::new(WorldId(1));
        let mut tx = source.begin(options(), claim()).unwrap();
        tx.create(EntityRef {
            id: EntityId(3),
            generation: 1,
        })
        .unwrap();
        tx.commit().unwrap();
        let hash = Hash256([9; 32]);
        let bytes = source.snapshot(hash).unwrap();
        let mut target = ReferenceWorld::new(WorldId(99));
        target.restore_snapshot(hash, &bytes).unwrap();
        assert_eq!(target.id(), WorldId(1));
        assert_eq!(target.snapshot(hash).unwrap(), bytes);
        let old = target.version();
        assert!(target.restore_snapshot(Hash256([0; 32]), &bytes).is_err());
        assert_eq!(target.version(), old);
    }

    #[test]
    fn snapshot_round_trips_sim_time_and_resource_refs() {
        use crate::ResourceRef;
        let mut world = ReferenceWorld::new(WorldId(1));
        world.advance_time(pwe_api::time::SimTime {
            tick: 42,
            nanos: 1234,
        });
        world.set_resource_ref(
            ResourceId(7),
            ResourceRef {
                content_hash: Hash256([1; 32]),
                schema_hash: Hash256([2; 32]),
            },
        );
        let hash = Hash256([9; 32]);
        let bytes = world.snapshot(hash).unwrap();
        let mut restored = ReferenceWorld::new(WorldId(99));
        restored.restore_snapshot(hash, &bytes).unwrap();
        assert_eq!(restored.sim_time().tick, 42);
        assert_eq!(restored.sim_time().nanos, 1234);
        assert_eq!(
            restored.resource_refs().get(&ResourceId(7)),
            Some(&ResourceRef {
                content_hash: Hash256([1; 32]),
                schema_hash: Hash256([2; 32]),
            })
        );
        assert_eq!(restored.snapshot(hash).unwrap(), bytes);
    }

    const OWNER: Ownership = Ownership {
        region: RegionId(7),
        epoch: OwnershipEpoch(1),
    };
    const POSITION: ComponentDescriptor = ComponentDescriptor {
        type_id: ComponentTypeId([4; 16]),
        schema_hash: Hash256([5; 32]),
        abi_major: 2,
        flags: 0,
        value_size: 4,
        value_align: 4,
    };

    fn world_with_entity() -> ReferenceWorld {
        let mut world = ReferenceWorld::new(WorldId(1));
        let capability = CapabilityClaims {
            region: RegionId(7),
            access: Access::CREATE.union(Access::WRITE),
            ..claim()
        };
        let mut tx = world.begin(options(), capability).unwrap();
        tx.create(EntityRef {
            id: EntityId(3),
            generation: 1,
        })
        .unwrap();
        tx.put_component(
            EntityRef {
                id: EntityId(3),
                generation: 1,
            },
            POSITION,
            &[1, 2, 3, 4],
        )
        .unwrap();
        tx.commit().unwrap();
        world
    }

    fn delta_ops() -> Vec<DeltaOp> {
        let entity = EntityRef {
            id: EntityId(3),
            generation: 1,
        };
        let new_entity = EntityRef {
            id: EntityId(9),
            generation: 1,
        };
        // Intentionally unsorted; encode must canonicalize the order.
        vec![
            DeltaOp::EventBatch {
                domain: 1,
                bytes: vec![0xaa],
            },
            DeltaOp::PatchComponent {
                entity,
                ownership: OWNER,
                type_id: POSITION.type_id,
                offset: 2,
                bytes: vec![9, 9],
            },
            DeltaOp::CreateEntity {
                entity: new_entity,
                ownership: OWNER,
            },
            DeltaOp::OwnershipUpdate {
                entity,
                ownership: OWNER,
                new_ownership: Ownership {
                    region: RegionId(8),
                    epoch: OwnershipEpoch(2),
                },
            },
        ]
    }

    #[test]
    fn delta_round_trips_and_applies_deterministically() {
        let hash = Hash256([7; 32]);
        let delta = StateDelta {
            schema_set_hash: hash,
            world: WorldId(1),
            base_version: WorldVersion(1),
            target_version: WorldVersion(2),
            operations: delta_ops(),
        };
        // Encoding is canonical: unsorted input encodes the same, and decoded
        // output re-encodes byte-identically.
        let bytes = delta.encode().unwrap();
        assert_eq!(delta.encode().unwrap(), bytes);
        let decoded = StateDelta::decode(&bytes).unwrap();
        assert_eq!(decoded.encode().unwrap(), bytes);
        assert!(decoded
            .operations
            .windows(2)
            .all(|pair| pair[0].sort_key() < pair[1].sort_key()));

        let before = world_with_entity();
        let mut applied = world_with_entity();
        applied.apply_delta(hash, &bytes).unwrap();
        assert_eq!(applied.version(), WorldVersion(2));
        let entity = EntityRef {
            id: EntityId(3),
            generation: 1,
        };
        assert_eq!(
            applied.component_bytes(entity, POSITION.type_id).unwrap(),
            &[1, 2, 9, 9]
        );
        assert_eq!(applied.ownership(entity).unwrap().region, RegionId(8));

        // Replay: same base snapshot + same delta bytes → identical snapshot.
        let snapshot_before = before.snapshot(hash).unwrap();
        let mut replay = ReferenceWorld::new(WorldId(55));
        replay.restore_snapshot(hash, &snapshot_before).unwrap();
        replay.apply_delta(hash, &bytes).unwrap();
        assert_eq!(
            replay.snapshot(hash).unwrap(),
            applied.snapshot(hash).unwrap()
        );
    }

    #[test]
    fn delta_resource_update_applies_to_resource_store() {
        let hash = Hash256([7; 32]);
        let reference = ResourceRef {
            content_hash: Hash256([1; 32]),
            schema_hash: Hash256([2; 32]),
        };
        let delta = StateDelta {
            schema_set_hash: hash,
            world: WorldId(1),
            base_version: WorldVersion(0),
            target_version: WorldVersion(1),
            operations: vec![DeltaOp::ResourceUpdate {
                id: ResourceId(77),
                reference,
            }],
        };
        let bytes = delta.encode().unwrap();
        // The delta applies to an empty world (no entity ops) and the resource
        // reference lands in the world's resource store.
        let mut world = ReferenceWorld::new(WorldId(1));
        world.apply_delta(hash, &bytes).unwrap();
        assert_eq!(world.resource_refs().get(&ResourceId(77)), Some(&reference));
        assert_eq!(world.version(), WorldVersion(1));
    }

    #[test]
    fn failed_delta_apply_changes_nothing() {
        let hash = Hash256([7; 32]);
        let good = StateDelta {
            schema_set_hash: hash,
            world: WorldId(1),
            base_version: WorldVersion(1),
            target_version: WorldVersion(2),
            operations: delta_ops(),
        }
        .encode()
        .unwrap();
        let mut cases: Vec<Vec<u8>> = Vec::new();
        // Truncated / malformed.
        cases.push(good[..good.len() - 5].to_vec());
        // Wrong base version.
        let mut bad = good.clone();
        bad[8 + 2 + 2 + 32 + 16..][..8].copy_from_slice(&7u64.to_le_bytes());
        cases.push(bad);
        // Non-advancing target version.
        let mut bad = good.clone();
        bad[8 + 2 + 2 + 32 + 16 + 8..][..8].copy_from_slice(&1u64.to_le_bytes());
        cases.push(bad);
        // Wrong world id.
        let mut bad = good.clone();
        bad[8 + 2 + 2 + 32] = 0xee;
        cases.push(bad);
        // Unsorted operations: valid header, hand-encoded reversed op list.
        let mut reversed_ops = delta_ops();
        reversed_ops.sort_by_key(DeltaOp::sort_key);
        reversed_ops.reverse();
        let mut reversed_body = Writer::new();
        for operation in &reversed_ops {
            operation.encode(&mut reversed_body).unwrap();
        }
        let header_len = 8 + 2 + 2 + 32 + 16 + 8 + 8 + 4;
        let mut unsorted = good[..header_len].to_vec();
        unsorted.extend_from_slice(&reversed_body.finish());
        assert!(StateDelta::decode(&unsorted).is_err());
        cases.push(unsorted);
        // Stale ownership epoch.
        let mut stale_ops = delta_ops();
        stale_ops.retain(|op| matches!(op, DeltaOp::PatchComponent { .. }));
        if let DeltaOp::PatchComponent { ownership, .. } = &mut stale_ops[0] {
            ownership.epoch = OwnershipEpoch(99);
        }
        cases.push(
            StateDelta {
                schema_set_hash: hash,
                world: WorldId(1),
                base_version: WorldVersion(1),
                target_version: WorldVersion(5),
                operations: stale_ops,
            }
            .encode()
            .unwrap(),
        );

        for (index, bytes) in cases.iter().enumerate() {
            let mut world = world_with_entity();
            let before = world.snapshot(hash).unwrap();
            assert!(
                world.apply_delta(hash, bytes).is_err(),
                "case {index} must fail"
            );
            assert_eq!(world.snapshot(hash).unwrap(), before, "case {index}");
        }
    }
}
