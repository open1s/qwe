//! Law-conformance tests: each PWE-language simulation is run and checked
//! against its analytic / objective physical, chemical, or biological result,
//! so correctness is a tested, gated property — not just spot-checked.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

fn state(rt: &LangRuntime, id: u128, slot: usize) -> f64 {
    rt.scene
        .get(EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[slot]
}

fn compile(src: &str) -> LangRuntime {
    LangRuntime::compile(src).expect("compile source")
}

/// Newton's law of cooling: T' = -k(T - T_env).
/// Discrete Euler: T_n = T_env + (T0 - T_env)·(1 - k·dt)^n.
#[test]
fn newton_law_of_cooling_matches_analytic() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity body { state = (90, 0) } }
        systems { update { dt = 0.01; s0 = -0.1 * (s0 - 20) } }
        "#,
    );
    rt.step_cross_n(100).unwrap();
    let t = state(&rt, 1, 0);
    let analytic = 20.0 + 70.0 * (1.0f64 - 0.1 * 0.01).powi(100);
    assert!((t - analytic).abs() < 1e-6, "T={t}, analytic={analytic}");
}

/// Radioactive decay: N' = -λN. Discrete: N_n = N0·(1-λ·dt)^n.
#[test]
fn radioactive_decay_matches_analytic() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity n { state = (100, 0) } }
        systems { update { dt = 1; s0 = -0.05 * s0 } }
        "#,
    );
    rt.step_cross_n(20).unwrap();
    let n = state(&rt, 1, 0);
    let analytic = 100.0 * (0.95f64).powi(20);
    assert!((n - analytic).abs() < 1e-6, "N={n}, analytic={analytic}");
}

/// Exponential growth: N' = rN. Discrete: N_n = N0·(1+r·dt)^n.
#[test]
fn exponential_growth_matches_analytic() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity n { state = (1, 0) } }
        systems { update { dt = 0.001; s0 = 0.5 * s0 } }
        "#,
    );
    rt.step_cross_n(1000).unwrap();
    let n = state(&rt, 1, 0);
    let analytic = (1.0f64 + 0.5 * 0.001).powi(1000);
    assert!((n - analytic).abs() < 1e-9, "N={n}, analytic={analytic}");
}

/// Logistic growth: N' = rN(1-N), analytic N(t) = 1/(1 + e^{-rt}) for N0=0.5.
#[test]
fn logistic_growth_matches_analytic() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity pop { state = (0.5, 0) } }
        systems { update { dt = 0.01; s0 = 1 * s0 * (1 - s0) } }
        "#,
    );
    rt.step_cross_n(100).unwrap(); // t = 1.0
    let n = state(&rt, 1, 0);
    let analytic = 1.0 / (1.0 + (-1.0f64).exp());
    assert!((n - analytic).abs() < 0.02, "N={n}, analytic={analytic}");
}

/// Projectile under gravity: y(t) = v0·t - ½·g·t² before ground contact.
#[test]
fn projectile_matches_ballistic_analytic() {
    let mut rt = compile(
        r#"
        world { gravity = (0, -9.81, 0)
            entity ball { position = (0, 0, 0); velocity = (0, 20, 0); mass = 1; dynamic = true; sphere = 0.1 }
        }
        systems { gravity { gravity_y = -9.81; dt = 1 / 60 } integrate { dt = 1 / 60 } }
        "#,
    );
    rt.step_cross_n(60).unwrap(); // t = 1.0 s (still ascending, no ground hit)
    let y = rt.scene.position(EntityId(1)).unwrap().y;
    let analytic = 20.0 * 1.0 - 0.5 * 9.81;
    assert!((y - analytic).abs() < 0.15, "y={y}, analytic={analytic}");
}

/// Harmonic oscillator: total mechanical energy is conserved.
#[test]
fn harmonic_oscillator_conserves_energy() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity m { state = (1, 0) } }
        systems { update { dt = 0.0005; s0 = s1; s1 = -10 * s0 } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    let (x, v) = (state(&rt, 1, 0), state(&rt, 1, 1));
    let energy = 0.5 * v * v + 0.5 * 10.0 * x * x;
    assert!((energy - 5.0).abs() < 0.05, "E={energy} (initial 5.0)");
}

/// RK4 harmonic oscillator: 4th-order integration conserves energy tightly at
/// a coarse `dt` where explicit Euler drifts. Proves `rk4` out-accuracies `update`.
#[test]
fn rk4_harmonic_oscillator_conserves_energy() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity m { state = (1, 0) } }
        systems { rk4 { dt = 0.05; s0 = s1; s1 = -10 * s0 } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    let (x, v) = (state(&rt, 1, 0), state(&rt, 1, 1));
    let energy = 0.5 * v * v + 0.5 * 10.0 * x * x;
    // dt = 0.05 is 100x the Euler test's dt; RK4 holds energy to ~0.1%.
    assert!(
        (energy - 5.0).abs() < 0.05,
        "RK4 E={energy} (initial 5.0, drift should be negligible)"
    );
}

