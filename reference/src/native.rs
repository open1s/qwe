//! Phase-3 native backend: an ahead-of-time/jit compiler for EIR functions
//! that emits C, compiles it with the system C compiler (`cc -O2`), and loads
//! the result with `dlopen`.
//!
//! This is a *real* native backend (machine code from the platform compiler).
//! It sits at the [`crate::aot::AotProgram`] `target` boundary ([`NATIVE_TARGET`]).
//!
//! **Scope / contract (RFC-0027 addendum).** Only *in-process* functions with no
//! world capability surface beyond component reads/writes are compiled; this is
//! a defined exemption from the full JIT lifecycle (see the addendum at the end
//! of `rfc/RFC-0027-jit-contract.md`). The module is validated (SSA types +
//! linear dominance) before any code is emitted, the artifact is content-hash
//! identified, and functions with effects (time/random/io/atomic/resources) or
//! spatial/field access stay on the interpreter. This backend is **not wired
//! into `pwe run`/`present`** — it is reachable only from its own API/tests, and
//! a future integration must run the full lifecycle.
//!
//! The interpreter remains the semantic oracle: the differential tests run the
//! same program natively and interpretively and require identical results,
//! including divisION-by-zero traps (detail 18), `signum` of ±0.0/NaN, and
//! bit-exact condition tests.
//!
//! Availability: requires a system `cc`; [`NativeProgram::compile`] returns an
//! error (detail 5) when no compiler is present, so this never becomes a hard
//! build/runtime dependency.

use crate::eir::{
    ComponentRef, EirModule, EirRuntime, Function, Immediate, Opcode, WorldWrite,
    EIR_EFFECT_BARRIER, EIR_EFFECT_READ_WORLD, EIR_EFFECT_WRITE_WORLD,
};
use pwe_api::{ComponentTypeId, Error, Hash256, Result, Status};
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

/// The C-ABI function table passed to generated code (so the compiled library
/// resolves runtime calls through pointers, not exported symbols).
#[repr(C)]
pub(crate) struct PweCtx {
    pub read: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32) -> f64,
    pub read_committed: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32) -> f64,
    pub write: unsafe extern "C" fn(*mut c_void, u64, u64, u32, u32, u32, u32, u32, f64),
    pub div_guard: unsafe extern "C" fn(*mut c_void, f64) -> c_int,
    pub env: *mut c_void,
}

/// The execution context handed to native shims: a borrowed runtime plus the
/// write accumulator, and the first runtime error (C cannot unwind, so shims
/// record and the caller re-raises).
pub(crate) struct NativeCtx<'a> {
    pub rt: &'a mut dyn EirRuntime,
    pub writes: &'a mut Vec<WorldWrite>,
    pub error: Option<Error>,
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

/// World-access shims (`extern "C"`, called through [`PweCtx`]).
///
/// # Safety
/// `env` must be a non-null pointer to a live [`NativeCtx`] for the duration of
/// the native call; [`NativeProgram`] only calls generated code with a pointer
/// it created for exactly that purpose.
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

/// RFC-0021 division/remainder guard: returns 1 (and records `EirInvalid` 18)
/// when `divisor` is exactly zero (`+0.0` or `-0.0`), else 0. Generated code
/// returns early on 1, so a divide-by-zero fails the step exactly like the
/// interpreter instead of yielding `inf`/`NaN`.
///
/// # Safety
/// Same contract as [`pwe_read_view`].
pub(crate) unsafe extern "C" fn pwe_div_guard(env: *mut c_void, divisor: f64) -> c_int {
    if divisor == 0.0 {
        let ctx = &mut *(env as *mut NativeCtx);
        ctx.error
            .get_or_insert_with(|| error(Status::EirInvalid, 18));
        1
    } else {
        0
    }
}

/// A loaded native library of EIR functions (`pwe_f_<id>`).
pub struct NativeProgram {
    #[cfg(unix)]
    handle: *mut c_void,
    /// id -> (arity, reads-or-writes-world) for every compiled function.
    #[cfg(unix)]
    metas: Vec<FnMeta>,
    /// The unique artifact directory (removed on drop).
    dir: std::path::PathBuf,
    /// Content identity of the emitted artifact (RFC-0035 style).
    pub artifact_hash: Hash256,
    /// The backend target (`NATIVE_TARGET`).
    pub target: u16,
}

