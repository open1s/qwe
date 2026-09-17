//! Convex-hull collision demo: a convex-hull block and a sphere both fall under
//! gravity and rest on a ground plane; a second hull drops onto a resting hull.
//! Exercises the exact SAT convex-hull narrow phase in a live, deterministic
//! simulation.
//!
//! Run: `cargo run --example physics_hull_demo`

use pwe_api::EntityId;
use pwe_reference::components::{Collider, RigidBody, Transform, Velocity};
use pwe_reference::math::Vec3;
use pwe_reference::physics::{Contact, PhysicsConfig, PhysicsSystem};
use pwe_reference::scene::{Entity, Scene};

fn hull(points: Vec<Vec3>) -> Collider {
    Collider::convex_hull(points)
}

fn main() {
    let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));

    // A convex-hull block (a slanted pyramid over a square base).
    let pyramid = hull(vec![
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::new(1.0, 0.0, -1.0),
        Vec3::new(-1.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(0.0, 2.0, 0.0),
    ]);
    // A flat convex hull "slab".
    let slab = hull(vec![
        Vec3::new(-1.2, 0.0, -0.6),
        Vec3::new(1.2, 0.0, -0.6),
        Vec3::new(-1.2, 0.0, 0.6),
        Vec3::new(1.2, 0.0, 0.6),
        Vec3::new(-1.2, 0.4, -0.6),
        Vec3::new(1.2, 0.4, -0.6),
        Vec3::new(-1.2, 0.4, 0.6),
        Vec3::new(1.2, 0.4, 0.6),
    ]);

    // body 1: pyramid at height 6; body 2: slab at height 3, offset in x so it
    // rests beside the pyramid; body 3: a sphere dropped onto the slab.
    insert(&mut scene, 1, Vec3::new(0.0, 6.0, 0.0), pyramid);
    insert(&mut scene, 2, Vec3::new(4.0, 3.0, 0.0), slab.clone());
    insert(
        &mut scene,
        3,
        Vec3::new(4.0, 7.0, 0.0),
        Collider::sphere(0.5),
    );

    let physics = PhysicsSystem::new(PhysicsConfig::default());
    let mut contacts: Vec<Contact> = Vec::new();
    let mut peak_contacts = 0usize;
    for _ in 0..900 {
        physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        peak_contacts = peak_contacts.max(contacts.len());
    }

    let y1 = scene.position(EntityId(1)).unwrap().y;
    let y2 = scene.position(EntityId(2)).unwrap().y;
    let y3 = scene.position(EntityId(3)).unwrap().y;
    println!("== convex-hull + sphere physics after 900 steps ==");
    println!("  pyramid final y (base)     = {y1:.4}");
    println!("  slab    final y (base)     = {y2:.4}");
    println!("  sphere  final y (center)   = {y3:.4}");
    println!("  max simultaneous contacts  = {peak_contacts}");

    // Determinism: replay must reproduce identical positions.
    let mut replay = Scene::new(Vec3::new(0.0, -9.81, 0.0));
    insert(
        &mut replay,
        1,
        Vec3::new(0.0, 6.0, 0.0),
        Collider::convex_hull(vec![
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, -1.0),
            Vec3::new(-1.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 2.0, 0.0),
        ]),
    );
    insert(&mut replay, 2, Vec3::new(4.0, 3.0, 0.0), slab);
    insert(
        &mut replay,
        3,
        Vec3::new(4.0, 7.0, 0.0),
        Collider::sphere(0.5),
    );
    let mut rc: Vec<Contact> = Vec::new();
    for _ in 0..900 {
        physics.step(&mut replay, 1.0 / 60.0, &mut rc);
    }
    println!(
        "  deterministic replay       : {}",
        scene.position(EntityId(1)).unwrap() == replay.position(EntityId(1)).unwrap()
            && scene.position(EntityId(2)).unwrap() == replay.position(EntityId(2)).unwrap()
    );
}

fn insert(scene: &mut Scene, id: u128, position: Vec3, collider: Collider) {
    let mut e = Entity::dynamic();
    e.transform = Some(Transform {
        position,
        ..Default::default()
    });
    e.velocity = Some(Velocity::default());
    e.rigid_body = Some(RigidBody::dynamic(2.0));
    e.collider = Some(collider);
    scene.insert(EntityId(id), e);
}
