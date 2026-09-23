//! Deterministic in-memory semantic reference for RFC-0023 and RFC-0024.

pub use pwe_api;
pub mod aot;
pub mod artifact;
pub mod broadphase;
pub mod capability;
pub mod channel;
pub mod cluster;
pub mod components;
pub mod continuum;
pub mod distributed;
pub mod domain_ir;
pub mod dominance;
pub mod dsl;
pub mod eir;
pub mod extension;
pub mod fence;
pub mod field;
pub mod jit;
pub mod lang;
pub mod math;
pub mod module;
pub mod ownership;
pub mod physics;
pub mod physics_eir;
pub mod physics_ir;
pub mod present;
pub mod render;
pub mod scene;
pub mod scheduler;
pub mod schema;
pub mod sensor_eir;
pub mod sha256;
pub mod simulation;
pub mod snapshot;
pub mod units;
pub mod wir;
pub mod wire;

use crate::schema::{Schema, SchemaRegistry};
use pwe_api::resource::ResourceId;
use pwe_api::time::SimTime;
use pwe_api::{
    Access, CapabilityClaims, Commit, ComponentDescriptor, ComponentTypeId, ConflictPolicy,
    EntityRef, Error, Hash256, Ownership, Result, Status, TransactionOptions, TransferId,
    TransferProposal, WorldId, WorldVersion,
};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// The limits this reference enforces (RFC-0034). They equal the defaults.
pub const ADVERTISED_LIMITS: pwe_api::limits::Limits = pwe_api::limits::DEFAULT_LIMITS;

#[derive(Debug)]
pub struct ReferenceWorld {
    pub(crate) id: WorldId,
    pub(crate) version: WorldVersion,
    pub(crate) sim_time: SimTime,
    pub(crate) entities: BTreeMap<EntityRef, Ownership>,
    pub(crate) components: BTreeMap<(EntityRef, ComponentTypeId), ComponentState>,
    pub(crate) resource_refs: BTreeMap<ResourceId, ResourceRef>,
    /// Committed events, append-only (RFC-0023 ordered events after success).
    pub(crate) events: Vec<WorldEvent>,
    /// Registered associative+commutative merges (RFC-0023 CommutativeMerge).
    pub(crate) merges: BTreeMap<ComponentTypeId, MergeFn>,
    /// Registered schemas used to validate component descriptors (RFC-0031).
    pub(crate) registry: SchemaRegistry,
    transfers: BTreeMap<TransferId, TransferProposal>,
}

/// A reference a world holds to a content-addressed resource (RFC-0025 header
/// resource refs). The authoritative bytes live in a resource store; a world
/// references them by id and their content/schema hashes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceRef {
    pub content_hash: Hash256,
    pub schema_hash: Hash256,
}

/// RFC-0023 transaction lifecycle. `OPEN -> PREPARED -> VALIDATED ->
/// COMMITTED | ABORTED`; no other transition exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxnState {
    Open,
    Prepared,
    Validated,
    Committed,
    Aborted,
}

/// An ordered event emitted by a transaction. It is committed only on success
/// (RFC-0023 "events appear only after success").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldEvent {
    pub world_version: WorldVersion,
    pub kind: u32,
    pub payload: u64,
}

/// A registered associative+commutative scalar merge for a component type.
/// Used by `CommutativeMerge` (and optionally `Merge`).
pub type MergeFn = fn(u64, u64) -> u64;

#[derive(Clone, Debug)]
pub(crate) struct ComponentState {
    pub(crate) descriptor: ComponentDescriptor,
    pub(crate) bytes: Vec<u8>,
}

