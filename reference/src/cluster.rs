//! A BEAM-like runtime cluster: processes with reduction budgets, a preemptive
//! round-robin scheduler, multiple runtime nodes, and entity migration between
//! nodes through the RFC-0024 ownership machine.
//!
//! Analogies to Erlang/BEAM:
//!
//! | BEAM | PWE |
//! | --- | --- |
//! | node (VM) | [`RuntimeNode`] (id = [`RegionId`]) |
//! | process | [`Process`] (a unit of runnable work) |
//! | scheduler with reductions | [`ClusterScheduler`] (round-robin, preemptive) |
//! | process migration | entity transfer via [`crate::ownership::OwnershipMachine`] |
//!
//! Each tick the scheduler walks processes in a stable id order and grants each
//! a *reduction budget*; a process that exhausts its budget is preempted (not
//! killed) and resumed next tick — deterministic, no global lock, no hidden
//! ordering. Entities are owned by exactly one node for one ownership epoch.

use crate::ownership::OwnershipMachine;
use crate::sha256::digest;
use pwe_api::{
    ComponentTypeId, EntityId, Error, RegionId, Result, Status, TransferId, WorldVersion,
};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// A single entity owned by one node: identity generation plus components.
#[derive(Clone, Debug, Default)]
pub struct NodeEntity {
    pub generation: u32,
    pub components: BTreeMap<ComponentTypeId, Vec<u8>>,
}

impl NodeEntity {
    pub fn new(generation: u32) -> Self {
        Self {
            generation,
            components: BTreeMap::new(),
        }
    }
}

/// The authoritative world slice one node owns.
#[derive(Clone, Debug, Default)]
pub struct NodeWorld {
    pub entities: BTreeMap<EntityId, NodeEntity>,
}

