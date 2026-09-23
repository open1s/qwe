//! Carbon molecule structure demo, served **live** over HTTP.
//!
//! Shows two iconic carbon molecules in a Three.js viewport:
//!   * **CO₂** (carbon dioxide) — linear O=C=O, two carbon–oxygen bonds.
//!   * **CH₄** (methane) — tetrahedral, carbon at the center bonded to four
//!     hydrogens (bond angle 109.5°).
//!
//! Atoms are expressed as `dynamic = false` entities (fixed position, mass →
//! visual size, element color); bonds are attached via `present::with_bonds`
//! and drawn as lines between bonded atoms. This is pure world/schema data in
//! the PWE language — no hard-coded 3D scene.
//!
//! Run: `cargo run --example molecule_demo [port]`  then open the printed URL.

use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)

        # --- Carbon dioxide CO2: linear O=C=O ---
        entity c_co2 { state = (-3.0, 0.0, 0.0, 0,0,0, 12.0); dynamic = false; color = 0x555555 }
        entity o1     { state = (-1.80, 0.0, 0.0, 0,0,0, 16.0); dynamic = false; color = 0xff4d4d }
        entity o2     { state = (-4.20, 0.0, 0.0, 0,0,0, 16.0); dynamic = false; color = 0xff4d4d }

        # --- Methane CH4: tetrahedral ---
        entity c_ch4 { state = ( 3.0, 0.0, 0.0, 0,0,0, 12.0); dynamic = false; color = 0x555555 }
        entity h1     { state = ( 3.63,  0.63,  0.63, 0,0,0, 1.0); dynamic = false; color = 0xffffff }
        entity h2     { state = ( 3.63, -0.63, -0.63, 0,0,0, 1.0); dynamic = false; color = 0xffffff }
        entity h3     { state = ( 2.37,  0.63, -0.63, 0,0,0, 1.0); dynamic = false; color = 0xffffff }
        entity h4     { state = ( 2.37, -0.63,  0.63, 0,0,0, 1.0); dynamic = false; color = 0xffffff }
    }

    systems {}
"#;

// Entity ids are assigned in declaration order starting at 1.
const C_CO2: u128 = 1;
const O1: u128 = 2;
const O2: u128 = 3;
const C_CH4: u128 = 4;
const H1: u128 = 5;
const H2: u128 = 6;
const H3: u128 = 7;
const H4: u128 = 8;

fn bond_len(rt: &LangRuntime, a: u128, b: u128) -> f64 {
    let pa = rt
        .scene
        .get(pwe_api::EntityId(a))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let pb = rt
        .scene
        .get(pwe_api::EntityId(b))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let dx = pa.values[0] - pb.values[0];
    let dy = pa.values[1] - pb.values[1];
    let dz = pa.values[2] - pb.values[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8001);
    let rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(
        Arc::clone(&live),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port,
    )
    .expect("serve live viewer");
    println!("live carbon molecule: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(4.0, 3.0, 5.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };
    let bonds = [
        (C_CO2, O1),
        (C_CO2, O2),
        (C_CH4, H1),
        (C_CH4, H2),
        (C_CH4, H3),
        (C_CH4, H4),
    ];

    let mut frame_no = 0u64;
    loop {
        frame_no += 1;
        let mut g = live.write().unwrap();
        g.step = frame_no;
        let base = rt.present_frame(Some(cam));
        g.frame = present::with_bonds(&base, &bonds);
        g.info = vec![
            "carbon molecules — CO2 (linear) & CH4 (tetrahedral)".to_string(),
            format!(
                "CO2: C=O {:.3}, {:.3}   CH4: C-H {:.3}, {:.3}, {:.3}, {:.3}",
                bond_len(&rt, C_CO2, O1),
                bond_len(&rt, C_CO2, O2),
                bond_len(&rt, C_CH4, H1),
                bond_len(&rt, C_CH4, H2),
                bond_len(&rt, C_CH4, H3),
                bond_len(&rt, C_CH4, H4)
            ),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
