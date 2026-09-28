//! Phase-3 native backend: an ahead-of-time/jit compiler for EIR functions
//! that emits C, compiles it with the system C compiler (`cc -O2`), and loads
//! the result with `dlopen`.
//!
//! This is a *real* native backend (not a validated cache): the emitted code is
//! machine code produced by the platform compiler. It sits at the
//! [`crate::aot::AotProgram`] `target` boundary ([`NATIVE_TARGET`]).
//!
//! Scope: pure functions *and* functions that read/write world components
//! (`ReadView`/`ReadCommitted`/`WriteView`) through `extern "C"` shims into a
//! [`NativeCtx`]. Functions with effects (`time`/`random`/events/IO), spatial
//! queries, or grid-field access are ineligible and stay on the interpreter.
//! The interpreter remains the semantic oracle: the differential test runs the
//! same program natively and interpretively and requires identical writes.
//!
//! Availability: requires a system `cc`; [`NativeProgram::compile`] returns an
//! error (detail 5) when no compiler is present, so this never becomes a hard
//! build/runtime dependency.

use crate::eir::{
    ComponentRef, EirModule, EirRuntime, Function, Immediate, Opcode, WorldWrite,
    EIR_EFFECT_BARRIER, EIR_EFFECT_READ_WORLD, EIR_EFFECT_WRITE_WORLD,
};
use pwe_api::{ComponentTypeId, Error, Result, Status};
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

/// The execution context handed to native world-access shims: a borrowed
/// runtime plus the write accumulator, and a slot for the first runtime error
/// (C cannot unwind, so shims record and the caller re-raises).
pub(crate) struct NativeCtx<'a> {
    pub rt: &'a mut dyn EirRuntime,
    pub writes: &'a mut Vec<WorldWrite>,
    pub error: Option<Error>,
}

/// The C-ABI function table passed to generated code (so the compiled library
/// resolves runtime calls through pointers, not exported symbols).
#[repr(C)]
pub(crate) struct PweCtx {
    pub read: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32) -> f64,
    pub read_committed: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32) -> f64,
    pub write: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32, f64),
    pub env: *mut c_void,
}

fn limbs(component: ComponentTypeId) -> (u32, u32, u32, u32) {
    let b = component.0;
    (
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
    )
}

fn component_from_limbs(c0: u32, c1: u32, c2: u32, c3: u32) -> ComponentTypeId {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&c0.to_le_bytes());
    b[4..8].copy_from_slice(&c1.to_le_bytes());
    b[8..12].copy_from_slice(&c2.to_le_bytes());
    b[12..16].copy_from_slice(&c3.to_le_bytes());
    ComponentTypeId(b)
}

fn target_of(
    entity_lo: u64,
    entity_hi: u64,
    c0: u32,
    c1: u32,
    c2: u32,
    c3: u32,
    offset: u32,
) -> ComponentRef {
    ComponentRef {
        entity: (entity_lo as u128) | ((entity_hi as u128) << 64),
        component: component_from_limbs(c0, c1, c2, c3),
        offset,
    }
}

/// World-access shims (`#[no_mangle]` so the emitted C can call them).
///
/// # Safety
/// `env` must be a non-null pointer to a live [`NativeCtx`] for the duration of
/// the native call; that is guaranteed by [`NativeProgram::execute_entries`],
/// which only calls generated code with a pointer it created.
pub(crate) unsafe extern "C" fn pwe_read_view(
    env: *mut c_void,
    entity_lo: u64,
    entity_hi: u64,
    c0: u32,
    c1: u32,
    c2: u32,
    c3: u32,
    offset: u32,
) -> f64 {
    let ctx = &mut *(env as *mut NativeCtx);
    let target = target_of(entity_lo, entity_hi, c0, c1, c2, c3, offset);
    match ctx.rt.read_field(target) {
        Ok(raw) => f64::from_bits(raw),
        Err(e) => {
            ctx.error.get_or_insert(e);
            f64::NAN
        }
    }
}

