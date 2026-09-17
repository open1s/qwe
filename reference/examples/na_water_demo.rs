//! **Water + sodium reaction as a particle-level fusion, live.**
//!
//! Shows the **atomic/molecular combination** of `Na + H₂O → NaOH + ½H₂`, not
//! just bulk concentrations. Three kinds of particles move in the 3D viewport:
//!
//! * a **sodium atom** (silver) and a **water molecule** (blue) start on
//!   opposite sides and **converge toward the centre** — they fuse;
//! * at the fusion point a **NaOH** body (white) **grows** as the product forms,
//!   and the reactor **heats** (`T` 300 → ~800 K);
//! * a **hydrogen** bubble (cyan) is **released and rises** out of the scene,
//!   sized by `[H₂]`.
//!
//! The master kinetics live on the central NaOH body (mass action, Arrhenius
//! `k(T)`, exotherm feedback); the particles read it via cross-entity
//! references. When sodium is exhausted the demo **resets and loops**, so the
//! fusion plays continuously. `interpreter == JIT` every step.
//!
//! Run: `cargo run --example na_water_demo [port]`  then open the printed URL.

use pwe_api::EntityId;
use pwe_reference::components::State;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        # Central NaOH body runs the kinetics. Named slots: position 0..2,
        # [Na], [H2O], temp, [NaOH] (drives visual size), [H2].
        entity naoh {
            state = (0, 0, 0, na = 2.0, water = 100.0, temp = 300.0, naoh = 0.0, h2 = 0.0)
            color = 0xe8e8ec
        }
        # Sodium atom (left) and water molecule (right): converge to fuse.
        entity na    { state = (x = -5.0, y = 0.0, z = 0.0, 0, 0, 0, 0.9, 0); color = 0xc0c4cc }
        entity water { state = (x =  5.0, y = 0.0, z = 0.0, 0, 0, 0, 1.4, 0); color = 0x2e6bd0 }
        # Hydrogen bubble (cyan): released at the fusion point, rises and recycles.
        entity h2    { state = (x = 0.0, y = 0.0, z = 0.0, 0, 0, 0, 0.3, 0); color = 0x38e1ff }
    }
    systems {
        # Master kinetics on the NaOH body: mass action, Arrhenius, exotherm.
        update { on = naoh; dt = 0.0005
            let k = 1.2 * exp(-900.0 / temp)
            na    = -k * na * water
            water = -k * na * water
            naoh  =  k * na * water
            h2    = 0.5 * k * na * water
            temp  = 260.0 * k * na * water
        }
        # Sodium atom: converge to the centre (fuse) and shrink as [Na] falls.
        update { on = na; dt = 0.0005
            let conv = -40.0
            x = conv * x
            y = conv * y
            z = conv * z
            s6 = 500.0 * (@naoh.na * 0.5 - s6) }
        # Water molecule: converge to the centre (fuse), stays large (excess).
        update { on = water; dt = 0.0005
            let conv = -40.0
            x = conv * x
            y = conv * y
            z = conv * z
            s6 = 500.0 * (@naoh.water * 0.014 - s6) }
        # Hydrogen bubble: rise out of the scene, sized by [H2], recycle at top.
        update { on = h2; dt = 0.0005
            let up = 3.0
            y = if(y > 5.0, -3.0 - y, up)
            s6 = 500.0 * (@naoh.h2 * 0.5 - s6) }
    }
"#;

/// Initial state for one reaction cycle, keyed by entity id.
fn reset_cycle(rt: &mut LangRuntime) {
    let init: &[(u128, [f64; 8])] = &[
        (1, [0.0, 0.0, 0.0, 2.0, 100.0, 300.0, 0.0, 0.0]), // naoh (reactor)
        (2, [-5.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0]),    // na atom
        (3, [5.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.4, 0.0]),     // water molecule
        (4, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.0]),     // h2 bubble
    ];
    for (id, vals) in init {
        if let Some(e) = rt.scene.get_mut(EntityId(*id)) {
            e.state = Some(State::new(vals.to_vec()));
        }
    }
}

fn c(rt: &LangRuntime, slot: usize) -> f64 {
    rt.scene
        .get(EntityId(1))
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
    present::serve_live(Arc::clone(&live), port).expect("serve live viewer");
    println!("live Na + H2O fusion (loops): open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(0.0, 2.0, 18.0),
        target: pwe_reference::math::Vec3::new(0.0, 0.0, 0.0),
    };

    let mut step = 0u64;
    let mut cycle = 0u64;
    loop {
        rt.step_cross()?; // interpreter == JIT, every step
        step += 1;

        // When sodium is exhausted, reset and show the fusion again.
        if c(&rt, 3) < 0.02 {
            cycle += 1;
            reset_cycle(&mut rt);
        }

        let mut g = live.write().unwrap();
        g.step = step;
        g.frame = rt.present_frame(Some(cam));
        let (na, w, oh, h2, t) = (c(&rt, 3), c(&rt, 4), c(&rt, 6), c(&rt, 7), c(&rt, 5));
        g.info = vec![
            format!("cycle {cycle}: Na + H2O fuse -> NaOH, releases H2  (cross-backend)"),
            format!("[Na] {na:6.3}  [H2O] {w:7.2}  [NaOH] {oh:7.3}  [H2] {h2:7.3}"),
            format!("T = {t:6.1} K   (exothermic Arrhenius: k = A·e^(−Ea/T))"),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
