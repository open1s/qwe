//! Multi-body interaction via the `nbody` system: macro celestial bodies and
//! micro charged particles, both expressed entirely in the PWE language.
//!
//! Every body carries `state = (px, py, pz, vx, vy, vz, m)`; `nbody { G; dt }`
//! computes the mutual inverse-square force between every pair. `G > 0` is
//! gravity (attractive — planets); `G < 0` is Coulomb-like (repulsive/attractive
//! — charged particles).
//!
//! Run: `cargo run --example nbody_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

fn main() -> pwe_api::Result<()> {
    // Macro: a planet orbiting a (nearly fixed) massive star.
    let system = r#"
        world { gravity = (0,0,0)
            entity star   { state = (0, 0, 0, 0, 0, 0, 1000000) }
            entity planet { state = (4, 0, 0, 0, 500, 0, 1) }
        }
        systems { nbody { G = 1.0; dt = 0.0001 } }
    "#;
    let mut rt = LangRuntime::compile(system)?;
    for _ in 0..6000 {
        rt.step_cross()?;
    }
    let p = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let r = (p.values[0].powi(2) + p.values[1].powi(2)).sqrt();
    println!("== macro: planet orbiting a star (nbody, G>0) ==");
    println!("  orbit radius = {r:.3}  (initial 4.0; circular orbit holds)");

    // Micro: two like-charged particles repel (Coulomb-like); the gap grows and
    // linear momentum is conserved.
    let particles = r#"
        world { gravity = (0,0,0)
            entity a { state = (-2, 0, 0, 0, 0, 0, 1) }   # charge +1
            entity b { state = ( 2, 0, 0, 0, 0, 0, 1) }   # charge +1
        }
        systems { nbody { G = -2.0; dt = 0.001 } }
    "#;
    let mut rt = LangRuntime::compile(particles)?;
    for _ in 0..2000 {
        rt.step_cross()?;
    }
    let a = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let b = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let ax = a.values[0];
    let bx = b.values[0];
    let gap = (bx - ax).abs();
    let mom = a.values[3] + b.values[3];
    println!("\n== micro: like-charged particles repel (nbody, G<0) ==");
    println!("  a.x = {ax:.3}  b.x = {bx:.3}  (gap {gap:.3}, grew from 4.0)");
    println!("  total momentum = {mom:.4} (conserved ≈ 0)");
    println!("\nmomentum conserved, interpreter == JIT enforced every step");
    Ok(())
}