/// See [`pwe_read_view`].
pub(crate) unsafe extern "C" fn pwe_read_committed(
    env: *mut c_void,
    entity_lo: u64,
    entity_hi: u64,
    c0: u32,
    c1: u32,
    c2: u32,
    c3: u32,
    offset: u32,
) -> f64 {
    let ctx = &mut *(env as *mut NativeCtx);
    let target = target_of(entity_lo, entity_hi, c0, c1, c2, c3, offset);
    match ctx.rt.read_committed_field(target) {
        Ok(raw) => f64::from_bits(raw),
        Err(e) => {
            ctx.error.get_or_insert(e);
            f64::NAN
        }
    }
}

/// See [`pwe_read_view`].
pub(crate) unsafe extern "C" fn pwe_write_view(
    env: *mut c_void,
    entity_lo: u64,
    entity_hi: u64,
    c0: u32,
    c1: u32,
    c2: u32,
    c3: u32,
    offset: u32,
    value: f64,
) {
    let ctx = &mut *(env as *mut NativeCtx);
    let target = target_of(entity_lo, entity_hi, c0, c1, c2, c3, offset);
    let bits = value.to_bits();
    ctx.rt.write_field(target, bits);
    ctx.writes.push(WorldWrite {
        entity: target.entity,
        component: target.component,
        offset: target.offset,
        value: bits,
    });
}

/// A loaded native library of EIR functions (`pwe_f_<id>`).
pub struct NativeProgram {
    #[cfg(unix)]
    handle: *mut c_void,
    /// Ids of the functions that were compiled.
    compiled: Vec<u64>,
}

// The handle is only used for read-only symbol lookup; the compiled code is
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

fn eligible(f: &Function) -> bool {
    // Component reads/writes go through the C-ABI shims, and the barrier bit is
    // a system-boundary marker, not a side effect. Any other effect bit
    // (resources/atomic/io/device/network/time/random) makes a function
    // ineligible.
    const SUPPORTED: u32 = EIR_EFFECT_BARRIER | EIR_EFFECT_READ_WORLD | EIR_EFFECT_WRITE_WORLD;
    if f.effect_mask & !SUPPORTED != 0 {
        return false;
    }
    f.instructions.iter().all(|i| match i.opcode {
        Opcode::ReadView | Opcode::ReadCommitted | Opcode::WriteView => i.target.is_some(),
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
        | Opcode::Trap
        | Opcode::Unreachable => i.target.is_none(),
        _ => false,
    })
}

