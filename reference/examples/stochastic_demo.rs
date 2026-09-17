//! **Stochastic ecology** — a non-physics requirement, expressed entirely in the
//! PWE language using the newly-added generality primitives `random()`, `time`
//! and `emit(...)`. This demonstrates the substrate is a *general-purpose*
//! simulation engine, not a physics-only DSL: a noisy logistic population with
//! a carrying capacity, demographic stochasticity, and per-step event emission.
//!
//! The model is a single-entity, three-slot `update` system:
//!   * `s0` = population (stochastic logistic growth + noise),
//!   * `s1` = visual y (population),
//!   * `s2` = visual x (elapsed time).
//!
//! It runs **cross-backend** (interpreter == JIT, enforced every step) and the
//! RNG is deterministically seeded, so two fresh runtimes reproduce the same
//! trajectory — the "general + reproducible" guarantee.
//!
//! Run: `cargo run --example stochastic_demo [port]`  then open the printed URL.

use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::Duration;

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        entity population { state = (0.3, 0, 0, 0) }
    }
    systems {
        # Stochastic logistic growth: dP/dt = r·P·(1 − P/K) + demographic noise.
        update { dt = 0.01
            s0 = (1.0 * s0 * (1 - s0)) + 0.2 * random()
            s1 = s0
            s2 = t
            s3 = emit(1, 0)
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8002);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(Arc::clone(&live), port).expect("serve live viewer");
    println!("live stochastic ecology: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(3.0, 2.0, 3.0),
        target: pwe_reference::math::Vec3::new(1.5, 0.5, 0.0),
    };

    let steps = 2000usize;
    let mut last = 0.0f64;
    for i in 0..steps {
        rt.step_cross()?; // interpreter == JIT enforced every step
        last = rt
            .scene
            .get(pwe_api::EntityId(1))
            .and_then(|e| e.state.as_ref())
            .map(|s| s.values[0])
            .unwrap_or(0.0);
        if i % 4 == 0 {
            let mut g = live.write().unwrap();
            g.step = i as u64;
            g.frame = rt.present_frame(Some(cam));
            g.info = vec![
                format!("step {i}/{steps}: stochastic logistic population"),
                format!(
                    "P = {last:.3}  (carrying capacity 1.0, r=1.8, σ=0.02)  events={}",
                    rt.emitted_events().len()
                ),
            ];
        }
    }

    // Generality proof assertions.
    assert!(
        last.is_finite() && last > 0.0 && last < 1.3,
        "population {last} stayed within the ecological bounds"
    );
    assert!(
        rt.emitted_events().len() >= steps,
        "events were emitted each step"
    );

    // Reproducibility: a fresh runtime with the same source reproduces the same
    // final population (seeded RNG) — the general + deterministic guarantee.
    let mut rt2 = LangRuntime::compile(SOURCE)?;
    for _ in 0..steps {
        rt2.step_cross()?;
    }
    let replay = rt2
        .scene
        .get(pwe_api::EntityId(1))
        .and_then(|e| e.state.as_ref())
        .map(|s| s.values[0])
        .unwrap();
    println!("final P = {last:.4}   replay P = {replay:.4}   (must match)");
    assert_eq!(replay, last, "reproducible stochastic trajectory");

    // A tiny ASCII trajectory of the noisy logistic growth.
    let mut rt3 = LangRuntime::compile(SOURCE)?;
    let mut line = String::from("P(t): ");
    for i in 0..120 {
        rt3.step_cross()?;
        if i % 2 == 0 {
            let p = rt3
                .scene
                .get(pwe_api::EntityId(1))
                .and_then(|e| e.state.as_ref())
                .unwrap()
                .values[0];
            line.push(match p {
                p if p < 0.1 => '.',
                p if p < 0.3 => '-',
                p if p < 0.55 => '*',
                p if p < 0.8 => 'o',
                _ => 'O',
            });
        }
    }
    println!("{line}");
    loop {
        sleep(Duration::from_millis(100));
    }
}