#[derive(Clone, Copy)]
struct FnMeta {
    id: u64,
    arity: u32,
    world: bool,
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
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn is_world(f: &Function) -> bool {
    f.effect_mask & (EIR_EFFECT_READ_WORLD | EIR_EFFECT_WRITE_WORLD) != 0
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
    /// COMPILE: validate the module, emit C for every eligible function whose
    /// `Call` targets are also eligible, compile it with `cc`, and load it.
    ///
    /// Ineligible functions stay on the interpreter. The artifact is keyed by
    /// its content hash, so two different programs never share a loaded image.
    pub fn compile(module: &EirModule) -> Result<Self> {
        // RFC-0027 Validate stage: never emit code for an unvalidated module.
        module.validate(true)?;
        module.verify_linear_dominance()?;

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
        // Artifact identity: SHA-256 of the emitted source (RFC-0035 style).
        let artifact_hash = crate::sha256::digest(source.as_bytes());
        let short = artifact_hash
            .0
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        static NATIVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NATIVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("pwe_native_{}_{seq}_{short}", std::process::id()));
        create_private_dir(&dir)?;
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
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error(Status::Invalid, 5));
        }
        #[cfg(unix)]
        {
            let cpath = std::ffi::CString::new(lib_path.to_string_lossy().as_bytes())
                .map_err(|_| error(Status::Invalid, 5))?;
            let handle = unsafe { dlopen(cpath.as_ptr(), RTLD_NOW) };
            if handle.is_null() {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error(Status::Invalid, 5));
            }
            Ok(Self {
                handle,
                metas: chosen
                    .iter()
                    .map(|f| FnMeta {
                        id: f.id,
                        arity: f.argument_count,
                        world: is_world(f),
                    })
                    .collect(),
                dir,
                artifact_hash,
                target: NATIVE_TARGET,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = &dir;
            Err(error(Status::SchemaUnsupported, 5))
        }
    }

    /// Whether function `id` was lowered natively.
    pub fn has(&self, id: u64) -> bool {
        self.meta(id).is_some()
    }

    fn meta(&self, id: u64) -> Option<FnMeta> {
        #[cfg(unix)]
        {
            self.metas.iter().copied().find(|m| m.id == id)
        }
        #[cfg(not(unix))]
        {
            let _ = id;
            None
        }
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

    /// Calls a compiled **pure** function with `args` (its parameters, in
    /// order). Errors (never traps) if the function is unknown, takes world
    /// access (use [`Self::execute_entries`]), or `args.len()` does not match
    /// the arity — the latter would otherwise read out of bounds in the
    /// generated code.
    pub fn call(&self, id: u64, args: &[f64]) -> Result<f64> {
        let meta = self.meta(id).ok_or_else(|| error(Status::Invalid, 5))?;
        if meta.world {
            return Err(error(Status::Invalid, 5));
        }
        if args.len() != meta.arity as usize {
            return Err(error(Status::Invalid, 5));
        }
        #[cfg(unix)]
        {
            let func = self.symbol(id)?;
            // A no-op runtime: pure functions never touch the world, but the
            // division guard still needs a live context to record traps.
            let mut noop = crate::eir::NoopRuntime;
            let mut writes: Vec<WorldWrite> = Vec::new();
            let mut ctx = NativeCtx {
                rt: &mut noop,
                writes: &mut writes,
                error: None,
            };
            let table = PweCtx {
                read: pwe_read_view,
                read_committed: pwe_read_committed,
                write: pwe_write_view,
                div_guard: pwe_div_guard,
                env: &mut ctx as *mut NativeCtx as *mut c_void,
            };
            let out = unsafe { func(&table as *const PweCtx as *mut c_void, args.as_ptr()) };
            match ctx.error.take() {
                Some(e) => Err(e),
                None => Ok(out),
            }
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
                div_guard: pwe_div_guard,
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

/// Creates `dir` with `0700` permissions (owner-only) on unix, rejecting a
/// pre-existing path so a planted symlink can't redirect our writes.
fn create_private_dir(dir: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|_| error(Status::Invalid, 5))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(dir).map_err(|_| error(Status::Invalid, 5))
    }
}

#[cfg(unix)]
type NativeFn = unsafe extern "C" fn(*mut c_void, *const f64) -> f64;

/// Emits one C translation unit with a function per entry.
fn emit_module(functions: &[&Function]) -> String {
    let mut s = String::from(
        "#include <math.h>\n#include <stddef.h>\n#include <string.h>\n\
         typedef struct {\n\
           double (*read)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned);\n\
           double (*read_committed)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned);\n\
           void (*write)(void*, unsigned long long, unsigned long long, unsigned, unsigned, unsigned, unsigned, unsigned, double);\n\
           int (*div_guard)(void*, double);\n\
           void* env;\n\
         } PweCtx;\n\
         static int pwe_truthy(double v){ unsigned long long b; memcpy(&b, &v, 8); return b != 0ULL; }\n\n",
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
        Opcode::Sign => return None, // exact signum handled explicitly
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

/// Emits the divide/remainder guard (RFC-0021 trap on a zero divisor).
fn div_guard(divisor: u32, dest: u32, expr: String) -> String {
    format!(
        "  if (((PweCtx*)ctx)->div_guard(((PweCtx*)ctx)->env, r[{divisor}])) return NAN;\n\
         \x20 r[{dest}] = {expr};\n"
    )
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
            Opcode::Rem => div_guard(o[1], res, format!("fmod(r[{}], r[{}])", o[0], o[1])),
            Opcode::Pow => format!("  r[{res}] = pow(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Atan2 => format!("  r[{res}] = atan2(r[{}], r[{}]);\n", o[0], o[1]),
            Opcode::Hypot => format!("  r[{res}] = hypot(r[{}], r[{}]);\n", o[0], o[1]),
            // Exact `f64::signum`: NaN -> NaN (same value), else ±1 by sign
            // (so ±0.0 map to ±1.0).
            Opcode::Sign => format!(
                "  r[{res}] = (r[{}] != r[{}]) ? r[{}] : copysign(1.0, r[{}]);\n",
                o[0], o[0], o[0], o[0]
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
                "  r[{res}] = pwe_truthy(r[{}]) ? r[{}] : r[{}];\n",
                o[0], o[1], o[2]
            ),
            Opcode::Br => format!("  goto L{};\n", o[0]),
            Opcode::CondBr => format!(
                "  if (pwe_truthy(r[{}])) goto L{}; else goto L{};\n",
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
                if other == Opcode::Div {
                    div_guard(o[1], res, format!("r[{}] / r[{}]", o[0], o[1]))
                } else if let Some(op) = bin_op(other) {
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

    /// Runs a one-`funcs` program interpretively, returning the world output of
    /// `f(a, b)` (a step failure surfaces as `Err`).
    fn interp2(src: &str, a: f64, b: f64) -> Result<f64> {
        let mut rt = crate::lang::LangRuntime::compile(src).unwrap();
        {
            let st = rt
                .scene
                .get_mut(pwe_api::EntityId(1))
                .unwrap()
                .state
                .as_mut()
                .unwrap();
            st.values[0] = a;
            st.values[1] = b;
        }
        rt.step_jit()?;
        Ok(rt
            .scene
            .get(pwe_api::EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[2])
    }

    const F2: &str = "world { gravity=(0,0,0) entity e { state=(a=0.0, b=0.0, x=0.0) } } \
                       funcs { f(a, b) { BODY } } \
                       systems { update { on=e; dt=1.0  x = f(a, b) } }";

    fn f2(body: &str) -> String {
        F2.replace("BODY", body)
    }

    #[test]
    fn native_pure_matches_interpreter_on_a_matrix() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        let inputs = [
            0.0,
            -0.0,
            1.5,
            -2.25,
            f64::MIN_POSITIVE,
            f64::from_bits(1), // subnormal
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        // Each body is (name, arity-2 body) exercised over the input matrix.
        let bodies = [
            "a * a + b * b + sin(a)",
            "a / b + 1.0",
            "sign(a) + sign(b)",
            "if(a, b, 0.0 - b)",
            "a % b",
        ];
        for body in bodies {
            let src = f2(body);
            let module = eir_of(&src);
            let native = NativeProgram::compile(&module).unwrap();
            let id = 0xF000_0000u64;
            for &x in &inputs {
                for &y in &inputs {
                    let interp = interp2(&src, x, y);
                    let nat = native.call(id, &[x, y]);
                    match (interp, nat) {
                        (Ok(i), Ok(n)) => assert!(
                            i.to_bits() == n.to_bits() || (i.is_nan() && n.is_nan()),
                            "body `{body}` at ({x},{y}): interp={i} native={n}"
                        ),
                        (Err(ei), Err(en)) => {
                            assert_eq!(ei.detail, en.detail, "body `{body}` trap detail")
                        }
                        (i, n) => panic!("body `{body}` at ({x},{y}): interp={i:?} native={n:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn native_arity_mismatch_is_an_error_not_a_crash() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        // f takes exactly one argument.
        let src = "world { gravity=(0,0,0) entity e { state=(a=0.0, x=0.0) } } \
                   funcs { f(a) { a * 2.0 } } \
                   systems { update { on=e; dt=1.0  x = f(a) } }";
        let native = NativeProgram::compile(&eir_of(src)).unwrap();
        let id = 0xF000_0000u64;
        assert_eq!(native.call(id, &[3.0]).unwrap(), 6.0);
        assert!(native.call(id, &[]).is_err(), "too few must be Err");
        assert!(
            native.call(id, &[1.0, 2.0]).is_err(),
            "too many must be Err"
        );
        assert!(native.call(999, &[1.0]).is_err(), "unknown id must be Err");
    }

    #[test]
    fn native_second_compile_is_independent() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
        let n1 = NativeProgram::compile(&eir_of(&f2("a + 1.0"))).unwrap();
        let n2 = NativeProgram::compile(&eir_of(&f2("a + 2.0"))).unwrap();
        let id = 0xF000_0000u64;
        assert_eq!(n1.call(id, &[10.0, 0.0]).unwrap(), 11.0);
        assert_eq!(
            n2.call(id, &[10.0, 0.0]).unwrap(),
            12.0,
            "must not reuse n1's code"
        );
        assert_ne!(n1.artifact_hash, n2.artifact_hash);
    }

    #[test]
    fn native_world_kernel_matches_interpreter_writes() {
        if !cc_available() {
            eprintln!("skipping: no `cc` available");
            return;
        }
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
