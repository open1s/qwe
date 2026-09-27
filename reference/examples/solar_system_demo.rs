//! Cross-entity coupling demo: a small solar system in the PWE language.
//!
//! The `update` system supports `@name.sN` — reading another entity's state slot.
//! A fixed massive `sun` anchors a planet, and the planet's gravity anchors a
//! moon. Every acceleration is Newton's inverse-square law, written entirely in
//! source: the planet falls toward the sun; the moon feels the sun *and* the
//! planet, and is bound to the planet (it sits inside the planet's Hill radius),
//! so it orbits the planet while the planet orbits the sun.
//!
//! Run: `cargo run --example solar_system_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

/// A_sun = GM_sun = 1; A_earth = GM_earth = 3e-6 (the real Earth/Sun mass ratio).
/// Circular orbit at radius r needs v = √(A/r); a body is bound to the planet
/// within its Hill radius r_H = a·(A_earth/3A_sun)^⅓ = 0.040.
const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)

        # Fixed central mass (dynamic = false, so it is not itself updated).
        entity sun { dynamic = false; state = (0, 0, 0, 0) }

        # Planet at r = 4, circular speed v = sqrt(1/4) = 0.5.
        entity planet { state = (4, 0, 0, 0.5) }

        # Moon at ~0.26 of the planet's Hill radius (d = 0.01028) with the
        # planet-relative circular speed sqrt(A_earth/d) = 0.01708: it orbits the
        # planet (~13.3 orbits per planet year) while the planet orbits the sun.
        entity moon  { state = (4.01028, 0, 0, 0.51708) }
    }

    systems {
        # Planet: inverse-square gravity toward the (fixed) sun.
        #   a⃗ = −A_sun·(r⃗ − r⃗_sun)/|r⃗ − r⃗_sun|³
        update { on = planet; dt = 0.0005
            let dx = s0 - @sun.s0
            let dy = s1 - @sun.s1
            let r  = sqrt(dx * dx + dy * dy)
            s0 = s0 + inte(  s2 )
            s1 = s1 + inte(  s3 )
            s2 = s2 + inte(  -1.0 * dx / (r * r * r) )
            s3 = s3 + inte(  -1.0 * dy / (r * r * r) )
        }
        # Moon: gravity from the sun AND from the planet.
        update { on = moon; dt = 0.0005
            let sx = s0 - @sun.s0
            let sy = s1 - @sun.s1
            let rs = sqrt(sx * sx + sy * sy)
            let ex = s0 - @planet.s0
            let ey = s1 - @planet.s1
            let re = sqrt(ex * ex + ey * ey)
            s0 = s0 + inte(  s2 )
            s1 = s1 + inte(  s3 )
            s2 = s2 + inte(  -1.0 * sx / (rs * rs * rs) - 3.0e-6 * ex / (re * re * re) )
            s3 = s3 + inte(  -1.0 * sy / (rs * rs * rs) - 3.0e-6 * ey / (re * re * re) )
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    println!("== solar system (cross-entity `@sun.sN` / `@planet.sN` coupling) ==");

    let mut last = (0.0f64, 0.0f64, 0.0f64);
    for step in 0..6000 {
        rt.step_cross()?; // interpreter == JIT enforced each step
        if step % 1000 == 0 {
            let pr = radius(&rt, 2);
            let md = dist(&rt, 3, 2);
            println!("  step {step:>5}  planet r = {pr:>6.3}  moon–planet = {md:>6.4}");
            last = (radius(&rt, 1), pr, md);
        }
    }
    println!("\n  sun stays fixed at r = {:.4}", last.0);
    println!(
        "  planet r = {:.4} (initial 4.0); moon–planet = {:.4} (initial 0.0103)",
        last.1, last.2
    );
    println!("  planet orbits the sun and the moon orbits the planet (inverse-square)");
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

fn dist(rt: &LangRuntime, a: u128, b: u128) -> f64 {
    let sa = rt
        .scene
        .get(EntityId(a))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let sb = rt
        .scene
        .get(EntityId(b))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (dx, dy) = (sa.values[0] - sb.values[0], sa.values[1] - sb.values[1]);
    (dx * dx + dy * dy).sqrt()
}
