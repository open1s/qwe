//! The concrete authoritative simulation scene.
//!
//! Whereas `ReferenceWorld` demonstrates the frozen transaction/ABI contract
//! over opaque component bytes, `Scene` is the typed authoritative World State
//! a simulation actually runs on. It is deterministic: entities and their
//! component updates iterate in stable `BTreeMap` order, and every value is a
//! lossless `f64` bit pattern.

use crate::components::{Camera, Collider, Force, Joint, RigidBody, Transform, Velocity};
use crate::math::Vec3;
use pwe_api::{EntityId, Error, Result, Status};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub transform: Option<Transform>,
    pub velocity: Option<Velocity>,
    pub force: Option<Force>,
    pub rigid_body: Option<RigidBody>,
    pub collider: Option<Collider>,
    pub camera: Option<Camera>,
    pub joint: Option<Joint>,
    /// Generic scalar state slots for user-defined dynamical systems.
    pub state: Option<crate::components::State>,
    /// Presentation color as `0xRRGGBB` (visualization only; not physics).
    pub color: Option<u32>,
}

impl Entity {
    pub fn dynamic() -> Self {
        Self::default()
    }
}

/// A deterministic, typed world of entities.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub entities: BTreeMap<EntityId, Entity>,
    /// Deterministic scalar grid fields (the PDE substrate), keyed by name.
    pub fields: BTreeMap<String, crate::field::Field>,
    /// Model parameters (`params { … }`), read by rules and overridable at
    /// runtime; part of world state (hashed/snapshotted like everything else).
    pub params: BTreeMap<String, f64>,
    /// Gravity applied to dynamic bodies each step (meters/second^2).
    pub gravity: Vec3,
    /// Global simulation clock (seconds), read by `t` in `update` expressions.
    pub sim_time: f64,
    /// Deterministic PRNG state, advanced each step; `rnd()` reads it.
    pub random_state: u64,
}

impl Scene {
    pub fn new(gravity: Vec3) -> Self {
        Self {
            entities: BTreeMap::new(),
            fields: BTreeMap::new(),
            params: BTreeMap::new(),
            gravity,
            sim_time: 0.0,
            random_state: 0x9E37_79B9_7F4A_7C15,
        }
    }
    pub fn insert(&mut self, id: EntityId, entity: Entity) -> Option<Entity> {
        self.entities.insert(id, entity)
    }
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }
    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.get_mut(&id)
    }
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &Entity)> {
        self.entities.iter().map(|(id, e)| (*id, e))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut Entity)> {
        self.entities.iter_mut().map(|(id, e)| (*id, e))
    }
    pub fn set_transform(&mut self, id: EntityId, pos: Vec3) -> Result<()> {
        let e = self.get_mut(id).ok_or(error(Status::HandleStale, 1))?;
        let t = e.transform.get_or_insert_with(Transform::default);
        t.position = pos;
        Ok(())
    }
    pub fn position(&self, id: EntityId) -> Result<Vec3> {
        self.get(id)
            .and_then(|e| e.transform)
            .map(|t| t.position)
            .ok_or(error(Status::HandleStale, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scene_insert_and_position_is_deterministic_order() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        for n in (0..4).rev() {
            let mut e = Entity::dynamic();
            e.transform = Some(Transform {
                position: Vec3::new(n as f64, 0.0, 0.0),
                ..Default::default()
            });
            scene.insert(EntityId(n), e);
        }
        // BTreeMap iteration is id-sorted, independent of insertion order.
        let xs: Vec<f64> = scene
            .iter()
            .map(|(_, e)| e.transform.unwrap().position.x)
            .collect();
        assert_eq!(xs, vec![0.0, 1.0, 2.0, 3.0]);
    }
}
