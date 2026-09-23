//! **Live expressiveness showcase** — the PWE language's general-purpose
//! features served **live** over HTTP: **named state slots**, **rich math
//! builtins**, **cross-entity property access**, **user-defined functions**,
//! **`let` local variables**, and **`print(...)` debugging**.
//!
//! A target orbits the origin (its own named-state position), and a glider
//! chases it by reading `@target.tx`/`@target.ty`. Both render from their named
//! state in the 3D viewer, sized by their `mass` slot (`state[6]` → visual size)
//! and colored by `color`. Runs **cross-backend** (interpreter == JIT) every
//! step and is reproducible.
//!
//! Run: `cargo run --example expressive_demo [port]`  then open the printed URL.

use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        # state[6] is the mass (visual size); color sets the hue.
        entity target { state = (tx = 3, ty = 4, 0, 0, 0, 0, 5); color = 0x4aa8ff }
        entity glider { state = (x = 0, y = 0, 0, vx = 0, vy = 0, vz = 0, 1.6); color = 0xffa03a }
    }
    funcs {
        clamp(a, lo, hi) { if(s0 < s1, s1, if(s0 > s2, s2, s0)) }
    }
    systems {
        # Target orbits the origin (perpendicular velocity => circular motion).
        update { on = target; dt = 0.02
            let omega = 0.5
            tx = -@self.ty * omega
            ty = @self.tx * omega
        }
        # Glider chases the target, speed-limited.
        update { on = glider; dt = 0.02
            let dx = @target.tx - @self.x
            let dy = @target.ty - @self.y
            let gain = 1.4
            vx = clamp(dx * gain, -2.5, 2.5)
            vy = clamp(dy * gain, -2.5, 2.5)
            x = @self.vx
            y = @self.vy
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8003);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(
        Arc::clone(&live),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port,
    )
    .expect("serve live viewer");
    println!("live expressive demo: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(0.0, 0.0, 16.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };

    // A few warm-up steps to show the print/log channel.
    let mut step = 0u64;
    loop {
        rt.step_cross()?; // interpreter == JIT, every step
        step += 1;

        let target = rt
            .scene
            .get(pwe_api::EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let glider = rt
            .scene
            .get(pwe_api::EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let gap = ((target.values[0] - glider.values[0]).powi(2)
            + (target.values[1] - glider.values[1]).powi(2))
        .sqrt();

        let mut g = live.write().unwrap();
        g.step = step;
        g.frame = rt.present_frame(Some(cam));
        g.info = vec![
            format!("step {step}: target orbits, glider chases (cross-backend)"),
            format!(
                "glider ({:.2},{:.2})  target ({:.2},{:.2})  gap {gap:.2}",
                glider.values[0], glider.values[1], target.values[0], target.values[1]
            ),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
