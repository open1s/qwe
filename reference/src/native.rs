//! Phase-3 native backend: an ahead-of-time/jit compiler for **pure EIR
//! functions** (`funcs`) that emits C, compiles it with the system C compiler
//! (`cc -O2`), and loads the result with `dlopen`.
//!
//! This is a *real* native backend (not a validated cache): the emitted code is
//! machine code produced by the platform compiler. It sits at the
//! [`crate::aot::AotProgram`] `target` boundary ([`NATIVE_TARGET`]).
//!
//! Scope: only functions with no world access and no effects — i.e. the pure
//! `funcs` bodies (arithmetic, comparisons, elementary math, calls to other pure
//! functions, and control flow). `ReadView`/`WriteView`/spatial queries/`time`/
//! `random`/events make a function ineligible. The interpreter remains the
//! semantic oracle: the differential test evaluates the same function natively
//! and interpretively and requires bit-identical results.
//!
//! Availability: requires a system `cc`; [`NativeProgram::compile`] returns an
//! error (detail 5) when no compiler is present, so this never becomes a hard
//! build/runtime dependency.

use crate::eir::{EirModule, Function, Immediate, Opcode};
use pwe_api::{Error, Result, Status};
use std::os::raw::{c_char, c_int, c_void};

/// The `target` value selecting the native C backend at the AOT boundary.
pub const NATIVE_TARGET: u16 = 3;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

#[cfg(unix)]
extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;

/// A loaded native library of pure EIR functions (`pwe_f_<id>`).
pub struct NativeProgram {
    #[cfg(unix)]
    handle: *mut c_void,
    /// Symbol name -> id for the functions that were compiled.
    compiled: Vec<u64>,
}

// The handle is only used behind `&self` read-only calls; the compiled code is
// immutable, so sharing across threads is sound.
unsafe impl Send for NativeProgram {}
unsafe impl Sync for NativeProgram {}

impl Drop for NativeProgram {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if !self.handle.is_null() {
                dlclose(self.handle);
            }
        }
    }
}

