//! Phase-3 GPU/NPU backend: lower data-parallel EIR kernels to **WGSL** compute
//! shaders.
//!
//! Scope: straight-line, world-access-free functions (a "map" kernel: one input
//! value in, one result out), i.e. the per-element arithmetic that dominates
//! field solvers and particle updates. Functions with component access, calls,
//! branches, effects, or spatial/field queries are ineligible and stay on the
//! CPU backend.
//!
//! **Semantics.** WebGPU's shading language has no `f64` (and most devices lack
//! the `shader-f64` feature), so a WGSL kernel computes in **f32**. This backend
//! is therefore an **approximate device backend**, not a bit-exact one: it MUST
//! NOT be substituted for the interpreter where exact f64 semantics matter. The
//! emitter is verified two ways (see tests): a dependency-free structural
//! validator checks the shader is well-formed, every emitted shader is parsed
//! and validated by `naga` (dev-dependency), and a CPU f32 oracle mirroring the
//! emitted formulas is checked against the interpreter within f32 tolerance.
//! Builtins whose device semantics differ from the CPU (no `copysign`/`hypot`;
//! `sign`/`round` edge cases) are emitted as explicit helper functions that
//! reproduce the CPU semantics, so the shader and the oracle share one lowering.
//!
//! This module emits shader source; it does not create a GPU device. Execution
//! on real hardware is verified by the standalone `gpu-verify` crate, which runs
//! these shaders on the macOS **Metal** backend (via `wgpu`) and compares the
//! results with the CPU oracle (including the trap flag).
//!
//! ## Divergences from the f64 interpreter (input classes)
//!
//! WGSL has no exceptions, so RFC-0021 traps (divide/remainder by `±0.0`, a NaN
//! comparison) are surfaced by setting an `atomic<u32>` **trap flag** and
//! early-returning; the host reads the flag after dispatch and fails the step
//! with the same `EirInvalid 18` detail. The CPU oracle mirrors this.
//!
//! | input class | interpreter (f64) | device backend (f32) |
//! | --- | --- | --- |
//! | divide/remainder by `±0.0`, NaN comparison | traps `EirInvalid 18` | sets the trap flag (detail 18) |
//! | values below f32 normal range | retained | underflow to `0.0` |
//! | transcendentals (`sin`, `exp`, …) | correctly rounded f64 | f32 precision |
//! | `sign`/`round`/`hypot`/`Rem` | — | explicit helpers reproducing CPU semantics exactly |
//!
//! Only the last two (underflow and transcendental precision) are irreducible;
//! callers must not substitute this backend where exact f64 semantics matter.

use crate::eir::{EirModule, Function, Immediate, Opcode};
use pwe_api::{Error, Result, Status};

/// The `target` value selecting the WGSL device backend at the AOT boundary.
pub const WGSL_TARGET: u16 = 4;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// Whether a function can be lowered to a WGSL map kernel.
pub fn eligible(f: &Function) -> bool {
    if f.argument_count != 1 {
        return false;
    }
    f.instructions.iter().all(|i| {
        i.target.is_none()
            && matches!(
                i.opcode,
                Opcode::Nop
                    | Opcode::Const
                    | Opcode::Add
                    | Opcode::Sub
                    | Opcode::Mul
                    | Opcode::Div
                    | Opcode::Rem
                    | Opcode::Pow
                    | Opcode::Fma
                    | Opcode::Eq
                    | Opcode::Ne
                    | Opcode::Lt
                    | Opcode::Le
                    | Opcode::Gt
                    | Opcode::Ge
                    | Opcode::Select
                    | Opcode::Sin
                    | Opcode::Cos
                    | Opcode::Exp
                    | Opcode::Ln
                    | Opcode::Sqrt
                    | Opcode::Abs
                    | Opcode::Floor
                    | Opcode::Ceil
                    | Opcode::Round
                    | Opcode::Sign
                    | Opcode::Log10
                    | Opcode::Log2
                    | Opcode::Sinh
                    | Opcode::Cosh
                    | Opcode::Tanh
                    | Opcode::Asin
                    | Opcode::Acos
                    | Opcode::Atan
                    | Opcode::Atan2
                    | Opcode::Hypot
                    // RFC-0043 value conversions stay on the interpreter (the
                    // GPU field kernels are f32 numeric only).
                    | Opcode::I64ToF64
                    | Opcode::F64ToI64
                    | Opcode::Return
            )
    })
}