impl ReferenceWorld {
    pub fn new(id: WorldId) -> Self {
        Self {
            id,
            version: WorldVersion(0),
            sim_time: SimTime::ZERO,
            entities: BTreeMap::new(),
            components: BTreeMap::new(),
            resource_refs: BTreeMap::new(),
            events: Vec::new(),
            merges: BTreeMap::new(),
            registry: SchemaRegistry::default(),
            transfers: BTreeMap::new(),
        }
    }
    /// Registers schemas against which component descriptors are validated
    /// (RFC-0031). Registration is idempotent; a conflicting hash for the same
    /// type is rejected.
    pub fn register_schemas(&mut self, schemas: Vec<Schema>) -> Result<()> {
        for schema in schemas {
            self.registry.register(schema)?;
        }
        Ok(())
    }
    /// RFC-0031: when the world has a schema registered for the descriptor's
    /// type, enforce `value_size == schema_size` and the schema hash. Unknown
    /// types fall back to the base descriptor validation only.
    pub fn validate_descriptor(&self, descriptor: &ComponentDescriptor) -> Result<()> {
        if self.registry.get(descriptor.type_id).is_some() {
            self.registry.validate_descriptor(descriptor)
        } else {
            descriptor.validate().map(|_| ())
        }
    }
    pub fn events(&self) -> &[WorldEvent] {
        &self.events
    }
    /// Registers an associative+commutative scalar merge for a component type
    /// (RFC-0023 `CommutativeMerge`).
    pub fn register_merge(&mut self, type_id: ComponentTypeId, f: MergeFn) {
        self.merges.insert(type_id, f);
    }
    pub fn id(&self) -> WorldId {
        self.id
    }
    pub fn version(&self) -> WorldVersion {
        self.version
    }
    pub fn sim_time(&self) -> SimTime {
        self.sim_time
    }
    /// Advances the world clock. This is authoritative state; normal system
    /// writes use a transaction, but the simulation clock is advanced by the
    /// scheduler step.
    pub fn advance_time(&mut self, sim_time: SimTime) {
        self.sim_time = sim_time;
    }
    pub fn resource_refs(&self) -> &BTreeMap<ResourceId, ResourceRef> {
        &self.resource_refs
    }
    /// Records a content-addressed resource reference (RFC-0025 header).
    pub fn set_resource_ref(&mut self, id: ResourceId, reference: ResourceRef) {
        self.resource_refs.insert(id, reference);
    }
    pub fn ownership(&self, entity: EntityRef) -> Result<Ownership> {
        self.entities
            .get(&entity)
            .copied()
            .ok_or(error(Status::HandleStale, 1))
    }
    pub fn component_bytes(&self, entity: EntityRef, component: ComponentTypeId) -> Result<&[u8]> {
        self.components
            .get(&(entity, component))
            .map(|state| state.bytes.as_slice())
            .ok_or(error(Status::HandleStale, 3))
    }
    /// PREPARE_TRANSFER: freezes later transfer attempts for this entity.
    pub fn prepare_transfer(&mut self, proposal: TransferProposal) -> Result<()> {
        if proposal.base_version != self.version || proposal.target == proposal.source.region {
            return Err(error(Status::Invalid, 5));
        }
        if self.ownership(proposal.entity)? != proposal.source {
            return Err(error(Status::OwnershipStale, 3));
        }
        if let Some(existing) = self.transfers.get(&proposal.id) {
            return if existing == &proposal {
                Ok(())
            } else {
                Err(error(Status::Conflict, 6))
            };
        }
        if self
            .transfers
            .values()
            .any(|pending| pending.entity == proposal.entity)
        {
            return Err(error(Status::Conflict, 7));
        }
        self.transfers.insert(proposal.id, proposal);
        Ok(())
    }
    /// INSTALLING -> OWNED: confirmation advances epoch exactly once; duplicates are idempotent.
    pub fn confirm_transfer(&mut self, id: TransferId) -> Result<Ownership> {
        let proposal = *self
            .transfers
            .get(&id)
            .ok_or(error(Status::HandleStale, 5))?;
        let current = self.ownership(proposal.entity)?;
        if current.region == proposal.target && current.epoch.0 == proposal.source.epoch.0 + 1 {
            return Ok(current);
        }
        if current != proposal.source {
            return Err(error(Status::OwnershipStale, 4));
        }
        let ownership = Ownership {
            region: proposal.target,
            epoch: pwe_api::OwnershipEpoch(
                proposal
                    .source
                    .epoch
                    .0
                    .checked_add(1)
                    .ok_or(error(Status::Limit, 1))?,
            ),
        };
        self.entities.insert(proposal.entity, ownership);
        self.version.0 += 1;
        Ok(ownership)
    }
    /// Token verification happens outside the runtime; this checks bounded claims.
    pub fn begin(
        &mut self,
        options: TransactionOptions,
        capability: CapabilityClaims,
    ) -> Result<ReferenceTransaction<'_>> {
        // Claims verify before any transaction is created (RFC-0033).
        capability::enforce_claims(&capability, &options, self.id, self.version)?;
        Ok(ReferenceTransaction {
            world: self,
            options,
            state: TxnState::Open,
            creates: Vec::new(),
            destroys: Vec::new(),
            reads: Vec::new(),
            writes: BTreeMap::new(),
            events: Vec::new(),
            capability,
        })
    }
}

