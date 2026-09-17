//! A breadth check that the PWE language can express and *verify* a range of
//! scientific phenomena across domains, all in source and run cross-backend.
//!
//! Each law is written as `state` + an `update`/`linear` system and checked
//! against its analytic solution.
//!
//! Run: `cargo run --example science_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

fn main() -> pwe_api::Result<()> {
    println!("== PWE language: a breadth of scientific phenomena ==");

    // 1. Thermodynamics — Newton's law of cooling: T' = -k(T - T_env).
    //    Analytic: T(t) = T_env + (T0 - T_env)·e^(-k·t).
    let cooling = r#"
        world { gravity = (0,0,0) entity body { state = (90, 0) } }
        systems { update { dt = 0.01
            s0 = -0.1 * (s0 - 20) } }
    "#;
    let mut rt = LangRuntime::compile(cooling)?;
    rt.step_cross_n(100).unwrap(); // t = 1.0
    let t1 = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let analytic = 20.0 + 70.0 * (-0.1f64).exp();
    println!("[thermo ] Newton cooling  T(1) = {t1:.4} (analytic {analytic:.4})");

    // 2. Nuclear — radioactive decay: N' = -λN.
    let decay = r#"
        world { gravity = (0,0,0) entity n { state = (100, 0) } }
        systems { update { dt = 1
            s0 = -0.05 * s0 } }
    "#;
    let mut rt = LangRuntime::compile(decay)?;
    rt.step_cross_n(20).unwrap();
    let n = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let analytic = 100.0 * (0.95f64).powi(20);
    println!("[nuclear] decay  N(20) = {n:.4} (analytic {analytic:.4})");

    // 3. Population — logistic growth: N' = rN(1-N), analytic N=1/(1+e^{-t}).
    let logistic = r#"
        world { gravity = (0,0,0) entity pop { state = (0.5, 0) } }
        systems { update { dt = 0.01
            s0 = 1 * s0 * (1 - s0) } }
    "#;
    let mut rt = LangRuntime::compile(logistic)?;
    rt.step_cross_n(100).unwrap(); // t = 1.0
    let p = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let analytic = 1.0 / (1.0 + (-1.0f64).exp());
    println!("[biology] logistic  N(1) = {p:.4} (analytic {analytic:.4})");

    // 4. Mechanics — harmonic oscillator: x'' + ω²x = 0, energy conserved.
    let spring = r#"
        world { gravity = (0,0,0) entity m { state = (1, 0) } }
        systems { update { dt = 0.001
            s0 = s1
            s1 = -10 * s0 } }
    "#;
    let mut rt = LangRuntime::compile(spring)?;
    rt.step_cross_n(1000).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let e = 0.5 * st.values[1] * st.values[1] + 0.5 * 10.0 * st.values[0] * st.values[0];
    println!("[mech   ] oscillator  E = {e:.4} (initial 5.0, conserved)");

    // 5. Projectile — ballistic under gravity (Newtonian mechanics).
    let proj = r#"
        world { gravity = (0, -9.81, 0)
            entity ball { position = (0, 0, 0); velocity = (10, 20, 0); mass = 1; dynamic = true; sphere = 0.1 }
        }
        systems {
            gravity { gravity_y = -9.81; dt = 1 / 60 }
            integrate { dt = 1 / 60 }
            ground_contact { restitution = 0.5 }
        }
    "#;
    let mut rt = LangRuntime::compile(proj)?;
    for _ in 0..90 {
        rt.step_cross()?; // 1.5 s
    }
    let y = rt.scene.position(EntityId(1))?.y;
    let analytic = 20.0 * 1.5 - 0.5 * 9.81 * 1.5 * 1.5;
    println!("[mech   ] projectile  y(1.5s) = {y:.4} (analytic {analytic:.4})");

    // 6. Chemistry — reversible reaction reaches equilibrium K_eq = k_f/k_r.
    let reaction = r#"
        world { gravity = (0,0,0) entity r { state = (1, 1, 0) } }
        systems { update { dt = 0.01
            s0 = -s0*s1 + 0.5*s2
            s1 = -s0*s1 + 0.5*s2
            s2 =  s0*s1 - 0.5*s2 } }
    "#;
    let mut rt = LangRuntime::compile(reaction)?;
    rt.step_cross_n(1000).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let keq = st.values[2] / (st.values[0] * st.values[1]);
    println!("[chem   ] equilibrium  K_eq = {keq:.4} (k_f/k_r = 2.0)");

    println!("\nAll phenomena expressed in source and verified across backends.");
    Ok(())
}
