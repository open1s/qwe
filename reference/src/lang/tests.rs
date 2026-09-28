//! Unit tests for the PWE language (parsing, lowering, systems, runtime).
use super::*;
use pwe_api::EntityId;

/// Regression: a `#` comment inside an `update` block must not swallow the
/// preceding scalar parameter's value (`dt = 0.0005\n # note`).
#[test]
fn comment_in_update_does_not_swallow_scalar_param() {
    let src = "world { gravity=(0,0,0) entity reactor { state=(0,0,0,na=2.0,water=100.0,naoh=0.0,temp=300.0,h2=0.0); color=0xffb347 } }\n systems { update { on = reactor; dt = 0.0005\n # kinetics\n let k = 3.0 * exp(-900.0 / temp)\n na = na + dt*(  -k * na * water\n ) temp = temp + dt*(  260.0 * k * na * water\n ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(10).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values
        .clone();
    assert!(st[3] < 2.0, "Na should be consumed: {st:?}");
    assert!(st[6] > 300.0, "temperature should rise: {st:?}");
}

/// Diagnostics: a missing required system param reports which system and
/// key, with a source caret via `render_diagnostic`.
#[test]
fn missing_param_yields_diagnostic_with_location() {
    clear_diagnostics();
    let src = "world { gravity=(0,0,0) entity b { state=(0,0,0) } }\n systems { gravity { } }";
    let err = match LangRuntime::compile(src) {
        Ok(_) => panic!("expected compile failure"),
        Err(e) => e,
    };
    let text = diagnose(src, &err);
    assert!(err.detail == 48, "expected missing-param error, got {text}");
    assert!(
        text.contains("gravity"),
        "diagnostic should name the system: {text}"
    );
    assert!(
        text.contains("gravity_y"),
        "diagnostic should name the param: {text}"
    );
    // `diagnose` rendered a source location (line/caret) from byte_offset.
    assert!(
        text.contains("line") && text.contains('^'),
        "diagnostic should carry a rendered source caret: {text}"
    );
}

const SOURCE: &str = r#"
        world {
            gravity = (0, -9.81, 0)
            entity vehicle {
                position = (0, 8, 0)
                velocity = (4, 0, 0)
                mass = 4
                dynamic = true
                box = (1, 0.5, 0.7)
            }
            entity ground {
                position = (0, -5, 0)
                dynamic = false
                box = (50, 5, 50)
            }
            entity camera {
                position = (0, 16, 24)
                camera = true
            }
        }
        systems {
            gravity { gravity_y = -9.81; dt = 1 / 60 }
            integrate { dt = 1 / 60 }
            ground_contact { restitution = 0.6 }
        }
    "#;

#[test]
fn parse_builds_world_and_systems() {
    let parsed = parse(SOURCE).unwrap();
    assert_eq!(parsed.model.entities.len(), 3);
    assert_eq!(parsed.systems.len(), 3);
    assert_eq!(parsed.systems[0].kind, "gravity");
    assert!((parsed.systems[0].params["dt"] - 1.0 / 60.0).abs() < 1e-9);
    // Camera and ground declarations survive.
    assert_eq!(parsed.model.entities[2].name, "camera");
    assert_eq!(parsed.model.entities[2].camera, Some(true));
}

#[test]
fn compiles_to_low_level_eir_that_validates() {
    let compiled = compile(SOURCE).unwrap();
    assert!(!compiled.eir.functions.is_empty());
    assert!(compiled.eir.validate(true).is_ok());
}

#[test]
fn interpreter_and_jit_agree_cross_backend() {
    let mut rt = LangRuntime::compile(SOURCE).unwrap();
    // Dynamic bodies only (ground is static, camera is not a body).
    assert_eq!(rt.program.entities, vec![1]);
    rt.step_cross_n(120).unwrap();
    let y = rt.scene.position(EntityId(1)).unwrap().y;
    // Vehicle settled onto the ground, never below it.
    assert!(y < 8.0, "vehicle fell to {y}");
    assert!(y >= -5.0, "vehicle should not tunnel, got {y}");
}

#[test]
fn cross_step_requires_backend_agreement_each_step() {
    let mut rt = LangRuntime::compile(SOURCE).unwrap();
    for _ in 0..50 {
        rt.step_cross().unwrap();
    }
    // The interpreter and JIT advanced the same authoritative world.
    assert!(rt.clock == 50);
}

#[test]
fn rejects_malformed_source() {
    assert!(parse("world { gravity = (0, -9.81) }").is_err());
    assert!(parse("systems {").is_err());
}

#[test]
fn compiled_program_passes_linear_dominance_gate() {
    let compiled = compile(SOURCE).unwrap();
    // The RFC-0021 dominance gate must hold for the compiled EIR.
    assert!(compiled.eir.verify_linear_dominance().is_ok());
}

#[test]
fn force_system_drifts_body_in_x() {
    // A wind/force in +x accelerates the vehicle's horizontal velocity.
    let src = r#"
            world {
                gravity = (0, 0, 0)
                entity body { position = (0, 0, 0); velocity = (0, 0, 0); mass = 1; dynamic = true; box = (1, 1, 1) }
            }
            systems {
                force { ax = 2; ay = 0; az = 0; dt = 1 / 60 }
                integrate { dt = 1 / 60 }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(60).unwrap(); // 1 second
    let vx = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.velocity)
        .unwrap()
        .linear
        .x;
    let px = rt.scene.position(EntityId(1)).unwrap().x;
    assert!((vx - 2.0).abs() < 1e-6, "vx = {vx}");
    assert!(px > 0.5, "body should drift in +x, px = {px}");
}

#[test]
fn damping_reduces_horizontal_velocity() {
    // Damping scales every velocity axis toward zero.
    let src = r#"
            world {
                gravity = (0, 0, 0)
                entity body { position = (0, 0, 0); velocity = (10, 5, 0); mass = 1; dynamic = true; box = (1, 1, 1) }
            }
            systems {
                damping { factor = 0.5 }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
    let v = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.velocity)
        .unwrap()
        .linear;
    // After 3 halvings: 10 -> 1.25, 5 -> 0.625.
    assert!((v.x - 1.25).abs() < 1e-6, "vx = {}", v.x);
    assert!((v.y - 0.625).abs() < 1e-6, "vy = {}", v.y);
}