/// Reversible reaction reaches chemical equilibrium: [C]/([A][B]) = k_f/k_r.
#[test]
fn reversible_reaction_reaches_equilibrium() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0) entity r { state = (1, 1, 0) } }
        systems { update { dt = 0.01
            s0 = -s0*s1 + 0.5*s2
            s1 = -s0*s1 + 0.5*s2
            s2 =  s0*s1 - 0.5*s2 } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    let (a, b, c) = (state(&rt, 1, 0), state(&rt, 1, 1), state(&rt, 1, 2));
    // Mass conservation.
    assert!((a + c - 1.0).abs() < 1e-6, "A+C={} (init 1)", a + c);
    // Equilibrium constant.
    let keq = c / (a * b);
    assert!((keq - 2.0).abs() < 1e-4, "K_eq={keq} (k_f/k_r=2)");
}

/// N-body gravity: a near-fixed massive body anchors a stable circular orbit.
#[test]
fn nbody_gravity_holds_circular_orbit() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity sun   { state = (0, 0, 0, 0, 0, 0, 1000000) }
            entity planet{ state = (4, 0, 0, 0, 500, 0, 1) } }
        systems { nbody { G = 1.0; dt = 0.0001 } }
        "#,
    );
    rt.step_cross_n(6000).unwrap();
    let (x, y) = (state(&rt, 2, 0), state(&rt, 2, 1));
    let r = (x * x + y * y).sqrt();
    assert!((r - 4.0).abs() < 0.5, "orbit radius {r} (initial 4.0)");
}

/// N-body Coulomb repulsion conserves linear momentum.
#[test]
fn nbody_conserves_momentum() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity a { state = (-2, 0, 0, 0, 0, 0, 1) }
            entity b { state = ( 2, 0, 0, 0, 0, 0, 1) } }
        systems { nbody { G = -2.0; dt = 0.001 } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    // Repulsion: gap grows; momentum conserved (equal masses, opposite velocities).
    let (ax, avx) = (state(&rt, 1, 0), state(&rt, 1, 3));
    let (bx, bvx) = (state(&rt, 2, 0), state(&rt, 2, 3));
    assert!(ax < -2.0, "a.x={ax}");
    assert!(bx > 2.0, "b.x={bx}");
    assert!((avx + bvx).abs() < 0.05, "momentum={}", avx + bvx);
}

/// A channel round-trips a value deterministically (send then recv).
#[test]
fn chan_system_round_trips_value() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            chan wire { value = 0 }
            entity probe { state = (3, 0) } }
        systems { send { chan = wire; value = s0 } recv { chan = wire; slot = 1 } }
        "#,
    );
    rt.step_cross_n(1).unwrap();
    assert!(
        (state(&rt, 1, 1) - 3.0).abs() < 1e-9,
        "received {}",
        state(&rt, 1, 1)
    );
}

/// A walled domain keeps a body inside and reflects velocity on impact.
#[test]
fn wall_boundary_keeps_body_inside_and_bounces() {
    let mut rt = compile(
        r#"
        world { gravity = (0, 0, 0)
            entity ball { position = (0, 0, 0); velocity = (6, 0, 0); mass = 1; dynamic = true; sphere = 0.5 }
        }
        systems { integrate { dt = 1/60 } wall { x = 5; z = 5; restitution = 0.8 } }
        "#,
    );
    let mut max_x = f64::NEG_INFINITY;
    for _ in 0..300 {
        rt.step_cross().unwrap();
        max_x = max_x.max(rt.scene.position(EntityId(1)).unwrap().x);
    }
    let min_x = rt
        .scene
        .position(EntityId(1))
        .unwrap()
        .x
        .min(-max_x.min(5.0));
    let _ = min_x;
    assert!(max_x <= 5.0 + 1e-9, "escaped wall, max_x={max_x}");
    let x = rt.scene.position(EntityId(1)).unwrap().x;
    assert!(x >= -5.0 - 1e-9, "escaped left wall, x={x}");
    // Speed is bounded by restitution damping (< the initial 6).
    let vx = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.velocity)
        .unwrap()
        .linear
        .x;
    assert!(vx.abs() < 6.0, "speed not damped, vx={vx}");
}

