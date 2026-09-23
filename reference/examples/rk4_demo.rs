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
        # Position renders from state[0..2] (x, y, z); state[6] is mass (visual
        # size); state[7] is the spin angle (self-rotation).
        entity a { state = (2, 0, 2, 0, 0, 0, 1, 0); color = 0x4aa8ff }
        entity b { state = (2, 0, 2, 0, 0, 0, 1, 0); color = 0xffa03a }
        entity c { state = (2, 0, 2, 0, 0, 0, 1, 0); color = 0x38e1ff }
    }
    systems {
        # x'' = -ωx²·x, y'' = -ωy²·y  =>  Lissajous ωx:ωy. RK4 keeps them closed.
        rk4 { on = a; dt = 0.02
            s0 = s1;  s1 = -4 * s0        # ωx:ωy = 2:3
            s2 = s3;  s3 = -9 * s2
            s7 = 0.3
        }
        rk4 { on = b; dt = 0.02
            s0 = s1;  s1 = -1 * s0        # 1:2
            s2 = s3;  s3 = -4 * s2
            s7 = -0.4
        }
        rk4 { on = c; dt = 0.02
            s0 = s1;  s1 = -9 * s0        # 3:1
            s2 = s3;  s3 = -1 * s2
            s7 = 0.2
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
        let e_a = 0.5 * state(&rt, 1, 1).powi(2)
            + 0.5 * 4.0 * state(&rt, 1, 0).powi(2)
            + 0.5 * state(&rt, 1, 3).powi(2)
            + 0.5 * 9.0 * state(&rt, 1, 2).powi(2);
        let e_b = 0.5 * state(&rt, 2, 1).powi(2)
            + 0.5 * 1.0 * state(&rt, 2, 0).powi(2)
            + 0.5 * state(&rt, 2, 3).powi(2)
            + 0.5 * 4.0 * state(&rt, 2, 2).powi(2);
        let e_c = 0.5 * state(&rt, 3, 1).powi(2)
            + 0.5 * 9.0 * state(&rt, 3, 0).powi(2)
            + 0.5 * state(&rt, 3, 3).powi(2)
            + 0.5 * 1.0 * state(&rt, 3, 2).powi(2);
        g.info = vec![
            format!("step {step}: RK4 Lissajous (2:3, 1:2, 3:1), cross-backend"),
            format!("energy A {e_a:.3}  B {e_b:.3}  C {e_c:.3}  (conserved by RK4)"),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