#[derive(Debug)]
pub struct ReferenceTransaction<'a> {
    world: &'a mut ReferenceWorld,
    options: TransactionOptions,
    state: TxnState,
    creates: Vec<EntityRef>,
    destroys: Vec<EntityRef>,
    reads: Vec<(EntityRef, ComponentTypeId)>,
    writes: BTreeMap<(EntityRef, ComponentTypeId), ComponentState>,
    events: Vec<WorldEvent>,
    capability: CapabilityClaims,
}

impl ReferenceTransaction<'_> {
    pub fn create(&mut self, entity: EntityRef) -> Result<()> {
        if !self.capability.access.contains(Access::CREATE) {
            return Err(error(Status::Capability, 2));
        }
        if self.state != TxnState::Open || entity.generation == 0 {
            return Err(error(Status::Invalid, 1));
        }
        if self.world.entities.contains_key(&entity) || self.creates.contains(&entity) {
            return Err(error(Status::Conflict, 2));
        }
        self.creates.push(entity);
        Ok(())
    }
    pub fn destroy(&mut self, entity: EntityRef) -> Result<()> {
        if !self.capability.access.contains(Access::DESTROY) {
            return Err(error(Status::Capability, 3));
        }
        if self.state != TxnState::Open || !self.world.entities.contains_key(&entity) {
            return Err(error(Status::HandleStale, 2));
        }
        if self.destroys.contains(&entity) {
            return Err(error(Status::Conflict, 3));
        }
        self.destroys.push(entity);
        Ok(())
    }
    /// Reads a component value, recording it in the read set (RFC-0023) for
    /// read/version conflict validation.
    pub fn read_component(&mut self, entity: EntityRef, type_id: ComponentTypeId) -> Result<&[u8]> {
        if self.state != TxnState::Open {
            return Err(error(Status::Invalid, 7));
        }
        let bytes = self.world.component_bytes(entity, type_id)?;
        self.reads.push((entity, type_id));
        Ok(bytes)
    }
    pub fn put_component(
        &mut self,
        entity: EntityRef,
        descriptor: ComponentDescriptor,
        bytes: &[u8],
    ) -> Result<()> {
        if !self.capability.access.contains(Access::WRITE) {
            return Err(error(Status::Capability, 4));
        }
        if self.state != TxnState::Open {
            return Err(error(Status::Invalid, 3));
        }
        self.world.validate_descriptor(&descriptor)?;
        if bytes.len() != descriptor.value_size as usize {
            return Err(error(Status::Invalid, 4));
        }
        if !self.world.entities.contains_key(&entity) && !self.creates.contains(&entity) {
            return Err(error(Status::HandleStale, 4));
        }
        if self.destroys.contains(&entity) {
            return Err(error(Status::Conflict, 5));
        }
        self.writes.insert(
            (entity, descriptor.type_id),
            ComponentState {
                descriptor,
                bytes: bytes.to_vec(),
            },
        );
        Ok(())
    }
    /// Emits an ordered event; it is committed only on successful commit.
    pub fn emit(&mut self, kind: u32, payload: u64) -> Result<()> {
        if !self.capability.access.contains(Access::EMIT) {
            return Err(error(Status::Capability, 6));
        }
        if self.state != TxnState::Open {
            return Err(error(Status::Invalid, 6));
        }
        self.events.push(WorldEvent {
            world_version: self.world.version,
            kind,
            payload,
        });
        Ok(())
    }
    pub fn commit(mut self) -> Result<Commit> {
        if self.state != TxnState::Open {
            return Err(error(Status::Invalid, 2));
        }
        self.state = TxnState::Prepared;
        let stale = self.options.base_version != self.world.version;
        for entity in &self.destroys {
            let ownership = self.world.ownership(*entity)?;
            if ownership != self.options.ownership {
                return Err(error(Status::OwnershipStale, 1));
            }
        }
        for ((entity, _), state) in &self.writes {
            state.descriptor.validate()?;
            if !self.creates.contains(entity)
                && self.world.ownership(*entity)? != self.options.ownership
            {
                return Err(error(Status::OwnershipStale, 2));
            }
        }
        self.state = TxnState::Validated;

        // Resolve a stale base version (a concurrent commit landed) per policy.
        if stale {
            match self.options.policy {
                ConflictPolicy::Reject => return Err(error(Status::Conflict, 4)),
                ConflictPolicy::LastWriterByPriority => {}
                ConflictPolicy::Merge | ConflictPolicy::CommutativeMerge => {
                    for (key, state) in &mut self.writes {
                        if let Some(committed) = self.world.components.get(key) {
                            state.bytes = merge_value(
                                self.options.policy,
                                &*self.world,
                                state,
                                &committed.bytes,
                            )?;
                        }
                    }
                }
            }
        }

        self.creates.sort();
        self.destroys.sort();
        for entity in &self.destroys {
            self.world.entities.remove(entity);
            self.world
                .components
                .retain(|(component_entity, _), _| component_entity != entity);
        }
        for entity in &self.creates {
            self.world.entities.insert(*entity, self.options.ownership);
        }
        for (key, state) in &self.writes {
            self.world.components.insert(*key, state.clone());
        }
        self.world.version.0 += 1;
        // Events appear only after success, stamped with the new version.
        for event in &self.events {
            self.world.events.push(WorldEvent {
                world_version: self.world.version,
                kind: event.kind,
                payload: event.payload,
            });
        }
        self.state = TxnState::Committed;
        Ok(Commit {
            world_version: self.world.version,
        })
    }
    pub fn abort(mut self) {
        self.state = TxnState::Aborted;
    }
}

