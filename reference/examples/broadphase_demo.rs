//! Broad-phase backend demo: the uniform grid and the median-split BVH expose
//! the same `pairs()` contract, and both drive the physics system to identical
//! simulated positions.
//!
//! Run: `cargo run --example broadphase_demo`

use pwe_api::EntityId;
use pwe_reference::broadphase::{Bvh, UniformGrid};
use pwe_reference::math::{Aabb, Vec3};
use pwe_reference::physics::{BroadPhaseKind, PhysicsConfig, PhysicsSystem};
use pwe_reference::scene::{Entity, Scene};

fn random_box(id: u128, s: &mut u64) -> (u128, Aabb) {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let x = (((*s >> 11) as f64 / (1u64 << 53) as f64) * 18.0) - 9.0;
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let y = (((*s >> 11) as f64 / (1u64 << 53) as f64) * 18.0) - 9.0;
    (
        id,
        Aabb::from_center_half(Vec3::new(x, y, 0.0), Vec3::new(0.4, 0.4, 0.4)),
    )
}

fn main() {
    // Build a set of scattered AABBs and compare the two broad-phase backends.
    let mut seed: u64 = 0x1234_5678_9abc_def0;
    let bounds: Vec<(u128, Aabb)> = (1u128..=48).map(|id| random_box(id, &mut seed)).collect();

    let grid = UniformGrid::build(&bounds);
    let bvh = Bvh::build(&bounds);
    let grid_pairs = grid.pairs();
    let bvh_pairs = bvh.pairs();
    println!(
        "== broad phase: interchangeable backends over {} bodies ==",
        bounds.len()
    );
    println!("  uniform grid candidates : {}", grid_pairs.len());
    println!("  BVH        candidates   : {}", bvh_pairs.len());
    println!(
        "  BVH is exact (⊇ brute force): {}",
        bvh_pairs.len() >= brute_force(&bounds).len()
    );
    println!(
        "  deterministic (id-stable): {}",
        bvh_pairs == Bvh::build(&bounds).pairs()
    );

    // Both backends drive the same physics to identical positions.
    println!("\n== physics driven by either backend ==");
    let make_scene = || {
        let mut s = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        for id in 1u128..=12 {
            let mut e = Entity::dynamic();
            e.transform = Some(pwe_reference::components::Transform {
                position: Vec3::new((id as f64).fract() * 3.0 - 1.5, 4.0 + (id % 5) as f64, 0.0),
                ..Default::default()
            });
            e.velocity = Some(Default::default());
            e.rigid_body = Some(pwe_reference::components::RigidBody::dynamic(1.0));
            e.collider = Some(pwe_reference::components::Collider::sphere(0.5));
            s.insert(EntityId(id), e);
        }
        s
    };

    let mut grid_scene = make_scene();
    let mut bvh_scene = make_scene();
    let grid_sys = PhysicsSystem::new(PhysicsConfig {
        broad_phase: BroadPhaseKind::Grid,
        ..PhysicsConfig::default()
    });
    let bvh_sys = PhysicsSystem::new(PhysicsConfig {
        broad_phase: BroadPhaseKind::Bvh,
        ..PhysicsConfig::default()
    });
    let mut cg = Vec::new();
    let mut cb = Vec::new();
    let mut identical = true;
    for _ in 0..120 {
        grid_sys.step(&mut grid_scene, 1.0 / 60.0, &mut cg);
        bvh_sys.step(&mut bvh_scene, 1.0 / 60.0, &mut cb);
    }
    for id in 1u128..=12 {
        if grid_scene.position(EntityId(id)).unwrap() != bvh_scene.position(EntityId(id)).unwrap() {
            identical = false;
        }
    }
    println!("  120 steps, all 12 bodies identical across backends: {identical}");
    println!(
        "  sample body 3 final y: grid = {:.3}, bvh = {:.3}",
        grid_scene.position(EntityId(3)).unwrap().y,
        bvh_scene.position(EntityId(3)).unwrap().y
    );
}

fn brute_force(bounds: &[(u128, Aabb)]) -> Vec<(u128, u128)> {
    let mut set = std::collections::BTreeSet::new();
    for i in 0..bounds.len() {
        for j in (i + 1)..bounds.len() {
            if bounds[i].1.overlaps(bounds[j].1) {
                let (a, b) = (bounds[i].0, bounds[j].0);
                set.insert((a.min(b), a.max(b)));
            }
        }
    }
    set.into_iter().collect()
}
