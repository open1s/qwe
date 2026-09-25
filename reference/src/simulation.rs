//! The deterministic simulation driver: Input → Physics → Commit → RenderPrepare.
//!
//! A `Simulation` owns a typed `Scene` (the authoritative World State), a
//! `PhysicsSystem`, and a fixed timestep. Each `step` advances the world, and
//! the whole state is content-hashable so replay determinism is provable: two
//! identical runs yield an identical state hash, and a snapshot→restore→continue
//! run matches an uninterrupted run.

use crate::components::Transform;
use crate::math::{Aabb, Vec3};
use crate::physics::{Contact, PhysicsSystem};
use crate::scene::{Entity, Scene};
use crate::sha256::digest;
use pwe_api::{EntityId, Hash256, Result};

/// A prepared render view: the camera pose and the bounding boxes of everything
/// that would be submitted this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderView {
    pub camera: Transform,
    pub camera_entity: EntityId,
    pub visible: Vec<(EntityId, Aabb)>,
}

/// A prepared physics view (RFC-0030): a read-only snapshot of every dynamic
/// body's position and linear velocity at the current state. Physics consumers
/// (AI, sensor, render, network) read this view; it is never a second
/// authoritative copy of the world.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicsView {
    pub step: u64,
    pub seconds: f64,
    /// `(entity, position, linear velocity)` for each dynamic body.
    pub bodies: Vec<(EntityId, Vec3, Vec3)>,
}

/// Simulation timeline: fixed timestep accumulated into simulation time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SimClock {
    pub steps: u64,
    pub seconds: f64,
}

pub struct Simulation {
    pub scene: Scene,
    pub physics: PhysicsSystem,
    pub dt: f64,
    pub clock: SimClock,
    pub last_contacts: Vec<Contact>,
}

impl Simulation {
    pub fn new(scene: Scene, dt: f64, physics: PhysicsSystem) -> Self {
        Self {
            scene,
            physics,
            dt,
            clock: SimClock::default(),
            last_contacts: Vec::new(),
        }
    }

    /// Advances one fixed step (Physics + Commit). Deterministic.
    pub fn step(&mut self) {
        self.physics
            .step(&mut self.scene, self.dt, &mut self.last_contacts);
        self.clock.steps += 1;
        self.clock.seconds += self.dt;
    }

