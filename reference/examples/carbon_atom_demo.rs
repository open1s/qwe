//! Carbon atom (micro-scale physics) in the PWE language, served **live** over
//! HTTP so the browser connects to the running runtime in real time.
//!
//! Carbon has 6 electrons arranged in shells — K (n=1) holds 2, L (n=2) holds 4.
//! Each electron orbits a central nucleus (charge +6) under the inverse-square
//! Coulomb force, expressed purely with the language's `update` system (EIR,
//! cross-backend). A red nucleus sits at the origin; the live view shows the
//! electron shells with per-shell colors, orbit rings, and velocity arrows.
//!
//! Run: `cargo run --example carbon_atom_demo`  then open `http://localhost:8000`.

use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)

        # Nucleus at the origin (charge +6), not a dynamic body.
        entity nucleus { state = (0, 0, 0, 0, 0, 0, 6); dynamic = false; color = 0xFF3333 }

        # K shell (n=1): 2 electrons at r=1, v=sqrt(A/r)=1.
        entity k1 { state = ( 1.0, 0.0, 0.0,  1.0, 0, 0, 0.01); color = 0x4aa8ff }
        entity k2 { state = (-1.0, 0.0, 0.0, -1.0, 0, 0, 0.01); color = 0x4aa8ff }

        # L shell (n=2): 4 electrons at r=2, v=1/sqrt(2).
        entity l1 { state = ( 2.0, 0.0, 0.0,  0.70710678, 0, 0, 0.01); color = 0x5cd46a }
        entity l2 { state = (-2.0, 0.0, 0.0, -0.70710678, 0, 0, 0.01); color = 0x5cd46a }
        entity l3 { state = ( 0.0, 2.0, -0.70710678, 0.0, 0, 0, 0.01); color = 0x5cd46a }
        entity l4 { state = ( 0.0, -2.0, 0.70710678, 0.0, 0, 0, 0.01); color = 0x5cd46a }
    }

    systems {
        # Coulomb binding toward the nucleus at the origin (A=1):
        #   ax = -x / r³, ay = -y / r³,  x' = vx, y' = vy
        update { dt = 0.001
            s0 = s2
            s1 = s3
            s2 = -1 * s0 / ((s0*s0 + s1*s1) * sqrt(s0*s0 + s1*s1))
            s3 = -1 * s1 / ((s0*s0 + s1*s1) * sqrt(s0*s0 + s1*s1))
        }
    }
"#;

fn orbit_r(rt: &LangRuntime, id: u128) -> f64 {
    let st = rt
        .scene
        .get(pwe_api::EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    (st.values[0].powi(2) + st.values[1].powi(2)).sqrt()
}

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(Arc::clone(&live), port).expect("serve live viewer");
    println!("live carbon atom: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(4.0, 3.0, 4.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };

    let mut frame_no = 0u64;
    loop {
        rt.step_cross()?; // interpreter == JIT enforced every step
        frame_no += 1;
        let mut g = live.write().unwrap();
        g.step = frame_no;
        g.frame = rt.present_frame(Some(cam));
        g.info = vec![
            format!("step {frame_no}: carbon atom — K(2)/L(4) electron shells"),
            format!(
                "K shells r = {:.3}, {:.3}   L shells r = {:.3}, {:.3}, {:.3}, {:.3}",
                orbit_r(&rt, 2),
                orbit_r(&rt, 3),
                orbit_r(&rt, 4),
                orbit_r(&rt, 5),
                orbit_r(&rt, 6),
                orbit_r(&rt, 7)
            ),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}