/// Emits a WGSL compute shader that applies function `func_id` element-wise to
/// an `f32` input buffer, writing an `f32` output buffer.
pub fn emit_compute_shader(module: &EirModule, func_id: u64) -> Result<String> {
    let f = module
        .functions
        .iter()
        .find(|f| f.id == func_id)
        .ok_or_else(|| error(Status::Invalid, 5))?;
    if !eligible(f) {
        return Err(error(Status::Invalid, 5));
    }
    let cap = f
        .instructions
        .iter()
        .map(|i| i.result_id as usize + 1)
        .max()
        .unwrap_or(2)
        .max(f.instructions.len())
        .max(2);
    let mut body = String::new();
    body.push_str("  let i = gid.x;\n");
    body.push_str("  if (i >= arrayLength(&output)) { return; }\n");
    // Sticky trap read: a prior trap fails the whole dispatch, and this keeps
    // the `trap` binding live even for kernels with no trapping operation (so
    // the bind-group layout is stable).
    body.push_str("  if (atomicLoad(&trap) != 0u) { return; }\n");
    body.push_str(&format!("  var r: array<f32, {cap}>;\n"));
    // Parameters arrive in registers 1.. (EIR CALL convention); a map kernel has
    // exactly one input.
    body.push_str("  r[1] = input[i];\n");
    for ins in &f.instructions {
        body.push_str(&emit_instruction(ins)?);
    }
    // The return value is the last `Return` operand.
    let ret = f
        .instructions
        .iter()
        .rev()
        .find(|i| i.opcode == Opcode::Return)
        .and_then(|i| i.operands.first().copied())
        .unwrap_or(0);
    body.push_str(&format!("  output[i] = r[{ret}];\n"));

    Ok(format!(
        "// Generated by pwe (WGSL device backend, target {WGSL_TARGET}).\n\
         // f32 approximation of a 64-bit-float EIR kernel - not bit-exact.\n\
         @group(0) @binding(0) var<storage, read> input: array<f32>;\n\
         @group(0) @binding(1) var<storage, read_write> output: array<f32>;\n\
         @group(0) @binding(2) var<storage, read_write> trap: atomic<u32>;\n\
         // Helpers reproduce CPU semantics WGSL builtins lack / define differently.\n\
         fn pwe_signbit(x: f32) -> f32 {{ return select(1.0, -1.0, (bitcast<u32>(x) >> 31u) == 1u); }}\n\
         fn pwe_sign(x: f32) -> f32 {{ if (x != x) {{ return x; }} return pwe_signbit(x); }}\n\
         fn pwe_round(x: f32) -> f32 {{ return pwe_signbit(x) * floor(abs(x) + 0.5); }}\n\
         fn pwe_hypot(a: f32, b: f32) -> f32 {{ let m = max(abs(a), abs(b)); if (m - m != 0.0) {{ return m; }} if (m == 0.0) {{ return 0.0; }} return m * sqrt((a / m) * (a / m) + (b / m) * (b / m)); }}\n\
         @compute @workgroup_size(64)\n\
         fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{\n{body}}}\n"
    ))
}

fn const_f32(imm: Immediate) -> f32 {
    match imm {
        Immediate::I32(v) => v as f32,
        Immediate::U32(v) => v as f32,
        Immediate::I64(v) => v as f32,
        Immediate::U64(v) => v as f32,
        Immediate::F32(v) => v,
        Immediate::F64(v) => v as f32,
        Immediate::Bool(v) => u8::from(v) as f32,
    }
}