#[test]
fn state_linear_system_models_radioactive_decay() {
    // N' = -λ N  with λ = 0.1 / step. After n steps, N = N0·(1−λ)^n.
    let src = r#"
            world {
                gravity = (0, 0, 0)
                entity isotope { state = (100, 0) }
            }
            systems {
                linear { slots = 2; dt = 1
                    row0 = (-0.1, 0, 0)
                    row1 = (0, 0, 0) }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(20).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let expected = 100.0 * (1.0f64 - 0.1).powi(20);
    assert!(
        (st.values[0] - expected).abs() < 1e-6,
        "N = {}",
        st.values[0]
    );
}

#[test]
fn state_linear_system_models_harmonic_oscillator() {
    // Spring: x' = v, v' = -ω²x with ω² = 10. Energy stays bounded.
    let src = r#"
            world {
                gravity = (0, 0, 0)
                entity spring { state = (1, 0) }
            }
            systems {
                linear { slots = 2; dt = 0.001
                    row0 = (0, 1, 0)
                    row1 = (-10, 0, 0) }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1000).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (x, v) = (st.values[0], st.values[1]);
    // E = ½v² + ½ω²x²; the exact solution conserves it, explicit Euler stays
    // within a small band.
    let energy = 0.5 * v * v + 0.5 * 10.0 * x * x;
    assert!((energy - 5.0).abs() < 0.1, "energy drifted: {energy}");
}

#[test]
fn parses_convex_hull_collider() {
    let src = r#"
            world {
                gravity = (0, -9.81, 0)
                entity hull {
                    position = (0, 2, 0); dynamic = true
                    hull = [ (0,0,0), (2,0,0), (0,0,2), (2,0,2), (1,2,1) ]
                }
            }
            systems {}
        "#;
    let parsed = parse(src).unwrap();
    let e = &parsed.model.entities[0];
    match &e.collider {
        Some(crate::dsl::ColliderDecl::ConvexHull { points }) => {
            assert_eq!(points.len(), 5);
            assert_eq!(*points.first().unwrap(), Vec3::new(0.0, 0.0, 0.0));
        }
        _ => panic!("expected ConvexHull collider"),
    }
    // It lowers into a scene with a convex-hull collider.
    let scene = parsed.model.build_scene();
    let collider = scene.get(EntityId(1)).unwrap().collider.as_ref().unwrap();
    assert!(matches!(
        collider,
        crate::components::Collider::ConvexHull { .. }
    ));
    // Too few points is rejected.
    assert!(parse(
        "world { gravity = (0,0,0) entity h { hull = [ (0,0,0), (1,0,0), (0,1,0) ] } } systems {}"
    )
    .is_err());
}

#[test]
fn update_system_models_logistic_growth() {
    // N' = r·N·(1−N), r=1, N0=0.5, dt=0.01. Analytic N(t)=1/(1+e^(−t)).
    let src = r#"
            world { gravity = (0,0,0) entity pop { state = (0.5, 0) } }
            systems {
                update { dt = 0.01
                    s0 = s0 + dt*(  1 * s0 * (1 - s0) ) }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(100).unwrap(); // t = 1.0
    let n = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let analytic = 1.0 / (1.0 + (-1.0f64).exp());
    assert!(
        (n - analytic).abs() < 0.02,
        "logistic N={n}, analytic {analytic}"
    );
    // Keep stepping: it must converge to (and never exceed) capacity 1.
    for _ in 0..2000 {
        rt.step_cross().unwrap();
    }
    let n2 = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    assert!((n2 - 1.0).abs() < 1e-3, "capacity N={n2}");
}

#[test]
fn update_system_models_coupled_nonlinear_system() {
    // Two coupled nonlinear slots: A' = a·A − b·A·B, B' = c·A·B (a simple
    // resource–consumer model). Both remain bounded and the product coupling
    // executes across backends identically.
    let src = r#"
            world { gravity = (0,0,0) entity sys { state = (1, 0.5) } }
            systems {
                update { dt = 0.001
                    s0 = s0 + dt*(  2 * s0 - 1 * s0 * s1 )
                    s1 = s1 + dt*(  1 * s0 * s1 - 1 * s1 ) }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(500).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // Values stay finite and positive.
    assert!(st.values[0].is_finite() && st.values[0] > 0.0);
    assert!(st.values[1].is_finite() && st.values[1] > 0.0);
    // Cross-backend agreement is enforced on every step by step_cross.
    assert!(rt.clock == 500);
}

#[test]
fn functions_support_recursion() {
    // Control-flow `if … { return … }` in a function body makes calls nest on
    // the interpreter's call stack, so recursion terminates (the eager
    // `if(c,a,b)` expression would evaluate the recursive branch forever).
    let src = r#"
            world { gravity=(0,0,0) entity e { state=(x = 0.0) } }
            funcs {
                fact(n) { if n < 1.0 { return 1.0 } else { return n * fact(n - 1.0) } }
                fib(n)  { if n < 2.0 { return n } return fib(n - 1.0) + fib(n - 2.0) }
            }
            systems { update { on = e; dt = 1.0 x = fact(5.0) + fib(10.0) } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let x = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert_eq!(x, 120.0 + 55.0, "fact(5)+fib(10) = {x}");
}

#[test]
fn system_barrier_within_system_is_simultaneous() {
    // One system, two entities: the later-id entity must read the earlier
    // entity's *start-of-system* value, not its same-step update.
    let src = r#"
            world { gravity=(0,0,0)
                entity a { state = (5.0) }
                entity b { state = (0.0) } }
            systems { update { dt = 1.0 s0 = s0 + @a.s0 } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let av = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    let bv = rt
        .scene
        .get(EntityId(2))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert!((av - 10.0).abs() < 1e-9, "a = {av}"); // 5 + committed(5)
    assert!((bv - 5.0).abs() < 1e-9, "b = {bv}"); // 0 + committed(a=5), not 10
}

#[test]
fn system_barrier_across_systems_sees_prior_writes() {
    // A later system observes an earlier system's write via `@name`.
    let src = r#"
            world { gravity=(0,0,0)
                entity a { state = (5.0) }
                entity c { state = (0.0) } }
            systems {
                update { on = a; dt = 1.0 s0 = s0 + 1.0 }
                update { on = c; dt = 1.0 s0 = @a.s0 }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let av = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    let cv = rt
        .scene
        .get(EntityId(2))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert!((av - 6.0).abs() < 1e-9, "a = {av}");
    assert!((cv - 6.0).abs() < 1e-9, "c = {cv}"); // sees a's committed write
}

#[test]
fn inte_and_deriv_operators() {
    // `inte(E)` is the increment dt·E; `deriv(E)` is the backward
    // difference (E - E_prev)/dt (0 on the first step).
    let src = r#"
            world { gravity=(0,0,0) entity e { state=(x=0.0, v=1.0, a=0.0) } }
            systems { update { on = e; dt = 0.1
                x = x + inte(v)
                a = deriv(x)
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(5).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!((st.values[0] - 0.5).abs() < 1e-9, "x = {}", st.values[0]);
    assert!((st.values[2] - 1.0).abs() < 1e-9, "a = {}", st.values[2]);
}

#[test]
fn inte_statement_integrates() {
    // `inte slot = rate` integrates (slot += dt·rate) without spelling dt.
    let src = r#"
            world { gravity=(0,0,0) entity e { state=(x=0.0, v=2.0) } }
            systems { update { on = e; dt = 0.1
                inte x = v
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(5).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!((st.values[0] - 1.0).abs() < 1e-9, "x = {}", st.values[0]);
}

#[test]
fn update_system_supports_transcendental_functions() {
    // exp() lowers and evaluates to e; sin()/cos() work in expressions.
    let src = r#"
            world { gravity = (0,0,0) entity a { state = (0, 0, 0) } }
            systems { update { dt = 1
                s0 = s0 + dt*(  exp(1) - 2.718281828459045 )  # exp(1) = e ≈ 0 drift
                s1 = s1 + dt*(  sin(0) )  # 0
                s2 = s2 + dt*(  cos(0) - 1 )  # 0
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert!(st.values[0].abs() < 1e-6, "s0 = {}", st.values[0]);
    assert!(st.values[1].abs() < 1e-6, "s1 = {}", st.values[1]);
    assert!(st.values[2].abs() < 1e-6, "s2 = {}", st.values[2]);
}

#[test]
fn nbody_system_models_circular_orbit() -> pwe_api::Result<()> {
    // Two-body gravity with a near-fixed massive sun: a small body orbits at
    // radius r with circular speed v = sqrt(G·M/r).
    let src = r#"
            world { gravity = (0,0,0)
                entity sun   { state = (0, 0, 0, 0, 0, 0, 1000000) }
                entity planet{ state = (4, 0, 0, 0, 500, 0, 1) }   # v = sqrt(1·1e6/4) = 500
            }
            systems { nbody { G = 1.0; dt = 0.0001 } }
        "#;
    let mut rt = LangRuntime::compile(src)?;
    for _ in 0..10000 {
        rt.step_cross()?;
    }
    let p = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (px, py) = (p.values[0], p.values[1]);
    let r = (px * px + py * py).sqrt();
    // The planet holds a ~circular orbit at r ≈ 4 (doesn't collapse or escape).
    assert!((r - 4.0).abs() < 0.5, "orbit radius {r}");
    Ok(())
}

#[test]
fn nbody_system_models_micro_particle_repulsion() -> pwe_api::Result<()> {
    // Two like-charged particles at rest repel (G < 0): the gap grows and
    // linear momentum is conserved (equal/opposite for equal masses).
    let src = r#"
            world { gravity = (0,0,0)
                entity a { state = (-2, 0, 0,  0, 0, 0, 1) }
                entity b { state = ( 2, 0, 0,  0, 0, 0, 1) }
            }
            systems { nbody { G = -1.0; dt = 0.001 } }
        "#;
    let mut rt = LangRuntime::compile(src)?;
    for _ in 0..2000 {
        rt.step_cross()?;
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
    // Repulsion pushes a left and b right.
    assert!(a.values[0] < -2.0, "a.x = {}", a.values[0]);
    assert!(b.values[0] > 2.0, "b.x = {}", b.values[0]);
    // Linear momentum is conserved: m_a·v_a + m_b·v_b ≈ initial 0.
    let mom = a.values[3] + b.values[3];
    assert!(mom.abs() < 0.1, "momentum = {mom}");
    Ok(())
}

#[test]
fn chan_system_sends_and_receives_across_entities() {
    // A producer entity publishes its slot-0 value to channel `wire`, then a
    // consumer receives it into its slot 1. Both are plain cross-entity EIR
    // reads/writes, so the interpreter and JIT agree byte-for-byte.
    let src = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity probe { state = (3, 0) }
            }
            systems {
                send { on = probe; chan = wire; value = s0 }
                recv { on = probe; chan = wire; slot = 1 }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let probe = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // The producer's value (3) travelled through the channel into slot 1.
    assert!(
        (probe.values[1] - 3.0).abs() < 1e-9,
        "slot1 = {}",
        probe.values[1]
    );
    // The channel entity (id 2) holds the latest published value.
    let wire = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert!(
        (wire.values[0] - 3.0).abs() < 1e-9,
        "wire = {}",
        wire.values[0]
    );
    // And the producer's own slot 0 is unchanged.
    assert!((probe.values[0] - 3.0).abs() < 1e-9);
}

#[test]
fn chan_system_round_trips_cross_backend() {
    // Two producers publishing to one channel: last sender wins.
    let src = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity a { state = (1, 0) }
                entity b { state = (7, 0) }
            }
            systems { send { on = a; chan = wire; value = s0 }
                       send { on = b; chan = wire; value = s0 } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    // b (id 2) is processed after a (id 1), so the channel holds b's value.
    let wire = rt
        .scene
        .get(EntityId(3))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert!(
        (wire.values[0] - 7.0).abs() < 1e-9,
        "wire = {}",
        wire.values[0]
    );
}

#[test]
fn chan_system_communicates_across_runtimes() -> pwe_api::Result<()> {
    // Runtime A publishes to `wire`; Runtime B receives it over the wire
    // transport (cross-runtime / cross-network world communication).
    let src_a = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity producer { state = (42, 0) }
            }
            systems { send { on = producer; chan = wire; value = s0 } }
        "#;
    let src_b = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity consumer { state = (0, 0) }
            }
            systems { recv { on = consumer; chan = wire; slot = 1 } }
        "#;
    let mut a = LangRuntime::compile_in_region(src_a, RegionId(1))?;
    let mut b = LangRuntime::compile_in_region(src_b, RegionId(2))?;
    a.set_peer(RegionId(2));
    b.set_peer(RegionId(1));

    // A publishes 42 to wire; transport to B; B receives it into slot 1.
    a.step_cross()?;
    a.exchange(&mut b)?;
    b.step_cross()?;

    let consumer = b
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert!(
        (consumer.values[1] - 42.0).abs() < 1e-9,
        "consumer slot1 = {}",
        consumer.values[1]
    );
    Ok(())
}

#[test]
fn update_system_supports_cross_entity_coupling() {
    // A dynamic earth body is attracted to a fixed massive `sun` at the
    // origin via `@sun.s0` (inverse-square gravity, A = 1). The sun is
    // declared `dynamic = false` (not updated), so only earth's state moves.
    let src = r#"
            world { gravity = (0,0,0)
                entity sun { dynamic = false; state = (0, 0) }   # fixed at x = 0
                entity earth { state = (5, 0) }                  # x = 5, vx = 0
            }
            systems { update { dt = 0.001
                s0 = s0 + dt*(  s1 )  # x' = vx
                s1 = s1 + dt*(  -1 * (s0 - @sun.s0) / ((s0 - @sun.s0)*(s0 - @sun.s0)*sqrt((s0 - @sun.s0)*(s0 - @sun.s0))) )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3000).unwrap();
    let earth_x = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    let sun_x = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    // Earth falls toward the fixed sun at the origin.
    assert!((sun_x - 0.0).abs() < 1e-9, "sun fixed at 0, got {sun_x}");
    assert!(earth_x < 5.0, "earth fell toward sun, x = {earth_x}");
    assert!(earth_x.is_finite());
    assert!(
        earth_x > -5.0,
        "earth did not fly past the sun, x = {earth_x}"
    );
}

#[test]
fn update_system_supports_time_t() {
    // Driven oscillator (Newton):  x' = v,  v' = -ω²x + F·sin(ω_d·t).
    // Off-resonance (ω²=1, ω_d=0.37) the response is bounded.
    let src = r#"
            world { gravity = (0,0,0) entity osc { state = (0, 0) } }
            systems { update { dt = 0.001
                s0 = s0 + dt*(  s1 )
                s1 = s1 + dt*(  -1 * s0 + 0.6 * sin(0.37 * t) ) }
            }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(4000).unwrap(); // 4 seconds
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // The clock advanced with the update dt.
    assert!(
        (rt.scene.sim_time - 4.0).abs() < 1e-6,
        "t = {}",
        rt.scene.sim_time
    );
    // The oscillator is driven but bounded (forced-response amplitude < F/(ω²−ω_d²)).
    assert!(st.values[0].abs() < 1.0, "x = {}", st.values[0]);
    assert!(st.values[1].abs() < 1.0, "v = {}", st.values[1]);
}

#[test]
fn parses_entity_color() {
    let src = r#"
            world { gravity = (0,0,0)
                entity red { position = (0,1,0); color = 0xFF0000; box = (1,1,1) }
            }
            systems {}
        "#;
    let parsed = parse(src).unwrap();
    assert_eq!(parsed.model.entities[0].color, Some(0xFF0000));
    // It lands on the scene entity for the presentation layer.
    let scene = parsed.model.build_scene();
    let e = scene.get(EntityId(1)).unwrap();
    assert_eq!(e.color, Some(0xFF0000));
}

#[test]
fn update_system_supports_unary_minus() {
    // -x lowers to x·(-1); a negative growth rate.
    let src = r#"
            world { gravity = (0,0,0) entity a { state = (10, 0) } }
            systems { update { dt = 1
                s0 = s0 + dt*(  -0.1 * s0 ) } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(10).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let expected = 10.0 * (0.9f64).powi(10);
    assert!(
        (st.values[0] - expected).abs() < 1e-6,
        "s0 = {}",
        st.values[0]
    );
}

#[test]
fn update_system_supports_comparisons_and_select() {
    // `update` integrates: state[i] += dt·expr. Use `if` as a saturating
    // integrator (grows to 10 then holds), and `min`/`max` as bounded terms.
    let src = r#"
            world { gravity = (0,0,0) entity c { state = (0, 0, 0) } }
            systems { update { dt = 1
                s0 = s0 + dt*(  if(s0 < 10, 1, 0) )  # s0 += 1 while < 10  -> saturates at 10
                s1 = s1 + dt*(  min(0.5, 10 - s0) )  # bounded increment
                s2 = s2 + dt*(  max(-1, s0 - 10) )  # 0 once s0 = 10
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(20).unwrap();
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // The `if` integrator saturates exactly at 10.
    assert!((st.values[0] - 10.0).abs() < 1e-9, "s0 = {}", st.values[0]);
    // `max` bounds s2 to -1/step; with simultaneous update it accumulates
    // -1 for the 10 steps before s0 crossed 10 -> -10, then stops at 0.
    assert!((st.values[2] + 10.0).abs() < 1e-9, "s2 = {}", st.values[2]);
    // `min` keeps s1 bounded and finite.
    assert!(st.values[1].is_finite());
}

#[test]
fn update_system_models_pendulum_with_sin() {
    // Nonlinear pendulum: θ'' = −(g/L)·sin(θ) via slots [θ, ω], L=1.
    let src = r#"
            world { gravity = (0,0,0) entity pend { state = (0.5, 0) } }
            systems { update { dt = 0.001
                s0 = s0 + dt*(  s1 )  # θ' = ω
                s1 = s1 + dt*(  -9.81 * sin(s0) )  # ω' = −(g/L)·sin(θ)
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(2000).unwrap(); // 2 seconds
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let (theta, omega) = (st.values[0], st.values[1]);
    assert!(theta.is_finite() && theta.abs() < 0.6, "θ = {theta}");
    // Energy E = ½ω² + g(1−cosθ) is conserved.
    let e = 0.5 * omega * omega + 9.81 * (1.0 - theta.cos());
    let initial = 9.81 * (1.0 - 0.5f64.cos());
    assert!(
        (e - initial).abs() < 0.1,
        "pendulum energy {e} vs {initial}"
    );
}

#[test]
fn random_and_emit_work_cross_backend_and_are_reproducible() {
    // A stochastic rule: each step, s0 += random(); and emit an event with
    // the current slot. Cross-backend must agree, and replaying from a fresh
    // runtime must reproduce the same trajectory (seeded RNG).
    let src = r#"
            world { gravity = (0,0,0) entity walker { state = (0, 0) } }
            systems { update { dt = 1
                s0 = s0 + dt*(  s0 + random() )  # stochastic walk
                s1 = s1 + dt*(  emit(7, s0) )  # emit an ordered event each step
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(5).unwrap(); // exercises interpreter == JIT on random/emit
    let walker = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    let final_state = walker.values[0];
    assert!(final_state.is_finite() && final_state > 0.0);

    // Reproducibility: a fresh runtime with the same source yields the same
    // trajectory (the RNG is deterministically seeded).
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.step_cross_n(5).unwrap();
    let walker2 = rt2
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert_eq!(walker2.values[0], final_state);
}

#[test]
fn user_defined_functions_lower_to_calls_and_run_cross_backend() {
    // Pure `funcs` (clamp + grow) called from an `update` rule via `CALL`.
    // The update system integrates s0 += dt·rule, so clamp caps the per-step
    // delta — the point here is that the functions are actually invoked by
    // both backends and the trajectory is reproducible.
    let src = r#"
            world { gravity = (0,0,0) entity x { state = (0.5, 0) } }
            funcs {
                # param `a` is slot s0, `b` is slot s1.
                clamp(a, b) { if(s0 < 0, 0, if(s0 > s1, s1, s0)) }
                grow(v)   { s0 * (1 - s0) }
            }
            systems { update { dt = 0.01
                s0 = s0 + dt*(  clamp(grow(s0) + s0, 0.9) )
                s1 = s1 + dt*(  s0 * 2 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(500).unwrap(); // interpreter == JIT on CALLs
    let read = |rt: &LangRuntime| -> (f64, f64) {
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        (st.values[0], st.values[1])
    };
    let (s0, _s1) = read(&rt);
    // The CALLs had an effect: the population moved off its initial value
    // and stayed finite/positive.
    assert!(s0.is_finite() && s0 > 0.0, "s0 = {s0}");
    // Reproducibility: a fresh runtime reproduces the same trajectory.
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.step_cross_n(500).unwrap();
    assert_eq!(read(&rt2).0, s0);
}

#[test]
fn named_state_slots_and_rich_builtins_work_cross_backend() {
    // Named state slots (`state = (x = 0, y = 0)`) accessed by name in rules
    // and via `@self.x`; rich builtins (abs, floor, sign, hypot, atan2, …).
    let src = r#"
            world { gravity = (0,0,0) entity bob { state = (x = 0.5, y = 0.1) } }
            systems { update { dt = 0.01
                x = x + dt*(  abs(sign(@self.x)) + floor(@self.x) )  # = 1 + 0 = 1
                y = y + dt*(  hypot(3, 4) + @self.x )  # = 5 + x
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(2).unwrap(); // interpreter == JIT
    let read = |rt: &LangRuntime| -> (f64, f64) {
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        (st.values[0], st.values[1])
    };
    let (x, y) = read(&rt);
    // x: starts 0.5, each step x += 0.01*(1 + 0) => 0.52. y integrates
    // dt*(hypot(3,4) + x): 0.1 + 0.01*((5+0.5) + (5+0.51)) = 0.2101.
    assert!((x - 0.52).abs() < 1e-9, "x = {x}");
    assert!((y - 0.2101).abs() < 1e-9, "y = {y}");
    // Reproducible.
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.step_cross_n(2).unwrap();
    assert_eq!(read(&rt2), (x, y));
}

#[test]
fn cross_entity_property_access_reads_physics_fields() {
    // `@other.mass`, `@other.position.x`, `@other.velocity.y` read the
    // corresponding physics components of another entity.
    let src = r#"
            world { gravity = (0,0,0)
                entity heavy { position = (3, 4, 0); velocity = (1, 2, 0); mass = 9 }
                entity probe { state = (0, 0) }
            }
            systems { update { dt = 0.01
                s0 = s0 + dt*(  @heavy.mass )  # 9
                s1 = s1 + dt*(  @heavy.position.x + @heavy.velocity.y )  # 3 + 2 = 5
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
    let st = rt
        .scene
        .get(EntityId(2))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    // s0 accumulates 9 per step (dt*9*3 = 0.27), s1 accumulates 5.
    let (s0, s1) = (st.values[0], st.values[1]);
    assert!((s0 - 3.0 * 0.01 * 9.0).abs() < 1e-9, "s0 = {s0}");
    assert!((s1 - 3.0 * 0.01 * 5.0).abs() < 1e-9, "s1 = {s1}");
}

#[test]
fn let_locals_enable_elegant_multi_step_rules() {
    // `let` locals compute intermediates once, reused by several rules —
    // the same model without inlining. Cross-backend + reproducible.
    let src = r#"
            world { gravity = (0,0,0)
                entity target { state = (tx = 3, ty = 4, 0) }
                entity g { state = (x = 0, y = 0, 0) }
            }
            systems { update { on = g; dt = 0.02
                let dx = @target.tx - @self.x
                let dy = @target.ty - @self.y
                let dist = hypot(dx, dy)
                let gain = min(dist * 0.1, 1.0)
                x = x + dt*(  dx * gain )
                y = y + dt*(  dy * gain )
                s2 = s2 + dt*(  dist )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(50).unwrap();
    let read = |rt: &LangRuntime| -> (f64, f64, f64) {
        let st = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        (st.values[0], st.values[1], st.values[2])
    };
    let (x, y, dist) = read(&rt);
    // The glider moves toward the target (both coordinates grow from 0), and
    // `dist` (s2) is finite/positive (the current gap).
    assert!(x.is_finite() && x > 0.0 && y > 0.0, "x={x} y={y}");
    assert!(dist.is_finite() && dist > 0.0, "dist={dist}");
    // Reproducible.
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.step_cross_n(50).unwrap();
    assert_eq!(read(&rt2), (x, y, dist));
}

#[test]
fn print_logs_values_for_debugging() {
    // `print(x)` is a transparent debug channel: it logs x and yields x, so
    // it never changes world state (deterministic).
    let src = r#"
            world { gravity = (0,0,0) entity e { state = (1, 0) } }
            systems { update { dt = 0.01
                s0 = s0 + dt*(  print(s0 + 1) )
                s1 = s1 + dt*(  s0 * 2 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
    // Each step logs the value of (s0+1): 3 logs after 3 steps.
    assert_eq!(rt.logs().len(), 3, "one print per step");
    // `print` is transparent: s0 grew by dt*(s0+1) each step (finite, > initial).
    let st = rt
        .scene
        .get(EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap();
    assert!(
        st.values[0].is_finite() && st.values[0] > 1.0,
        "s0 = {}",
        st.values[0]
    );
}

/// Logical operators (`and` / `or` / `not`) yield 1.0 / 0.0 and combine
/// comparisons for compound branch conditions.
#[test]
fn logical_operators_drive_branching() {
    let src = r#"
            world { gravity = (0,0,0) entity a { state = (s0=0.5, 0,0,0,0,0,1.0,0) } }
            systems { update { on = a; dt = 1.0
                let warm = s0 > 0.0
                s1 = s1 + dt*(  warm and (s0 < 1.0) )
                s2 = s2 + dt*(  (s0 > 2.0) or (s0 > 0.1) )
                s3 = s3 + dt*(  not (s0 > 0.0) )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!((st.values[1], st.values[2], st.values[3]), (1.0, 1.0, 0.0));
}

/// A `let` bound to a reserved token (`t`, `pi`, `e`, `sN`) is rejected:
/// the grammar resolves those names before bare idents, so such a binding
/// could never be read back.
#[test]
fn shadowed_let_name_is_rejected() {
    for name in ["t", "pi", "e", "s0"] {
        let src = format!(
                "world {{ gravity=(0,0,0) entity a {{ state=(0,0,0,0,0,0,1,0); color=0x112233 }} }} \
                 systems {{ update {{ on = a; dt = 1.0 let {name} = 1.0 s5 = s5 + dt*( {name} ) }} }}"
            );
        match LangRuntime::compile(&src) {
            Ok(_) => panic!("let {name} should be rejected"),
            Err(e) => assert_eq!(e.detail, 67, "let {name}"),
        }
    }
}

/// `repeat n { … }` unrolls: Newton iteration converges to sqrt(2) within
/// one step.
#[test]
fn repeat_loop_iterates_newton_sqrt() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let g = s1
                repeat 8 { let g = (g + s0 / g) * 0.5 }
                s1 = s1 + dt*(  g - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
        "s1 = {}",
        st.values[1]
    );
}

/// `for i in lo..hi` binds the index per iteration.
#[test]
fn for_loop_binds_index() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let acc = s1
                for i in 1..6 { let acc = acc + i }
                s1 = s1 + dt*(  acc - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 15.0, "1+2+3+4+5");
}

/// `break if (cond)` stops the loop early; convergence is exact.
#[test]
fn break_exits_loop_early() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let g = s1
                repeat 100 {
                    let g = (g + s0 / g) * 0.5
                    break if (abs(g * g - s0) < 1e-12)
                }
                s1 = s1 + dt*(  g - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
        "s1 = {}",
        st.values[1]
    );
}

/// `repeat n until (cond)` exits after an iteration when cond is true;
/// `repeat n while (cond)` skips an iteration when cond is false.
#[test]
fn until_and_while_gates_exit_early() {
    let until_src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=100.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let v = s1
                repeat 100 until (v < 4.0) { let v = v * 0.5 }
                s1 = s1 + dt*(  v - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(until_src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 3.125, "100/2^5 = 3.125");

    let while_src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=5.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let v = s1
                repeat 100 while (v < 32.0) { let v = v * 2.0 }
                s1 = s1 + dt*(  v - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(while_src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 40.0, "5*2^3 stops at 40 >= 32");
}

/// `continue if (cond)` skips the rest of the iteration only.
#[test]
fn continue_skips_rest_of_iteration() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let a = s1
                repeat 3 {
                    let a = a + 1.0
                    continue if (a > 2.0)
                    let a = a * 2.0
                }
                s1 = s1 + dt*(  a - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 4.0);
}

/// A `break` exits only the innermost loop; the outer loop continues.
#[test]
fn nested_break_exits_inner_loop_only() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let a = s1
                repeat 2 {
                    repeat 5 {
                        let a = a + 1.0
                        break if (a > 2.0)
                    }
                    let a = a + 10.0
                }
                s1 = s1 + dt*(  a - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 24.0);
}

/// `rk4` supports loops too (shared let lowering): compiles and steps.
#[test]
fn rk4_supports_repeat_loop() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { rk4 { on = n; dt = 0.01
                let g = s1
                repeat 8 { let g = (g + s0 / g) * 0.5 }
                inte s1 =  s0 - s1 * s1
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(10).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(st.values[1].is_finite(), "s1 = {}", st.values[1]);
}

/// Loop validation: bad counts, descending ranges, and stray `break` are
/// rejected with specific diagnostics.
#[test]
fn loop_validation_errors() {
    let base = "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } systems { update { on = a; dt = 1.0 ";
    let cases: &[(&str, u32)] = &[
        ("repeat 0 { let x = 1.0 } s0 = x } }", 65),
        ("repeat 1001 { let x = 1.0 } s0 = x } }", 65),
        ("break s0 = x } }", 60),
        ("continue s0 = x } }", 60),
        ("for i in 5..3 { let x = i } s0 = x } }", 68),
    ];
    for (body, want) in cases {
        let src = format!("{base}{body}");
        match LangRuntime::compile(&src) {
            Ok(_) => panic!("should reject: {body}"),
            Err(e) => assert_eq!(e.detail, *want, "{body}"),
        }
    }
}

/// Scientific notation literals (`1e-12`) parse as single numbers.
#[test]
fn scientific_notation_parses() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=1e-3, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                s1 = s1 + dt*(  s0 * 1e2 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!((st.values[0] - 1e-3).abs() < 1e-12);
    assert!((st.values[1] - 0.1).abs() < 1e-9, "s1 = {}", st.values[1]);
}

/// A function body may be a sequence of `let` bindings ending with an
/// explicit `return expr`; parameter names bind as locals.
#[test]
fn function_return_with_lets() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, s2=0.0, 0,0,1.0,0) } }
            funcs {
                norm(a, b) {
                    let d = a - b
                    return abs(d)
                }
            }
            systems { update { on = n; dt = 1.0  s2 = s2 + dt*(  norm(2.0, 1.0) ) } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[2], 1.0, "|2-1|");
}

/// Function bodies support bounded loops (`repeat` / `for`) before the
/// `return`, with break/continue.
#[test]
fn function_return_with_loop() {
    let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            funcs {
                newton(x) {
                    let g = s0
                    repeat 8 { let g = (g + x / g) * 0.5 }
                    return g
                }
                newtonb(x) {
                    let g = s0
                    repeat 100 {
                        let g = (g + x / g) * 0.5
                        break if (abs(g * g - x) < 1e-12)
                    }
                    return g
                }
            }
            systems { update { on = n; dt = 1.0
                s1 = s1 + dt*(  newton(2.0) - s1 )
                s1 = s1 + dt*(  newtonb(2.0) - s1 )
            } }
        "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
        "s1 = {}",
        st.values[1]
    );
}

/// A statement-form function body without `return` is rejected (the bare
/// expression form remains available).
#[test]
fn function_body_requires_return() {
    let src = "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } \
                   funcs { f(a) { let d = s0 * 2.0 } } \
                   systems { update { on = a; dt = 1.0 s0 = s0 + dt*(  f(1.0) ) } }";
    match LangRuntime::compile(src) {
        Ok(_) => panic!("statement body without return should be rejected"),
        // The body must end in `return` (or a terminal control-flow `if`).
        Err(e) => assert_eq!(e.detail, 55),
    }
    // Bare-expression bodies still work.
    let src2 = "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } \
                    funcs { f(a) { s0 * 2.0 } } \
                    systems { update { on = a; dt = 1.0 s0 = s0 + dt*(  f(1.0) ) } }";
    LangRuntime::compile(src2).unwrap();
}

/// An invariant over a truthy expression holds across steps.
#[test]
fn invariant_holds_when_expression_truthy() {
    let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = a + dt*(  0.0 * a ) b = b + dt*(  0.0 * b ) } \
                   invariant { on = chem; expr = (a + b) == 1.0 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(5).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 1.0);
    assert_eq!(st.values[1], 0.0);
}

/// A violated invariant fails the step before any write is applied: the
/// scene keeps its pre-step state.
#[test]
fn invariant_violation_fails_step_and_preserves_scene() {
    let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = a + dt*(  0.0 - 1.0 ) } \
                   invariant { on = chem; expr = (a + b) == 1.0 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    match rt.step_cross() {
        Ok(_) => panic!("violated invariant should fail the step"),
        Err(e) => {
            assert_eq!(e.status, pwe_api::Status::EirInvalid);
            assert_eq!(e.detail, 69);
        }
    }
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 1.0, "scene must be untouched");
}

/// A NaN state fails the invariant step too (the EIR's own comparison
/// rejection surfaces first), with the scene untouched.
#[test]
fn invariant_catches_nan_state() {
    let src = "world { gravity=(0,0,0) entity chem { state=(a=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = a + dt*(  sqrt(0.0 - 1.0) ) } \
                   invariant { on = chem; expr = a == a } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    // The post-update check sees the NaN the step itself produced.
    assert!(rt.step_cross().is_err(), "NaN state must fail the step");
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 0.0, "scene must be untouched");
}

/// `neighbor_count` / `nearest_dist` read the scene through EIR, shared by
/// both backends (cross-checked via `step_cross`).
#[test]
fn spatial_neighbor_count_and_nearest_dist() {
    let src = "world { gravity=(0,0,0) \
                   entity a { state=(x=0.0,y=0.0,z=0.0) } \
                   entity b { state=(x=1.0,y=0.0,z=0.0) } \
                   entity c { state=(x=5.0,y=0.0,z=0.0) } } \
                   systems { update { on = a; dt = 0.01 \
                     let n = neighbor_count(2.0); let d = nearest_dist() \
                     s3 = s3 + dt*(  n ) s4 = s4 + dt*(  d ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[3] / 0.01, 1.0, "one neighbor within 2.0");
    assert!(
        (st.values[4] / 0.01 - 1.0).abs() < 1e-9,
        "nearest at distance 1"
    );
}

/// Gradual dimensional analysis: a correctly unit-annotated model compiles;
/// an inconsistent rule is rejected with detail 77. Unit-free models are
/// unaffected (wildcards never error).
#[test]
fn units_check_consistent_and_reject_mismatch() {
    let ok = "world { gravity=(0,0,0) params { k = 4.0 [1/s^2] } \
                  entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) } } \
                  systems { update { on = e; dt = 0.1 [s] \
                    vx = vx + dt*(  0.0 - k * x \n ) x = x + dt*(  vx ) } }";
    LangRuntime::compile(ok).unwrap();

    let bad = "world { gravity=(0,0,0) params { k = 4.0 [1/s^2] } \
                   entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) } } \
                   systems { update { on = e; dt = 0.1 [s] \
                     x = x + dt*(  0.0 - k * x \n ) vx = vx + dt*(  vx ) } }";
    match LangRuntime::compile(bad) {
        Ok(_) => panic!("dimension mismatch must be rejected"),
        Err(e) => assert_eq!(e.detail, 77),
    }

    // Unannotated model: never errors.
    let free = "world { gravity=(0,0,0) entity e { state=(x=1.0) } } \
                    systems { update { on = e; dt = 0.1; x = x + dt*(  0.0 - x ) } }";
    LangRuntime::compile(free).unwrap();
}

/// Units also cover `when` gates (must be dimensionless) and the internal
/// consistency of `invariant` expressions.
#[test]
fn units_cover_when_and_invariant() {
    let bad_when = "world { gravity=(0,0,0) entity e { state=(x=1.0 [m]) } } \
                        systems { update { on = e; dt = 0.1 when = x x = x + dt*(  0.0 - x ) } }";
    match LangRuntime::compile(bad_when) {
        Ok(_) => panic!("a dimensioned `when` gate must be rejected"),
        Err(e) => assert_eq!(e.detail, 77),
    }
    // `x` (m) + `t` (s) is dimensionally inconsistent inside an invariant.
    let bad_inv = "world { gravity=(0,0,0) entity e { state=(x=1.0 [m]) } } \
                       systems { invariant { on = e; expr = x + t } }";
    match LangRuntime::compile(bad_inv) {
        Ok(_) => panic!("m + s must be rejected"),
        Err(e) => assert_eq!(e.detail, 77),
    }
    // A dimensionless gate over a dimensioned model is fine.
    let ok = "world { gravity=(0,0,0) \
                  entity e { state=(x=1.0 [m], vx=0.5 [m/s], gate=0.0) } } \
                  systems { update { on = e; dt = 0.1 [s] when = gate > 0.0 x = x + dt*(  vx ) } }";
    LangRuntime::compile(ok).unwrap();
}

/// Scheduled events: `at(T)` fires in exactly one step (the window that
/// contains `T`); `periodic(P)` fires once per period.
#[test]
fn scheduled_events_fire_exactly_once() {
    let src = "world { gravity=(0,0,0) entity e { state=(x=0.0, fires=0.0) } } \
                   systems { update { on = e; dt = 0.5 \
                     x = x + dt*(  0.0 + 1.0 )
                     fires = fires + dt*(  if(at(1.0), 1.0, 0.0) + if(periodic(1.0), 1.0, 0.0) ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    // 6 steps at dt=0.5 -> t = 0, 0.5, 1.0, 1.5, 2.0, 2.5.
    rt.step_cross_n(6).unwrap();
    let fires = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[1];
    // at(1.0) fires once; periodic(1.0) fires at t=0,1,2 (three times).
    // fires += dt * (1 + 3) = 0.5 * 4 = 2.0.
    assert!((fires - 2.0).abs() < 1e-12, "fires={fires}");
}

/// Python-style modules: `import "m"` binds namespace `m` (`m::f`, `m::G`),
/// `from "m" import f` binds bare, packages resolve via `__init__.pwe`, and
/// a module's own systems resolve their bare parameter/function names within
/// their own namespace.
#[test]
fn modules_packages_and_namespaces() {
    let dir = std::env::temp_dir().join(format!("pwe_modules_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("shapes")).unwrap();
    std::fs::write(
        dir.join("shapes/__init__.pwe"),
        "world { entity body { state=(x=1.0, vx=0.0) } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("physics.pwe"),
        "world { params { G = 2.0 } }\nfuncs { thrust(m) { m * 0.5 } }\n\
             systems { update { on = body; dt = 0.1 vx = vx + dt*(  0.0 - G * x ) } }\n",
    )
    .unwrap();
    // Qualified access (`physics.G`, `physics.thrust`) + package entity.
    std::fs::write(
        dir.join("main.pwe"),
        "world { gravity=(0,0,0)\n import \"shapes\"\n import \"physics\" }\n\
             systems { update { on = body; dt = 0.1 \
               vx = vx + dt*(  0.0 - physics.G * x + 0.0 * physics.thrust(2.0) ) } }\n",
    )
    .unwrap();
    let rt = LangRuntime::compile_file(&dir.join("main.pwe")).unwrap();
    assert_eq!(rt.scene.entities.len(), 1, "package entity present");
    assert_eq!(rt.scene.params.get("physics.G").copied(), Some(2.0));
    // `from … import …` binds bare.
    std::fs::write(
            dir.join("from.pwe"),
            "from \"physics\" import thrust\n\
             world { gravity=(0,0,0)\n import \"shapes\" }\n\
             systems { update { on = body; dt = 0.1 vx = vx + dt*(  0.0 - 0.0 * x + 0.0 * thrust(3.0) ) } }\n",
        )
        .unwrap();
    LangRuntime::compile_file(&dir.join("from.pwe")).unwrap();
    // Missing module.
    std::fs::write(
        dir.join("missing.pwe"),
        "world { gravity=(0,0,0) }\nimport \"nope\"\n",
    )
    .unwrap();
    match LangRuntime::compile_file(&dir.join("missing.pwe")) {
        Ok(_) => panic!("missing module must fail"),
        Err(e) => assert_eq!(e.detail, 76),
    }
    // Duplicate entity across modules.
    std::fs::write(dir.join("dup.pwe"), "world { entity body { state=(0) } }\n").unwrap();
    std::fs::write(
        dir.join("dup2.pwe"),
        "world { gravity=(0,0,0) }\nimport \"shapes\"\nimport \"dup\"\n",
    )
    .unwrap();
    match LangRuntime::compile_file(&dir.join("dup2.pwe")) {
        Ok(_) => panic!("duplicate entity must be rejected"),
        Err(e) => assert_eq!(e.detail, 76),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bare name that resolves to no local/slot/param reads 0.0 (the
/// documented unresolved-reference convention) instead of failing the step
/// — an unresolved world-level component must not trip the entity lookup.
/// RFC-0038: `spawn` activates one free slot per step and copies the
/// caller's state; inactive slots are hidden; `despawn` clears them.
#[test]
fn pool_spawn_despawn_roundtrip() {
    let src = "world { gravity=(0,0,0) \
                   entity emitter { state = (x = 5.0, y = 1.0) } \
                   pool p[3] { state = (x = 0.0, y = 0.0) } } \
                   systems { spawn { on = emitter; pool = p } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    // Boot: only the emitter (id 1) is active.
    assert_eq!(rt.scene.entities.values().filter(|e| e.active).count(), 1);
    rt.step_cross().unwrap();
    rt.step_cross().unwrap();
    let active: Vec<u128> = rt
        .scene
        .entities
        .iter()
        .filter(|(_, e)| e.active)
        .map(|(id, _)| id.0)
        .collect();
    assert!(
        active.contains(&2) && active.contains(&3),
        "active={active:?}"
    );
    assert!(!active.contains(&4), "third step not yet taken: {active:?}");
    // The slot received the caller's state.
    let s = rt.scene.get(EntityId(2)).unwrap().state.as_ref().unwrap();
    assert_eq!((s.values[0], s.values[1]), (5.0, 1.0));

    // `despawn` with a `when` that never holds leaves the slots active.
    let src2 = "world { gravity=(0,0,0) \
                    entity emitter { state = (x = 5.0) } \
                    pool p[2] { state = (x = 0.0) } } \
                    systems { spawn { on = emitter; pool = p } \
                              despawn { on = p; when = x > 100.0 } }";
    let mut rt2 = LangRuntime::compile(src2).unwrap();
    rt2.step_cross().unwrap();
    assert!(rt2.scene.get(EntityId(2)).unwrap().active);
    // A `when` that always holds clears them.
    let src3 = "world { gravity=(0,0,0) \
                    entity emitter { state = (x = 5.0) } \
                    pool p[2] { state = (x = 0.0) } } \
                    systems { spawn { on = emitter; pool = p } \
                              despawn { on = p; when = active() and x > 0.0 } }";
    let mut rt3 = LangRuntime::compile(src3).unwrap();
    rt3.step_cross().unwrap();
    assert!(!rt3.scene.get(EntityId(2)).unwrap().active);
}

/// RFC-0038: `spawn` batches (`count = n`) and phases (`every`/`phase`).
#[test]
fn pool_spawn_batch_and_phase() {
    let count_active = |rt: &LangRuntime| rt.scene.entities.values().filter(|e| e.active).count();
    // Batch: three slots per step.
    let batch = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                     pool p[6] { state = (x = 0.0) } } \
                     systems { spawn { on = e; pool = p; count = 3 } }";
    let mut rt = LangRuntime::compile(batch).unwrap();
    rt.step_cross().unwrap();
    assert_eq!(count_active(&rt), 1 + 3);
    rt.step_cross().unwrap();
    assert_eq!(count_active(&rt), 1 + 6);
    // Phased: emit only on even steps.
    let phased = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                      pool p[6] { state = (x = 0.0) } } \
                      systems { spawn { on = e; pool = p; every = 2 } }";
    let mut rt = LangRuntime::compile(phased).unwrap();
    rt.step_cross().unwrap(); // step 0 -> emit
    assert_eq!(count_active(&rt), 1 + 1);
    rt.step_cross().unwrap(); // step 1 -> skip
    assert_eq!(count_active(&rt), 1 + 1);
    rt.step_cross().unwrap(); // step 2 -> emit
    assert_eq!(count_active(&rt), 1 + 2);
    // Phase offset: `every = 2; phase = 1` emits on odd steps.
    let offset = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                      pool p[6] { state = (x = 0.0) } } \
                      systems { spawn { on = e; pool = p; every = 2; phase = 1 } }";
    let mut rt = LangRuntime::compile(offset).unwrap();
    rt.step_cross().unwrap(); // step 0 -> skip
    assert_eq!(count_active(&rt), 1);
    rt.step_cross().unwrap(); // step 1 -> emit
    assert_eq!(count_active(&rt), 1 + 1);
}

/// RFC-0039: distance joints converge to the rest length, prismatic joints
/// pull a body onto the axis, and rotational joints are rejected clearly.
#[test]
fn joint_family_behaves() {
    let pos = |rt: &LangRuntime, id: u128| {
        rt.scene
            .get(EntityId(id))
            .unwrap()
            .transform
            .unwrap()
            .position
    };
    // distance: bodies 2 apart converge to rest length 1.
    let d = "world { gravity=(0,0,0) \
                 entity a { position=(0,0,0) mass=1.0 dynamic=true } \
                 entity b { position=(2,0,0) mass=1.0 dynamic=true } } \
                 systems { joint { on = a; other = b; type = distance; length = 1.0; iterations = 8 } }";
    let mut rt = LangRuntime::compile(d).unwrap();
    rt.step_cross_n(20).unwrap();
    let (pa, pb) = (pos(&rt, 1), pos(&rt, 2));
    let dist = ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2) + (pb.z - pa.z).powi(2)).sqrt();
    assert!((dist - 1.0).abs() < 1e-3, "distance {dist}");
    // the correction is symmetric for equal masses.
    assert!(
        (pa.x + pb.x - 2.0).abs() < 1e-6,
        "centre preserved: {} {}",
        pa.x,
        pb.x
    );

    // prismatic along +x removes the perpendicular offset.
    let p = "world { gravity=(0,0,0) \
                 entity a { position=(0,0,0) mass=1.0 dynamic=false } \
                 entity b { position=(1,2,3) mass=1.0 dynamic=true } } \
                 systems { joint { on = a; other = b; type = prismatic; axis = (1,0,0); iterations = 8 } }";
    let mut rt = LangRuntime::compile(p).unwrap();
    rt.step_cross_n(10).unwrap();
    let pb = pos(&rt, 2);
    assert!(pb.y.abs() < 1e-3 && pb.z.abs() < 1e-3, "on axis: {pb:?}");
    assert!((pb.x - 1.0).abs() < 1e-6, "x free: {}", pb.x);

    // rotational joints are rejected.
    let bad = "world { gravity=(0,0,0) entity a { position=(0,0,0) mass=1 } \
                   entity b { position=(1,0,0) mass=1 } } \
                   systems { joint { on = a; other = b; type = cone } }";
    assert!(LangRuntime::compile(bad).is_err(), "cone must be rejected");
}

/// RFC-0040: a soft-body grid keeps its spacing and falls under gravity.
#[test]
fn soft_body_falls_and_keeps_spacing() {
    let src = "world { gravity=(0,-9.81,0) \
                   soft cloth { nx=4; ny=4; spacing=1.0; origin=(0,5,0); mass=0.1 } } \
                   systems { gravity { gravity_y=-9.81; dt=0.01 } \
                             integrate { dt=0.01 } \
                             soft { body=cloth; stiffness=1.0; iterations=6 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    let p = |rt: &LangRuntime, id: u128| {
        rt.scene
            .get(EntityId(id))
            .unwrap()
            .transform
            .unwrap()
            .position
    };
    rt.step_cross_n(1).unwrap();
    let (a, b) = (p(&rt, 1), p(&rt, 2));
    let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt();
    assert!((d - 1.0).abs() < 1e-6, "spacing preserved: {d}");
    // The whole sheet falls under gravity (top row starts at y = 5+3 = 8).
    rt.step_cross_n(50).unwrap();
    assert!(p(&rt, 1).y < 8.0 - 0.5, "sheet fell: {}", p(&rt, 1).y);
    // And it stays cohesive (spacing still ~1).
    let (a, b) = (p(&rt, 1), p(&rt, 2));
    let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt();
    assert!((d - 1.0).abs() < 0.05, "cohesive after falling: {d}");
}

/// RFC-0040 (3D): a volumetric soft grid keeps spacing along all three axes.
#[test]
fn soft_body_3d_grid() {
    let src = "world { gravity=(0,0,0) \
                   soft gel { nx=3; ny=3; nz=3; spacing=1.0; origin=(0,0,0); mass=0.1 } } \
                   systems { soft { body=gel; stiffness=1.0; iterations=8 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    let p = |rt: &LangRuntime, id: u128| {
        rt.scene
            .get(EntityId(id))
            .unwrap()
            .transform
            .unwrap()
            .position
    };
    // 27 particles; particle k = (kz*ny + ky)*nx + kx, ids 1..=27.
    assert_eq!(rt.scene.entities.len(), 27);
    rt.step_cross_n(5).unwrap();
    let p0 = p(&rt, 1); // (0,0,0)
    let px = p(&rt, 2); // (1,0,0)
    let pj = p(&rt, 4); // (0,1,0)  -> k=3
    let pk = p(&rt, 10); // (0,0,1) -> k=9
    let d = |a: crate::math::Vec3, b: crate::math::Vec3| (a - b).length();
    assert!((d(p0, px) - 1.0).abs() < 1e-6, "x spacing {}", d(p0, px));
    assert!((d(p0, pj) - 1.0).abs() < 1e-6, "y spacing {}", d(p0, pj));
    assert!((d(p0, pk) - 1.0).abs() < 1e-6, "z spacing {}", d(p0, pk));
}

/// A `rotation = (rx, ry, rz)` entity field tilts the body (Transform).
#[test]
fn rotation_field_tilts_a_body() {
    let src = "world { gravity=(0,0,0) \
                   entity e { position=(0,0,0); rotation=(0.0, 0.0, 1.5707963267948966) } }";
    let rt = LangRuntime::compile(src).unwrap();
    let q = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .transform
        .unwrap()
        .rotation;
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert!((q.z - h).abs() < 1e-6, "{q:?}");
    assert!((q.w - h).abs() < 1e-6, "{q:?}");
}

/// A shape may include another declared shape (`part <other> at (...)`) and
/// the reference is inlined; reference cycles are rejected.
#[test]
fn shape_can_include_another_shape() {
    let src = "world { gravity=(0,0,0) \
                   shape ball { part sphere = 0.2 at (0,0,0); } \
                   shape stack { part ball at (0,0,0); part ball at (0,0.4,0); } \
                   entity e { position=(0,0,0); shape = stack } }";
    let rt = LangRuntime::compile(src).unwrap();
    let parts = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .render
        .as_ref()
        .unwrap()
        .parts
        .clone()
        .unwrap();
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|p| p.kind == 1));
    assert!(
        (parts[1].offset.1 - 0.4).abs() < 1e-9,
        "{:?}",
        parts[1].offset
    );

    let cyc = "world { gravity=(0,0,0) \
                   shape a { part b at (0,0,0); } shape b { part a at (0,0,0); } \
                   entity e { position=(0,0,0); shape = a } }";
    assert!(LangRuntime::compile(cyc).is_err(), "cycle must be rejected");
}

/// `orient = true` marks a state body to read its render orientation from
/// state slots 7/8/9 (pitch, yaw, roll).
#[test]
fn orient_flag_sets_render_orientation() {
    let src = "world { gravity=(0,0,0) \
                   entity e { state=(x=0,y=0,z=0,a=0,b=0,c=0,d=0, rx=0.1, ry=0.2, rz=0.0) \
                              orient = true } }";
    let rt = LangRuntime::compile(src).unwrap();
    assert!(
        rt.scene
            .get(EntityId(1))
            .unwrap()
            .render
            .as_ref()
            .unwrap()
            .orient
    );
}

/// `name = <number>` is a **rule** unless `name` is a recognised parameter
/// of that system kind (so `update { x = 1.0 }` integrates x, while
/// `update { dt = 0.01 }` is the dt parameter).
#[test]
fn bare_number_rhs_is_a_rule_not_a_parameter() {
    let src = "world { gravity=(0,0,0) entity m { state = (x = 0.0) } } \
                   systems { update { on = m; dt = 1.0 x = x + dt*(  1.0 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
    let x = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert!(
        (x - 3.0).abs() < 1e-9,
        "x should integrate by dt=1 per step: {x}"
    );
}

/// RFC-0042: `struct` record types flatten to dotted state slots; nested
/// types, inline records, dotted rule LHS, and `@name.a.b` access all work.
#[test]
fn struct_record_types_layout_and_rules() {
    let src = "world { \
          struct V { x = 1.0; y = 2.0 } \
          struct Body { pos = V; vel = V; mass = 5.0 } \
          entity a { state = Body } \
          entity c { state = (pos = (x = 7.0, y = 8.0)) } \
          entity probe { state = (d = 0.0) } } \
          systems { \
            update { on = a; dt = 1.0 pos.x = pos.x + dt*(  vel.x )  vel.x = vel.x + dt*(  0.0 + 1.0 ) } \
            update { on = probe; dt = 1.0 d = @a.pos.x } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
    let a = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values
        .clone();
    assert!((a[0] - 7.0).abs() < 1e-9, "pos.x = {}", a[0]);
    assert!((a[2] - 4.0).abs() < 1e-9, "vel.x = {}", a[2]);
    let c = rt
        .scene
        .get(EntityId(2))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values
        .clone();
    assert_eq!((c[0], c[1]), (7.0, 8.0));
    let pr = rt
        .scene
        .get(EntityId(3))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values
        .clone();
    assert!((pr[0] - 7.0).abs() < 1e-9, "probe = a.pos.x");
}

#[test]
fn unknown_struct_type_is_an_error() {
    let bad = "world { gravity=(0,0,0) entity e { state = Nope } }";
    assert!(LangRuntime::compile(bad).is_err());
}

#[test]
fn unresolved_bare_name_reads_zero() {
    let src = "world { gravity=(0,0,0) entity e { state=(x=0.0) } } \
                   systems { update { on = e; dt = 0.5 x = x + dt*(  3.0 + zzz ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!((st.values[0] - 1.5).abs() < 1e-12, "got {}", st.values[0]);
}

/// The `wave` solver integrates `u_tt = c²∇²u` with a leapfrog over two
/// fields: a symmetric initial pulse stays symmetric and splits outwards
/// from the centre, and remains finite.
#[test]
fn wave_solver_propagates_symmetrically() {
    let src = "world { gravity=(0,0,0) \
                   field u { width=41; height=1; dx=1.0 } \
                   field um { width=41; height=1; dx=1.0 } \
                   entity e { state=(x=0.0) } } \
                   systems { wave { field = u; prev = um; velocity = 1.0; dt = 0.5 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.scene.fields.get_mut("u").unwrap().set(20, 0, 1.0);
    rt.scene.fields.get_mut("um").unwrap().set(20, 0, 1.0);
    rt.step_cross_n(8).unwrap();
    let u = rt.scene.fields.get("u").unwrap();
    for k in 0..20usize {
        assert!(
            (u.value(20 - k, 0) - u.value(20 + k, 0)).abs() < 1e-9,
            "asymmetric at k={k}"
        );
    }
    assert!(u.value(20, 0).abs() < 1.0, "centre should give up energy");
    assert!(u.value(20, 0).is_finite());
    assert!(
        u.value(20 + 5, 0).abs() > 1e-6 || u.value(20 + 8, 0).abs() > 1e-6,
        "the pulse must have spread outwards"
    );
}

/// 3D field solvers: `diffuse` is exactly conservative in a 3D grid, and
/// `poisson` yields a potential well around a positive source (depth > 1).
#[test]
fn field_solver_systems_3d() {
    let d3 = "world { gravity=(0,0,0) field heat { width=5; height=5; depth=5; dx=1.0 } \
                  entity e { state=(x=0.0) } } \
                  systems { diffuse { field = heat; rate = 0.1 } \
                    update { on = e; dt = 1.0 \
                      let _ = fset(heat, 2.0, 2.0, 2.0, fget(heat, 2.0, 2.0, 2.0) + 1.0) \
                      x = x + dt*(  0.0 + 1.0 ) } }";
    let mut rt = LangRuntime::compile(d3).unwrap();
    rt.step_cross_n(12).unwrap();
    let f = rt.scene.fields.get("heat").unwrap();
    assert_eq!((f.width, f.height, f.depth), (5, 5, 5));
    assert!(
        (f.total() - 12.0).abs() < 1e-9,
        "3D diffuse conserves: {}",
        f.total()
    );

    let poi = "world { gravity=(0,0,0) field phi { width=5; height=5; depth=5; dx=1.0 } \
                   field rho { width=5; height=5; depth=5; dx=1.0 } entity e { state=(x=0.0) } } \
                   systems { poisson { field = phi; source = rho; iters = 20 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(rho, 2.0, 2.0, 2.0, 1.0) x = x + dt*(  0.0 + 1.0 ) } }";
    let mut rt = LangRuntime::compile(poi).unwrap();
    rt.step_cross_n(2).unwrap();
    let f = rt.scene.fields.get("phi").unwrap();
    assert!(
        f.value3(2, 2, 2) < 0.0,
        "3D potential well: {}",
        f.value3(2, 2, 2)
    );
}

/// Per-entity render attributes (`shape`/`size`/`opacity`/`glow`/`label`)
/// parse into the entity's render style (presentation only).
#[test]
fn entity_render_attributes_parse() {
    let src = "world { gravity=(0,0,0) entity a { state=(x=0.0) shape=sphere; size=1.5; \
                   color=0xFF6B4A; glow=1.2; opacity=0.6; label=false } }";
    let model = parse(src).unwrap().model;
    let e = &model.entities[0];
    let r = e.render.as_ref().expect("render style");
    assert_eq!(r.shape, Some(1));
    assert_eq!(r.size, Some(1.5));
    assert_eq!(r.glow, Some(1.2));
    assert_eq!(r.opacity, Some(0.6));
    assert_eq!(r.label, Some(false));
    assert_eq!(e.color, Some(0xFF6B4A));
}

/// User-defined custom shapes (`shape <name> { part … }`) parse into the
/// model and resolve to the entity's render parts at build time.
#[test]
fn custom_shapes_resolve_from_parts() {
    let src = "world { gravity=(0,0,0) \
            shape gizmo { part capsule = (0.05, 0.3, 0.05); part sphere = 0.1 at (0, 0.2, 0.0); \
              part svg = \"M 0,0 L 1,0 L 1,1 Z\" depth 0.2 scale 0.5 at (0, 1.0, 0.0); \
              part hull = [(0,0,0), (1,0,0), (0,1,0), (0,0,1)]; \
              part poly = [(0,1,0), (1,0,1), (-1,0,1)] faces = [[0,1,2]] scale 2.0; } \
            entity e { state=(x=0.0) shape = gizmo } }";
    let model = parse(src).unwrap().model;
    let g = model.shapes.get("gizmo").unwrap();
    assert_eq!(g.len(), 5);
    assert_eq!(g[2].kind, 4, "svg part");
    assert_eq!(g[2].path.as_deref(), Some("M 0,0 L 1,0 L 1,1 Z"));
    assert!((g[2].a - 0.2).abs() < 1e-9, "svg depth");
    assert!((g[2].scale - 0.5).abs() < 1e-9, "svg scale");
    assert_eq!(g[3].kind, 5, "hull part");
    assert_eq!(g[3].points.len(), 4);
    assert_eq!(g[4].kind, 6, "poly part");
    assert_eq!(g[4].faces, vec![vec![0u32, 1, 2]]);
    assert!((g[4].scale - 2.0).abs() < 1e-9, "poly amplitude");
    let scene = model.build_scene();
    let ent = scene.entities.values().next().unwrap();
    let parts = ent
        .render
        .as_ref()
        .unwrap()
        .parts
        .as_ref()
        .expect("resolved parts");
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[1].kind, 1);
    assert_eq!(parts[1].offset, (0.0, 0.2, 0.0));
}

/// A named state slot that merely starts with `s` (e.g. `speed`) is not a
/// numeric `sN` token and must still resolve to its slot.
#[test]
fn named_slots_starting_with_s_resolve() {
    let src = "world { gravity=(0,0,0) entity e { state=(speed=0.0, s0x=0.0) } } \
                   systems { update { on = e; dt = 1.0
                     speed = speed + dt*(  4.0 + 0.0 )
                     s0x = s0x + dt*(  9.0 + 0.0 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!((st.values[0], st.values[1]), (4.0, 9.0));
}

/// Field solver system kinds: `diffuse` conserves the injected total
/// exactly (Jacobi sweep), and `poisson` relaxes toward a negative
/// potential around a positive source.
#[test]
fn field_solver_systems() {
    let src = "world { gravity=(0,0,0) field heat { width=8; height=8; dx=1.0 } \
                   entity e { state=(x=0.0) } } \
                   systems { diffuse { field = heat; rate = 0.2 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(heat, 4.0, 4.0, fget(heat, 4.0, 4.0) + 1.0) \
                       x = x + dt*(  0.0 + 1.0 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(10).unwrap();
    let total = rt.scene.fields.get("heat").unwrap().total();
    assert!(
        (total - 10.0).abs() < 1e-9,
        "diffuse must conserve: {total}"
    );

    let poi = "world { gravity=(0,0,0) field phi { width=6; height=6; dx=1.0 } \
                   field rho { width=6; height=6; dx=1.0 } entity e { state=(x=0.0) } } \
                   systems { poisson { field = phi; source = rho; iters = 40 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(rho, 3.0, 3.0, 1.0) x = x + dt*(  0.0 + 1.0 ) } }";
    let mut rt = LangRuntime::compile(poi).unwrap();
    rt.step_cross_n(3).unwrap();
    let f = rt.scene.fields.get("phi").unwrap();
    assert!(
        f.value(3, 3) < 0.0,
        "positive source yields a negative potential well: {}",
        f.value(3, 3)
    );
}

/// Circular imports resolve (a module is loaded once and reachable by every
/// alias): `a` imports `b` which imports `a`, and both are called.
#[test]
fn circular_imports_and_multiple_aliases_resolve() {
    let dir = std::env::temp_dir().join(format!("pwe_cycle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("x.pwe"),
        "world { params { G = 1.0 } }\nfuncs { fx(v) { v + G } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("a.pwe"),
        "import \"x\" as alpha\nworld { }\nimport \"b\"\n\
             systems { update { on = e; dt = 1.0 s0 = s0 + dt*(  alpha.fx(0.0) + b.g(0.0) ) } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.pwe"),
        "import \"a\"\nimport \"x\"\nworld { }\n\
             funcs { g(v) { v + 100.0 } }\n\
             systems { update { on = e; dt = 1.0 s1 = s1 + dt*(  x.fx(0.0) ) } }\n",
    )
    .unwrap();
    std::fs::write(
            dir.join("main.pwe"),
            "import \"a\"\nimport \"b\"\nworld { gravity=(0,0,0) entity e { state=(s0=0.0, s1=0.0) } }\n",
        )
        .unwrap();
    let mut rt = LangRuntime::compile_file(&dir.join("main.pwe")).unwrap();
    // Both aliases of the same module are usable, and the cycle merges once.
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        st.values[0] > 0.0 && st.values[1] > 0.0,
        "both alias references resolved: {:?}",
        st.values
    );
    // `--param`-style override reaches every alias of one parameter.
    let canon = rt
        .scene
        .params
        .keys()
        .filter(|k| k.ends_with(".G") || *k == "G")
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        canon.len() >= 2,
        "module imported under two namespaces: {canon:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `params { … }` declares runtime-settable model parameters that rules
/// read by name (overridable on the scene before running).
#[test]
fn model_params_read_and_override() {
    let src =
            "world { gravity=(0,0,0) params { G = 10.0 } entity e { state=(x=1.0,vx=0.0) } } \
                   systems { update { on = e; dt = 0.1; vx = vx + dt*(  0.0 - G * x ) x = x + dt*(  vx ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    assert_eq!(rt.scene.params.get("G").copied(), Some(10.0));
    rt.step_cross_n(5).unwrap();
    let a = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[1];
    // Same model, overridden parameter -> different trajectory.
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.scene.params.insert("G".to_string(), 1.0);
    rt2.step_cross_n(5).unwrap();
    let b = rt2
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[1];
    assert_ne!(a, b, "overriding G must change the trajectory");
}

/// Neighborhood aggregates and directional sensing: `neighbor_mean`    /// Neighborhood aggregates and directional sensing: `neighbor_mean`
/// averages a State slot over neighbours within a radius (0 when none),
/// and `nearest_dx/dy/dz` give the offset to the closest neighbour.
#[test]
fn neighborhood_aggregates_and_offsets() {
    let src = "world { gravity=(0,0,0) \
                   entity a { state=(x=0.0, y=0.0, z=0.0, vx=10.0, vy=0.0, vz=0.0) } \
                   entity b { state=(x=2.0, y=0.0, z=0.0, vx=20.0, vy=0.0, vz=0.0) } \
                   entity c { state=(x=9.0, y=0.0, z=0.0, vx=90.0, vy=0.0, vz=0.0) } } \
                   systems { update { on = a; dt = 1.0 \
                     let n = neighbor_count(3.0) \
                     let mx = neighbor_mean(0.0, 3.0) \
                     let mv = neighbor_mean(3.0, 3.0) \
                     s6 = s6 + dt*(  n ) s7 = s7 + dt*(  mx ) s8 = s8 + dt*(  mv ) s9 = s9 + dt*(  nearest_dx() ) s10 = s10 + dt*(  nearest_dy() ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let v = &rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values;
    assert_eq!(v[6], 1.0, "one neighbour within 3.0 (b, not c)");
    assert_eq!(v[7], 2.0, "mean x of neighbours = b.x");
    assert_eq!(v[8], 20.0, "mean vx of neighbours = b.vx");
    assert_eq!(v[9], 2.0, "nearest_dx = b.x - a.x");
    assert_eq!(v[10], 0.0, "nearest_dy");

    // Alone: aggregate and offset are 0.
    let lone = "world { gravity=(0,0,0) entity only { state=(0.0,0.0,0.0) } } \
                    systems { update { on = only; dt = 1.0 \
                      s3 = s3 + dt*(  neighbor_mean(0.0, 5.0) ) s4 = s4 + dt*(  nearest_dx() ) } }";
    let mut rt = LangRuntime::compile(lone).unwrap();
    rt.step_cross_n(1).unwrap();
    let v = &rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values;
    assert_eq!(v[3], 0.0);
    assert_eq!(v[4], 0.0);
}

/// A spatial query in a function body is rejected at compile time: there
/// is no per-entity context to target.    /// A spatial query in a function body is rejected at compile time: there
/// is no per-entity context to target.
#[test]
fn spatial_query_rejected_in_function_body() {
    let src = "world { gravity=(0,0,0) entity a { state=(x=0.0) } } \
                   funcs { f() { neighbor_count(2.0) } } \
                   systems { update { on = a; dt = 0.01 s3 = s3 + dt*(  f() ) } }";
    match LangRuntime::compile(src) {
        Ok(_) => panic!("query in a function body should be rejected"),
        Err(e) => assert_eq!(e.detail, 70),
    }
}

/// Two invariants coexist: each owns its own verdict field in the hidden
/// check component.
#[test]
fn multiple_invariants_coexist() {
    let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = a + dt*(  0.0 * a ) b = b + dt*(  0.0 * b ) } \
                   invariant { on = chem; expr = (a + b) == 1.0 } \
                   invariant { on = chem; expr = a >= 0.0 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(3).unwrap();
}

/// `noise()` draws a seeded standard normal; a fresh runtime reproduces it.
#[test]
fn noise_is_seeded_and_reproducible() {
    let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0) } } \
                   systems { update { on = e; dt = 1.0 let n = noise() s1 = s1 + dt*(  n ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let v = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[1];
    assert!(v.is_finite(), "noise must be finite, got {v}");
    let mut rt2 = LangRuntime::compile(src).unwrap();
    rt2.step_cross_n(1).unwrap();
    let v2 = rt2
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[1];
    assert_eq!(v, v2, "seeded draws must reproduce");
}

/// `vlen`/`vdot`/`vdist` compute over scalar components (pure arithmetic).
#[test]
fn vector_builtins_compute_lengths_and_dots() {
    let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     let l = vlen(3.0, 4.0, 0.0) \
                     let d = vdot(1.0, 0.0, 0.0, 0.0, 1.0, 0.0) \
                     let dist = vdist(0.0,0.0,0.0, 3.0,4.0,0.0) \
                     s1 = s1 + dt*(  l ) s2 = s2 + dt*(  d ) s3 = s3 + dt*(  dist ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 5.0);
    assert_eq!(st.values[2], 0.0);
    assert_eq!(st.values[3], 5.0);
}

/// `when = expr` gates every rule's write: the state is untouched when the
/// gate is 0 (mode semantics).
#[test]
fn when_gates_rule_writes_by_mode() {
    let src = "world { gravity=(0,0,0) entity m { state=(mode=1.0,x=0.0) } } \
                   systems { update { on = m; dt = 1.0 when = mode == 0.0 x = x + dt*(  0.0 + 1.0 ) } \
                   update { on = m; dt = 1.0 when = mode == 1.0 mode = mode + dt*(  0.0 - 2.0 * mode ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 0.0, "gated rule must not apply");
    assert_eq!(st.values[0], -1.0, "ungated rule must apply");
}

/// `substeps = n` runs the integration n times with dt/n; finer Euler
/// tracks the analytic solution better.
#[test]
fn substeps_improve_euler_accuracy() {
    let mk = |sub: usize| {
        format!(
                "world {{ gravity=(0,0,0) entity o {{ state=(x=1.0) }} }} \
                 systems {{ update {{ on = o; dt = 0.5; substeps = {sub}; x = x + dt*( 0.0 - x ) }} }}"
            )
    };
    let analytic = (-1.0f64).exp();
    let mut rt = LangRuntime::compile(&mk(1)).unwrap();
    rt.step_cross_n(2).unwrap();
    let a1 = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    let mut rt = LangRuntime::compile(&mk(4)).unwrap();
    rt.step_cross_n(2).unwrap();
    let a4 = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert!(
        (a4 - analytic).abs() < (a1 - analytic).abs(),
        "substeps=4 ({a4}) must track the analytic {analytic} better than substeps=1 ({a1})"
    );
}

/// `substeps = n` on `rk4` repeats the whole RK4 step with dt/n; RK4's
/// accuracy makes substeps redundant for smooth laws, but they must run.
#[test]
fn rk4_substeps_run() {
    let mk = |sub: usize| {
        format!(
            "world {{ gravity=(0,0,0) entity o {{ state=(x=1.0) }} }} \
                 systems {{ rk4 {{ on = o; dt = 0.5; substeps = {sub}; inte x = 0.0 - x }} }}"
        )
    };
    let analytic = (-1.0f64).exp();
    let mut rt = LangRuntime::compile(&mk(2)).unwrap();
    rt.step_cross_n(2).unwrap();
    let a = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert!(
        (a - analytic).abs() < 1e-4,
        "rk4 with substeps must stay accurate: {a} vs {analytic}"
    );
}

/// Grid field access: `fset` writes a cell, `fget` reads it back in the
/// same step (the write overlay), `flap` computes the zero-flux stencil
/// laplacian; the cell write persists in the scene's field.
#[test]
fn field_access_reads_and_writes_cells() {
    let src = "world { gravity=(0,0,0) \
                   field heat { width = 4; height = 4; dx = 1.0 } \
                   entity e { state=(s0=0.0,s1=0.0,s2=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     fset(heat, 1.0, 2.0, 0.0 + 7.0) \
                     s0 = s0 + dt*(  fget(heat, 1.0, 2.0) )
                     s1 = s1 + dt*(  flap(heat, 1.0, 2.0) )
                     s2 = s2 + dt*(  fget(heat, 3.0, 3.0) ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 7.0, "fget sees the same-step write");
    assert_eq!(st.values[1], -28.0, "laplacian of a lone 7 among zeros");
    assert_eq!(st.values[2], 0.0);
    let f = rt.scene.fields.get("heat").unwrap();
    assert_eq!(f.value(1, 2), 7.0, "the cell write persists");
    assert_eq!(f.total(), 7.0);
}

/// A field declaration without dimensions is rejected.
#[test]
fn field_declaration_requires_dimensions() {
    let src = "world { gravity=(0,0,0) field heat { dx = 1.0 } }";
    match LangRuntime::compile(src) {
        Ok(_) => panic!("a field without width/height should be rejected"),
        Err(e) => assert_eq!(e.detail, 75),
    }
}

/// `vec3 pos` in a state list reserves N consecutive slots named `pos`,
/// `pos.0` … `pos.{N-1}`; `@self.state.pos.1` reads component 1.
#[test]
fn vecn_state_reserves_named_slots() {
    let src = "world { gravity=(0,0,0) entity e { state = (vec3 pos, mass = 1.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     s0 = s0 + dt*(  0.0 + 5.0 )
                     s5 = s5 + dt*(  @self.state.pos.1 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 5.0, "pos.0 (slot 0) written");
    assert_eq!(st.values[3], 1.0, "mass follows the 3 vec slots");
    // `pos.1` (slot 1) reads the pre-step value (simultaneous rules).
    assert_eq!(st.values[5], 0.0);
}

/// The world `title = "..."` and an entity's `parent = <name>` parse into
/// the model (`pwe present` uses them for the run-info panel).
#[test]
fn title_and_parent_parse() {
    let src = "world { title = \"solar system (8 planets + Moon)\" gravity=(0,0,0) \
                   entity moon { parent = earth; state=(0) } entity earth { state=(0) } }";
    let p = parse(src).unwrap();
    assert_eq!(
        p.model.title.as_deref(),
        Some("solar system (8 planets + Moon)")
    );
    assert_eq!(p.model.entities[0].parent.as_deref(), Some("earth"));
    assert_eq!(p.model.entities[1].parent, None);
}

/// `%` is a remainder operator (fmod semantics) — used by grid walks.
#[test]
fn modulo_operator_computes_remainder() {
    let src = "world { gravity=(0,0,0) entity e { state=(0,0) } } \
                   systems { update { on = e; dt = 1.0 s0 = s0 + dt*(  255.0 % 16.0 ) s1 = s1 + dt*(  7.0 % 3.0 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 15.0);
    assert!((st.values[1] - 1.0).abs() < 1e-12);
}

/// An out-of-range grid field access fails the step with a clear error
/// instead of panicking in the runtime path.
#[test]
fn out_of_range_field_access_is_an_error() {
    let src = "world { gravity=(0,0,0) field g { width=4; height=4; dx=1.0 } \
                   entity e { state=(0) } } \
                   systems { update { on = e; dt = 1.0 s0 = s0 + dt*(  fget(g, 2.0, 9.0) ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    match rt.step_cross() {
        Ok(_) => panic!("out-of-range field access should fail the step"),
        Err(e) => assert_eq!(e.detail, 7),
    }
}

/// `last_event(kind)` reads the most recent event of that kind emitted so
/// far in the step; unknown kinds read 0. Events are cleared each step.
#[test]
fn last_event_reads_within_the_step_and_clears() {
    let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0,s1=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     emit(7.0, 42.0) \
                     s0 = s0 + dt*(  last_event(7.0) )
                     s1 = s1 + dt*(  last_event(9.0) ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 42.0, "kind 7 resolves to its payload");
    assert_eq!(st.values[1], 0.0, "unknown kind reads 0");
    assert_eq!(rt.emitted_events().len(), 1, "one event this step");
    assert_eq!(
        rt.emitted_events()[0].kind,
        7,
        "the kind is the value, not bits"
    );
    rt.step_cross_n(1).unwrap();
    assert_eq!(
        rt.emitted_events().len(),
        1,
        "events are cleared per step (not accumulated)"
    );
}

/// `every = n` runs the system only when step % n == 0.
#[test]
fn every_runs_only_on_matching_steps() {
    let src = "world { gravity=(0,0,0) entity e { state=(x=0.0) } } \
                   systems { update { on = e; dt = 1.0 every = 3 x = x + dt*(  0.0 + 1.0 ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(4).unwrap();
    let x = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert_eq!(x, 2.0, "steps 0 and 3 apply; 1 and 2 are skipped");
}

/// `watch` flags a strict sign change between consecutive steps; the flag
/// and the watch memory are ordinary state slots.
#[test]
fn watch_flags_zero_crossing() {
    let src = "world { gravity=(0,0,0) entity e { state=(x=-1.0,m=-1.0,f=0.0) } } \
                   systems { update { on = e; dt = 0.1 x = x + dt*(  0.0 + 6.0 ) } \
                   watch { on = e; expr = x; mem = 1; into = 2 } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    let flag = |rt: &LangRuntime| {
        rt.scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[2]
    };
    rt.step_cross().unwrap();
    assert_eq!(flag(&rt), 0.0, "no crossing yet");
    rt.step_cross().unwrap();
    assert_eq!(flag(&rt), 1.0, "x crossed zero between the steps");
    rt.step_cross().unwrap();
    assert_eq!(flag(&rt), 0.0, "no further crossing");
}

/// `s[i]` dynamic indexing reads and writes the State slot at a runtime
/// index, shared by both backends (cross-checked via `step_cross`).
#[test]
fn dynamic_slot_index_reads_and_writes() {
    let src = "world { gravity=(0,0,0) entity e { state=(s0=10.0,s1=20.0,s2=0.0,s3=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     let i = 0.0 + 1.0 \
                     s[i] = s[i] + dt*(  0.0 + 100.0 )
                     s2 = s2 + dt*(  s[0.0 + 1.0] ) } }";
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[1], 120.0, "s[1] += 1*100");
    assert_eq!(st.values[2], 20.0, "the read sees the pre-step state");
}

/// A dynamic slot LHS is rejected in `rk4`: its working states are
/// compile-time register chains, so a runtime-index LHS cannot feed them.
/// The `update` system supports dynamic LHS.
#[test]
fn dynamic_lhs_rejected_in_rk4() {
    let src = "world { gravity=(0,0,0) entity e { state=(s0=1.0) } } \
                   systems { rk4 { on = e; dt = 0.1 inte s[0.0] =  0.0 + 1.0 } }";
    match LangRuntime::compile(src) {
        Ok(_) => panic!("dynamic LHS in rk4 should be rejected"),
        Err(e) => assert_eq!(e.detail, 73),
    }
}

#[test]
fn lang_version_pragma_accepted_and_rejected() {
    let base = |ver: &str| {
        format!(
            "world {{ lang_version = \"{ver}\" gravity=(0,0,0) entity e {{ state=(0.0) }} }} \
             systems {{ update {{ on = e; dt = 1.0 s0 = s0 + inte(1.0) }} }}"
        )
    };
    LangRuntime::compile(&base("0.3")).unwrap();
    match LangRuntime::compile(&base("0.1")) {
        Ok(_) => panic!("unsupported lang_version must be rejected"),
        Err(e) => assert_eq!(e.detail, 83, "detail = {}", e.detail),
    }
    // `12` (unquoted) is a parse error, not an accepted version.
    assert!(LangRuntime::compile("world { lang_version = 12 }").is_err());
}

#[test]
fn migrate_v02_to_v03_source() {
    // Old implicit integration (`slot = rate`) and the old `deriv` statement /
    // `deriv(E)` operator.
    let legacy = r#"world { gravity=(0,0,0) entity e { state=(x=1.0, vx=0.0) } }
systems {
  update { on = e; dt = 0.1
    vx = 0.0 - 2.0 * x
    x = vx
  }
}
"#;
    let m = migrate_v02_to_v03(legacy).unwrap();
    assert!(m.source.contains("lang_version = \"0.3\""), "{}", m.source);
    assert!(m.source.contains("inte vx = 0.0 - 2.0 * x"), "{}", m.source);
    assert!(m.source.contains("inte x = vx"), "{}", m.source);
    assert_eq!(m.rules_migrated, 2);
    LangRuntime::compile(&m.source).unwrap();

    // Idempotent: an already-v0.3 source is returned unchanged.
    let again = migrate_v02_to_v03(&m.source).unwrap();
    assert!(again.already_current);
    assert_eq!(again.rules_migrated, 0);

    // rk4 `deriv` statements and the `deriv(E)` operator.
    let rk = r#"world { gravity=(0,0,0) entity e { state=(x=0.0, v=1.0, a=0.0) } }
systems {
  rk4 { on = e; dt = 0.1
    deriv x = v
    deriv v = 0.0 - 4.0 * x
  }
  update { on = e; dt = 0.1 a = a + deriv(v) }
}
"#;
    let m2 = migrate_v02_to_v03(rk).unwrap();
    assert!(m2.source.contains("inte x = v"), "{}", m2.source);
    assert!(
        m2.source.contains("inte v = 0.0 - 4.0 * x"),
        "{}",
        m2.source
    );
    assert!(m2.source.contains("a = a + inte(v)"), "{}", m2.source);
    LangRuntime::compile(&m2.source).unwrap();
}

#[test]
fn send_recv_honor_on() {
    // `recv { on = dst; … }` must write only `dst`, not every entity.
    let src = r#"
        world { gravity=(0,0,0)
            chan c { value = 0.0 }
            entity src { state=(v = 0.0) }
            entity dst { state=(r = 0.0) } }
        systems {
            send { on = src; chan = c; value = 42.0 }
            recv { on = dst; chan = c; slot = 0 }
        }
    "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    // ids: src = 1, dst = 2 (the channel entity follows the bodies).
    let v = rt
        .scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    let r = rt
        .scene
        .get(EntityId(2))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[0];
    assert_eq!(v, 0.0, "src must be untouched by recv: {v}");
    assert_eq!(r, 42.0, "dst receives the channel value: {r}");
}

#[test]
fn side_effect_only_update_compiles() {
    // A `let`/bare-call-only `update` runs for its side effect (no slot rules).
    let src = r#"
        world { gravity=(0,0,0) entity e { state=(x = 1.0) } }
        systems { update { on = e; dt = 1.0
            let _ = print(x)
        } }
    "#;
    LangRuntime::compile(src).unwrap();
}

#[test]
fn params_units_are_checked() {
    // Parameter unit annotations are recorded and enforced (detail 77).
    let bad = r#"
        world { gravity=(0,0,0)
            params { k = 4.0 [1/s^2] }
            entity e { state = (x = 1.0 [m]) } }
        systems { update { on = e; dt = 0.1 [s]  x = k } }
    "#;
    match LangRuntime::compile(bad) {
        Ok(_) => panic!("param/state unit mismatch must be rejected"),
        Err(e) => assert_eq!(e.detail, 77, "detail = {}", e.detail),
    }
    // A malformed unit annotation is a diagnostic (not silently ignored).
    let malformed = "world { gravity=(0,0,0) params { k = 4.0 [1/s^] } }";
    assert!(LangRuntime::compile(malformed).is_err());
}

#[test]
fn solver_stability_is_checked() {
    // Explicit diffusion needs rate <= dx²/(2·dim); the wave needs c·dt/dx <= 1/√dim.
    let detail = |src: &str| match LangRuntime::compile(src) {
        Ok(_) => panic!("expected a stability error"),
        Err(e) => e.detail,
    };
    let unstable_diffuse = r#"
        world { gravity=(0,0,0) field t { width=8; height=8; dx=1.0 } entity p { state=(0.0) } }
        systems { diffuse { field=t; rate=0.5 } update { on=p; dt=1.0 s0=s0+inte(0.0) } }
    "#;
    assert_eq!(detail(unstable_diffuse), 86);
    let unstable_wave = r#"
        world { gravity=(0,0,0) field u { width=8; height=8; dx=1.0 }
            field um { width=8; height=8; dx=1.0 } entity p { state=(0.0) } }
        systems { wave { field=u; prev=um; velocity=5.0; dt=0.5 }
            update { on=p; dt=0.5 s0=s0+inte(0.0) } }
    "#;
    assert_eq!(detail(unstable_wave), 86);
    let stable = r#"
        world { gravity=(0,0,0) field t { width=8; height=8; dx=1.0 } entity p { state=(0.0) } }
        systems { diffuse { field=t; rate=0.2 } update { on=p; dt=1.0 s0=s0+inte(0.0) } }
    "#;
    LangRuntime::compile(stable).unwrap();
}

#[test]
fn finite_check_catches_divergence() {
    // `x = x * 2.0` diverges to +inf; the finite check turns it into detail 88.
    let src = r#"
        world { gravity=(0,0,0) entity e { state=(x = 1.0) } }
        systems { update { on = e; dt = 1.0 x = x * 2.0 } }
    "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.set_finite_check(true);
    let mut caught = false;
    for _ in 0..2000 {
        if let Err(e) = rt.step_cross() {
            assert_eq!(e.detail, 88, "detail = {}", e.detail);
            caught = true;
            break;
        }
    }
    assert!(caught, "divergence must be caught by the finite check");
}

#[test]
fn conserved_quantity_is_tracked_and_enforced() {
    // RK4 conserves the oscillator energy to ~1e-9; the drift is reported.
    let ok = r#"
        world { gravity=(0,0,0) entity o { state=(x = 1.0, v = 0.0) } }
        systems {
            rk4 { on = o; dt = 0.01
                inte x = v
                inte v = -4.0 * x
            }
            conserved { on = o; expr = 0.5*v*v + 2.0*x*x; tolerance = 1e-6 }
        }
    "#;
    let mut rt = LangRuntime::compile(ok).unwrap();
    rt.step_cross_n(1000).unwrap();
    let d = rt.conserved_drifts();
    assert_eq!(d.len(), 1);
    assert!(d[0].1 < 1e-7, "energy drift too large: {}", d[0].1);

    // A damped rule breaks conservation: detail 87 before tolerance is exceeded.
    let bad = r#"
        world { gravity=(0,0,0) entity o { state=(x = 1.0, v = 0.0) } }
        systems {
            update { on = o; dt = 0.1
                inte x = v
                inte v = -4.0 * x - 0.1 * v
            }
            conserved { on = o; expr = 0.5*v*v + 2.0*x*x; tolerance = 1e-3 }
        }
    "#;
    let mut rt = LangRuntime::compile(bad).unwrap();
    let mut caught = false;
    for _ in 0..2000 {
        if let Err(e) = rt.step_cross() {
            assert_eq!(e.detail, 87, "detail = {}", e.detail);
            caught = true;
            break;
        }
    }
    assert!(caught, "damping must break the conserved quantity");
}

#[test]
fn derived_units_are_supported() {
    // Named coherent-SI units expand to base dimensions (N = kg·m/s²).
    let ok = r#"
        world { gravity=(0,0,0)
            params { F = 10.0 [N] }
            entity e { state = (mass = 1.0 [kg], a = 0.0 [m/s^2]) } }
        systems { update { on = e; dt = 0.1 [s]  a = F / mass } }
    "#;
    LangRuntime::compile(ok).unwrap();
    // `a = F` mismatches (m/s² vs N).
    let bad = r#"
        world { gravity=(0,0,0)
            params { F = 10.0 [N] }
            entity e { state = (a = 0.0 [m/s^2]) } }
        systems { update { on = e; dt = 0.1 [s]  a = F } }
    "#;
    match LangRuntime::compile(bad) {
        Ok(_) => panic!("N vs m/s^2 must be rejected"),
        Err(e) => assert_eq!(e.detail, 77, "detail = {}", e.detail),
    }
}

#[test]
fn let_type_annotations_are_checked() {
    let src = |body: &str| {
        format!(
            "world {{ gravity=(0,0,0) entity e {{ state=(x=1.0, y=0.0) }} }} \
             systems {{ update {{ on=e; dt=1.0
{body}
 y = x }} }}"
        )
    };
    // Boolean and numeric annotations that agree compile.
    LangRuntime::compile(&src("let flag: bool = x > 0.0")).unwrap();
    LangRuntime::compile(&src("let v: f64 = x * 2.0")).unwrap();
    // Mismatches are detail 89.
    for bad in ["let n: f64 = x > 0.0", "let b: bool = 1.0"] {
        match LangRuntime::compile(&src(bad)) {
            Ok(_) => panic!("`{bad}` must be rejected"),
            Err(e) => assert_eq!(e.detail, 89, "detail = {}", e.detail),
        }
    }
}

#[test]
fn func_signature_units_are_checked() {
    // `func f(x: [m]) : [m/s]` — arguments and the result are checked at calls.
    let ok = r#"
        world { gravity=(0,0,0)
            entity e { state=(x = 1.0 [m], t = 1.0 [s], v = 0.0 [m/s]) } }
        funcs { speed(d: [m], dt: [s]) : [m/s] { d / dt } }
        systems { update { on = e; dt = 1.0 [s]  v = speed(x, t) } }
    "#;
    LangRuntime::compile(ok).unwrap();
    // Swapped arguments (s vs m) must be rejected.
    let bad = r#"
        world { gravity=(0,0,0)
            entity e { state=(x = 1.0 [m], t = 1.0 [s], v = 0.0 [m/s]) } }
        funcs { speed(d: [m], dt: [s]) : [m/s] { d / dt } }
        systems { update { on = e; dt = 1.0 [s]  v = speed(t, x) } }
    "#;
    match LangRuntime::compile(bad) {
        Ok(_) => panic!("swapped units must be rejected"),
        Err(e) => assert_eq!(e.detail, 77, "detail = {}", e.detail),
    }
}

#[test]
fn int_let_uses_integer_semantics() {
    let src = r#"
        world { gravity=(0,0,0) entity e { state=(x = 0.0, y = 0.0) } }
        systems { update { on = e; dt = 1.0
            let n: i64 = 7 / 2
            let r: i64 = 7 % 3
            x = n + 0.0
            y = r + 0.0
        } }
    "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 3.0, "7/2 must be 3 (integer)");
    assert_eq!(st.values[1], 1.0, "7%3 must be 1");
}

#[test]
fn units_strict_requires_annotations() {
    let miss = r#"
        world { gravity=(0,0,0) units = "strict"
            params { k = 1.0 }
            entity e { state=(x = 1.0 [m]) } }
        systems { update { on = e; dt = 1.0 [s]  x = k * x } }
    "#;
    match LangRuntime::compile(miss) {
        Ok(_) => panic!("strict units must require annotations"),
        Err(e) => assert_eq!(e.detail, 90, "detail = {}", e.detail),
    }
    let ok = r#"
        world { gravity=(0,0,0) units = "strict"
            params { k = 1.0 [1/s] }
            entity e { state=(x = 1.0 [m], v = 0.0 [m/s]) } }
        systems { update { on = e; dt = 1.0 [s]  v = k * x } }
    "#;
    LangRuntime::compile(ok).unwrap();
}

#[test]
fn nbody_layout_is_validated() {
    let bad = r#"
        world { gravity=(0,0,0)
            entity a { state=(1.0, 0.0, 0.0, 0.0, 0.5, 0.0) }
            entity b { state=(-1.0, 0.0, 0.0, 0.0, -0.5, 0.0) } }
        systems { nbody { G = 1.0; dt = 0.001 } }
    "#;
    match LangRuntime::compile(bad) {
        Ok(_) => panic!("nbody needs 7 slots (m at index 6)"),
        Err(e) => assert_eq!(e.detail, 91, "detail = {}", e.detail),
    }
}

#[test]
fn f64_divide_by_zero_traps() {
    let src = r#"
        world { gravity=(0,0,0) entity e { state=(x = 0.0) } }
        systems { update { on = e; dt = 1.0  x = 1.0 / 0.0 } }
    "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    match rt.step_cross() {
        Ok(_) => panic!("1.0/0.0 must trap (RFC-0021)"),
        Err(e) => assert_eq!(e.detail, 18, "detail = {}", e.detail),
    }
}

#[test]
fn reviewed_bugs_0018_0019_0022_0023_0024_0027_0028() {
    let detail = |src: &str| match LangRuntime::compile(src) {
        Ok(_) => 0,
        Err(e) => e.detail,
    };
    // #19 schedule compiles (void opcode).
    assert_eq!(
        detail(
            "world { gravity=(0,0,0) entity e { state=(x=0.0) } } systems { update { on=e; dt=1.0 \
             let _ = schedule(at(1.0), 0.5, 7.0, 1.0)  x = x + 0.0 } }"
        ),
        0
    );
    // #22 user-func arity.
    assert_eq!(
        detail(
            "world { gravity=(0,0,0) entity e { state=(x=0.0) } } funcs { f(a,b) { a+b } } \
             systems { update { on=e; dt=1.0 x = f(1.0, 2.0, 3.0) } }"
        ),
        59
    );
    // #23 rk4 plain assignment rejected.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0) } } systems { rk4 { on=e; dt=0.1 s0 = 1.0 } }"),
        93
    );
    // #24 out-of-range `sN` assignment.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0) } } systems { update { on=e; dt=1.0 s99 = 1.0 + 0.0 } }"),
        52
    );
    // #27 loop-body `let` parses and is type-checked.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0) } } systems { update { on=e; dt=1.0 repeat 2 { let n: i64 = 7 / 2 } x = x } }"),
        0
    );
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0) } } systems { update { on=e; dt=1.0 repeat 2 { let n: f64 = 1.0 > 0.0 } x = x } }"),
        89
    );
    // #28 bool alias accepted.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0) } } systems { update { on=e; dt=1.0 let f: bool = x > 0.0  let b: bool = f  x = x } }"),
        0
    );
    // #18 dt-slot collision is a warning, not an error (compiles).
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(dt=0.0, x=0.0) } } systems { update { on=e; dt=1.0 x = x + 1.0 } }"),
        0
    );
}

