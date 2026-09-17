//! The PWE language in action: parse source → compile to low-level EIR →
//! run cross-backend (interpreter + CPU JIT) through a runtime.
//!
//! Run with: `cargo run --example language_demo`

use pwe_api::EntityId;
use pwe_reference::lang::{compile, LangRuntime};

const SOURCE: &str = r#"
    world {
        gravity = (0, -9.81, 0)
        entity vehicle {
            position = (0, 8, 0)
            velocity = (4, 0, 0)
            mass = 4
            dynamic = true
            box = (1, 0.5, 0.7)
        }
        entity ground {
            position = (0, -5, 0)
            dynamic = false
            box = (50, 5, 50)
        }
        entity camera {
            position = (0, 16, 24)
            camera = true
        }
    }
    systems {
        gravity { gravity_y = -9.81; dt = 1 / 60 }
        integrate { dt = 1 / 60 }
        ground_contact { restitution = 0.6 }
    }
"#;

fn main() {
    // 1. Parse + compile to low-level EIR.
    let compiled = compile(SOURCE).expect("compile PWE source");
    println!(
        "compiled {} physics bodies -> {} EIR functions",
        compiled.program.entities.len(),
        compiled.eir.functions.len()
    );

    // 2. Boot a runtime over the same IR.
    let mut rt = LangRuntime::compile(SOURCE).expect("boot runtime");
    println!("\nstep    vehicle.y     vx     cross-backend agreement");

    for frame in 0..=8 {
        if frame > 0 {
            rt.step_cross_n(30).expect("step");
        }
        let y = rt.scene.position(EntityId(1)).unwrap().y;
        let vx = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .unwrap()
            .linear
            .x;
        let step = frame * 30;
        println!(
            "{:>5} {:>10.3} {:>7.2}  interpreter == JIT (asserted)",
            step, y, vx
        );
    }

    // 3. Cross-backend guarantee is enforced on every step.
    println!("\ncross-backend invariant: interpreter writes == JIT writes (enforced each step)");
    println!(
        "final vehicle y = {:.3}",
        rt.scene.position(EntityId(1)).unwrap().y
    );
}