    /// Runs `n` steps.
    pub fn step_n(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Moves `camera` to `target_position + offset` pointing at the target.
    /// The camera transform is kinematic — this never couples to physics.
    pub fn track_camera(&mut self, camera: EntityId, target: EntityId, offset: Vec3) -> Result<()> {
        let target_pos = self.scene.position(target)?;
        let e = self.scene.get_mut(camera).ok_or(pwe_api::Error {
            status: pwe_api::Status::HandleStale,
            detail: 4,
            byte_offset: 0,
        })?;
        let t = e.transform.get_or_insert_with(Transform::default);
        t.position = target_pos + offset;
        let dir = target_pos - t.position;
        t.rotation = crate::math::Quat::look_rotation(dir, Vec3::new(0.0, 1.0, 0.0));
        Ok(())
    }

    /// RenderPrepare: build the frame view for a tracking camera entity.
    pub fn render_view(&self, camera_entity: EntityId) -> Result<RenderView> {
        let camera = self
            .scene
            .get(camera_entity)
            .and_then(|e| e.transform)
            .ok_or(pwe_api::Error {
                status: pwe_api::Status::HandleStale,
                detail: 3,
                byte_offset: 0,
            })?;
        let visible: Vec<(EntityId, Aabb)> = self
            .scene
            .iter()
            .filter(|(id, _)| *id != camera_entity)
            .filter_map(|(id, e)| {
                let collider = e.collider.as_ref()?;
                let transform = e.transform?;
                Some((id, collider.world_aabb(transform.position)))
            })
            .collect();
        Ok(RenderView {
            camera,
            camera_entity,
            visible,
        })
    }

    /// A read-only snapshot of the current physics state. This is the
    /// `PhysicsView` peer of `RenderView`: consumers observe it and never mutate
    /// authoritative world state through it.
    pub fn physics_view(&self) -> PhysicsView {
        let bodies = self
            .scene
            .iter()
            .filter_map(|(id, e)| {
                let position = e.transform?.position;
                let velocity = e.velocity?.linear;
                Some((id, position, velocity))
            })
            .collect();
        PhysicsView {
            step: self.clock.steps,
            seconds: self.clock.seconds,
            bodies,
        }
    }

    /// Canonical deterministic state hash over the entire world.
    ///
    /// Serializes gravity, the clock, and every entity's present components in
    /// stable id order into SHA-256. Any identical scene + trajectory yields an
    /// identical hash.
    pub fn state_hash(&self) -> Hash256 {
        let mut bytes = Vec::new();
        push_f64(&mut bytes, self.scene.gravity.x);
        push_f64(&mut bytes, self.scene.gravity.y);
        push_f64(&mut bytes, self.scene.gravity.z);
        bytes.extend_from_slice(&self.clock.steps.to_le_bytes());
        push_f64(&mut bytes, self.clock.seconds);

        for (id, e) in &self.scene.entities {
            bytes.extend_from_slice(&id.0.to_le_bytes());
            // Component tag + presence + bytes, in a fixed order.
            if let Some(t) = e.transform {
                bytes.push(1);
                bytes.extend_from_slice(&t.encode());
            } else {
                bytes.push(0);
            }
            if let Some(v) = e.velocity {
                bytes.push(1);
                bytes.extend_from_slice(&v.encode());
            } else {
                bytes.push(0);
            }
            if let Some(rb) = e.rigid_body {
                bytes.push(1);
                bytes.extend_from_slice(&rb.encode());
            } else {
                bytes.push(0);
            }
            if let Some(c) = &e.collider {
                bytes.push(1);
                bytes.extend_from_slice(&c.encode());
            } else {
                bytes.push(0);
            }
            if let Some(cam) = e.camera {
                bytes.push(1);
                bytes.extend_from_slice(&cam.encode());
            } else {
                bytes.push(0);
            }
            if let Some(st) = &e.state {
                bytes.push(1);
                bytes.extend_from_slice(&st.encode());
            } else {
                bytes.push(0);
            }
            // RFC-0038: the active flag is part of world state.
            bytes.push(e.active as u8);
        }
        // Grid fields: the name + the canonical field bytes, in name order.
        for (name, f) in &self.scene.fields {
            bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(&(f.width as u64).to_le_bytes());
            bytes.extend_from_slice(&(f.height as u64).to_le_bytes());
            bytes.extend_from_slice(&f.dx.to_bits().to_le_bytes());
            for c in f.cells() {
                bytes.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        digest(&bytes)
    }

    /// Snapshots the world to a deterministic byte string.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.state_hash().0);
        out.extend_from_slice(&self.scene.gravity.x.to_bits().to_le_bytes());
        out.extend_from_slice(&self.scene.gravity.y.to_bits().to_le_bytes());
        out.extend_from_slice(&self.scene.gravity.z.to_bits().to_le_bytes());
        out.extend_from_slice(&self.dt.to_bits().to_le_bytes());
        out.extend_from_slice(&self.clock.steps.to_le_bytes());
        out.extend_from_slice(&self.clock.seconds.to_bits().to_le_bytes());
        out.extend_from_slice(&(self.scene.entities.len() as u64).to_le_bytes());
        for (id, e) in &self.scene.entities {
            out.extend_from_slice(&id.0.to_le_bytes());
            out.extend_from_slice(&component_flags(e).to_le_bytes());
            for bytes in component_blobs(e) {
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(&bytes);
            }
        }
        out.extend_from_slice(&(self.scene.fields.len() as u64).to_le_bytes());
        for (name, f) in &self.scene.fields {
            out.extend_from_slice(&(name.len() as u64).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&(f.width as u64).to_le_bytes());
            out.extend_from_slice(&(f.height as u64).to_le_bytes());
            out.extend_from_slice(&f.dx.to_bits().to_le_bytes());
            out.extend_from_slice(&(f.cells().len() as u64).to_le_bytes());
            for c in f.cells() {
                out.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        // RFC-0038: inactive-slot list, appended (absent in older snapshots).
        let inactive: Vec<u128> = self
            .scene
            .entities
            .iter()
            .filter(|(_, e)| !e.active)
            .map(|(id, _)| id.0)
            .collect();
        out.extend_from_slice(&(inactive.len() as u64).to_le_bytes());
        for id in &inactive {
            out.extend_from_slice(&id.to_le_bytes());
        }
        out
    }

    /// Restores the world from a snapshot, replacing all state atomically.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        let mut cursor = 0;
        let _expected_hash = take_32(bytes, &mut cursor)?;
        let g = Vec3::new(
            take_f64_s(bytes, &mut cursor)?,
            take_f64_s(bytes, &mut cursor)?,
            take_f64_s(bytes, &mut cursor)?,
        );
        let dt = take_f64_s(bytes, &mut cursor)?;
        let steps = take_u64(bytes, &mut cursor)?;
        let seconds = take_f64_s(bytes, &mut cursor)?;
        let count = take_u64(bytes, &mut cursor)? as usize;

        let mut scene = Scene::new(g);
        for _ in 0..count {
            let id = EntityId(take_u128(bytes, &mut cursor)?);
            let flags = take_u64(bytes, &mut cursor)?;
            let mut e = Entity::dynamic();
            for index in 0..7u64 {
                if flags & (1 << index) != 0 {
                    let len = take_u64(bytes, &mut cursor)? as usize;
                    let blob = bytes.get(cursor..cursor + len).ok_or(pwe_api::Error {
                        status: pwe_api::Status::Invalid,
                        detail: 1,
                        byte_offset: cursor as u64,
                    })?;
                    cursor += len;
                    assign_component(&mut e, index as usize, blob)?;
                }
            }
            scene.insert(id, e);
        }
        let field_count = take_u64(bytes, &mut cursor)? as usize;
        for _ in 0..field_count {
            let name_len = take_u64(bytes, &mut cursor)? as usize;
            let name_bytes = bytes.get(cursor..cursor + name_len).ok_or(pwe_api::Error {
                status: pwe_api::Status::Invalid,
                detail: 1,
                byte_offset: cursor as u64,
            })?;
            cursor += name_len;
            let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| pwe_api::Error {
                status: pwe_api::Status::Invalid,
                detail: 1,
                byte_offset: cursor as u64,
            })?;
            let width = take_u64(bytes, &mut cursor)? as usize;
            let height = take_u64(bytes, &mut cursor)? as usize;
            let dx = take_f64_s(bytes, &mut cursor)?;
            let cell_count = take_u64(bytes, &mut cursor)? as usize;
            // The depth is recoverable from the cell count: the snapshot format
            // is unchanged by extending fields to 3D.
            let depth = (cell_count / (width * height).max(1)).max(1);
            let mut f = crate::field::Field::new3(width, height, depth, dx);
            for idx in 0..cell_count {
                let v = take_f64_s(bytes, &mut cursor)?;
                f.set_linear(idx, v);
            }
            scene.fields.insert(name, f);
        }

        // RFC-0038: the inactive-slot list is appended (absent in older
        // snapshots, where every entity is active).
        if cursor < bytes.len() {
            let k = take_u64(bytes, &mut cursor)?;
            for _ in 0..k {
                let id = take_u128(bytes, &mut cursor)?;
                if let Some(e) = scene.get_mut(EntityId(id)) {
                    e.active = false;
                }
            }
        }

        let physics = PhysicsSystem::new(crate::physics::PhysicsConfig::default());
        self.scene = scene;
        self.dt = dt;
        self.clock = SimClock { steps, seconds };
        self.physics = physics;
        Ok(())
    }
}

fn push_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn component_flags(e: &Entity) -> u64 {
    let mut flags = 0u64;
    if e.transform.is_some() {
        flags |= 1 << 0;
    }
    if e.velocity.is_some() {
        flags |= 1 << 1;
    }
    if e.rigid_body.is_some() {
        flags |= 1 << 2;
    }
    if e.collider.is_some() {
        flags |= 1 << 3;
    }
    if e.camera.is_some() {
        flags |= 1 << 4;
    }
    if e.joint.is_some() {
        flags |= 1 << 5;
    }
    if e.state.is_some() {
        flags |= 1 << 6;
    }
    flags
}

fn component_blobs(e: &Entity) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    if let Some(t) = e.transform {
        v.push(t.encode());
    }
    if let Some(vel) = e.velocity {
        v.push(vel.encode());
    }
    if let Some(rb) = e.rigid_body {
        v.push(rb.encode());
    }
    if let Some(c) = &e.collider {
        v.push(c.encode());
    }
    if let Some(cam) = e.camera {
        v.push(cam.encode());
    }
    if let Some(j) = e.joint {
        v.push(j.encode());
    }
    if let Some(st) = &e.state {
        v.push(st.encode());
    }
    v
}

fn assign_component(e: &mut Entity, index: usize, blob: &[u8]) -> Result<()> {
    match index {
        0 => e.transform = Some(Transform::decode(blob)?),
        1 => e.velocity = Some(crate::components::Velocity::decode(blob)?),
        2 => e.rigid_body = Some(crate::components::RigidBody::decode(blob)?),
        3 => e.collider = Some(crate::components::Collider::decode(blob)?),
        4 => e.camera = Some(crate::components::Camera::decode(blob)?),
        5 => e.joint = Some(crate::components::Joint::decode(blob)?),
        6 => {
            e.state = Some(crate::components::State::decode(blob)?);
        }
        _ => {
            return Err(pwe_api::Error {
                status: pwe_api::Status::Invalid,
                detail: 2,
                byte_offset: 0,
            });
        }
    }
    Ok(())
}

fn take_32(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 32]> {
    let s = bytes.get(*cursor..*cursor + 32).ok_or(pwe_api::Error {
        status: pwe_api::Status::Invalid,
        detail: 3,
        byte_offset: *cursor as u64,
    })?;
    *cursor += 32;
    Ok(s.try_into().unwrap())
}
fn take_f64_s(bytes: &[u8], cursor: &mut usize) -> Result<f64> {
    let b = take_8(bytes, cursor)?;
    Ok(f64::from_bits(u64::from_le_bytes(b)))
}
fn take_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let b = take_8(bytes, cursor)?;
    Ok(u64::from_le_bytes(b))
}
fn take_u128(bytes: &[u8], cursor: &mut usize) -> Result<u128> {
    let b = take_16(bytes, cursor)?;
    Ok(u128::from_le_bytes(b))
}
fn take_8(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 8]> {
    let s = bytes.get(*cursor..*cursor + 8).ok_or(pwe_api::Error {
        status: pwe_api::Status::Invalid,
        detail: 4,
        byte_offset: *cursor as u64,
    })?;
    *cursor += 8;
    Ok(s.try_into().unwrap())
}
fn take_16(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 16]> {
    let s = bytes.get(*cursor..*cursor + 16).ok_or(pwe_api::Error {
        status: pwe_api::Status::Invalid,
        detail: 5,
        byte_offset: *cursor as u64,
    })?;
    *cursor += 16;
    Ok(s.try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Collider, RigidBody, Velocity};

    fn vehicle_scene() -> Scene {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        // A dynamic "vehicle" box at height 10.
        let mut vehicle = Entity::dynamic();
        vehicle.transform = Some(Transform {
            position: Vec3::new(0.0, 10.0, 0.0),
            ..Default::default()
        });
        vehicle.velocity = Some(Velocity {
            linear: Vec3::new(3.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        vehicle.rigid_body = Some(RigidBody::dynamic(4.0));
        vehicle.collider = Some(Collider::aabb(Vec3::new(1.0, 0.5, 0.7)));
        scene.insert(EntityId(1), vehicle);

        // A kinematic camera tracking the vehicle.
        let mut camera = Entity::dynamic();
        camera.transform = Some(Transform {
            position: Vec3::new(0.0, 15.0, 20.0),
            ..Default::default()
        });
        camera.camera = Some(crate::components::Camera::default());
        scene.insert(EntityId(2), camera);
        scene
    }

    #[test]
    fn identical_runs_produce_identical_state_hash() {
        let mut a = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        let mut b = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        a.step_n(300);
        b.step_n(300);
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn snapshot_restore_matches_uninterrupted_run() {
        let mut uninterrupted = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        uninterrupted.step_n(200);
        let hash_at_200 = uninterrupted.state_hash();

        let mut replay = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        replay.step_n(100);
        let snap = replay.snapshot();
        replay.restore(&snap).unwrap();
        replay.step_n(100);

        assert_eq!(replay.state_hash(), hash_at_200);
    }

    /// Snapshot/restore covers the grid fields and entity state slots: a
    /// restore preserves both, and the replay hash matches an uninterrupted
    /// run that mutates them.
    #[test]
    fn snapshot_covers_fields_and_state_slots() {
        let scene = || {
            let mut scene = Scene::new(Vec3::ZERO);
            let mut e = Entity::dynamic();
            e.state = Some(crate::components::State::new(vec![
                7.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ]));
            scene.insert(EntityId(1), e);
            let mut f = crate::field::Field::new(4, 4, 1.0);
            f.set(1, 2, 7.0);
            scene.fields.insert("heat".to_string(), f);
            scene
        };
        let mutate = |sim: &mut Simulation| {
            // Mutate the state slot and a field cell so the hash distinguishes.
            if let Some(st) = sim.scene.get_mut(EntityId(1)).unwrap().state.as_mut() {
                st.values[1] = 3.0;
            }
            let f = sim.scene.fields.get_mut("heat").unwrap();
            f.set(2, 2, 5.0);
        };

        let mut uninterrupted =
            Simulation::new(scene(), 1.0 / 60.0, PhysicsSystem::new(Default::default()));
        mutate(&mut uninterrupted);
        let target = uninterrupted.state_hash();

        let mut replay =
            Simulation::new(scene(), 1.0 / 60.0, PhysicsSystem::new(Default::default()));
        let snap = replay.snapshot();
        replay.restore(&snap).unwrap();
        // Before the mutation, the restored hash must differ from the target
        // (the mutations are not yet applied) but the fields/state must be present.
        assert_eq!(
            replay.scene.fields.get("heat").unwrap().value(1, 2),
            7.0,
            "the field cell must survive restore"
        );
        assert_eq!(
            replay
                .scene
                .get(EntityId(1))
                .unwrap()
                .state
                .as_ref()
                .unwrap()
                .values[0],
            7.0,
            "the state slot must survive restore"
        );
        assert_ne!(replay.state_hash(), target, "unmutated replay must differ");
        mutate(&mut replay);
        assert_eq!(replay.state_hash(), target, "mutated replay must match");
    }

    #[test]
    fn vehicle_falls_and_moves_forward() {
        let mut sim = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        let start_x = sim.scene.position(EntityId(1)).unwrap().x;
        let start_y = sim.scene.position(EntityId(1)).unwrap().y;
        sim.step_n(120);
        let p = sim.scene.position(EntityId(1)).unwrap();
        assert!(p.x > start_x, "vehicle should move forward");
        assert!(p.y < start_y, "vehicle should fall");
        assert!(p.y >= 0.0, "vehicle should not tunnel through ground");
    }

    #[test]
    fn camera_tracks_target_with_offset() {
        let mut sim = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        sim.track_camera(EntityId(2), EntityId(1), Vec3::new(0.0, 5.0, 10.0))
            .unwrap();
        let cam = sim.scene.get(EntityId(2)).unwrap().transform.unwrap();
        let veh = sim.scene.position(EntityId(1)).unwrap();
        // Camera sits at target + offset.
        assert!((cam.position - (veh + Vec3::new(0.0, 5.0, 10.0))).length() < 1e-9);
        // Camera forward (-Z rotated) points at the vehicle.
        let forward = cam.rotation.rotate(Vec3::new(0.0, 0.0, -1.0));
        let to_target = (veh - cam.position).normalized();
        assert!(forward.dot(to_target) > 0.999);
    }

    #[test]
    fn render_view_sees_vehicle_and_camera() {
        let sim = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        let view = sim.render_view(EntityId(2)).unwrap();
        assert_eq!(view.camera_entity, EntityId(2));
        assert!(!view.visible.is_empty(), "camera should see the vehicle");
    }

    #[test]
    fn physics_view_is_read_only_and_sees_dynamic_bodies() {
        let mut sim = Simulation::new(
            vehicle_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(Default::default()),
        );
        sim.step_n(30);
        let before = sim.state_hash();
        let view = sim.physics_view();
        assert_eq!(view.step, 30);
        assert!(
            view.bodies.iter().any(|(id, _, _)| *id == EntityId(1)),
            "the dynamic vehicle is present in the physics view"
        );
        // Reading the view never mutates authoritative state.
        assert_eq!(sim.state_hash(), before);
    }
}