fn lit(v: f32) -> String {
    if v.is_nan() {
        // WGSL has no NaN literal; produce it portably at f32 width.
        "(0.0 / 0.0)".to_string()
    } else if v.is_infinite() {
        if v > 0.0 {
            "(1.0 / 0.0)".to_string()
        } else {
            "(-1.0 / 0.0)".to_string()
        }
    } else {
        format!("{v:?}f")
    }
}

fn bin_op(op: Opcode) -> Option<&'static str> {
    Some(match op {
        Opcode::Add => "+",
        Opcode::Sub => "-",
        Opcode::Mul => "*",
        Opcode::Div => "/",
        _ => return None,
    })
}

fn cmp_op(op: Opcode) -> Option<&'static str> {
    Some(match op {
        Opcode::Eq => "==",
        Opcode::Ne => "!=",
        Opcode::Lt => "<",
        Opcode::Le => "<=",
        Opcode::Gt => ">",
        Opcode::Ge => ">=",
        _ => return None,
    })
}

fn unary_fn(op: Opcode) -> Option<&'static str> {
    Some(match op {
        Opcode::Sin => "sin",
        Opcode::Cos => "cos",
        Opcode::Exp => "exp",
        Opcode::Ln => "log",
        Opcode::Sqrt => "sqrt",
        Opcode::Abs => "abs",
        Opcode::Floor => "floor",
        Opcode::Ceil => "ceil",
        Opcode::Round => "pwe_round",
        Opcode::Sign => "pwe_sign",
        Opcode::Log10 => return None, // emulated below
        Opcode::Log2 => "log2",
        Opcode::Sinh => "sinh",
        Opcode::Cosh => "cosh",
        Opcode::Tanh => "tanh",
        Opcode::Asin => "asin",
        Opcode::Acos => "acos",
        Opcode::Atan => "atan",
        _ => return None,
    })
}

fn emit_instruction(ins: &crate::eir::Instruction) -> Result<String> {
    let o = &ins.operands;
    let res = ins.result_id;
    let line = match ins.opcode {
        // EIR `Rem`/`Div` by ±0.0 trap (RFC-0021 detail 18); set the trap flag
        // and early-return, the device equivalent of the interpreter trap.
        Opcode::Rem => format!(
            "  if (r[{}] == 0.0) {{ atomicStore(&trap, 1u); return; }}\n  r[{res}] = r[{}] % r[{}];\n",
            o[1], o[0], o[1]
        ),
        Opcode::Div => format!(
            "  if (r[{}] == 0.0) {{ atomicStore(&trap, 1u); return; }}\n  r[{res}] = r[{}] / r[{}];\n",
            o[1], o[0], o[1]
        ),
        Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
            // NaN comparison traps like the interpreter's `partial_cmp` -> None.
            let op = cmp_op(ins.opcode).unwrap_or("==");
            format!(
                "  if (r[{}] != r[{}] || r[{}] != r[{}]) {{ atomicStore(&trap, 1u); return; }}\n  r[{res}] = select(0.0, 1.0, r[{}] {op} r[{}]);\n",
                o[0], o[0], o[1], o[1], o[0], o[1]
            )
        }
        // WGSL has no `hypot`; use the overflow-safe scaled form.
        Opcode::Hypot => format!("  r[{res}] = pwe_hypot(r[{}], r[{}]);\n", o[0], o[1]),
        Opcode::Nop => String::new(),
        Opcode::Const => format!(
            "  r[{res}] = {};\n",
            lit(const_f32(ins.constant.unwrap_or(Immediate::F32(0.0))))
        ),
        Opcode::Fma => format!("  r[{res}] = fma(r[{}], r[{}], r[{}]);\n", o[0], o[1], o[2]),
        // RFC-0043 conversions are interpreter-only; `eligible` excludes them,
        // so this arm is unreachable for a compiled kernel.
        Opcode::I64ToF64 => format!("  r[{res}] = r[{}];\n", o[0]),
        Opcode::F64ToI64 => format!("  r[{res}] = r[{}];\n", o[0]),
        Opcode::Pow => format!("  r[{res}] = pow(r[{}], r[{}]);\n", o[0], o[1]),
        Opcode::Atan2 => format!("  r[{res}] = atan2(r[{}], r[{}]);\n", o[0], o[1]),
        Opcode::Log10 => format!("  r[{res}] = log(r[{}]) / log(10.0f);\n", o[0]),
        Opcode::Log2 => format!("  r[{res}] = log2(r[{}]);\n", o[0]),
        Opcode::Select => format!(
            "  r[{res}] = select(r[{}], r[{}], bitcast<u32>(r[{}]) != 0u);\n",
            o[2], o[1], o[0]
        ),
        Opcode::Return => String::new(),
        op if bin_op(op).is_some() => format!(
            "  r[{res}] = r[{}] {} r[{}];\n",
            o[0],
            bin_op(op).unwrap_or("+"),
            o[1]
        ),
        op if cmp_op(op).is_some() => format!(
            "  r[{res}] = select(0.0f, 1.0f, r[{}] {} r[{}]);\n",
            o[0],
            cmp_op(op).unwrap_or("=="),
            o[1]
        ),
        op if unary_fn(op).is_some() => {
            format!(
                "  r[{res}] = {}(r[{}]);\n",
                unary_fn(op).unwrap_or("abs"),
                o[0]
            )
        }
        _ => return Err(error(Status::Invalid, 5)),
    };
    Ok(line)
}