/// Newton's third law: two bodies sensing each other exert equal & opposite
/// forces, so total linear momentum is conserved while they accelerate toward
/// each other (action = -reaction).
#[test]
fn nbody_action_reaction_conserves_momentum() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity a { state = (-2, 0, 0, 0, 0, 0, 2) }   # mass 2
            entity b { state = ( 2, 0, 0, 0, 0, 0, 1) }   # mass 1
        }
        systems { nbody { G = 1.0; dt = 0.0005 } }
        "#,
    );
    for _ in 0..2000 {
        rt.step_cross().unwrap();
    }
    let a = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let b = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // They attract: a moves right, b moves left.
    assert!(a.values[0] > -2.0, "a.x={}", a.values[0]);
    assert!(b.values[0] < 2.0, "b.x={}", b.values[0]);
    // Total momentum m_a·v_a + m_b·v_b = 0 (action = -reaction).
    let mom = 2.0 * a.values[3] + 1.0 * b.values[3];
    assert!(mom.abs() < 0.05, "momentum {mom}");
}

/// Kepler: a body on an elliptical orbit around a near-fixed sun conserves
/// angular momentum L_z = m·(x·v_y − y·v_x).
#[test]
fn nbody_elliptical_orbit_conserves_angular_momentum() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity sun   { state = (0, 0, 0, 0, 0, 0, 1000000) }
            entity planet{ state = (5, 0, 0, 0, 350, 0, 1) } }   # v < circular -> ellipse
        systems { nbody { G = 1.0; dt = 0.0001 } }
        "#,
    );
    let l0 = {
        let st = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let (x, y, vx, vy) = (st.values[0], st.values[1], st.values[3], st.values[4]);
        x * vy - y * vx
    };
    for _ in 0..8000 {
        rt.step_cross().unwrap();
    }
    let st = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (x, y, vx, vy) = (st.values[0], st.values[1], st.values[3], st.values[4]);
    let l = x * vy - y * vx;
    assert!(
        (l - l0).abs() / l0.abs() < 0.05,
        "angular momentum {l} vs {l0}"
    );
}

/// `update { on = name }` applies the rule only to the named entity; the Moon
/// can be excluded from nbody and driven by a targeted rule.
#[test]
fn update_on_targets_specific_entity() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity a { state = (1, 0, 0, 0, 0, 0, 1, 0) }   # evolves
            entity b { state = (1, 0, 0, 0, 0, 0, 1, 0) }   # untouched
        }
        systems { update { on = a; dt = 1; s0 = -0.5 * s0 } }
        "#,
    );
    rt.step_cross_n(4).unwrap();
    let a = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let b = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // a decays (1 -> 0.5^4), b is untouched (stays 1).
    assert!((a.values[0] - 0.0625).abs() < 1e-6, "a.s0={}", a.values[0]);
    assert!((b.values[0] - 1.0).abs() < 1e-9, "b.s0={}", b.values[0]);
}

/// An entity with `nbody = false` is excluded from mutual gravity (e.g. a Moon).
#[test]
fn nbody_false_excludes_entity() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity a { state = (-2, 0, 0, 0, 0, 0, 1) }
            entity b { state = ( 2, 0, 0, 0, 0, 0, 1); nbody = false }
        }
        systems { nbody { G = -1.0; dt = 0.001 } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    let a = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let b = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // a has no partner in nbody, so it stays put; b is excluded.
    assert!((a.values[0] + 2.0).abs() < 1e-6, "a.x={}", a.values[0]);
    assert!((b.values[0] - 2.0).abs() < 1e-6, "b.x={}", b.values[0]);
}

/// Water + sodium reaction: 2Na + 2H2O -> 2NaOH + H2. Mass action with an
/// Arrhenius rate constant; species are conserved (NaOH forms exactly as Na is
/// consumed, H2 forms at half rate), and the exotherm raises temperature.
#[test]
fn water_sodium_reaction_conserves_stoichiometry() {
    let mut rt = compile(
        r#"
        world { gravity = (0,0,0)
            entity reactor { state = (0, 0, 0, na = 2.0, water = 100.0, naoh = 0.0, temp = 300.0, h2 = 0.0) } }
        systems { update { on = reactor; dt = 0.0005
            let k = 3.0 * exp(-900.0 / temp)
            na    = -k * na * water
            water = -k * na * water
            naoh  =  k * na * water
            h2    = 0.5 * k * na * water
            temp  = 260.0 * k * na * water
        } }
        "#,
    );
    rt.step_cross_n(2000).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values
        .clone();
    let (na, naoh, h2, temp) = (st[3], st[5], st[7], st[6]);
    // Sodium is consumed; hydroxide forms in exact 1:1 Na:NaOH proportion.
    assert!(na < 1e-3, "[Na] should deplete: {na}");
    assert!(
        (naoh - 2.0).abs() < 1e-2,
        "[NaOH] should reach 2.0 (all Na): {naoh}"
    );
    // Stoichiometry: 2 Na -> 1 H2, so H2 = half the consumed Na.
    assert!(
        (h2 - 1.0).abs() < 1e-2,
        "[H2] should reach 1.0 (half of Na): {h2}"
    );
    // Exothermic: temperature rises well above the 300 K start.
    assert!(temp > 700.0, "exotherm should heat the reactor: {temp}");
}