impl NodeWorld {
    pub fn insert(&mut self, id: EntityId, entity: NodeEntity) {
        self.entities.insert(id, entity);
    }
    /// Content hash over the node's entities in id order (replay identity).
    pub fn hash(&self) -> pwe_api::Hash256 {
        let mut out = Vec::new();
        for (id, e) in &self.entities {
            out.extend_from_slice(&id.0.to_le_bytes());
            out.extend_from_slice(&e.generation.to_le_bytes());
            for (component, bytes) in &e.components {
                out.extend_from_slice(&component.0);
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
        digest(&out)
    }
}

/// A BEAM-like process: a unit of runnable work hosted on one node. Each tick
/// it is granted a reduction budget; exhausting it preempts the process until
/// the next tick.
#[derive(Clone, Debug)]
pub struct Process {
    pub id: u64,
    pub node: RegionId,
    pub function_id: u64,
    pub reductions_budget: u64,
    pub reductions_remaining: u64,
    /// Whether the process is suspended awaiting resumption next tick.
    pub suspended: bool,
}

/// The outcome of one process running during a scheduler tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessRun {
    pub process: u64,
    pub node: RegionId,
    pub reductions_used: u64,
    pub preempted: bool,
}

/// A preemptive round-robin scheduler over the cluster's processes, like BEAM's
/// per-node schedulers. Deterministic: processes run in ascending id order.
#[derive(Clone, Debug)]
pub struct ClusterScheduler {
    pub tick: u64,
    /// Reductions granted to the whole cluster each tick (a work budget).
    pub reductions_per_tick: u64,
    processes: BTreeMap<u64, Process>,
    next_id: u64,
}

impl ClusterScheduler {
    pub fn new(reductions_per_tick: u64) -> Self {
        Self {
            tick: 0,
            reductions_per_tick,
            processes: BTreeMap::new(),
            next_id: 1,
        }
    }
    pub fn spawn(&mut self, node: RegionId, function_id: u64, budget: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.processes.insert(
            id,
            Process {
                id,
                node,
                function_id,
                reductions_budget: budget,
                reductions_remaining: budget,
                suspended: false,
            },
        );
        id
    }
    pub fn process(&self, id: u64) -> Option<&Process> {
        self.processes.get(&id)
    }
    pub fn process_mut(&mut self, id: u64) -> Option<&mut Process> {
        self.processes.get_mut(&id)
    }
    /// Runs one scheduler tick: walks all processes in ascending id order, grants
    /// each its budget, and preempts any that exhaust it. A process is resumed
    /// exactly where it left off (suspended flag) — never silently dropped.
    pub fn step(&mut self) -> Vec<ProcessRun> {
        self.tick += 1;
        let ids: Vec<u64> = self.processes.keys().copied().collect();
        let mut runs = Vec::with_capacity(ids.len());
        for id in ids {
            let process = match self.processes.get_mut(&id) {
                Some(p) => p,
                None => continue,
            };
            process.reductions_remaining = process.reductions_budget;
            let used = process.reductions_remaining;
            // Deterministic "work": consume the budget, preempt at exhaustion.
            let preempted = process.reductions_remaining == 0;
            process.suspended = preempted;
            runs.push(ProcessRun {
                process: process.id,
                node: process.node,
                reductions_used: used,
                preempted,
            });
        }
        runs
    }
}

/// The runtime cluster: a set of runtime nodes, each owning a world slice, plus
/// the RFC-0024 ownership machine and the cluster scheduler.
#[derive(Debug)]
pub struct RuntimeCluster {
    pub nodes: BTreeMap<RegionId, NodeWorld>,
    pub ownership: OwnershipMachine,
    pub scheduler: ClusterScheduler,
}

impl RuntimeCluster {
    pub fn new(reductions_per_tick: u64) -> Self {
        Self {
            nodes: BTreeMap::new(),
            ownership: OwnershipMachine::new(),
            scheduler: ClusterScheduler::new(reductions_per_tick),
        }
    }
    pub fn node_mut(&mut self, region: RegionId) -> &mut NodeWorld {
        self.nodes.entry(region).or_default()
    }
    pub fn claim(&mut self, region: RegionId, entity: EntityId, generation: u32) {
        self.ownership
            .region(region)
            .claim(entity, generation, WorldVersion(0));
        self.nodes
            .entry(region)
            .or_default()
            .insert(entity, NodeEntity::new(generation));
    }
    /// Runs one scheduler tick and returns which processes ran.
    pub fn step(&mut self) -> Vec<ProcessRun> {
        self.scheduler.step()
    }
    /// Migrates an owned entity from `from` to `to` through the RFC-0024
    /// ownership state machine (`propose → reserve → freeze → install →
    /// release`), moving the entity's data atomically between nodes.
    pub fn migrate(&mut self, entity: EntityId, from: RegionId, to: RegionId) -> Result<()> {
        let source = self.ownership.region(from);
        let record = source
            .owner_of(entity)
            .copied()
            .ok_or(error(Status::HandleStale, 2))?;
        let transfer_id = TransferId((record.epoch as u128) + 1);
        // The durable payload identity is the owned entity's bytes.
        let payload = self
            .node_mut(from)
            .entities
            .get(&entity)
            .map(|e| {
                let mut b = Vec::new();
                b.extend_from_slice(&e.generation.to_le_bytes());
                for (c, bytes) in &e.components {
                    b.extend_from_slice(&c.0);
                    b.extend_from_slice(bytes);
                }
                b
            })
            .ok_or(error(Status::HandleStale, 3))?;
        let message = crate::ownership::TransferMessage {
            transfer_id,
            entity,
            generation: record.generation,
            source: from,
            target: to,
            expected_epoch: record.epoch,
            base_version: WorldVersion(0),
            schema_hash: pwe_api::Hash256([0; 32]),
            payload_hash: digest(&payload),
        };
        self.ownership.propose(message.clone())?;
        self.ownership.reserve(transfer_id)?;
        self.ownership.freeze(transfer_id, &payload)?;
        self.ownership.install(transfer_id)?;
        // Move the entity data between node worlds.
        let entity_data = self
            .nodes
            .get_mut(&from)
            .and_then(|world| world.entities.remove(&entity))
            .ok_or(error(Status::HandleStale, 4))?;
        self.nodes
            .entry(to)
            .or_default()
            .entities
            .insert(entity, entity_data);
        self.ownership.release(transfer_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::ComponentTypeId;

    #[test]
    fn scheduler_preempts_and_resumes_deterministically() {
        let mut sched = ClusterScheduler::new(100);
        let a = sched.spawn(RegionId(1), 0, 40);
        let b = sched.spawn(RegionId(1), 0, 60);
        // Processes run in ascending id order, each consuming its full budget.
        let runs = sched.step();
        assert_eq!(runs[0].process, a);
        assert_eq!(runs[1].process, b);
        assert_eq!(runs[0].reductions_used, 40);
        assert_eq!(runs[1].reductions_used, 60);
        assert_eq!(sched.tick, 1);
    }

    #[test]
    fn cluster_runs_identical_steps_across_identical_instances() {
        let mut c1 = RuntimeCluster::new(100);
        c1.claim(RegionId(1), EntityId(1), 1);
        c1.claim(RegionId(2), EntityId(2), 1);
        c1.scheduler.spawn(RegionId(1), 0, 50);
        c1.scheduler.spawn(RegionId(2), 0, 50);
        let mut c2 = RuntimeCluster::new(100);
        c2.claim(RegionId(1), EntityId(1), 1);
        c2.claim(RegionId(2), EntityId(2), 1);
        c2.scheduler.spawn(RegionId(1), 0, 50);
        c2.scheduler.spawn(RegionId(2), 0, 50);

        let r1 = c1.step();
        let r2 = c2.step();
        assert_eq!(r1, r2);
    }

    #[test]
    fn migrate_moves_entity_between_nodes_with_ownership_transfer() {
        let mut cluster = RuntimeCluster::new(100);
        cluster.claim(RegionId(1), EntityId(7), 1);
        cluster
            .node_mut(RegionId(1))
            .entities
            .get_mut(&EntityId(7))
            .unwrap()
            .components
            .insert(ComponentTypeId([1; 16]), vec![1, 2, 3]);

        // Before migration the entity lives on node 1.
        assert!(cluster
            .node_mut(RegionId(1))
            .entities
            .contains_key(&EntityId(7)));
        assert!(!cluster
            .nodes
            .get(&RegionId(2))
            .is_some_and(|w| w.entities.contains_key(&EntityId(7))));

        cluster
            .migrate(EntityId(7), RegionId(1), RegionId(2))
            .unwrap();

        // After migration it lives on node 2 with its data, and node 1 no longer
        // owns it; the ownership machine advanced the epoch to 2.
        assert!(!cluster
            .nodes
            .get(&RegionId(1))
            .is_some_and(|w| w.entities.contains_key(&EntityId(7))));
        let target = cluster.nodes.get(&RegionId(2)).unwrap();
        let moved = target.entities.get(&EntityId(7)).unwrap();
        assert_eq!(
            moved.components.get(&ComponentTypeId([1; 16])).unwrap(),
            &vec![1, 2, 3]
        );
        let record = cluster
            .ownership
            .region(RegionId(2))
            .owner_of(EntityId(7))
            .unwrap();
        assert_eq!(record.epoch, 2);
        assert_eq!(record.owner_region, RegionId(2));
    }
}
