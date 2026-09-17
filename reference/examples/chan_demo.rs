//! Go-style channels in the PWE language: a `chan` system sends a value to a
//! channel entity and receives it back — plain cross-entity EIR reads/writes, so
//! the interpreter and JIT agree byte-for-byte.
//!
//! Run: `cargo run --example chan_demo`

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)

        chan wire { value = 0 }             # a channel (mailbox entity)
        entity probe { state = (10, 0) }    # publishes state[0], reads into state[1]
    }
    systems {
        send { chan = wire; value = s0 }
        recv { chan = wire; slot = 1 }
    }
"#;

fn main() -> pwe_api::Result<()> {
    let mut rt = LangRuntime::compile(SOURCE)?;
    println!("== Go-style channels in the PWE language (`chan` system) ==");
    for step in 0..4 {
        rt.step_cross()?; // interpreter == JIT enforced each step
        let p = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let w = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        println!(
            "  step {step}  probe[0]={} -> wire={} -> probe[1]={}",
            p.values[0], w.values[0], p.values[1]
        );
    }
    Ok(())
}
