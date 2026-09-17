//! Presentation layer demo: run a physics simulation, snapshot frames, and write
//! a self-contained 3D web viewport (`science_viewer.html`) to view in a browser.
//!
//! Run: `cargo run --example present_demo`  then open `science_viewer.html`.
//!
//! The generated HTML uses Three.js (loaded from a CDN) and plays the frames
//! back with orbit controls, a timeline slider, and a state/channel panel.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual};

const SOURCE: &str = r#"
    world {
        gravity = (0, -9.81, 0)
        entity vehicle {
            position = (0, 6, 0); velocity = (3, 0, 0)
            mass = 4; dynamic = true; box = (1, 0.5, 0.7)
        }
        entity ground {
            position = (0, -2, 0); dynamic = false; box = (40, 2, 40)
        }
        chan telemetry { value = 0 }
    }
    systems {
        gravity { gravity_y = -9.81; dt = 1 / 60 }
        force { ax = 0.5; ay = 0; az = 0; dt = 1 / 60 }
        integrate { dt = 1 / 60 }
        ground_contact { restitution = 0.5 }
        send { chan = telemetry; value = s0 + s1 }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "science_viewer.html".to_string());

    let mut frames = Vec::new();
    let camera = CameraVisual {
        position: pwe_reference::math::Vec3::new(12.0, 10.0, 14.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };
    for _ in 0..120 {
        rt.step_cross()?; // 2 seconds of simulation
        frames.push(rt.present_frame(Some(camera)));
    }

    present::write_viewer(&out, &frames).expect("write viewer");
    let body = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.transform)
        .unwrap()
        .position;
    println!(
        "recorded {} frames; vehicle at ({:.2}, {:.2}, {:.2})",
        frames.len(),
        body.x,
        body.y,
        body.z
    );
    println!("open '{out}' in a browser to watch the 3D simulation");
    Ok(())
}