#[test]
fn send_recv_require_on() {
    let detail = |src: &str| match LangRuntime::compile(src) {
        Ok(_) => 0,
        Err(e) => e.detail,
    };
    // `on` is required: an `on`-less `send`/`recv` would broadcast and the
    // last-writer payload is undefined.
    assert_eq!(
        detail(
            "world { gravity=(0,0,0) chan c { value=0.0 } entity e { state=(v=1.0) } } \
             systems { send { chan = c; value = v } }"
        ),
        48
    );
    assert_eq!(
        detail(
            "world { gravity=(0,0,0) chan c { value=0.0 } entity e { state=(v=1.0) } } \
             systems { recv { chan = c; slot = 0 } }"
        ),
        48
    );
}

#[test]
fn explicit_numeric_casts() {
    // `i64` truncates toward zero; `f64` is identity; `bool` is x != 0.
    let src = r#"
        world { gravity=(0,0,0) entity e { state=(x = 0.0, y = 0.0, b = 0.0) } }
        systems { update { on = e; dt = 1.0
            x = i64( 3.9 ) + 0.0
            y = i64( -3.9 ) + 0.0
            b = bool( 5.0 ) + 0.0
        } }
    "#;
    let mut rt = LangRuntime::compile(src).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert_eq!(st.values[0], 3.0, "i64(3.9) = 3");
    assert_eq!(st.values[1], -3.0, "i64(-3.9) = -3 (toward zero)");
    assert_eq!(st.values[2], 1.0, "bool(5.0) = 1");
}

