//! Compile-stack demo: PWE source → EIR → interpreter, CPU JIT, and AOT all
//! produce byte-identical world writes, and the RFC-0021 dominance + RFC-0022
//! fence contracts are exercised along the way.
//!
//! Run: `cargo run --example compile_stack_demo`

use pwe_api::{WorldId, WorldVersion};
use pwe_reference::aot::AotProgram;
use pwe_reference::dominance::{Block, Terminator};
use pwe_reference::eir::{EirModule, Opcode};
use pwe_reference::fence::{CpuGpuHandoff, Direction, Fence, MemoryOrder};
use pwe_reference::lang;
use pwe_reference::lang::LangRuntime;

const SOURCE: &str = r#"
    world {
        gravity = (0, -9.81, 0)
        entity vehicle { position = (0, 8, 0); velocity = (4, 0, 0); mass = 4; dynamic = true; box = (1, 0.5, 0.7) }
        entity ground { position = (0, -5, 0); dynamic = false; box = (50, 5, 50) }
    }
    systems {
        gravity { gravity_y = -9.81; dt = 1 / 60 }
        integrate { dt = 1 / 60 }
        ground_contact { restitution = 0.6 }
    }
"#;

fn main() {
    // 1. Parse + compile to EIR.
    let compiled = lang::compile(SOURCE).expect("compile PWE source");
    println!("== compile stack: source → EIR ==");
    println!(
        "  entities: {}, EIR functions: {}",
        compiled.program.entities.len(),
        compiled.eir.functions.len()
    );

    // 2. Dominance verification over the compiled single-block functions.
    println!("\n== RFC-0021 dominance + block-target verification ==");
    for f in &compiled.eir.functions {
        let block = Block::new(0, 0, f.instructions.len(), Terminator::Return);
        let ok = pwe_reference::dominance::verify_dominance(&f.instructions, &[block]);
        println!(
            "  function {}: {} instructions, dominance {}",
            f.id,
            f.instructions.len(),
            if ok.is_ok() { "valid" } else { "INVALID" }
        );
    }
    // A deliberately-broken use-before-def must be rejected.
    let broken = broken_module(&compiled.eir);
    println!(
        "  broken (use-before-def) rejected: {}",
        pwe_reference::dominance::verify_dominance(
            &broken,
            &[Block::new(0, 0, broken.len(), Terminator::Return)]
        )
        .is_err()
    );

    // 3. Interpreter == JIT == AOT on the same EIR.
    println!("\n== interpreter == JIT == AOT agreement ==");
    let mut rt = LangRuntime::compile(SOURCE).expect("boot runtime");
    // step_cross runs BOTH backends on the same tick and asserts byte-identity.
    let cross = rt.step_cross().expect("cross step");
    println!(
        "  interpreter == JIT (same tick, enforced): {} writes",
        cross.len()
    );

    // AOT shares the interpreter's semantics over the same EIR and no-op
    // runtime: a fresh compile yields byte-identical writes to EirModule::interpret.
    let aot = AotProgram::compile(&compiled.eir, 0).expect("aot compile");
    let aot_writes = aot
        .execute(WorldId(1), WorldVersion(0))
        .expect("aot execute");
    let base_writes = compiled
        .eir
        .interpret(WorldId(1), WorldVersion(0))
        .expect("base interpret");
    println!(
        "  AOT == interpreter (same EIR): {}",
        aot_writes == base_writes
    );

    // 4. AOT artifact codec (RFC-0035 self-authenticating).
    let bytes = aot.encode().expect("aot encode");
    let loaded = AotProgram::decode(&bytes).expect("aot decode");
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    println!("\n== RFC-0035 AOT artifact codec ==");
    println!("  artifact bytes: {}", bytes.len());
    println!(
        "  loaded.hash == original.hash: {}",
        loaded.artifact_hash == aot.artifact_hash
    );
    println!(
        "  tampered artifact rejected   : {}",
        AotProgram::decode(&tampered).is_err()
    );

    // 5. RFC-0022 explicit fence for a CPU→GPU handoff.
    println!("\n== RFC-0022 CPU/GPU fence ==");
    let good = CpuGpuHandoff {
        resource: 7,
        fence: Fence {
            order: MemoryOrder::Release,
            tag: 42,
        },
        ownership_transfer: 42,
    };
    println!(
        "  release-fence + transfer validated: {}",
        good.validate(Direction::CpuToGpu).is_ok()
    );
    let weak = CpuGpuHandoff {
        resource: 7,
        fence: Fence {
            order: MemoryOrder::Relaxed,
            tag: 42,
        },
        ownership_transfer: 42,
    };
    println!(
        "  relaxed (too weak) rejected       : {}",
        weak.validate(Direction::CpuToGpu).is_err()
    );
}

/// A deliberately invalid instruction list: %2 used before %1 is defined.
fn broken_module(_m: &EirModule) -> Vec<pwe_reference::eir::Instruction> {
    vec![
        pwe_reference::eir::Instruction {
            opcode: Opcode::Add,
            result_id: 2,
            result_type: Some(pwe_reference::eir::ValueType::U64),
            operands: vec![1, 1],
            constant: None,
            target: None,
        },
        pwe_reference::eir::Instruction {
            opcode: Opcode::Const,
            result_id: 1,
            result_type: Some(pwe_reference::eir::ValueType::U64),
            operands: vec![],
            constant: Some(pwe_reference::eir::Immediate::U64(5)),
            target: None,
        },
        pwe_reference::eir::Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        },
    ]
}
