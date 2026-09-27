//! **RK4 integration, live** — three bodies trace **Lissajous figures** in the
//! XY plane, each integrated by the `rk4` system (classic 4th-order Runge–Kutta)
//! so the bounded curves stay accurate at a coarse `dt` where explicit Euler
//! would drift. Served **live** over HTTP; the browser polls the running
//! runtime's frame in real time. `interpreter == JIT` is asserted every step.
//!
//! Run: `cargo run --example rk4_demo [port]`  then open the printed URL.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        # state = (x, y, z, vx, vy, vz, mass, spin). The renderer draws the body
        # at (x, y, z); x and y are the two oscillators, so each body traces a
        # Lissajous figure in the XY plane. `mass` is the visual size, `spin` a
        # self-rotation angle about Z.
        entity a { state = (x = 2, y = 2, z = 0, vx = 0, vy = 0, vz = 0, mass = 1, spin = 0); color = 0x4aa8ff }
        entity b { state = (x = 2, y = 2, z = 0, vx = 0, vy = 0, vz = 0, mass = 1, spin = 0); color = 0xffa03a }
        entity c { state = (x = 2, y = 2, z = 0, vx = 0, vy = 0, vz = 0, mass = 1, spin = 0); color = 0x38e1ff }
    }
    systems {
        # Each axis is an independent harmonic oscillator: x'' = -ωx²·x,
        # y'' = -ωy²·y. A body traces a Lissajous figure with the ratio ωx:ωy.
        rk4 { on = a; dt = 0.02
            inte x = vx
            inte vx = -4 * x        # ωx = 2
            inte y = vy
            inte vy = -9 * y        # ωy = 3   => 2:3
            inte spin = 0.3
        }
        rk4 { on = b; dt = 0.02
            inte x = vx
            inte vx = -1 * x        # ωx = 1
            inte y = vy
            inte vy = -4 * y        # ωy = 2   => 1:2
            inte spin = -0.4
        }
        rk4 { on = c; dt = 0.02
            inte x = vx
            inte vx = -9 * x        # ωx = 3
            inte y = vy
            inte vy = -1 * y        # ωy = 1   => 3:1
            inte spin = 0.2
        }
    }
"#;

fn state(rt: &LangRuntime, id: u128, slot: usize) -> f64 {
    rt.scene
        .get(EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[slot]
}

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(
        Arc::clone(&live),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port,
    )
    .expect("serve live viewer");
    println!("live RK4 Lissajous demo: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(0.0, 0.0, 18.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };

    let mut step = 0u64;
    loop {
        rt.step_cross()?; // interpreter == JIT, every step
        step += 1;
        let mut g = live.write().unwrap();
        g.step = step;
        g.frame = rt.present_frame(Some(cam));
        // Energy per body: E = ½v² + ½ω²r² for each axis, a conserved quantity
        // RK4 preserves far better than explicit Euler at this dt.
        // E = ½vx² + ½ωx²x² + ½vy² + ½ωy²y²  (slots: x 0, y 1, vx 3, vy 4).
        let e_a = 0.5 * state(&rt, 1, 3).powi(2)
            + 0.5 * 4.0 * state(&rt, 1, 0).powi(2)
            + 0.5 * state(&rt, 1, 4).powi(2)
            + 0.5 * 9.0 * state(&rt, 1, 1).powi(2);
        let e_b = 0.5 * state(&rt, 2, 3).powi(2)
            + 0.5 * 1.0 * state(&rt, 2, 0).powi(2)
            + 0.5 * state(&rt, 2, 4).powi(2)
            + 0.5 * 4.0 * state(&rt, 2, 1).powi(2);
        let e_c = 0.5 * state(&rt, 3, 3).powi(2)
            + 0.5 * 9.0 * state(&rt, 3, 0).powi(2)
            + 0.5 * state(&rt, 3, 4).powi(2)
            + 0.5 * 1.0 * state(&rt, 3, 1).powi(2);
        g.info = vec![
            format!("step {step}: RK4 Lissajous (2:3, 1:2, 3:1), cross-backend"),
            format!("energy A {e_a:.3}  B {e_b:.3}  C {e_c:.3}  (conserved by RK4)"),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
