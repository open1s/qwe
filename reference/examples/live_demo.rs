//! Live presentation demo: the HTML viewer interfaces with the running runtime
//! over HTTP to show the simulation in real time (not static frames).
//!
//! Run: `cargo run --example live_demo`  then open `http://localhost:8000`.
//!
//! The runtime steps the simulation, publishes a `LiveState` (frame + step +
//! procedure log), and serves it to the browser, which polls `/state` and
//! renders the 3D scene live.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, -9.81, 0)
        entity vehicle {
            position = (0, 6, 0); velocity = (3, 2, 0)
            mass = 4; dynamic = true; box = (1, 0.5, 0.7); color = 0x4aa8ff
        }
        entity ground {
            position = (0, -2, 0); dynamic = false; box = (20, 2, 20); color = 0x3a4a6b
        }
        # Visible walls: static boxes on the boundary of the `wall` domain.
        # They reach from the ground top (y = -1) up to y = 3 (center y=1, height 4),
        # so there is no gap between wall and ground.
        entity wall_n { position = (0, 1, -9); dynamic = false; box = (17, 4, 0.5); color = 0xFF5555 }
        entity wall_s { position = (0, 1,  9); dynamic = false; box = (17, 4, 0.5); color = 0xFF5555 }
        entity wall_e { position = ( 9, 1, 0); dynamic = false; box = (0.5, 4, 17); color = 0xFF7777 }
        entity wall_w { position = (-9, 1, 0); dynamic = false; box = (0.5, 4, 17); color = 0xFF7777 }
        chan telemetry { value = 0 }
    }
    systems {
        gravity { gravity_y = -9.81; dt = 1 / 60 }
        force { ax = 0.5; ay = 0; az = 0; dt = 1 / 60 }
        integrate { dt = 1 / 60 }
        ground_contact { restitution = 0.5 }
        # The wall clamps the vehicle *center*. The box half-width is 0.5, so use
        # 8.5 so the box's edge (9.0) stays inside the visible wall at 9.
        wall { x = 8.5; z = 8.5; restitution = 0.7 }
        send { chan = telemetry; value = s0 + s1 }
    }
"#;

/// Entity ids: vehicle=1, ground=2, walls 3..6, telemetry channel=7.
const TELEMETRY: u128 = 7;

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(Arc::clone(&live), port).expect("serve live viewer");
    println!("live viewer: open http://localhost:{port} in a browser");

    let mut frame_no = 0u64;
    loop {
        rt.step_cross()?;
        frame_no += 1;
        let vehicle = rt.scene.get(EntityId(1)).and_then(|e| e.transform).unwrap();
        let vel = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .unwrap()
            .linear;
        let telemetry = rt
            .scene
            .get(EntityId(TELEMETRY))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        let mut live_guard = live.write().unwrap();
        live_guard.frame = rt.present_frame(None);
        live_guard.step = frame_no;
        live_guard.info = vec![
            format!("step {frame_no}: gravity→force→integrate→ground_contact→send"),
            format!("vehicle y = {:.3}, vx = {:.2}", vehicle.position.y, vel.x),
            format!("telemetry = {:.3}", telemetry),
        ];
        drop(live_guard);
        // Small pause so the browser can observe the live progression.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