/// Merges a new write value against the currently committed value per policy.
fn merge_value(
    policy: ConflictPolicy,
    world: &ReferenceWorld,
    new: &ComponentState,
    committed: &[u8],
) -> Result<Vec<u8>> {
    let a = value_from_bytes(committed);
    let b = value_from_bytes(&new.bytes);
    let len = new.bytes.len();
    match policy {
        ConflictPolicy::Merge => match world.merges.get(&new.descriptor.type_id) {
            Some(f) => Ok(value_into_bytes(f(a, b), len)),
            None => Ok(new.bytes.clone()),
        },
        ConflictPolicy::CommutativeMerge => match world.merges.get(&new.descriptor.type_id) {
            Some(f) => Ok(value_into_bytes(f(a, b), len)),
            None => Err(error(Status::SchemaUnsupported, 5)),
        },
        _ => Ok(new.bytes.clone()),
    }
}

fn value_from_bytes(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(buf)
}

fn value_into_bytes(value: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let n = len.min(8);
    out[..n].copy_from_slice(&value.to_le_bytes()[..n]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::{ConflictPolicy, EntityId, Hash256, OwnershipEpoch, RegionId, TransferId};
    fn entity(n: u128) -> EntityRef {
        EntityRef {
            id: EntityId(n),
            generation: 1,
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
    const COMPONENT: ComponentDescriptor = ComponentDescriptor {
        type_id: ComponentTypeId([9; 16]),
        schema_hash: pwe_api::Hash256([3; 32]),
        abi_major: 2,
        flags: 0,
        value_size: 4,
        value_align: 4,
    };
    #[test]
    fn commit_is_atomic_and_ordered() {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world
            .begin(options(0), capability(Access::CREATE.union(Access::WRITE)))
            .unwrap();
        tx.create(entity(2)).unwrap();
        tx.create(entity(1)).unwrap();
        assert_eq!(tx.commit().unwrap().world_version, WorldVersion(1));
        assert_eq!(world.ownership(entity(1)).unwrap().region, RegionId(7));
    }
    #[test]
    fn empty_capability_and_stale_version_fail_closed() {
        let mut world = ReferenceWorld::new(WorldId(1));
        assert_eq!(
            world
                .begin(
                    options(0),
                    CapabilityClaims {
                        world: WorldId(99),
                        ..capability(Access::CREATE)
                    }
                )
                .unwrap_err()
                .status,
            Status::Capability
        );
        let mut tx = world
            .begin(options(0), capability(Access::CREATE.union(Access::WRITE)))
            .unwrap();
        tx.create(entity(1)).unwrap();
        tx.commit().unwrap();
        // A transaction begun at a stale base version still fails closed under
        // the Reject policy, but at commit (RFC-0023 VALIDATED) rather than begin.
        let mut tx = world.begin(options(0), capability(Access::CREATE)).unwrap();
        tx.create(entity(2)).unwrap();
        assert_eq!(tx.commit().unwrap_err().status, Status::Conflict);
    }
    #[test]
    fn component_writes_are_invisible_until_atomic_commit() {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world
            .begin(options(0), capability(Access::CREATE.union(Access::WRITE)))
            .unwrap();
        tx.create(entity(1)).unwrap();
        tx.put_component(entity(1), COMPONENT, &[1, 2, 3, 4])
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            world.component_bytes(entity(1), COMPONENT.type_id).unwrap(),
            &[1, 2, 3, 4]
        );
        let mut tx = world
            .begin(options(1), capability(Access::DESTROY))
            .unwrap();
        tx.destroy(entity(1)).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            world
                .component_bytes(entity(1), COMPONENT.type_id)
                .unwrap_err()
                .status,
            Status::HandleStale
        );
    }
    #[test]
    fn transfer_advances_epoch_once_and_duplicate_confirmation_is_idempotent() {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world.begin(options(0), capability(Access::CREATE)).unwrap();
        tx.create(entity(1)).unwrap();
        tx.commit().unwrap();
        let proposal = TransferProposal {
            id: TransferId(99),
            entity: entity(1),
            source: options(1).ownership,
            target: RegionId(8),
            base_version: WorldVersion(1),
        };
        world.prepare_transfer(proposal).unwrap();
        let installed = world.confirm_transfer(TransferId(99)).unwrap();
        assert_eq!(
            installed,
            Ownership {
                region: RegionId(8),
                epoch: OwnershipEpoch(2)
            }
        );
        assert_eq!(world.confirm_transfer(TransferId(99)).unwrap(), installed);
        assert_eq!(world.version(), WorldVersion(2));
    }

    fn options_with(version: u64, policy: ConflictPolicy) -> TransactionOptions {
        TransactionOptions {
            base_version: WorldVersion(version),
            ownership: options(version).ownership,
            policy,
        }
    }

    fn world_with_entity_value(value: [u8; 4]) -> ReferenceWorld {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world
            .begin(options(0), capability(Access::CREATE.union(Access::WRITE)))
            .unwrap();
        tx.create(entity(1)).unwrap();
        tx.put_component(entity(1), COMPONENT, &value).unwrap();
        tx.commit().unwrap();
        world
    }

    #[test]
    fn conflict_policy_reject_rejects_stale_base() {
        let mut world = world_with_entity_value([1, 0, 0, 0]);
        let mut tx = world
            .begin(
                options_with(0, ConflictPolicy::Reject),
                capability(Access::WRITE),
            )
            .unwrap();
        tx.put_component(entity(1), COMPONENT, &[2, 0, 0, 0])
            .unwrap();
        assert_eq!(tx.commit().unwrap_err().status, Status::Conflict);
        assert_eq!(world.version(), WorldVersion(1));
    }

    #[test]
    fn conflict_policy_last_writer_wins() {
        let mut world = world_with_entity_value([1, 0, 0, 0]);
        let mut tx = world
            .begin(
                options_with(0, ConflictPolicy::LastWriterByPriority),
                capability(Access::WRITE),
            )
            .unwrap();
        tx.put_component(entity(1), COMPONENT, &[2, 0, 0, 0])
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            world.component_bytes(entity(1), COMPONENT.type_id).unwrap(),
            &[2, 0, 0, 0]
        );
    }

    #[test]
    fn conflict_policy_commutative_merge_uses_registered_fn() {
        let mut world = world_with_entity_value([1, 0, 0, 0]);
        world.register_merge(COMPONENT.type_id, |a, b| a.wrapping_add(b));
        let mut tx = world
            .begin(
                options_with(0, ConflictPolicy::CommutativeMerge),
                capability(Access::WRITE),
            )
            .unwrap();
        tx.put_component(entity(1), COMPONENT, &[2, 0, 0, 0])
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            world.component_bytes(entity(1), COMPONENT.type_id).unwrap(),
            &[3, 0, 0, 0]
        );
    }

    #[test]
    fn commutative_merge_requires_registered_fn() {
        let mut world = world_with_entity_value([1, 0, 0, 0]);
        let mut tx = world
            .begin(
                options_with(0, ConflictPolicy::CommutativeMerge),
                capability(Access::WRITE),
            )
            .unwrap();
        tx.put_component(entity(1), COMPONENT, &[2, 0, 0, 0])
            .unwrap();
        assert_eq!(tx.commit().unwrap_err().status, Status::SchemaUnsupported);
    }

    #[test]
    fn events_commit_only_after_success() {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut tx = world.begin(options(0), capability(Access::EMIT)).unwrap();
        tx.emit(1, 100).unwrap();
        tx.abort();
        assert!(world.events().is_empty());

        let mut tx = world.begin(options(0), capability(Access::EMIT)).unwrap();
        tx.emit(1, 200).unwrap();
        tx.commit().unwrap();
        assert_eq!(world.events().len(), 1);
        assert_eq!(world.events()[0].world_version, WorldVersion(1));
        assert_eq!(world.events()[0].payload, 200);
    }

    #[test]
    fn read_component_records_read_set() {
        let mut world = world_with_entity_value([7, 0, 0, 0]);
        let mut tx = world.begin(options(1), capability(Access::WRITE)).unwrap();
        let bytes = tx.read_component(entity(1), COMPONENT.type_id).unwrap();
        assert_eq!(bytes, &[7, 0, 0, 0]);
        assert_eq!(tx.reads.len(), 1);
    }
}
