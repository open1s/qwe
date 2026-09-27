//! A live solar system: a star with all 8 planets plus Earth's Moon — every
//! body **revolves** (公转) around the Sun via `nbody` and **self-rotates**
//! (自转) via an `update` rule that advances a spin angle (`state[7]`).
//!
//! Run: `cargo run --example solar_demo`  then open `http://localhost:8000`.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOLAR: &str = r#"
    world {
        gravity = (0, 0, 0)
        # Barycentric initial state (total momentum 0): real orbital-radius
        # ratios scaled so Earth is at r = 4, real planet masses scaled to
        # M_sun = 1000, each at its circular speed. The Sun recoils and is
        # offset from the origin — required for a stable many-body solution.
        entity sun      { state = (-0.029169, -0.01204, 0.0, 0.004654, -0.006533, 0.0, 1000, 0); color = 0xFFD24A }
        entity mercury  { state = (-0.501099, -1.486349, 0.0, 24.211153, -7.755095, 0.0, 1.66e-4, 0); color = 0xB0A8A0 }
        entity venus    { state = (-2.919442, -0.111961, 0.0, 0.64713, -18.590631, 0.0, 2.447e-3, 0); color = 0xFFB347 }
        entity earth    { state = (-0.755365, 3.921487, 0.0, -15.543979, -2.877076, 0.0, 3.003e-3, 0); color = 0x4aa8ff }
        entity moon     { state = (-0.757232, 3.9316, 0.0, -16.075391, -2.975183, 0.0, 3.69e-5, 0); color = 0xFFFFFF }
        entity mars     { state = (6.047451, -0.497751, 0.0, 1.025148, 12.760635, 0.0, 3.226e-4, 0); color = 0xFF6B4A }
        entity jupiter  { state = (17.153348, 11.731063, 0.0, -3.906565, 5.716365, 0.0, 0.9543, 0); color = 0xE8C39E }
        entity saturn   { state = (24.451066, 29.245232, 0.0, -3.922032, 3.279016, 0.0, 0.2857, 0); color = 0xEED484 }
        entity uranus   { state = (53.350601, -55.172695, 0.0, 2.598397, 2.50347, 0.0, 0.04365, 0); color = 0x8FE3E8 }
        entity neptune  { state = (67.838429, -99.315956, 0.0, 2.385198, 1.62041, 0.0, 0.05148, 0); color = 0x5B6EE8 }
    }
    systems {
        # Revolution (公转): mutual gravity among the Sun, 8 planets, and Moon.
        nbody { G = 1.0; dt = 0.004 }
        # Self-rotation (自转): every body spins about Z (visible as it revolves).
        update { dt = 0.004; s7 = s7 + inte(  0.3 ) }
    }
"#;

fn orbit_r(rt: &LangRuntime, id: u128) -> f64 {
    let st = rt
        .scene
        .get(EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    (st.values[0].powi(2) + st.values[1].powi(2)).sqrt()
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
    let dx = sa.values[0] - sb.values[0];
    let dy = sa.values[1] - sb.values[1];
    (dx * dx + dy * dy).sqrt()
}

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let mut rt = LangRuntime::compile(SOLAR)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(
        Arc::clone(&live),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port,
    )
    .expect("serve live viewer");
    println!("live solar system (8 planets revolve + self-rotate; Moon orbits Earth): open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(24.0, 18.0, 26.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };

    let mut frame_no = 0u64;
    loop {
        rt.step_cross()?;
        frame_no += 1;
        let mut g = live.write().unwrap();
        g.step = frame_no;
        g.frame = rt.present_frame(Some(cam));
        g.info = vec![
            format!("step {frame_no}: solar system (8 planets + Moon)"),
            format!(
                "merc {:.2} venus {:.2} earth {:.2} mars {:.2} jup {:.2}",
                orbit_r(&rt, 2),
                orbit_r(&rt, 3),
                orbit_r(&rt, 4),
                orbit_r(&rt, 5),
                orbit_r(&rt, 6)
            ),
            format!(
                "sat {:.2} ura {:.2} nep {:.2}   moon: {:.2} from sun, {:.3} from earth",
                orbit_r(&rt, 7),
                orbit_r(&rt, 8),
                orbit_r(&rt, 9),
                orbit_r(&rt, 10),
                dist(&rt, 10, 4)
            ),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}