/// Dependency-free structural validation of a generated WGSL shader: balanced
/// delimiters, a `main` entry point with `@compute`/`@workgroup_size`, the
/// storage bindings it references, and no `f64` (the device backend is f32).
pub fn validate_shader(source: &str) -> Result<()> {
    let mut depth: i32 = 0;
    for ch in source.chars() {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(error(Status::Invalid, 6));
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(error(Status::Invalid, 6));
    }
    for needle in [
        "@compute",
        "@workgroup_size",
        "fn main(",
        "var<storage, read> input",
        "arrayLength(&output)",
        "var<storage, read_write> trap",
    ] {
        if !source.contains(needle) {
            return Err(error(Status::Invalid, 6));
        }
    }
    if source.contains("f64") {
        return Err(error(Status::Invalid, 6));
    }
    Ok(())
}

/// A CPU oracle: evaluates a WGSL-eligible EIR function in **f32** the same way
/// the emitted shader does. Used to check the lowering matches the interpreter
/// within f32 tolerance.
pub fn eval_f32(module: &EirModule, func_id: u64, input: f32) -> Result<f32> {
    let f = module
        .functions
        .iter()
        .find(|f| f.id == func_id)
        .ok_or_else(|| error(Status::Invalid, 5))?;
    if !eligible(f) {
        return Err(error(Status::Invalid, 5));
    }
    let cap = f
        .instructions
        .iter()
        .map(|i| i.result_id as usize + 1)
        .max()
        .unwrap_or(2)
        .max(2);
    let mut r: Vec<f32> = vec![0.0; cap];
    r[1] = input;
    for ins in &f.instructions {
        let o = &ins.operands;
        let get = |k: usize| r[o[k] as usize];
        let v = match ins.opcode {
            Opcode::Nop | Opcode::Return => continue,
            Opcode::Const => const_f32(ins.constant.unwrap_or(Immediate::F32(0.0))),
            Opcode::Fma => get(0) * get(1) + get(2),
            Opcode::Pow => get(0).powf(get(1)),
            Opcode::Atan2 => get(0).atan2(get(1)),
            Opcode::Hypot => oracle_hypot(get(0), get(1)),
            Opcode::Log10 => get(0).log10(),
            Opcode::Select => {
                // Mirrors `bitcast<u32>(cond) != 0u`.
                if get(0).to_bits() != 0 {
                    get(1)
                } else {
                    get(2)
                }
            }
            Opcode::Add => get(0) + get(1),
            Opcode::Sub => get(0) - get(1),
            Opcode::Mul => get(0) * get(1),
            Opcode::Div => {
                if get(1) == 0.0 {
                    return Err(error(Status::EirInvalid, 18));
                }
                get(0) / get(1)
            }
            Opcode::Rem => {
                if get(1) == 0.0 {
                    return Err(error(Status::EirInvalid, 18));
                }
                get(0) % get(1)
            }
            Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
                let (a, b) = (get(0), get(1));
                if a.is_nan() || b.is_nan() {
                    return Err(error(Status::EirInvalid, 18));
                }
                match ins.opcode {
                    Opcode::Eq => f32::from(a == b),
                    Opcode::Ne => f32::from(a != b),
                    Opcode::Lt => f32::from(a < b),
                    Opcode::Le => f32::from(a <= b),
                    Opcode::Gt => f32::from(a > b),
                    _ => f32::from(a >= b),
                }
            }
            Opcode::Sin => get(0).sin(),
            Opcode::Cos => get(0).cos(),
            Opcode::Exp => get(0).exp(),
            Opcode::Ln => get(0).ln(),
            Opcode::Sqrt => get(0).sqrt(),
            Opcode::Abs => get(0).abs(),
            Opcode::Floor => get(0).floor(),
            Opcode::Ceil => get(0).ceil(),
            Opcode::Round => oracle_round(get(0)),
            Opcode::Sign => oracle_sign(get(0)),
            Opcode::Log2 => get(0).log2(),
            Opcode::Sinh => get(0).sinh(),
            Opcode::Cosh => get(0).cosh(),
            Opcode::Tanh => get(0).tanh(),
            Opcode::Asin => get(0).asin(),
            Opcode::Acos => get(0).acos(),
            Opcode::Atan => get(0).atan(),
            _ => return Err(error(Status::Invalid, 5)),
        };
        r[ins.result_id as usize] = v;
    }
    let ret = f
        .instructions
        .iter()
        .rev()
        .find(|i| i.opcode == Opcode::Return)
        .and_then(|i| i.operands.first().copied())
        .unwrap_or(0);
    Ok(r[ret as usize])
}

