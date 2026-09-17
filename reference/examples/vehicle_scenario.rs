//! Ground + vehicle + camera scenario (RFC-0030).
//!
//! Demonstrates the full pipeline Input → Physics → Commit → RenderPrepare on a
//! real deterministic simulation: a dynamic vehicle falls onto a static ground
//! plane under gravity while drifting forward, and a kinematic camera tracks it.
//! It also proves replay determinism by running the same scene twice and
//! comparing the state hash, plus a snapshot→restore→continue replay.
//!
//! Run with: `cargo run --example vehicle_scenario`

use pwe_api::EntityId;
use pwe_reference::components::{Camera, Collider, RigidBody, Transform};
use pwe_reference::math::Vec3;
use pwe_reference::physics::{PhysicsConfig, PhysicsSystem};
use pwe_reference::scene::{Entity, Scene};
use pwe_reference::simulation::Simulation;

fn build_scene() -> Scene {
    let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));

    // Static ground plane (a large flat AABB centered below y=0).
    let mut ground = Entity::dynamic();
    ground.transform = Some(Transform {
        position: Vec3::new(0.0, -5.0, 0.0),
        ..Default::default()
    });
    ground.rigid_body = Some(RigidBody::r#static());
    ground.collider = Some(Collider::aabb(Vec3::new(50.0, 5.0, 50.0)));
    scene.insert(EntityId(0), ground);

    // Dynamic vehicle: a box with forward velocity and height.
    let mut vehicle = Entity::dynamic();
    vehicle.transform = Some(Transform {
        position: Vec3::new(0.0, 8.0, 0.0),
        ..Default::default()
    });
    vehicle.velocity = Some(pwe_reference::components::Velocity {
        linear: Vec3::new(4.0, 0.0, 0.0),
        angular: Vec3::ZERO,
    });
    vehicle.rigid_body = Some(RigidBody {
        mass: 4.0,
        restitution: 0.4,
        friction: 0.6,
        is_dynamic: 1,
    });
    vehicle.collider = Some(Collider::aabb(Vec3::new(1.0, 0.5, 0.7)));
    scene.insert(EntityId(1), vehicle);

    // Kinematic camera tracking behind and above the vehicle.
    let mut camera = Entity::dynamic();
    camera.transform = Some(Transform {
        position: Vec3::new(0.0, 16.0, 24.0),
        ..Default::default()
    });
    camera.camera = Some(Camera::default());
    scene.insert(EntityId(2), camera);

    scene
}

fn run() -> Simulation {
    let mut sim = Simulation::new(
        build_scene(),
        1.0 / 60.0,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    sim.step_n(600);
    sim
}

fn main() {
    let dt = 1.0 / 60.0;
    println!(
        "PWE ground + vehicle + camera scenario ({} Hz)",
        (1.0 / dt) as u32
    );
    println!("gravity = (0, -9.81, 0), initial vehicle y = 8.0 m, vx = 4.0 m/s\n");

    // Determinism proof: two identical runs.
    let a = run();
    let b = run();
    println!(
        "determinism: run_a_hash == run_b_hash -> {}",
        a.state_hash() == b.state_hash()
    );

    // Snapshot / restore / continue replay proof.
    let mut uninterrupted = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    uninterrupted.step_n(300);
    let uninterrupted_hash = uninterrupted.state_hash();

    let mut replay = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    replay.step_n(150);
    let snap = replay.snapshot();
    println!("snapshot size at frame 150: {} bytes", snap.len());
    replay.restore(&snap).unwrap();
    replay.step_n(150);
    println!(
        "replay (snapshot@150 + 150 steps) == uninterrupted@300 -> {}",
        replay.state_hash() == uninterrupted_hash
    );

    // Frame log: show the vehicle trajectory.
    let mut live = Simulation::new(
        build_scene(),
        dt,
        PhysicsSystem::new(PhysicsConfig::default()),
    );
    println!("\nframe   t(s)    vehicle.y     vx     frame_bounds");
    for frame in 0..=10 {
        let step = frame * 30;
        if frame > 0 {
            live.step_n(30);
        }
        let v = live.scene.position(EntityId(1)).unwrap();
        let vel = live
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .unwrap()
            .linear;
        // Camera tracks the vehicle with a fixed chase offset each frame.
        live.track_camera(EntityId(2), EntityId(1), Vec3::new(0.0, 5.0, 10.0))
            .unwrap();
        let view = live.render_view(EntityId(2)).unwrap();
        println!(
            "{:>5} {:>7.2} {:>10.3} {:>7.2}  camera@{:?} visible={}",
            step,
            step as f64 * dt,
            v.y,
            vel.x,
            (
                view.camera.position.x,
                view.camera.position.y,
                view.camera.position.z
            ),
            view.visible.len(),
        );
    }

    let final_pos = live.scene.position(EntityId(1)).unwrap();
    let state = live.state_hash();
    let hex: String = state.0.iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "\nfinal vehicle position = {:?}",
        (final_pos.x, final_pos.y, final_pos.z)
    );
    println!("final state hash = {hex}");
}
