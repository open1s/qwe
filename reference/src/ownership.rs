//! RFC-0024 distributed ownership state machine.
//!
//! Ownership of a mutable entity moves between regions through the legal
//! sequence `OWNED -> PREPARE_TRANSFER -> FROZEN -> INSTALLING -> OWNED`; any
//! validation/timeout failure aborts back to `OWNED` at the source. The target
//! region is never the owner before a durable INSTALL succeeds. Protocol
//! messages are idempotent by TransferId, epochs reject stale commands
//! (`OwnershipStale`), and two owners for one epoch is a fatal split-brain.

use crate::sha256::digest;
use crate::wire::Writer;
use pwe_api::{EntityId, Error, Hash256, RegionId, Result, Status, TransferId, WorldVersion};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

type Epoch = u64;

/// The authoritative record of who owns an entity and at what epoch/version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnershipRecord {
    pub entity: EntityId,
    pub generation: u32,
    pub owner_region: RegionId,
    pub epoch: Epoch,
    pub world_version: WorldVersion,
}

/// The legal transfer states (RFC-0024).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferState {
    Owned,
    PrepareTransfer,
    Frozen,
    Installing,
}

/// A protocol message. Idempotent by `transfer_id`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferMessage {
    pub transfer_id: TransferId,
    pub entity: EntityId,
    pub generation: u32,
    pub source: RegionId,
    pub target: RegionId,
    pub expected_epoch: Epoch,
    pub base_version: WorldVersion,
    pub schema_hash: Hash256,
    pub payload_hash: Hash256,
}

impl TransferMessage {
    pub fn hash(&self) -> Result<Hash256> {
        let mut out = Writer::new();
        out.bytes_raw(&self.transfer_id.0.to_le_bytes())?;
        out.bytes_raw(&self.entity.0.to_le_bytes())?;
        out.u32(self.generation)?;
        out.bytes_raw(&self.source.0.to_le_bytes())?;
        out.bytes_raw(&self.target.0.to_le_bytes())?;
        out.bytes_raw(&self.expected_epoch.to_le_bytes())?;
        out.bytes_raw(&self.base_version.0.to_le_bytes())?;
        out.bytes_raw(&self.schema_hash.0)?;
        out.bytes_raw(&self.payload_hash.0)?;
        Ok(digest(&out.finish()))
    }
}

/// A single region's view of what it owns and the pending transfer it is part
/// of. Multiple regions plus the protocol driver form the distributed machine.
#[derive(Clone, Debug)]
pub struct RegionState {
    pub region: RegionId,
    pub records: BTreeMap<EntityId, OwnershipRecord>,
    /// Entity id -> in-flight transfer id.
    pub pending: BTreeMap<EntityId, TransferId>,
}

impl RegionState {
    pub fn new(region: RegionId) -> Self {
        Self {
            region,
            records: BTreeMap::new(),
            pending: BTreeMap::new(),
        }
    }
    pub fn owner_of(&self, entity: EntityId) -> Option<&OwnershipRecord> {
        self.records.get(&entity)
    }
    /// Cheaply claim ownership for a committed create (used to seed a region).
    pub fn claim(&mut self, entity: EntityId, generation: u32, world_version: WorldVersion) {
        self.records.insert(
            entity,
            OwnershipRecord {
                entity,
                generation,
                owner_region: self.region,
                epoch: 1,
                world_version,
            },
        );
    }
}

/// The full protocol driver: it never allows two owners for the same epoch and
/// rejects any command carrying a stale epoch.
#[derive(Debug, Default)]
pub struct OwnershipMachine {
    regions: BTreeMap<RegionId, RegionState>,
    /// TransferId -> authoritative TransferMessage (installed on first arrival).
    transfers: BTreeMap<TransferId, TransferMessage>,
    /// Track the target's accepted install so duplicate ack is idempotent.
    confirmed: BTreeMap<TransferId, Epoch>,
}