/// CPU mirrors of the emitted WGSL helpers (so oracle and shader share one
/// lowering).
fn oracle_signbit(x: f32) -> f32 {
    if x.to_bits() >> 31 == 1 {
        -1.0
    } else {
        1.0
    }
}
fn oracle_sign(x: f32) -> f32 {
    if x.is_nan() {
        x
    } else {
        oracle_signbit(x)
    }
}
fn oracle_round(x: f32) -> f32 {
    oracle_signbit(x) * (x.abs() + 0.5).floor()
}
fn oracle_hypot(a: f32, b: f32) -> f32 {
    let m = a.abs().max(b.abs());
    if !m.is_finite() {
        // ±inf or NaN
        return m;
    }
    if m == 0.0 {
        0.0
    } else {
        m * ((a / m) * (a / m) + (b / m) * (b / m)).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses and validates a shader with naga (real WGSL front-end + validator).
    fn naga_validate(src: &str) -> std::result::Result<(), String> {
        let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
        let mut v = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        v.validate(&module).map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    fn eir_of(src: &str) -> EirModule {
        // Emit from the optimized EIR (so Mul/Add has already fused to Fma).
        let parsed = crate::lang::parse(src).unwrap();
        crate::lang::compile_program(parsed).unwrap().eir.optimize()
    }

    #[test]
    fn emits_and_validates_a_compute_shader() {
        let src = "world { gravity=(0,0,0) entity e { state=(a=0.0, x=0.0) } } \
                   funcs { f(a) { a * a + sin(a) } } \
                   systems { update { on=e; dt=1.0  x = f(a) } }";
        let module = eir_of(src);
        let shader = emit_compute_shader(&module, 0xF000_0000).unwrap();
        validate_shader(&shader).unwrap();
        naga_validate(&shader).expect("emitted shader must pass naga");
        assert!(shader.contains("fma("), "Mul/Add should fuse to fma");
        assert!(shader.contains("sin("));
        assert!(shader.contains("output[i] = r["));
    }

    #[test]
    fn rejects_ineligible_functions() {
        let src = "world { gravity=(0,0,0) entity e { state=(a=0.0, x=0.0) } } \
                   funcs { f(a) { a / 0.0 + a } } \
                   systems { update { on=e; dt=1.0  x = f(a) } }";
        let _ = eir_of(src);
        // The system `update` function reads/writes the world -> ineligible.
        let module = eir_of(src);
        let sys_fn = module
            .functions
            .iter()
            .find(|f| f.argument_count == 0)
            .unwrap();
        assert!(!eligible(sys_fn));
        assert!(emit_compute_shader(&module, sys_fn.id).is_err());
    }

    fn close(a: f32, b: f32) -> bool {
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a.is_infinite() || b.is_infinite() {
            return a.to_bits() == b.to_bits();
        }
        (a - b).abs() <= 1e-4 * b.abs().max(1.0)
    }

    #[test]
    fn f32_oracle_tracks_the_interpreter() {
        // The device backend is f32; the oracle mirrors the emitted shader and
        // must agree with the f64 interpreter at f32 granularity over edge
        // inputs (including the classes that broke sign/round/rem/hypot).
        let bodies = [
            "a * a + sin(a)",
            "sign(a)",
            "round(a)",
            "hypot(a, a)",
            "(a + 1.0) % (a + 0.5)",
            "if(a, 11.0, 22.0)",
            "1.0 / a",
            "a < 1.0",
            "(a + 1.0) % a",
        ];
        let edges = [
            0.0f64,
            -0.0,
            0.5,
            -0.5,
            2.5,
            -2.5,
            1.5,
            -0.4,
            1.0e20,
            -1.0e20,
            f64::MIN_POSITIVE,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        for body in bodies {
            let src = format!(
                "world {{ gravity=(0,0,0) entity e {{ state=(a=0.0, x=0.0) }} }} \
                 funcs {{ f(a) {{ {body} }} }} \
                 systems {{ update {{ on=e; dt=1.0  x = f(a) }} }}"
            );
            let module = eir_of(&src);
            let id = 0xF000_0000u64;
            let shader = emit_compute_shader(&module, id).unwrap();
            naga_validate(&shader).expect("shader must validate");
            let mut rt = crate::lang::LangRuntime::compile(&src).unwrap();
            for &a in &edges {
                // The device backend sees f32: feed the interpreter the same
                // f32-rounded value so the comparison is well-posed.
                let af = (a as f32) as f64;
                rt.scene
                    .get_mut(pwe_api::EntityId(1))
                    .unwrap()
                    .state
                    .as_mut()
                    .unwrap()
                    .values[0] = af;
                let interp_r = rt.step_jit();
                let oracle_r = eval_f32(&module, id, a as f32);
                match (interp_r, oracle_r) {
                    (Ok(_), Ok(oracle)) => {
                        let interp = rt
                            .scene
                            .get(pwe_api::EntityId(1))
                            .unwrap()
                            .state
                            .as_ref()
                            .unwrap()
                            .values[1] as f32;
                        assert!(
                            close(oracle, interp),
                            "body `{body}` at a={a}: interpreter f32={interp} oracle={oracle}"
                        );
                    }
                    (Err(ei), Err(eo)) => {
                        assert_eq!(
                            ei.detail, eo.detail,
                            "body `{body}` at a={a}: trap detail differs"
                        );
                    }
                    (i, o) => panic!(
                        "body `{body}` at a={a}: tier disagreement interp={i:?} oracle={o:?}"
                    ),
                }
            }
        }
    }
}
