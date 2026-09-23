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
        entity sun    { state = (0, 0, 0, 0, 0, 0, 1000000, 0); color = 0xFFD24A }
        entity mercury{ state = (2, 0, 0, 0, 707, 0, 0.06, 0);  color = 0xB0A8A0 }
        entity venus  { state = (3, 0, 0, 0, 577, 0, 0.82, 0);  color = 0xFFB347 }
        entity earth  { state = (4, 0, 0, 0, 500, 0, 1, 0);     color = 0x4aa8ff }
        entity mars   { state = (5.5, 0, 0, 0, 426, 0, 0.11, 0);color = 0xFF6B4A }
        entity jupiter{ state = (8, 0, 0, 0, 354, 0, 318, 0);   color = 0xE8C39E }
        entity saturn { state = (10, 0, 0, 0, 316, 0, 95, 0);   color = 0xEED484 }
        entity uranus { state = (13, 0, 0, 0, 277, 0, 15, 0);   color = 0x8FE3E8 }
        entity neptune{ state = (16, 0, 0, 0, 250, 0, 17, 0);   color = 0x5B6EE8 }
        # Moon: a bright small body revolving near Earth's orbit.
        entity moon   { state = (4.2, 0, 0, 0, 488, 0, 0.01, 0); color = 0xFFFFFF }
    }
    systems {
        # Revolution (公转): mutual gravity among the Sun, 8 planets, and Moon.
        nbody { G = 1.0; dt = 0.0001 }
        # Self-rotation (自转): every body spins about Z (visible as it revolves).
        update { dt = 0.0001; s7 = s7 + 0.3 }
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
    println!("live solar system (8 planets revolve + self-rotate, Moon follows Earth): open http://localhost:{port}");

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
