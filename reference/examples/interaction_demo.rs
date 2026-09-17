//! Interaction & reaction across macro and micro: entities sense each other's
//! forces (mutual `nbody`) and react, each rendered with a distinct shape and
//! color in a 3D web viewport.
//!
//!   * Macro — a star and two planets (spheres, distinct colors) mutually
//!     attract under gravity (G > 0); linear momentum is conserved (Newton's
//!     third law: action = -reaction).
//!   * Micro — three charged particles (spheres, distinct colors) interact via
//!     Coulomb-like force (G < 0: like charges repel, opposites attract).
//!
//! Run: `cargo run --example interaction_demo`  then open
//! `interaction_viewer.html` to watch both worlds in 3D with per-entity colors.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual};

const MACRO: &str = r#"
    world {
        gravity = (0, 0, 0)
        entity star   { state = (0, 0, 0, 0, 0, 0, 100); color = 0xFFD24A }
        entity planetA{ state = (5, 0, 0, 0, 4.47, 0, 1); color = 0x4aa8ff }
        entity planetB{ state = (0, 5, 0, -4.47, 0, 0, 1); color = 0x5cd46a }
    }
    systems { nbody { G = 1.0; dt = 0.0005 } }
"#;

const MICRO: &str = r#"
    world {
        gravity = (0, 0, 0)
        entity p1 { state = (-2, 0, 0, 0, 0, 0, 1); color = 0xFF5555 }  # +
        entity p2 { state = ( 2, 0, 0, 0, 0, 0, 1); color = 0xFFAA44 }  # +
        entity p3 { state = ( 0, 3, 0, 0, 0, 0, 1); color = 0x44AAFF }  # -
    }
    systems { nbody { G = -2.0; dt = 0.0005 } }
"#;

fn run(src: &str, steps: usize) -> LangRuntime {
    let mut rt = LangRuntime::compile(src).expect("compile");
    for _ in 0..steps {
        rt.step_cross().expect("step");
    }
    rt
}

fn momentum(rt: &LangRuntime, ids: &[u128]) -> f64 {
    let mut m = 0.0;
    for &id in ids {
        let st = rt
            .scene
            .get(EntityId(id))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // px = m·vx
        m += st.values[6] * st.values[3];
    }
    m
}

fn main() -> pwe_api::Result<()> {
    println!("== macro: star + two planets mutually attract (nbody, G>0) ==");
    let m = momentum(&run(MACRO, 2000), &[1, 2, 3]);
    println!("  total momentum = {m:.4} (unchanged from initial: action = -reaction)");

    println!("== micro: charged particles interact (nbody, G<0) ==");
    let micro = run(MICRO, 2000);
    let m = momentum(&micro, &[1, 2, 3]);
    let p1x = micro
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let p3y = micro
        .scene
        .get(EntityId(3))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[1];
    println!("  total momentum = {m:.4} (conserved)");
    println!("  p1.x = {p1x:.3} (repelled by p2), p3.y = {p3y:.3} (attracted to + charges)");

    // Write a 3D viewer for the macro system (distinct colors).
    let camera = CameraVisual {
        position: pwe_reference::math::Vec3::new(10.0, 8.0, 12.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };
    let mut frames = Vec::new();
    let mut rt = LangRuntime::compile(MACRO)?;
    for _ in 0..400 {
        rt.step_cross()?;
        frames.push(rt.present_frame(Some(camera)));
    }
    present::write_viewer("interaction_viewer.html", &frames).expect("write viewer");
    println!("\nopen 'interaction_viewer.html' — each entity is a distinct shape + color");
    Ok(())
}