#[test]
fn reviewed_bugs_0020_0022_0026_0028_followup() {
    let detail = |src: &str| match LangRuntime::compile(src) {
        Ok(_) => 0,
        Err(e) => e.detail,
    };
    let sys = |body: &str| {
        format!("world {{ gravity=(0,0,0) entity e {{ state=(x=1.0) }} }} systems {{ update {{ on=e; dt=0.01 {body} }} }}")
    };
    // #20: 1-arg math builtins are arity-checked (detail 59), not deferred to EIR.
    assert_eq!(detail(&sys("x = sin(1.0, 2.0)")), 59, "sin arity");
    assert_eq!(detail(&sys("x = min(1.0)")), 59, "min arity");
    // #20: an unknown function name is a clear error, not EIR detail 6.
    assert_eq!(detail(&sys("x = tan(1.0)")), 59, "tan unknown");
    // #22: user `funcs` calls are arity-checked.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0) } } funcs { f(a) { a*2.0 } } systems { update { on=e; dt=0.01 x = f(1.0, 2.0) } }"),
        59
    );
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0) } } funcs { f(a,b) { a*b } } systems { update { on=e; dt=0.01 x = f(5.0) } }"),
        59
    );
    // #26: `watch { into = flag }` is reported as wrong-kind, not "missing".
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0) } } systems { watch { on=e; expr=1.0; mem=0; into=flag } }"),
        48
    );
    // #26: `send` without `chan` is a missing-parameter error (detail 48).
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0) } } systems { send { on=e; value=1.0 } }"),
        48
    );
    // #28: a bool-typed binding may be aliased by a `bool` let.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0, y=0.0) } } systems { update { on=e; dt=1.0 let a: bool = x > 0.0  let b: bool = a  y = b } }"),
        0
    );
    // #28: a bool used as a number is rejected (detail 89).
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0, y=0.0) } } systems { update { on=e; dt=1.0 let b: bool = x > 0.0  let c: f64 = b + 1.0  y = c } }"),
        89
    );
    // #25: a color literal must be exactly `0xRRGGBB` (detail 64).
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0); color=0x12345 } } systems { update { on=e; dt=1.0 x = x } }"),
        64
    );
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=0.0); color=0xFF6B4ACC } } systems { update { on=e; dt=1.0 x = x } }"),
        64
    );
    // #28: an unknown type annotation is detail 89, not a grammar error.
    assert_eq!(
        detail("world { gravity=(0,0,0) entity e { state=(x=1.0, y=0.0) } } systems { update { on=e; dt=1.0 let a: boolean = x > 0.0  y = a } }"),
        89
    );
}

