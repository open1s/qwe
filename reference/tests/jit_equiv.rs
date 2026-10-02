//! Native-JIT equivalence under hotness **promotion** (#91).
//!
//! The interpreter is the semantic oracle and the JIT must agree with it
//! byte-for-byte (AGENTS §4). The unoptimized JIT always did; the *promoted*
//! native backend did not, on one path: `robot.pwe`'s forward kinematics call a
//! `sin`/`cos` pair (in `std/robotics`), and `-O2` compiled those to clang's
//! `__sincos_stret`, whose sine is up to 1 ULP off from libm's `sin` — the
//! function the interpreter calls. The backend now passes `-fno-builtin`
//! (alongside `-ffp-contract=off` for `Mul;Add` two-rounding), so both agree.
//!
//! This steps the demo under promotion with an **every-step** cross — stricter
//! than `run`/`present`'s 16-step batches — well past `NATIVE_PROMOTE` and past
//! the step (79 with per-step checking) where the divergence surfaced. Without
//! it, the flag can be dropped again and only a `pwe present` run would notice.

use pwe_reference::jit::NATIVE_PROMOTE;
use pwe_reference::lang::LangRuntime;

#[test]
fn robot_forward_kinematics_matches_the_interpreter_under_promotion() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/examples/robot.pwe");
    let mut rt = LangRuntime::compile_file(&path).expect("compile robot.pwe");
    rt.enable_native_jit(true);

    // 25x the promotion threshold — far past step 1376, the first *checked*
    // divergent step of the batched `present` phase.
    let steps = NATIVE_PROMOTE * 25;
    for step in 0..steps {
        rt.step_cross().unwrap_or_else(|e| {
            panic!(
                "native JIT diverged from the interpreter at step {step} (detail {}): \
                 promoted codegen must round like the interpreter",
                e.detail
            )
        });
    }
    assert!(
        rt.native_executions() > 0,
        "the native backend must actually have run — otherwise this proves nothing"
    );
}
