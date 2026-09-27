//! **User-defined functions in the PWE language** — reusable, named
//! computations lowered to EIR `CALL` functions, run **cross-backend**
//! (interpreter == JIT, enforced every step). This makes the language a general
//! modeling substrate: define a helper once (`smoothstep`, `clamp`, `logistic`)
//! and call it from any `update` rule.
//!
//! Run: `cargo run --example functions_demo`

use pwe_reference::lang::LangRuntime;

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        entity x { state = (0.2, 0, 0) }
    }

    funcs {
        # Reusable pure functions with ordinary named parameters.
        clamp(v, hi)     { if(v < 0, 0, if(v > hi, hi, v)) }
        smoothstep(t)    { t * t * (3 - 2 * t) }
        logistic(p)      { p * (1 - p) }            # r = 1

        # Recursive functions use control-flow `if … { return … }` so each call
        # pushes a frame on the interpreter's call stack (the eager
        # `if(c,a,b)` expression would evaluate the recursive branch forever).
        fact(n) { if n < 1.0 { return 1.0 } else { return n * fact(n - 1.0) } }
    }

    systems {
        # dP/dt = clamp( smoothstep(P) + logistic(P), 0.08 ) — a saturating,
        # noise-free population update whose step is bounded by a helper.
        update { dt = 0.02
            s0 = s0 + inte(  clamp(smoothstep(s0) + logistic(s0), 0.08) )
            s1 = s1 + inte(  s0 )
            s2 = fact(5.0)              # recursion: 5! = 120
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    let mut trace = String::new();
    let mut final_p = 0.0f64;
    for i in 0..800 {
        rt.step_cross()?; // interpreter == JIT on CALLs, every step
        final_p = rt
            .scene
            .get(pwe_api::EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        if i % 8 == 0 {
            trace.push(match final_p {
                p if p < 0.15 => '.',
                p if p < 0.3 => '-',
                p if p < 0.5 => '*',
                _ => 'o',
            });
        }
    }
    println!("functions_demo: user-defined functions (clamp/smoothstep/logistic) via EIR CALL");
    println!("P(t): {trace}");
    println!("final P = {final_p:.4}");
    assert!(final_p.is_finite() && final_p > 0.0, "P = {final_p}");

    // Reproducibility: a fresh runtime reproduces the exact trajectory.
    let mut rt2 = LangRuntime::compile(SOURCE)?;
    for _ in 0..800 {
        rt2.step_cross()?;
    }
    let replay = rt2
        .scene
        .get(pwe_api::EntityId(1))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[0];
    println!("replay P = {replay:.4}   (must match)");
    assert_eq!(replay, final_p);
    Ok(())
}
