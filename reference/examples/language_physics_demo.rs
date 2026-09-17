//! How to use the PWE language: define a physics world in source, compile it to
//! low-level EIR, and run it cross-backend.
//!
//!  1. Write a `world { … }` model (gravity, entities with shape/physics) and a
//!     `systems { … }` list (gravity / integrate / damping / ground_contact).
//!  2. `lang::parse` reads it; `lang::compile` builds typed EIR systems.
//!  3. `LangRuntime` boots over the same IR; `step_cross` runs it on BOTH the
//!     interpreter and the CPU JIT and asserts byte-identical world writes.
//!
//! Run: `cargo run --example language_physics_demo`

use pwe_api::EntityId;
use pwe_reference::lang;

// A world model: one dynamic box "vehicle" launched across a static ground
// plane, plus a dynamic sphere "ball" dropped from higher up.
const SOURCE: &str = r#"
    # world model: gravity + entities
    world {
        gravity = (0, -9.81, 0)

        entity vehicle {
            position = (0, 8, 0); velocity = (4, 0, 0)
            mass = 4; dynamic = true; box = (1, 0.5, 0.7)
            restitution = 0.5
        }
        entity ball {
            position = (2, 12, 0); velocity = (0, 0, 0)
            mass = 1; dynamic = true; sphere = 0.5
        }
        entity wedge {
            position = (-2, 6, 0); velocity = (0, 0, 0)
            mass = 3; dynamic = true
            hull = [ (-0.5,0,-0.5), (0.5,0,-0.5), (-0.5,0,0.5), (0.5,0,0.5), (0,1,0) ]
        }
        entity ground {
            position = (0, -5, 0); dynamic = false; box = (50, 5, 50)
        }
    }

    # systems: ordered behavior that acts on the dynamic bodies
    systems {
        gravity        { gravity_y = -9.81; dt = 1 / 60 }
        force          { ax = 0.8; ay = 0; az = 0; dt = 1 / 60 }  # a +x wind
        integrate      { dt = 1 / 60 }
        ground_contact { restitution = 0.5 }
    }
"#;

fn main() -> pwe_api::Result<()> {
    // 1. Parse the source into a structured world model.
    let parsed = lang::parse(SOURCE)?;
    println!("== 1. parse: source -> world model ==");
    println!("   gravity       : {:?}", parsed.model.gravity);
    for e in &parsed.model.entities {
        let shape = match &e.collider {
            Some(pwe_reference::dsl::ColliderDecl::Box { dims }) => format!("box {dims:?}"),
            Some(pwe_reference::dsl::ColliderDecl::Sphere { radius }) => {
                format!("sphere r={radius}")
            }
            Some(pwe_reference::dsl::ColliderDecl::ConvexHull { points }) => {
                format!("convex hull ({} pts)", points.len())
            }
            None => "none".to_string(),
        };
        let kind = if e.camera == Some(true) {
            "camera"
        } else if e.dynamic == Some(false) {
            "static"
        } else {
            "dynamic"
        };
        println!("   entity {:<10} {kind:<8} {shape}", e.name);
    }
    for s in &parsed.systems {
        println!("   system {:<14} params: {:?}", s.kind, s.params);
    }

    // 2. Compile: model + systems -> typed low-level EIR.
    let compiled = lang::compile(SOURCE)?;
    println!("\n== 2. compile: world model -> EIR ==");
    println!("   dynamic bodies       : {:?}", compiled.program.entities);
    println!(
        "   compiled EIR functions: {}",
        compiled.eir.functions.len()
    );
    for f in &compiled.eir.functions {
        println!(
            "     fn {} ({} instructions, effect_mask=0x{:X})",
            f.id,
            f.instructions.len(),
            f.effect_mask
        );
    }

    // 3. Run cross-backend and read entity state back from the Scene.
    let mut rt = lang::LangRuntime::compile(SOURCE)?;
    println!("\n== 3. run: interpreter == JIT on every step ==");
    println!("   step | vehicle.y | ball.y | wedge.y | vehicle.vx");
    for frame in 0..=6 {
        if frame > 0 {
            rt.step_cross_n(30)?; // 30 fixed steps of 1/60 s each
        }
        let vy = rt.scene.position(EntityId(1))?.y;
        let by = rt.scene.position(EntityId(2))?.y;
        let wy = rt.scene.position(EntityId(3))?.y;
        let vx = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .map(|v| v.linear.x)
            .unwrap_or(0.0);
        println!(
            "   {:>4} | {:>9.3} | {:>6.3} | {:>7.3} | {:>9.2}",
            frame * 30,
            vy,
            by,
            wy,
            vx
        );
    }

    // The cross-backend invariant is enforced (and would panic) on every step.
    let settled = rt.scene.position(EntityId(1))?.y;
    let wedge_x = rt.scene.position(EntityId(3))?.x;
    let wedge_vx = rt
        .scene
        .get(EntityId(3))
        .and_then(|e| e.velocity)
        .unwrap()
        .linear
        .x;
    println!("\nvehicle settled y = {settled:.3} — interpreter and JIT agreed on every step");
    println!("wedge drifted +x under the force system: x = {wedge_x:.3}, vx = {wedge_vx:.2}");
    Ok(())
}
