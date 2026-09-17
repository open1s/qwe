//! Terrain scenario (RFC-0030 + RFC-0008 extension).
//!
//! A compound vehicle (chassis + 4 wheels) rolls down a non-flat heightfield
//! terrain. Demonstrates compound colliders, heightfield contact, wheels
//! (compound spheres), and a tracking camera. Deterministic across runs.
//!
//! Run with: `cargo run -p pwe-reference --example terrain_scenario`

use pwe_api::EntityId;
use pwe_reference::components::{Camera, Collider, Heightfield, RigidBody, Transform};
use pwe_reference::math::Vec3;
use pwe_reference::physics::{PhysicsConfig, PhysicsSystem};
use pwe_reference::scene::{Entity, Scene};
use pwe_reference::simulation::Simulation;

fn build_terrain() -> Collider {
    // A 64x64 heightfield sloping down in +x with a roll/camber bump.
    let mut hf = Heightfield::new(64, 64, 1.0, 1.0, Vec3::new(-32.0, 0.0, -32.0));
    for r in 0..hf.rows as usize {
        for c in 0..hf.cols as usize {
            let x = c as f64;
            let z = r as f64;
            hf.heights[r * 64 + c] = -0.12 * x + 0.15 * (x * 0.2).sin() + 0.10 * (z * 0.3).cos();
        }
    }
    Collider::heightfield(hf)
}

fn build_buggy() -> Entity {
    // A chassis box + 4 sphere wheels (a simple 4-wheeled buggy).
    let chassis = Collider::Box {
        dims: Vec3::new(1.1, 0.4, 0.6),
        offset: Vec3::new(0.0, 0.6, 0.0),
    };
    let wheel_fl = Collider::Sphere {
        radius: 0.4,
        offset: Vec3::new(-0.8, 0.0, 0.5),
    };
    let wheel_fr = Collider::Sphere {
        radius: 0.4,
        offset: Vec3::new(0.8, 0.0, 0.5),
    };
    let wheel_rl = Collider::Sphere {
        radius: 0.4,
        offset: Vec3::new(-0.8, 0.0, -0.5),
    };
    let wheel_rr = Collider::Sphere {
        radius: 0.4,
        offset: Vec3::new(0.8, 0.0, -0.5),
    };
    let mut e = Entity::dynamic();
    e.transform = Some(Transform {
        position: Vec3::new(0.0, 2.0, 0.0),
        ..Default::default()
    });
    e.velocity = Some(pwe_reference::components::Velocity {
        linear: Vec3::new(3.0, 0.0, 0.0),
        angular: Vec3::ZERO,
    });
    e.rigid_body = Some(RigidBody::dynamic(80.0));
    e.collider = Some(Collider::compound(vec![
        wheel_fl, wheel_fr, wheel_rl, wheel_rr, chassis,
    ]));
    e
}

fn build_camera() -> Entity {
    let mut camera = Entity::dynamic();
    camera.transform = Some(Transform {
        position: Vec3::new(-5.0, 5.0, 10.0),
        ..Default::default()
    });
    camera.camera = Some(Camera::default());
    camera
}

fn build_scene() -> Scene {
    let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
    let mut hosting = Entity::dynamic();
    hosting.collider = Some(build_terrain());
    hosting.rigid_body = Some(RigidBody::r#static());
    scene.insert(EntityId(0), hosting);

    scene.insert(EntityId(1), build_buggy());
    scene.insert(EntityId(2), build_camera());
    scene
}

fn main() {
    let dt = 1.0 / 60.0;
    let physics = PhysicsSystem::new(PhysicsConfig::default());
    let mut sim = Simulation::new(build_scene(), dt, physics);

    println!("PWE terrain buggy scenario ({} Hz)", (1.0 / dt) as u64);

    println!("frame  vehicle(y,x,z)            camera(y,x,z)              visible");
    for step_block in 0..=8 {
        if step_block > 0 {
            sim.step_n(30);
        }
        sim.track_camera(EntityId(2), EntityId(1), Vec3::new(-4.0, 3.0, 6.0))
            .unwrap();
        let v = sim.scene.position(EntityId(1)).unwrap();
        let c = sim.scene.position(EntityId(2)).unwrap();
        let view = sim.render_view(EntityId(2)).unwrap();
        println!(
            "{:>4}  ({:<5.2}, {:.2}, {:.2})  ({:<5.2}, {:.2}, {:.2})  {}",
            step_block * 30,
            v.y,
            v.x,
            v.z,
            c.y,
            c.x,
            c.z,
            view.visible.len()
        );
    }

    // Determinism: two identical un-tracked runs must match.
    let mut a = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    let mut b = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    a.step_n(600);
    b.step_n(600);
    assert_eq!(a.state_hash(), b.state_hash());

    // Snapshot/replay: restore at 300, then run 300 more — must match
    // the uninterrupted 600-step run.
    let mut uninterrupted = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    uninterrupted.step_n(600);
    let target = uninterrupted.state_hash();

    let mut replay = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    replay.step_n(300);
    let snap = replay.snapshot();
    replay.restore(&snap).unwrap();
    replay.step_n(300);
    assert_eq!(replay.state_hash(), target);

    let hash: String = sim
        .state_hash()
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("\nfinal state hash = {hash}");
    println!("determinism verified: yes (state hash matches on rerun)");
    println!("snapshot+replay: verified (restored and matched)");
}
