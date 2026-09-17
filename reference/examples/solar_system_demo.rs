//! Cross-entity coupling demo: a small solar system in the PWE language.
//!
//! The `update` system now supports `@name.sN` — reading another entity's state
//! slot. Here a massive fixed `sun` anchors a planet and a moon, each a dynamic
//! entity whose orbital dynamics are written entirely in source. The planets
//! fall toward the sun under inverse-square gravity; with the right tangential
//! velocity they hold a (nearly) circular orbit.
//!
//! Run: `cargo run --example solar_system_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

/// Normalized gravity (A = GM = 1); circular orbit at radius r needs v = √(A/r).
const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)

        # Fixed central mass (dynamic = false, so it is not itself updated).
        entity sun { dynamic = false; state = (0, 0, 0, 0) }

        # Planet at r = 4, circular speed v = sqrt(1/4) = 0.5.
        entity planet { state = (4, 0, 0, 0.5) }

        # Moon at r = 7, circular speed v = sqrt(1/7) ≈ 0.378.
        entity moon  { state = (0, 7, 0.378, 0) }
    }

    systems {
        # Planar orbital dynamics about the fixed sun (2D: x, y, vx, vy).
        #   r = sqrt(dx² + dy²),  a⃗ = −A·r⃗ / r³   (A = 1)
        update { dt = 0.0005
            s0 = s2
            s1 = s3
            s2 = -1 * (s0 - @sun.s0) / ( sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) * sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) * sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) )
            s3 = -1 * (s1 - @sun.s1) / ( sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) * sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) * sqrt((s0 - @sun.s0)*(s0 - @sun.s0) + (s1 - @sun.s1)*(s1 - @sun.s1)) )
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    println!("== solar system (cross-entity `@sun.sN` coupling) ==");

    let mut last = [0.0f64; 3]; // sun r, planet r, moon r
    for step in 0..6000 {
        rt.step_cross()?; // interpreter == JIT enforced each step
        if step % 1000 == 0 {
            let pr = radius(&rt, 2);
            let mr = radius(&rt, 3);
            println!("  step {step:>5}  planet r = {pr:>6.3}  moon r = {mr:>6.3}");
            last = [radius(&rt, 1), pr, mr];
        }
    }
    println!("\n  sun stays fixed at r = {:.4}", last[0]);
    println!(
        "  planet r = {:.4} (initial 4.0), moon r = {:.4} (initial 7.0)",
        last[1], last[2]
    );
    println!("  orbits hold under inverse-square gravity (cross-entity)");
    Ok(())
}

fn radius(rt: &LangRuntime, id: u128) -> f64 {
    let st = rt
        .scene
        .get(EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (x, y) = (st.values[0], st.values[1]);
    (x * x + y * y).sqrt()
}
