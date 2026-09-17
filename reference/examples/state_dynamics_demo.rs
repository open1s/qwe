//! User-defined dynamical systems in the PWE language. The generic `state`
//! entity field plus the `linear` system express a large class of physical
//! phenomena — radioactive decay, population growth, mixing, RLC circuits,
//! damped motion — entirely from source-declared matrices, no Rust needed.
//!
//! Run: `cargo run --example state_dynamics_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

fn main() -> pwe_api::Result<()> {
    // Radioactive decay:  N' = -λ N,  λ = 0.05 / step.
    let decay = r#"
        world { gravity = (0,0,0) entity isotope { state = (100, 0) } }
        systems { linear { slots = 2; dt = 1; row0 = (-0.05, 0, 0); row1 = (0, 0, 0) } }
    "#;
    let mut rt = LangRuntime::compile(decay)?;
    for _ in 0..50 {
        rt.step_cross()?;
    }
    let n = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    println!("== radioactive decay  N' = -λN,  λ=0.05/step, 50 steps ==");
    println!(
        "  N(50) = {n:.2}  (analytic 100·0.95^50 = {:.2})",
        100.0 * 0.95f64.powi(50)
    );

    // Harmonic oscillator (spring):  x' = v,  v' = -ω²x.
    let spring = r#"
        world { gravity = (0,0,0) entity mass { state = (1, 0) } }
        systems { linear { slots = 2; dt = 0.0005; row0 = (0, 1, 0); row1 = (-100, 0, 0) } }
    "#;
    let mut rt = LangRuntime::compile(spring)?;
    let mut x = 0.0;
    let mut v = 0.0;
    for k in 0..4000 {
        rt.step_cross()?;
        if k % 500 == 0 {
            let st = rt
                .scene
                .get(EntityId(1))
                .and_then(|e| e.state.as_ref())
                .unwrap();
            x = st.values[0];
            v = st.values[1];
            println!("  t={:>4.2}  x={x:>6.3}  v={v:>6.3}", k as f64 * 0.0005);
        }
    }
    let energy = 0.5 * v * v + 0.5 * 100.0 * x * x;
    println!("== harmonic oscillator  x'' = -100 x  ==");
    println!("  mechanical energy E = {energy:.4} (initial 50; explicit-Euler drift is bounded)");

    // Logistic growth (nonlinear):  N' = r·N·(1−N), expressed with the `update`
    // system's scalar-expression rules.
    let logistic = r#"
        world { gravity = (0,0,0) entity pop { state = (0.1, 0) } }
        systems { update { dt = 0.01
            s0 = 1 * s0 * (1 - s0) } }
    "#;
    let mut rt = LangRuntime::compile(logistic)?;
    let mut last = 0.0;
    for k in 0..500 {
        rt.step_cross()?;
        last = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        if k % 100 == 0 {
            println!("  t={k:>4}  N={last:.4}");
        }
    }
    println!("== logistic growth  N' = N(1−N), N0=0.1 ==");
    println!("  N(5.0) = {last:.4} (→ carrying capacity 1)");

    // Nonlinear pendulum (macro mechanics):  θ'' = −(g/L)·sin(θ), via sin().
    let pend = r#"
        world { gravity = (0,0,0) entity pend { state = (1.2, 0) } }
        systems { update { dt = 0.0005
            s0 = s1
            s1 = -9.81 * sin(s0) } }
    "#;
    let mut rt = LangRuntime::compile(pend)?;
    let mut theta = 0.0;
    let mut omega = 0.0;
    let mut e_min = f64::INFINITY;
    let mut e_max = f64::NEG_INFINITY;
    for _ in 0..4000 {
        rt.step_cross()?;
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        theta = st.values[0];
        omega = st.values[1];
        let e = 0.5 * omega * omega + 9.81 * (1.0 - theta.cos());
        e_min = e_min.min(e);
        e_max = e_max.max(e);
    }
    println!("== nonlinear pendulum  θ'' = −g·sin(θ), L=1 ==");
    println!("  θ(2.0s) = {theta:.3}, ω = {omega:.3}, energy band [{e_min:.4}, {e_max:.4}]");

    // Exact exponential growth (micro-scale process):  N' = 0.5·N.
    let expo = r#"
        world { gravity = (0,0,0) entity n { state = (1, 0) } }
        systems { update { dt = 0.001
            s0 = 0.5 * s0 } }
    "#;
    let mut rt = LangRuntime::compile(expo)?;
    rt.step_cross_n(1000).unwrap(); // t = 1.0
    let n = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    println!("== exponential growth  N' = ½N, N0=1 ==");
    println!("  N(1.0) = {n:.6}  (analytic e^0.5 = {:.6})", 0.5f64.exp());

    // Forced (driven) oscillator with explicit time:  x'' + ω²x = F·sin(ω_d·t).
    let driven = r#"
        world { gravity = (0,0,0) entity osc { state = (0, 0) } }
        systems { update { dt = 0.001
            s0 = s1
            s1 = -4 * s0 + 1.0 * sin(1.7 * t) } }
    "#;
    let mut rt = LangRuntime::compile(driven)?;
    let mut x = 0.0;
    for _ in 0..4000 {
        rt.step_cross()?;
        x = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
    }
    println!("== driven oscillator  x'' + 4x = sin(1.7·t) ==");
    println!(
        "  x(4.0s) = {x:.4}, t = {:.2} (bounded forced response; 物理规律: 受迫振动)",
        rt.scene.sim_time
    );
    Ok(())
}