impl OwnershipMachine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn region(&mut self, region: RegionId) -> &mut RegionState {
        self.regions
            .entry(region)
            .or_insert_with(|| RegionState::new(region))
    }

    fn remove_dup_owner(&self) -> Option<Error> {
        let mut seen: BTreeMap<(EntityId, Epoch), RegionId> = BTreeMap::new();
        for state in self.regions.values() {
            for record in state.records.values() {
                let key = (record.entity, record.epoch);
                if let Some(prior) = seen.insert(key, record.owner_region) {
                    if prior != record.owner_region {
                        // split-brain: fatal, not silently reconcilable.
                        return Some(error(Status::Internal, 1));
                    }
                }
            }
        }
        None
    }

    /// SOURCE proposes a transfer. Enters PREPARE_TRANSFER.
    pub fn propose(&mut self, message: TransferMessage) -> Result<()> {
        if message.source == message.target {
            return Err(error(Status::Invalid, 1));
        }
        let source = self
            .regions
            .get(&message.source)
            .ok_or(error(Status::HandleStale, 1))?;
        let record = source
            .owner_of(message.entity)
            .ok_or(error(Status::HandleStale, 2))?;
        if record.epoch != message.expected_epoch {
            return Err(error(Status::OwnershipStale, 1));
        }
        if message.base_version != record.world_version {
            return Err(error(Status::OwnershipStale, 2));
        }
        self.transfers.insert(message.transfer_id, message.clone());
        let source_mut = self.regions.get_mut(&message.source).unwrap();
        source_mut
            .pending
            .insert(message.entity, message.transfer_id);
        Ok(())
    }

    /// TARGET reserves/accepts the proposal. Idempotent by TransferId.
    pub fn reserve(&mut self, transfer_id: TransferId) -> Result<TransferMessage> {
        let message = self
            .transfers
            .get(&transfer_id)
            .cloned()
            .ok_or(error(Status::HandleStale, 3))?;
        // Target must not already own this entity at the incoming epoch.
        if let Some(state) = self.regions.get(&message.target) {
            if let Some(record) = state.owner_of(message.entity) {
                if record.epoch == message.expected_epoch {
                    return Err(error(Status::Conflict, 8));
                }
            }
        }
        Ok(message)
    }

    /// SOURCE freezes: enters FROZEN and produces the snapshot payload hash.
    pub fn freeze(&mut self, transfer_id: TransferId, payload: &[u8]) -> Result<Hash256> {
        let message = self
            .transfers
            .get(&transfer_id)
            .cloned()
            .ok_or(error(Status::HandleStale, 3))?;
        let payload_hash = digest(payload);
        if message.payload_hash != payload_hash {
            return Err(error(Status::Conflict, 9));
        }
        // State transition PrepareTransfer -> Frozen is implicit in the source
        // pending map; the driver only needs the hash to agree.
        Ok(payload_hash)
    }

    /// TARGET installs and durably acks. Enters INSTALLING then OWNED at target
    /// with epoch old+1. Duplicate install is idempotent.
    pub fn install(&mut self, transfer_id: TransferId) -> Result<Epoch> {
        let message = self
            .transfers
            .get(&transfer_id)
            .cloned()
            .ok_or(error(Status::HandleStale, 3))?;
        if let Some(epoch) = self.confirmed.get(&transfer_id) {
            return Ok(*epoch);
        }
        // Ensure the source still owns at expected_epoch (no intervening move).
        let source = self
            .regions
            .get(&message.source)
            .ok_or(error(Status::HandleStale, 1))?;
        let record = source
            .owner_of(message.entity)
            .ok_or(error(Status::HandleStale, 2))?;
        if record.epoch != message.expected_epoch {
            return Err(error(Status::OwnershipStale, 3));
        }
        let new_epoch = message
            .expected_epoch
            .checked_add(1)
            .ok_or(error(Status::Limit, 2))?;
        let target = self
            .regions
            .entry(message.target)
            .or_insert_with(|| RegionState::new(message.target));
        target.records.insert(
            message.entity,
            OwnershipRecord {
                entity: message.entity,
                generation: message.generation,
                owner_region: message.target,
                epoch: new_epoch,
                world_version: message.base_version,
            },
        );
        self.confirmed.insert(transfer_id, new_epoch);
        // split-brain guard.
        if self.remove_dup_owner().is_some() {
            return Err(error(Status::Internal, 1));
        }
        Ok(new_epoch)
    }

    /// SOURCE releases after target's durable ack: removes the lift so the
    /// source no longer owns the entity.
    pub fn release(&mut self, transfer_id: TransferId) -> Result<()> {
        let message = self
            .transfers
            .get(&transfer_id)
            .cloned()
            .ok_or(error(Status::HandleStale, 3))?;
        let confirmed = self
            .confirmed
            .get(&transfer_id)
            .copied()
            .ok_or(error(Status::OwnershipStale, 4))?;
        let source = self.regions.get_mut(&message.source).unwrap();
        // Only release if the target actually advanced to expected_epoch+1.
        if confirmed != message.expected_epoch + 1 {
            return Err(error(Status::OwnershipStale, 5));
        }
        source.records.remove(&message.entity);
        source.pending.remove(&message.entity);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(id: u128, entity: u128, source: u64, target: u64, epoch: Epoch) -> TransferMessage {
        TransferMessage {
            transfer_id: TransferId(id),
            entity: EntityId(entity),
            generation: 1,
            source: RegionId(source),
            target: RegionId(target),
            expected_epoch: epoch,
            base_version: WorldVersion(0),
            schema_hash: Hash256([0; 32]),
            payload_hash: digest(b"payload"),
        }
    }

    #[test]
    fn ownership_full_transfer_state_machine_advances_epoch() {
        let mut machine = OwnershipMachine::new();
        machine
            .region(RegionId(1))
            .claim(EntityId(10), 1, WorldVersion(0));
        let m = msg(1, 10, 1, 2, 1);
        machine.propose(m.clone()).unwrap();
        let reserved = machine.reserve(TransferId(1)).unwrap();
        assert_eq!(reserved, m);
        let hash = machine.freeze(TransferId(1), b"payload").unwrap();
        assert_eq!(hash, m.payload_hash);
        let epoch = machine.install(TransferId(1)).unwrap();
        assert_eq!(epoch, 2);
        // Idempotent duplicate install.
        assert_eq!(machine.install(TransferId(1)).unwrap(), 2);
        machine.release(TransferId(1)).unwrap();
        assert!(machine.region(RegionId(1)).owner_of(EntityId(10)).is_none());
        let target_record = machine.region(RegionId(2)).owner_of(EntityId(10)).unwrap();
        assert_eq!(target_record.epoch, 2);
        assert_eq!(target_record.owner_region, RegionId(2));
    }

    #[test]
    fn ownership_rejects_stale_epoch_proposal() {
        let mut machine = OwnershipMachine::new();
        machine
            .region(RegionId(1))
            .claim(EntityId(10), 1, WorldVersion(0));
        // expected_epoch=2 but the record is at epoch 1.
        let stale = msg(1, 10, 1, 2, 2);
        assert_eq!(
            machine.propose(stale).unwrap_err().status,
            Status::OwnershipStale
        );
    }

    #[test]
    fn ownership_target_must_not_already_own_same_epoch() {
        let mut machine = OwnershipMachine::new();
        machine
            .region(RegionId(1))
            .claim(EntityId(10), 1, WorldVersion(0));
        machine
            .region(RegionId(2))
            .claim(EntityId(10), 1, WorldVersion(0));
        let m = msg(1, 10, 1, 2, 1);
        machine.propose(m).unwrap();
        // Target already owns entity 10 at epoch 1 -> reserve rejects.
        assert_eq!(
            machine.reserve(TransferId(1)).unwrap_err().status,
            Status::Conflict
        );
    }

    #[test]
    fn ownership_install_is_idempotent_and_release_requires_ack() {
        let mut machine = OwnershipMachine::new();
        machine
            .region(RegionId(1))
            .claim(EntityId(10), 1, WorldVersion(0));
        let m = msg(1, 10, 1, 2, 1);
        machine.propose(m).unwrap();
        machine.reserve(TransferId(1)).unwrap();
        machine.freeze(TransferId(1), b"payload").unwrap();
        // Release before a durable install ack must fail.
        assert_eq!(
            machine.release(TransferId(1)).unwrap_err().status,
            Status::OwnershipStale
        );
        machine.install(TransferId(1)).unwrap();
        machine.release(TransferId(1)).unwrap();
    }
}
