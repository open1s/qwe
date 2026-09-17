//! Chemistry demo: a reversible bimolecular reaction following mass-action
//! kinetics, in the PWE language.
//!
//!   A + B  ⇌  C       rate = k_f·[A]·[B] − k_r·[C]
//!
//! The law of mass action says the forward rate is proportional to the product
//! of the reactant concentrations. Each species is a state slot on one `reactor`
//! entity; the system integrates the coupled ODEs, and the mixture relaxes to
//! chemical equilibrium where the net rate vanishes.
//!
//! Run: `cargo run --example chemistry_reaction_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        entity reactor { state = (1.0, 1.0, 0.0) }   # [A], [B], [C]
    }

    systems {
        # Mass-action kinetics:  dA/dt = -k_f·A·B + k_r·C, etc.
        # k_f = 1.0, k_r = 0.5  ->  equilibrium K_eq = k_f/k_r = 2.
        update { dt = 0.01
            s0 = -1.0 * s0 * s1 + 0.5 * s2     # d[A]/dt
            s1 = -1.0 * s0 * s1 + 0.5 * s2     # d[B]/dt
            s2 =  1.0 * s0 * s1 - 0.5 * s2     # d[C]/dt
        }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    println!("== reversible bimolecular reaction  A + B ⇌ C ==");
    println!("   k_f = 1.0, k_r = 0.5 (K_eq = 2); initial [A]=[B]=1, [C]=0");

    let mut last = [0.0; 3];
    for step in 0..2000 {
        rt.step_cross()?; // interpreter == JIT enforced each step
        if step % 50 == 0 {
            let st = rt
                .scene
                .get(EntityId(1))
                .and_then(|e| e.state.as_ref())
                .unwrap();
            let (a, b, c) = (st.values[0], st.values[1], st.values[2]);
            last = [a, b, c];
            println!("   t={step:>4}  [A]={a:.4}  [B]={b:.4}  [C]={c:.4}");
        }
    }
    let (a, b, c) = (last[0], last[1], last[2]);
    println!("\n== chemical equilibrium ==");
    // Conservation: total A-units A + C and B-units B + C are invariant.
    println!(
        "   mass conservation: A + C = {:.6} (init 1), B + C = {:.6} (init 1)",
        a + c,
        b + c
    );
    // Equilibrium condition:  [C]/([A][B]) = K_eq = k_f/k_r = 2.
    let keq = c / (a * b);
    println!("   equilibrium constant [C]/([A][B]) = {keq:.4}  (k_f/k_r = 2.0)");
    Ok(())
}