#[test]
fn nbody_orient_slot_warning() {
    // #6: an nbody body exposing slot 7 without `orient = true` is advisory
    // (detail 95): slot 7 is read as a Z-spin, not a euler angle.
    clear_diagnostics();
    let src = "world { gravity=(0,0,0) entity b { state=(px=0.,py=0.,pz=0.,vx=0.,vy=0.,vz=0.,m=1.,s7=0.) } }                systems { nbody { G=1.0; dt=0.1 } }";
    LangRuntime::compile(src).unwrap();
    let ds = take_diagnostics();
    assert!(
        ds.iter().any(|d| d.detail == 95),
        "expected detail 95 warning, got {ds:?}"
    );
    clear_diagnostics();
}

#[test]
fn optimizer_fuses_fma_and_preserves_semantics() {
    // The optimizing tier must emit `Fma` for `a*b + c` chains and produce
    // byte-identical writes to the generic interpreter (verified by running the
    // optimized and generic modules on the same scene).
    let mut src = String::from("world { gravity=(0,0,0)\n");
    for i in 0..8 {
        let a = i as f64 * 0.37;
        src += &format!(
            "  entity b{i} {{ state = ({:.4}, {:.4}, 0, 0, 0, 0, 1.0) }}\n",
            a.cos() * 5.0,
            a.sin() * 5.0
        );
    }
    src += "}\nsystems { nbody { G = 0.001; dt = 0.001 } }\n";
    let mut rt = LangRuntime::compile(&src).unwrap();
    let fma: usize = rt
        .optimized
        .functions
        .iter()
        .map(|f| {
            f.instructions
                .iter()
                .filter(|i| i.opcode.name() == "Fma")
                .count()
        })
        .sum();
    assert!(fma > 0, "optimizer must emit Fma for the nbody kernel");
    // Optimized output validates (type/dominance preserved by the passes).
    rt.optimized.validate(true).unwrap();
    rt.optimized.verify_linear_dominance().unwrap();
    // Byte-identical across the optimizing and generic backends.
    rt.step_cross().unwrap();
    let expect = rt.present_frame(None);
    rt.step_cross().unwrap();
    let got = rt.present_frame(None);
    assert_eq!(expect.entities.len(), got.entities.len());
}

