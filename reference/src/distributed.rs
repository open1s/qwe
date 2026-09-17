//! Distributed continuum: run the generic electromagnetic/chemistry field on a
//! BEAM-like [`crate::cluster::RuntimeCluster`], advancing one field per node
//! under the preemptive scheduler, and migrate a node's field to another node
//! through the RFC-0024 ownership machine.
//!
//! This ties the generic continuum runtime (`field`/`continuum`) to the cluster
//! (`cluster`) end to end: entity migration moves the field's state and its
//! authoritative ownership together, so a migrated field resumes identically on
//! the receiving node with no split-brain.

use crate::cluster::RuntimeCluster;
use crate::continuum::diffuse;
use crate::field::Field;
use pwe_api::{ComponentTypeId, EntityId, RegionId};
use std::collections::BTreeMap;

/// The component type carrying a field's serialized bytes on a node entity.
pub const FIELD_COMPONENT: ComponentTypeId = ComponentTypeId([0xFE; 16]);

/// The field entity id used on every node (one field per node).
pub fn field_entity(region: RegionId) -> EntityId {
    EntityId((region.0 as u128) * 1_000_000 + 1)
}

/// Serializes a field's cells to little-endian bytes for component storage.
fn field_to_bytes(field: &Field) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.cells().len() * 8);
    for c in field.cells() {
        out.extend_from_slice(&c.to_bits().to_le_bytes());
    }
    out
}

/// Rehydrates a field from stored bytes.
fn field_from_bytes(field: &Field, bytes: &[u8]) -> Field {
    debug_assert_eq!(bytes.len(), field.cells().len() * 8);
    let mut out = Field::new(field.width, field.height, field.dx);
    for (i, chunk) in bytes.chunks_exact(8).enumerate() {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(chunk);
        let idx_row = i / field.width;
        let idx_col = i % field.width;
        out.set(idx_col, idx_row, f64::from_bits(u64::from_le_bytes(buf)));
    }
    out
}

/// A cluster of continuum fields, one per node, evolved by the scheduler and
/// migratable between nodes.
#[derive(Debug)]
pub struct DistributedContinuum {
    pub cluster: RuntimeCluster,
    /// Authoritative per-node fields, mirroring each node's field entity.
    fields: BTreeMap<RegionId, Field>,
    dt: f64,
    diffusivity: f64,
}

impl DistributedContinuum {
    pub fn new(reductions_per_tick: u64, dt: f64, diffusivity: f64) -> Self {
        Self {
            cluster: RuntimeCluster::new(reductions_per_tick),
            fields: BTreeMap::new(),
            dt,
            diffusivity,
        }
    }

    /// Registers a node owning `field` (claimed as its field entity) and spawns a
    /// process on that node so the scheduler advances it each tick.
    pub fn add_node(&mut self, region: RegionId, field: Field) {
        self.cluster.claim(region, field_entity(region), 1);
        self.cluster
            .node_mut(region)
            .entities
            .get_mut(&field_entity(region))
            .unwrap()
            .components
            .insert(FIELD_COMPONENT, field_to_bytes(&field));
        // One process per node carries the reduction budget the scheduler grants.
        self.cluster.scheduler.spawn(region, 0, 1000);
        self.fields.insert(region, field);
    }

    /// Runs one scheduler tick: every process runs (advancing its node's field
    /// by one diffusion step), and the field bytes are written back into the
    /// node world so the authoritative world reflects the physics.
    pub fn step(&mut self) {
        let runs = self.cluster.step();
        for run in runs {
            if let Some(field) = self.fields.get_mut(&run.node) {
                diffuse(field, self.dt, self.diffusivity);
                let bytes = field_to_bytes(field);
                if let Some(entity) = self
                    .cluster
                    .node_mut(run.node)
                    .entities
                    .get_mut(&field_entity(run.node))
                {
                    entity.components.insert(FIELD_COMPONENT, bytes);
                }
            }
        }
    }

    pub fn field(&self, region: RegionId) -> Option<&Field> {
        self.fields.get(&region)
    }

    /// A deterministic combined identity over every node's field (replay check).
    pub fn global_hash(&self) -> pwe_api::Hash256 {
        let mut out = Vec::new();
        for (region, field) in &self.fields {
            out.extend_from_slice(&region.0.to_le_bytes());
            out.extend_from_slice(field.hash().0.as_slice());
        }
        crate::sha256::digest(&out)
    }

    /// Migrates the field on `from` to `to` through the RFC-0024 ownership
    /// machine, moving its state with its ownership. On success the receiving
    /// node holds the identical field and `from` no longer does.
    pub fn migrate_field(&mut self, from: RegionId, to: RegionId) -> Result<(), pwe_api::Error> {
        self.cluster.migrate(field_entity(from), from, to)?;
        // The migrated entity retains its source id; rehydrate its field bytes
        // on the target node.
        let bytes = self
            .cluster
            .node_mut(to)
            .entities
            .get(&field_entity(from))
            .and_then(|e| e.components.get(&FIELD_COMPONENT))
            .cloned()
            .expect("migrated entity must carry the field component");
        let source = self.fields.remove(&from).expect("source node has a field");
        let rehydrated = field_from_bytes(&source, &bytes);
        self.fields.insert(to, rehydrated);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    #[test]
    fn identical_clusters_advance_identically() {
        let mut a = DistributedContinuum::new(100, 1e-3, 0.5);
        let mut b = DistributedContinuum::new(100, 1e-3, 0.5);
        let mut fa = Field::new(16, 1, 0.1);
        fa.set(8, 0, 5.0);
        let mut fb = Field::new(16, 1, 0.1);
        fb.set(8, 0, 5.0);
        a.add_node(RegionId(1), fa.clone());
        a.add_node(RegionId(2), fa);
        b.add_node(RegionId(1), fb.clone());
        b.add_node(RegionId(2), fb);

        for _ in 0..100 {
            a.step();
            b.step();
        }
        assert_eq!(a.global_hash(), b.global_hash());
        assert_eq!(
            a.field(RegionId(1)).unwrap().hash(),
            b.field(RegionId(1)).unwrap().hash()
        );
    }

    #[test]
    fn migration_moves_field_state_and_ownership() {
        let mut dc = DistributedContinuum::new(100, 1e-3, 0.5);
        let mut f1 = Field::new(16, 1, 0.1);
        f1.set(4, 0, 3.0);
        f1.set(11, 0, 2.0);
        let mut f2 = Field::new(16, 1, 0.1);
        f2.set(8, 0, 5.0);
        dc.add_node(RegionId(1), f1);
        dc.add_node(RegionId(2), f2);
        for _ in 0..20 {
            dc.step();
        }
        let before = dc.field(RegionId(2)).unwrap().hash();

        dc.migrate_field(RegionId(2), RegionId(3)).unwrap();

        // The field now lives on node 3 with identical state; node 2 lost it.
        assert!(dc.field(RegionId(3)).is_some());
        assert!(dc.field(RegionId(2)).is_none());
        assert_eq!(dc.field(RegionId(3)).unwrap().hash(), before);
        // Ownership moved: node 3 owns the (source-id) field entity at epoch 2.
        let record = dc
            .cluster
            .ownership
            .region(RegionId(3))
            .owner_of(field_entity(RegionId(2)))
            .unwrap();
        assert_eq!(record.owner_region, RegionId(3));
        assert_eq!(record.epoch, 2);
    }
}