impl NativeProgram {
    /// Whether a function can be lowered natively (pure, no world access).
    pub fn eligible(f: &Function) -> bool {
        f.effect_mask == 0
            && f.instructions.iter().all(|i| {
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
                            | Opcode::Br
                            | Opcode::CondBr
                            | Opcode::Call
                            | Opcode::Return
                    )
            })
    }

    /// COMPILE: emit C for every eligible function, compile it with `cc`, and
    /// load it. Ineligible functions are skipped (they stay on the interpreter).
    pub fn compile(module: &EirModule) -> Result<Self> {
        let chosen: Vec<&Function> = module
            .functions
            .iter()
            .filter(|f| f.argument_count > 0 && Self::eligible(f))
            .collect();
        if chosen.is_empty() {
            return Err(error(Status::Invalid, 5));
        }
        let source = emit_module(&chosen);
        let dir = std::env::temp_dir().join(format!("pwe_native_{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|_| error(Status::Invalid, 5))?;
        let c_path = dir.join("pwe_native.c");
        let lib_path = dir.join(if cfg!(target_os = "macos") {
            "libpwe_native.dylib"
        } else {
            "libpwe_native.so"
        });
        std::fs::write(&c_path, source).map_err(|_| error(Status::Invalid, 5))?;
        let status = std::process::Command::new("cc")
            .arg("-O2")
            .arg("-shared")
            .arg("-fPIC")
            // Keep two-rounding `Mul;Add` semantics (EIR `Fma`): no contraction.
            .arg("-ffp-contract=off")
            .arg("-o")
            .arg(&lib_path)
            .arg(&c_path)
            .arg("-lm")
            .status()
            .map_err(|_| error(Status::Invalid, 5))?;
        if !status.success() {
            return Err(error(Status::Invalid, 5));
        }
        #[cfg(unix)]
        {
            let cpath = std::ffi::CString::new(lib_path.to_string_lossy().as_bytes())
                .map_err(|_| error(Status::Invalid, 5))?;
            let handle = unsafe { dlopen(cpath.as_ptr(), RTLD_NOW) };
            if handle.is_null() {
                return Err(error(Status::Invalid, 5));
            }
            Ok(Self {
                handle,
                compiled: chosen.iter().map(|f| f.id).collect(),
            })
        }
        #[cfg(not(unix))]
        {
            Err(error(Status::SchemaUnsupported, 5))
        }
    }

    /// Whether function `id` was lowered natively.
    pub fn has(&self, id: u64) -> bool {
        self.compiled.contains(&id)
    }

    /// Calls a compiled pure function with `args` (its parameters, in order).
    pub fn call(&self, id: u64, args: &[f64]) -> Result<f64> {
        #[cfg(unix)]
        {
            if !self.has(id) {
                return Err(error(Status::Invalid, 5));
            }
            let sym = std::ffi::CString::new(format!("pwe_f_{id}"))
                .map_err(|_| error(Status::Invalid, 5))?;
            let ptr = unsafe { dlsym(self.handle, sym.as_ptr()) };
            if ptr.is_null() {
                return Err(error(Status::Invalid, 5));
            }
            type Native = unsafe extern "C" fn(*const f64) -> f64;
            let f: Native = unsafe { std::mem::transmute(ptr) };
            Ok(unsafe { f(args.as_ptr()) })
        }
        #[cfg(not(unix))]
        {
            let _ = (id, args);
            Err(error(Status::SchemaUnsupported, 5))
        }
    }
}

/// Emits one C translation unit with a function per entry.
fn emit_module(functions: &[&Function]) -> String {
    let mut s = String::from("#include <math.h>\n#include <stddef.h>\n\n");
    for f in functions {
        s.push_str(&emit_function(f));
    }
    s
}

fn c_double(v: f64) -> String {
    if v.is_nan() {
        "NAN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "INFINITY" } else { "-INFINITY" }.to_string()
    } else {
        format!("{v:.17e}")
    }
}

fn imm_double(imm: Immediate) -> f64 {
    match imm {
        Immediate::I32(v) => v as f64,
        Immediate::U32(v) => v as f64,
        Immediate::I64(v) => v as f64,
        Immediate::U64(v) => v as f64,
        Immediate::F32(v) => v as f64,
        Immediate::F64(v) => v,
        Immediate::Bool(v) => u8::from(v) as f64,
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
        Opcode::Abs => "fabs",
        Opcode::Floor => "floor",
        Opcode::Ceil => "ceil",
        Opcode::Round => "round",
        Opcode::Sign => return None, // emulated with copysign
        Opcode::Log10 => "log10",
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

fn emit_function(f: &Function) -> String {
    let n = f.instructions.len();
    let cap = f
        .instructions
        .iter()
        .map(|i| i.result_id as usize + 1)
        .max()
        .unwrap_or(1)
        .max(f.argument_count as usize + 1);
    let mut s = format!(
        "double pwe_f_{}(const double* a) {{\n  double r[{}];\n",
        f.id, cap
    );
    for k in 0..n {
        s.push_str(&format!("  r[{}] = 0.0;\n", k));
    }
    // Parameters live in registers 1..=n (EIR CALL convention).
    for (i, _) in (0..f.argument_count).enumerate() {
        s.push_str(&format!("  r[{}] = a[{}];\n", i + 1, i));
    }
    for (k, ins) in f.instructions.iter().enumerate() {
        s.push_str(&format!("L{k}:\n"));
        let o = &ins.operands;
        let res = ins.result_id;
        let line = match ins.opcode {
            Opcode::Nop => String::new(),
            Opcode::Const => format!(
                "  r[{res}] = {};\n",
                c_double(imm_double(ins.constant.unwrap_or(Immediate::F64(0.0))))
            ),
            Opcode::Fma => format!("  r[{res}] = r[{}] * r[{}] + r[{}];\n", o[0], o[1], o[2]),
            Opcode::Rem => format!("  r[{res}] = fmod(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Pow => format!("  r[{res}] = pow(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Atan2 => format!("  r[{res}] = atan2(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Hypot => format!("  r[{res}] = hypot(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Sign => format!(
                "  r[{res}] = (r[{}] == 0.0) ? 0.0 : copysign(1.0, r[{}]);\n",
                o[0], o[0]
            ),
            Opcode::Select => format!(
                "  r[{res}] = (r[{}] != 0.0) ? r[{}] : r[{}];\n",
                o[0], o[1], o[2]
            ),
            Opcode::Br => format!("  goto L{};\n", o[0]),
            Opcode::CondBr => format!(
                "  if (r[{}] != 0.0) goto L{}; else goto L{};\n",
                o[0], o[1], o[2]
            ),
            Opcode::Call => {
                let target = o[0];
                let args = &o[1..];
                let mut init = String::new();
                for (i, a) in args.iter().enumerate() {
                    init.push_str(&format!("  double t{i} = r[{a}];\n"));
                }
                let mut arr = String::from("  { ");
                for i in 0..args.len() {
                    arr.push_str(&format!("t{i}, "));
                }
                arr.push('}');
                format!("{init}  double ta[] = {arr};\n  r[{res}] = pwe_f_{target}(ta);\n")
            }
            Opcode::Return => {
                if let Some(v) = o.first() {
                    format!("  return r[{v}];\n")
                } else {
                    "  return 0.0;\n".to_string()
                }
            }
            Opcode::Trap | Opcode::Unreachable => "  return NAN;\n".to_string(),
            other => {
                if let Some(op) = bin_op(other) {
                    format!("  r[{res}] = r[{}] {op} r[{}];\n", o[0], o[1])
                } else if let Some(op) = cmp_op(other) {
                    format!("  r[{res}] = (r[{}] {op} r[{}]) ? 1.0 : 0.0;\n", o[0], o[1])
                } else if let Some(fname) = unary_fn(other) {
                    format!("  r[{res}] = {fname}(r[{}]);\n", o[0])
                } else {
                    "  return NAN;\n".to_string()
                }
            }
        };
        s.push_str(&line);
    }
    s.push_str("  return 0.0;\n}\n\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc_available() -> bool {
        std::process::Command::new("cc")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn native_pure_function_matches_interpreter() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        // A pure `funcs` program: f(a,b) = a*a + b*b + sin(a) (exercises Fma,
        // Mul, Add, Sin).
        let src = r#"
            world { gravity=(0,0,0) entity e { state=(a=0.0, b=0.0, x=0.0) } }
            funcs { f(a, b) { a * a + b * b + sin(a) } }
            systems { update { on=e; dt=1.0  x = f(a, b) } }
        "#;
        let parsed = crate::lang::parse(src).unwrap();
        let compiled = crate::lang::compile_program(parsed).unwrap();
        let module = compiled.eir.clone();
        let func_id = 0xF000_0000u64;
        let native = NativeProgram::compile(&module).unwrap();
        assert!(native.has(func_id), "pure `f` must be native-eligible");

        let mut rt = crate::lang::LangRuntime::compile(src).unwrap();
        for i in 0..64 {
            let a = (i as f64) * 0.31 - 4.0;
            let b = (i as f64) * -0.17 + 2.0;
            // Interpreter: set inputs, step once, read x.
            rt.scene
                .get_mut(pwe_api::EntityId(1))
                .unwrap()
                .state
                .as_mut()
                .unwrap()
                .values[0] = a;
            rt.scene
                .get_mut(pwe_api::EntityId(1))
                .unwrap()
                .state
                .as_mut()
                .unwrap()
                .values[1] = b;
            rt.step_jit().unwrap();
            let interp = rt
                .scene
                .get(pwe_api::EntityId(1))
                .unwrap()
                .state
                .as_ref()
                .unwrap()
                .values[2];
            let nat = native.call(func_id, &[a, b]).unwrap();
            assert_eq!(
                interp.to_bits(),
                nat.to_bits(),
                "native vs interpreter mismatch at a={a} b={b}: {interp} vs {nat}"
            );
        }
    }
}