#[test]
fn orient_slot_write_warning() {
    let warns = |src: &str| -> Vec<u32> {
        clear_diagnostics();
        let _ = LangRuntime::compile(src).unwrap();
        take_diagnostics()
            .iter()
            .map(|d| d.detail)
            .filter(|d| *d == 96)
            .collect()
    };
    // #6: a rule writing slot 7 without `orient = true` warns (detail 96).
    assert_eq!(
        warns("world { entity e { state=(x=1.0,y=0.,z=0.,a=0.,b=0.,c=0.,d=0.,s7=0.) } }                systems { update { on=e; dt=1.0  s7 = 0.5 } }"),
        vec![96]
    );
    // With `orient = true` (and slots 7/8/9 present) no warning.
    assert!(
        warns("world { entity e { orient = true; state=(x=1.,y=0.,z=0.,a=0.,b=0.,c=0.,d=0.,rx=0.,ry=0.,rz=0.) } }                systems { update { on=e; dt=1.0  ry = 0.5 } }")
            .is_empty()
    );
    // Writing only slots < 7 never warns.
    assert!(
        warns("world { entity e { state=(x=1.0,y=0.,z=0.,a=0.,b=0.,c=0.,d=0.,s7=0.) } }                systems { update { on=e; dt=1.0  x = 2.0 } }")
            .is_empty()
    );
    // `orient = true` with too few slots warns.
    assert_eq!(
        warns("world { entity e { orient = true; state=(x=1.0,y=0.,z=0.) } }                systems { update { on=e; dt=1.0  x = 2.0 } }"),
        vec![96]
    );
    clear_diagnostics();
}

#[test]
fn native_jit_promotes_and_matches_interpreter() {
    // A fully-native-eligible kernel (nbody) run past the promotion threshold:
    // `step_cross` compares the interpreter against the JIT each step, so once
    // the native backend engages it is differentially verified byte-for-byte.
    let mut src = String::from("world { gravity=(0,0,0)\n");
    for i in 0..4 {
        let a = i as f64 * 0.37;
        src += &format!(
            "  entity b{i} {{ state = ({:.4}, {:.4}, 0, 0, 0, 0, 1.0) }}\n",
            a.cos() * 5.0,
            a.sin() * 5.0
        );
    }
    src += "}\nsystems { nbody { G = 0.001; dt = 0.001 } }\n";
    let mut rt = LangRuntime::compile(&src).unwrap();
    rt.enable_native_jit(true);
    rt.step_cross_n(crate::jit::NATIVE_PROMOTE + 32).unwrap();
    // If a C compiler is available, promotion must have run native code.
    if std::process::Command::new("cc")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        assert!(
            rt.native_executions() > 0,
            "native JIT must promote and run on a hot, eligible kernel"
        );
    }
}