impl NativeProgram {
    /// COMPILE: emit C for every eligible function whose `Call` targets are
    /// also eligible, compile it with `cc`, and load it. Ineligible functions
    /// stay on the interpreter.
    pub fn compile(module: &EirModule) -> Result<Self> {
        // Start from the eligible set, then close under `Call` (a function that
        // calls an ineligible function cannot be emitted).
        let mut chosen: Vec<&Function> = module.functions.iter().filter(|f| eligible(f)).collect();
        loop {
            let ids: std::collections::BTreeSet<u64> = chosen.iter().map(|f| f.id).collect();
            let next: Vec<&Function> = chosen
                .iter()
                .copied()
                .filter(|f| {
                    f.instructions
                        .iter()
                        .all(|i| i.opcode != Opcode::Call || ids.contains(&(i.operands[0] as u64)))
                })
                .collect();
            if next.len() == chosen.len() {
                break;
            }
            chosen = next;
        }
        if chosen.is_empty() {
            return Err(error(Status::Invalid, 5));
        }
        let source = emit_module(&chosen);
        // Unique per compile: tests run in parallel threads sharing a pid, so a
        // pid-only path would let one compile clobber another's library.
        static NATIVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NATIVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pwe_native_{}_{seq}", std::process::id()));
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

    #[cfg(unix)]
    fn symbol(&self, id: u64) -> Result<NativeFn> {
        if !self.has(id) {
            return Err(error(Status::Invalid, 5));
        }
        let sym =
            std::ffi::CString::new(format!("pwe_f_{id}")).map_err(|_| error(Status::Invalid, 5))?;
        let ptr = unsafe { dlsym(self.handle, sym.as_ptr()) };
        if ptr.is_null() {
            return Err(error(Status::Invalid, 5));
        }
        Ok(unsafe { std::mem::transmute::<*mut c_void, NativeFn>(ptr) })
    }

    /// Calls a compiled pure function with `args` (its parameters, in order).
    /// Passes a null context — only valid for functions with no world access.
    pub fn call(&self, id: u64, args: &[f64]) -> Result<f64> {
        #[cfg(unix)]
        {
            let f = self.symbol(id)?;
            Ok(unsafe { f(std::ptr::null_mut(), args.as_ptr()) })
        }
        #[cfg(not(unix))]
        {
            let _ = (id, args);
            Err(error(Status::SchemaUnsupported, 5))
        }
    }

    /// EXECUTE: runs every argument-less entry function natively in the module's
    /// deterministic order, applying the system barriers exactly as the
    /// interpreter does, and returns the accumulated ordered writes.
    pub fn execute_entries(
        &self,
        module: &EirModule,
        rt: &mut dyn EirRuntime,
    ) -> Result<Vec<WorldWrite>> {
        let index = module.prepare_index();
        let mut writes: Vec<WorldWrite> = Vec::new();
        for &entry in &index.order {
            let f = &module.functions[entry];
            if f.argument_count != 0 {
                continue;
            }
            if !self.has(f.id) {
                return Err(error(Status::Invalid, 5));
            }
            if f.effect_mask & EIR_EFFECT_BARRIER != 0 {
                rt.commit_barrier();
            }
            let func = self.symbol(f.id)?;
            let mut ctx = NativeCtx {
                rt,
                writes: &mut writes,
                error: None,
            };
            let table = PweCtx {
                read: pwe_read_view,
                read_committed: pwe_read_committed,
                write: pwe_write_view,
                env: &mut ctx as *mut NativeCtx as *mut c_void,
            };
            let empty: [f64; 0] = [];
            unsafe {
                func(&table as *const PweCtx as *mut c_void, empty.as_ptr());
            }
            if let Some(e) = ctx.error.take() {
                return Err(e);
            }
        }
        Ok(writes)
    }
}

#[cfg(unix)]
type NativeFn = unsafe extern "C" fn(*mut c_void, *const f64) -> f64;

/// Emits one C translation unit with a function per entry.
fn emit_module(functions: &[&Function]) -> String {
    let mut s = String::from(
        "#include <math.h>\n#include <stddef.h>\n\
         typedef struct {\n\
           double (*read)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned);\n\
           double (*read_committed)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned);\n\
           void (*write)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned, double);\n\
           void* env;\n\
         } PweCtx;\n\n",
    );
    for f in functions {
        s.push_str(&format!("double pwe_f_{}(void*, const double*);\n", f.id));
    }
    s.push('\n');
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

fn shim_call(field: &str, target: ComponentRef, value: Option<u32>) -> String {
    let (c0, c1, c2, c3) = limbs(target.component);
    let lo = target.entity as u64;
    let hi = (target.entity >> 64) as u64;
    let head = format!(
        "((PweCtx*)ctx)->{field}(((PweCtx*)ctx)->env, {lo}ULL, {hi}ULL, {c0}u, {c1}u, {c2}u, {c3}u, {}",
        target.offset
    );
    match value {
        Some(v) => format!("  {head}, r[{v}]);\n"),
        None => format!("{head})"),
    }
}

fn emit_function(f: &Function) -> String {
    let n = f.instructions.len();
    let cap = f
        .instructions
        .iter()
        .map(|i| i.result_id as usize + 1)
        .max()
        .unwrap_or(1)
        .max(f.argument_count as usize + 1)
        .max(n);
    let mut s = format!(
        "double pwe_f_{}(void* ctx, const double* a) {{\n  (void)ctx; double r[{}];\n",
        f.id, cap
    );
    for k in 0..n {
        s.push_str(&format!("  r[{}] = 0.0;\n", k));
    }
    for i in 0..f.argument_count {
        s.push_str(&format!("  r[{}] = a[{}];\n", i + 1, i));
    }
    for (k, ins) in f.instructions.iter().enumerate() {
        // `;` after the label: a declaration may not directly follow a label
        // in C < 23 (and the `Call` arm declares temporaries).
        s.push_str(&format!("L{k}: ;\n"));
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
            Opcode::ReadView => format!(
                "  r[{res}] = {};\n",
                shim_call("read", ins.target.unwrap_or_else(default_target), None)
            ),
            Opcode::ReadCommitted => format!(
                "  r[{res}] = {};\n",
                shim_call(
                    "read_committed",
                    ins.target.unwrap_or_else(default_target),
                    None
                )
            ),
            Opcode::WriteView => shim_call(
                "write",
                ins.target.unwrap_or_else(default_target),
                o.first().copied(),
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
                format!("{init}  double ta[] = {arr};\n  r[{res}] = pwe_f_{target}(ctx, ta);\n")
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

/// Placeholder target for an ineligible instruction (never reached: eligibility
/// requires `ReadView`/`WriteView` to carry a target).
fn default_target() -> ComponentRef {
    ComponentRef {
        entity: 0,
        component: ComponentTypeId([0u8; 16]),
        offset: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::ExecEnv;
    use crate::physics_eir::SceneRuntime;

    fn cc_available() -> bool {
        std::process::Command::new("cc")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn eir_of(src: &str) -> EirModule {
        let parsed = crate::lang::parse(src).unwrap();
        crate::lang::compile_program(parsed).unwrap().eir
    }

    #[test]
    fn native_pure_function_matches_interpreter() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        // A pure `funcs` program: f(a,b) = a*a + b*b + sin(a).
        let src = r#"
            world { gravity=(0,0,0) entity e { state=(a=0.0, b=0.0, x=0.0) } }
            funcs { f(a, b) { a * a + b * b + sin(a) } }
            systems { update { on=e; dt=1.0  x = f(a, b) } }
        "#;
        let module = eir_of(src);
        let func_id = 0xF000_0000u64;
        let native = NativeProgram::compile(&module).unwrap();
        assert!(native.has(func_id), "pure `f` must be native-eligible");

        let mut rt = crate::lang::LangRuntime::compile(src).unwrap();
        for i in 0..64 {
            let a = (i as f64) * 0.31 - 4.0;
            let b = (i as f64) * -0.17 + 2.0;
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

    #[test]
    fn tmp_dump_c() {
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
        let module = eir_of(&src);
        let chosen: Vec<&Function> = module.functions.iter().filter(|f| eligible(f)).collect();
        let c = emit_module(&chosen);
        std::fs::write("/tmp/pwe_open/nb.c", &c).unwrap();
        std::process::Command::new("cc")
            .args([
                "-O2",
                "-shared",
                "-fPIC",
                "-ffp-contract=off",
                "-o",
                "/tmp/pwe_open/nb.dylib",
                "/tmp/pwe_open/nb.c",
                "-lm",
            ])
            .status()
            .unwrap();
    }

    #[test]
    fn native_world_kernel_matches_interpreter_writes() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        // A world-access kernel: nbody reads state slots (ReadCommitted) and
        // writes velocities (WriteView) — exercises the C-ABI shims.
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
        let module = eir_of(&src);
        let native = NativeProgram::compile(&module).unwrap();
        assert!(
            module.functions.iter().all(|f| native.has(f.id)),
            "a pure world kernel must be fully native-eligible"
        );

        let rt = crate::lang::LangRuntime::compile(&src).unwrap();
        let index = module.prepare_index();

        let scene_a = rt.scene.clone();
        let mut env_a = ExecEnv {
            step_dt: 0.001,
            ..Default::default()
        };
        let mut rt_a = SceneRuntime::new(&scene_a);
        let interp = module
            .execute_with_index(&mut rt_a, &mut env_a, &index)
            .unwrap();

        let scene_b = rt.scene.clone();
        let mut rt_b = SceneRuntime::new(&scene_b);
        let nat = native.execute_entries(&module, &mut rt_b).unwrap();

        assert_eq!(interp.len(), nat.len(), "write count differs");
        for (a, b) in interp.iter().zip(nat.iter()) {
            assert_eq!(a.entity, b.entity);
            assert_eq!(a.component, b.component);
            assert_eq!(a.offset, b.offset);
            assert_eq!(a.value, b.value, "write value differs");
        }
    }
}
