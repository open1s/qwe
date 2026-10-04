//! RFC-0021 bounded typed EIR: validator and deterministic interpreter.
//!
//! This is the semantic oracle slice: a typed, SSA-like subset with declared
//! effects. The validator rejects use-before-def, type mismatch, invalid
//! opcodes, and effect violations; the interpreter emits ordered transaction
//! writes in deterministic order.

use crate::sha256::digest;
use crate::wire::{crc32c, Reader, Writer, MAX_DOCUMENT_BYTES};
use pwe_api::{ComponentTypeId, Error, Hash256, Result, Status, WorldId, WorldVersion};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32, offset: usize) -> Error {
    Error {
        status,
        detail,
        byte_offset: offset as u64,
    }
}

pub const EIR_EFFECT_READ_WORLD: u32 = 1;
pub const EIR_EFFECT_WRITE_WORLD: u32 = 2;
pub const EIR_EFFECT_READ_RESOURCE: u32 = 4;
pub const EIR_EFFECT_WRITE_RESOURCE: u32 = 8;
pub const EIR_EFFECT_ATOMIC: u32 = 16;
pub const EIR_EFFECT_IO: u32 = 32;
pub const EIR_EFFECT_DEVICE: u32 = 64;
pub const EIR_EFFECT_NETWORK: u32 = 128;
pub const EIR_EFFECT_TIME: u32 = 256;
pub const EIR_EFFECT_RANDOM: u32 = 512;
/// Marks a function that begins a system: the engine commits the previous
/// systems' writes before running it (start-of-system snapshot for committed
/// reads). Not produced by any opcode.
pub const EIR_EFFECT_BARRIER: u32 = 1024;

const NONDETERMINISTIC: u32 =
    EIR_EFFECT_TIME | EIR_EFFECT_RANDOM | EIR_EFFECT_IO | EIR_EFFECT_DEVICE | EIR_EFFECT_NETWORK;

/// RFC-0021 module envelope.
pub const EIR_MAGIC: [u8; 8] = *b"PWEEIR2\0";
pub const EIR_MAJOR: u16 = 2;
pub const EIR_MINOR: u16 = 0;
pub const TARGET_GENERIC: u16 = 0;
pub const TARGET_CPU: u16 = 1;
pub const TARGET_GPU: u16 = 2;
pub const TARGET_NPU: u16 = 3;
const KIND_TYPES: u16 = 1;
const KIND_FUNCTIONS: u16 = 4;
const KIND_MAX: u16 = 7;
/// RFC-0046: upper bound on explicit blocks per function (decode guard).
const MAX_BLOCKS: usize = 1 << 20;
const HEADER_LEN: usize = 120;
const DIR_ENTRY_LEN: usize = 32;
const MODULE_HASH_OFFSET: usize = 16;
const MAX_SECTIONS: u32 = pwe_api::limits::DEFAULT_LIMITS.max_sections;
/// Instruction `flags` bits: an inline constant / component target follows the
/// RFC-0021 instruction record (a bounded EIR-model extension payload).
const FLAG_CONSTANT: u16 = 1;
const FLAG_TARGET: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ValueType {
    I32 = 1,
    U32 = 2,
    I64 = 3,
    U64 = 4,
    F32 = 5,
    F64 = 6,
    Bool = 7,
}

impl ValueType {}

// ---------------------------------------------------------------------------
// EIR opcode table — the SINGLE SOURCE OF TRUTH for opcode metadata.
//
// Adding an opcode is one entry here (name + wire value + default result type);
// the enum, `ALL`, `from_u16`, `name`, and `default_result_type` are generated.
// The validator and interpreter still have per-opcode arms (behaviour), but the
// compiler enforces their exhaustiveness. The default result type replicates the
// historical inference (`bool` comparisons, `f64` value ops, `none` for
// `Const`/`HistWrite`, `u64` otherwise).
// ---------------------------------------------------------------------------
macro_rules! declare_opcodes {
    ($( $(#[$meta:meta])* $variant:ident = $value:expr => $ty:ident, )*) => {
        /// An EIR opcode. Numeric values are the frozen RFC-0021 wire encoding.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u16)]
        pub enum Opcode {
            $( $(#[$meta])* $variant = $value, )*
        }
        impl Opcode {
            /// Every opcode, in declaration order.
            pub const ALL: &'static [Opcode] = &[ $( Opcode::$variant, )* ];
            /// Decode a wire opcode number (RFC-0021).
            pub fn from_u16(raw: u16) -> Option<Opcode> {
                match raw {
                    $( v if v == Opcode::$variant as u16 => Some(Opcode::$variant), )*
                    _ => None,
                }
            }
            /// The opcode's mnemonic (diagnostics/tests).
            pub fn name(self) -> &'static str {
                match self { $( Opcode::$variant => stringify!($variant), )* }
            }
            /// Default result type when an instruction has no explicit
            /// `result_type` and no constant (see `infer_instruction_type`).
            pub fn default_result_type(self) -> Option<ValueType> {
                match self { $( Opcode::$variant => declare_opcodes!(@ty $ty), )* }
            }
        }
    };
    (@ty bool) => { Some(ValueType::Bool) };
    (@ty f64)  => { Some(ValueType::F64) };
    (@ty u64)  => { Some(ValueType::U64) };
    (@ty none) => { None };
}

declare_opcodes! {
    Nop = 0 => u64,
    Const = 1 => none,
    Add = 16 => f64,
    Sub = 17 => f64,
    Mul = 18 => f64,
    Div = 19 => f64,
    Rem = 20 => f64,
    /// Unary transcendental / elementary functions (single f64 operand).
    Sin = 128 => f64,
    Cos = 129 => f64,
    Exp = 130 => f64,
    Ln = 131 => f64,
    Sqrt = 132 => f64,
    /// `pow(base, exponent)`.
    Pow = 133 => f64,
    /// Fused multiply-add superinstruction: `(a*b) + c` with the **same two
    /// roundings** as `Mul` then `Add` (not a single-rounding `mul_add`), so it
    /// is bit-identical to the two-instruction form. Emitted by the optimizer
    /// (`opt::optimize`) to cut one dispatch per fused pair.
    Fma = 235 => f64,
    Eq = 32 => bool,
    Ne = 33 => bool,
    Lt = 34 => bool,
    Le = 35 => bool,
    Gt = 36 => bool,
    Ge = 37 => bool,
    /// Select: if cond (operand 0) is nonzero, result = operand 1 else operand 2.
    Select = 40 => u64,
    /// Intra-module call: operand 0 = target function id, operands 1.. = argument
    /// value ids. Result = the callee's return value (its `Return` operand 0).
    Call = 48 => u64,
    /// Read one scalar field of a component into the value stack (field offset
    /// is a compile-time operand). RFC-0021 `READ_VIEW=64`. Yields the field's
    /// f64/u64 value.
    ReadView = 64 => f64,
    /// Write one scalar field of a component from the value stack.
    /// RFC-0021 `WRITE_VIEW=65`.
    WriteView = 65 => u64,
    Load = 66 => u64,
    Store = 67 => u64,
    /// Atomic read-modify-write on a component field (add). Reads the field,
    /// writes field + rhs, yields the old value. Effect `ATOMIC`.
    Atomic = 68 => u64,
    /// Emit an ordered event `(kind, payload)` (RFC-0023). Operands:
    /// `kind`, `payload`. Effect `IO`.
    EmitEvent = 69 => u64,
    /// Read the explicit sim time from the execution context. Effect `TIME`.
    Time = 70 => f64,
    /// Draw a (deterministically seeded) random value. Effect `RANDOM`.
    Random = 71 => f64,
    /// External input/output through the execution context. Effect `IO`.
    Io = 72 => f64,
    // -- extended math (unary f64 -> f64) --
    Abs = 192 => f64,
    Floor = 193 => f64,
    Ceil = 194 => f64,
    Round = 195 => f64,
    Sign = 196 => f64,
    Log10 = 197 => f64,
    Log2 = 198 => f64,
    Sinh = 199 => f64,
    Cosh = 200 => f64,
    Tanh = 201 => f64,
    Asin = 202 => f64,
    Acos = 203 => f64,
    Atan = 204 => f64,
    // -- extended math (binary f64 x f64 -> f64) --
    Atan2 = 205 => f64,
    Hypot = 206 => f64,
    /// Debugging `print`: logs operand 0 to the execution context and yields it
    /// back unchanged (semantically transparent). No effect bit — a debug
    /// side-channel that never changes world state.
    Print = 207 => f64,
    // -- spatial queries (read-only, deterministic; served by the EirRuntime) --
    /// Count the entities (other than the target entity) whose position lies
    /// within `radius` of the target entity's position. Operand 0 = radius
    /// value id; target = the querying entity. Yields the count as f64.
    NeighborCount = 208 => f64,
    /// Distance to the nearest entity other than the target entity; `f64::MAX`
    /// when the target entity is alone. Target = the querying entity. Yields
    /// the distance as f64.
    NearestDist = 209 => f64,
    /// Read the host-advanced step counter from the execution context.
    /// Deterministic (advances by 1 per step); no effect bit.
    Step = 210 => f64,
    // -- dynamic slot access (runtime index into the State component) --
    /// Read the State slot at a runtime index. Operand 0 = index value id;
    /// target = the owning entity (component = the State component). Yields
    /// the slot's f64 value. The byte offset is `index * STATE_SLOT_STRIDE`.
    ReadSlotDyn = 211 => f64,
    /// Write the State slot at a runtime index. Operand 0 = index value id,
    /// operand 1 = value id; target = the owning entity.
    WriteSlotDyn = 212 => u64,
    // -- grid field access (the PDE substrate; runtime cell coordinates) --
    /// Read a grid field cell. Operand 0 = i value id, operand 1 = j value id;
    /// target = ComponentRef whose component is the field's canonical id and
    /// whose offset is the field's width (compile-time, from the model).
    /// Yields the cell's f64 value.
    ReadFieldCell = 213 => f64,
    /// Write a grid field cell. Operand 0 = i, operand 1 = j, operand 2 =
    /// value id; target as for `ReadFieldCell`.
    WriteFieldCell = 214 => u64,
    /// Discrete Laplacian of a grid field cell (the Field's zero-flux stencil).
    /// Operand 0 = i, operand 1 = j; target as for `ReadFieldCell`. Yields f64.
    FieldLaplacian = 215 => f64,
    /// Read the payload of the most recent event with a given kind, from the
    /// events emitted so far in this interpretation. Operand 0 = kind value
    /// id. Yields the payload as f64, or 0.0 when no event of that kind has
    /// been emitted (deterministic reverse scan).
    ReadEvent = 216 => f64,
    // -- neighborhood aggregates / directional sensing (spatial queries v2) --
    /// Mean of a State slot over the neighbors within `radius` of the target
    /// entity (0.0 when there are none). Operands: slot value id, radius value
    /// id. Target = the sensing entity.
    NeighborMean = 217 => f64,
    /// X component of `(nearest neighbor position - target entity position)`;
    /// 0.0 when the target entity is alone. Target = the sensing entity.
    NearestOffsetX = 218 => f64,
    /// Y component of the nearest-neighbor offset (see `NearestOffsetX`).
    NearestOffsetY = 219 => f64,
    /// Z component of the nearest-neighbor offset (see `NearestOffsetX`).
    NearestOffsetZ = 220 => f64,
    // -- scheduled events (discrete-event scheduling on the step grid) --
    /// Fires (1.0) exactly in the one step whose time window
    /// `[time, time + step_dt)` contains the instant `T`; 0.0 otherwise.
    /// Operand 0 = T. Exact-once, stateless, deterministic.
    FiredAt = 221 => f64,
    /// Fires (1.0) in the step whose window contains a periodic instant
    /// `phase + k·period`; 0.0 otherwise. Operands: period, phase (default 0).
    FiredEvery = 222 => f64,
    /// Schedule an event `(kind, payload)` to fire `delay` seconds from now,
    /// but only when `gate` (operand 0) is nonzero. Operands: gate, delay,
    /// kind, payload. Yields nothing. Gate on a per-step pulse (`at`/`periodic`
    /// or `last_event`) to schedule exactly once.
    ScheduleEvent = 223 => u64,
    /// RFC-0048 slice C1: like `ScheduleEvent`, but the new entry carries an
    /// explicit **priority** (operand 4, truncated to i64) and the calendar is
    /// ordered by `(time, priority, seq)` — lower priority first, ties by
    /// insertion. Operands: gate, delay, kind, payload, priority. Yields
    /// nothing.
    ScheduleEventAt = 246 => u64,
    /// RFC-0048 slice C1: the priority of the earliest pending calendar entry,
    /// or 0.0 when empty. No operands; result F64.
    NextEventPriority = 247 => f64,
    /// RFC-0048 slice C2: capacity-gated resources (`seize`/`release`). The
    /// resource id rides in `constant` (`Immediate::U64`); the busy count lives
    /// in the execution context's resource table (parallel to the event
    /// calendar), so both backends derive it identically and `step_cross`
    /// compares it byte-for-byte. `SeizeResource` is non-blocking: operand 0 is
    /// the capacity, and it returns 1.0 and increments `busy` when
    /// `busy < capacity`, else 0.0.
    SeizeResource = 248 => f64,
    /// RFC-0048 slice C2: release one unit (saturating at 0); returns the
    /// remaining busy count. Resource id in `constant`; no operands.
    ReleaseResource = 249 => f64,
    /// RFC-0048 slice C2: current busy count (0 when unknown). No operands;
    /// resource id in `constant`; result F64.
    ResourceBusy = 250 => f64,
    /// RFC-0048 slice C2: the declared capacity (0 when unknown). No operands;
    /// resource id in `constant`; result F64.
    ResourceCapacity = 251 => f64,
    /// RFC-0044 (deferred item, landed): a runtime array-index bound check.
    /// Operands: index, base, len. `index` is the 0-based position inside the
    /// array; `base` is the array's first State slot and `len` its declared
    /// length. Traps (detail 18, the RFC-0021 load-class trap) when `index` is
    /// not a finite integer in `[0, len)`, else yields the absolute slot
    /// `base + index`. Lets a runtime-indexed `name[i]` read/write be
    /// bounds-checked before the slot is touched.
    BoundsCheck = 252 => f64,
    /// RFC-0037: one Jacobi diffusion sweep `T += rate·∇²T` over a grid field
    /// (the whole sweep in one instruction). Operand 0 = rate; target = field.
    FieldDiffuse = 224 => u64,
    /// RFC-0037: one leapfrog wave step over a grid field. Operands: the four
    /// little-endian `u32` limbs of the `prev` field id, then `cfl`, `damping`,
    /// `absorb`, `absorb_width`.
    FieldWave = 225 => u64,
    /// RFC-0037: `iters` Gauss–Seidel sweeps of `∇²φ = ρ·scale`. Operands: the
    /// four limbs of the `source` field id (all zero = none), `iters`, `scale`.
    FieldPoisson = 226 => u64,
    /// RFC-0038: the lowest-id inactive slot in a pool. Operands: the four
    /// little-endian `u32` limbs of the pool's first slot id, then the slot
    /// count. Result: the slot id, or 0 when the pool is full.
    FindFreeSlot = 227 => u64,
    /// RFC-0038: activate one free slot of a pool and copy the caller's state
    /// into it. Operands as for `FindFreeSlot`; target = the caller entity.
    /// Result: the slot id, or 0 when the pool is full. Emits the activation and
    /// state writes.
    SpawnInto = 228 => u64,
    /// RFC-0043: widen an exact `I64` value to `F64` (the coercion applied when
    /// an integer is used where a number is required). Operand 0 = the integer.
    I64ToF64 = 233 => f64,
    /// RFC-0043: convert a numeric value to `I64`, truncating toward zero and
    /// trapping (detail 18) on a non-finite input — the `i64(x)` cast and the
    /// implicit demotion at an integer-annotated `let`.
    F64ToI64 = 234 => none,
    /// `deriv(E)` history: read the previous (sub)step's value stored for a
    /// call site. The site id rides in `constant` (`Immediate::U64`). Result:
    /// the stored f64, or NaN when the site has no history yet.
    HistRead = 229 => f64,
    /// Store the current (sub)step's value for a `deriv(E)` call site so the
    /// next (sub)step can difference against it. Operand 0 = value; the site id
    /// rides in `constant` (`Immediate::U64`). Yields no SSA value.
    HistWrite = 230 => none,
    /// Whether a `deriv(E)` call site has history yet (1.0) or not (0.0). The
    /// site id rides in `constant`; result is F64. `deriv` gates its difference
    /// on this so the first (sub)step yields 0 instead of a NaN sentinel.
    HistHas = 231 => f64,
    /// Read a component field from the **committed** scene, ignoring
    /// in-interpretation writes. Target as `ReadView`; result is F64.
    ReadCommitted = 232 => f64,
    // -- RFC-0048 zero-crossing detection (runtime-owned per-site history) --
    /// RFC-0048: 1.0 on the (sub)step where the two operands change sign
    /// *strictly* (`prev ≠ 0 ∧ cur ≠ 0 ∧ sign(prev) ≠ sign(cur)`), else 0.0.
    /// Operands: `prev`, `cur`. Pure; the site's previous value is remembered
    /// by the lowerer via `HistWrite`, exactly like `deriv`.
    CrossDown = 236 => f64,
    /// RFC-0048: 1.0 on a strict upward transition (`prev ≤ 0 ∧ cur > 0`),
    /// else 0.0. Operands: `prev`, `cur`. Pure.
    RiseEdge = 237 => f64,
    /// RFC-0048: 1.0 on a strict downward transition (`prev ≥ 0 ∧ cur < 0`),
    /// else 0.0. Operands: `prev`, `cur`. Pure.
    FallEdge = 238 => f64,
    /// RFC-0048: simulation time of the most recent crossing at a call site
    /// (0.0 before any crossing). The site id rides in `constant`
    /// (`Immediate::U64`); result is F64. Pure.
    LastCross = 239 => f64,
    // -- RFC-0048 slice B: the event calendar as a first-class value --
    /// Number of *pending* (not-yet-due) calendar entries at the current step.
    /// No operands; result F64.
    EventCount = 240 => f64,
    /// Simulation time of the earliest pending calendar entry, or `f64::MAX`
    /// when the calendar is empty (the same "no entity" sentinel
    /// `NearestDist` uses, so the value stays finite). No operands; F64.
    NextEventTime = 241 => f64,
    /// Kind of the earliest pending calendar entry, or 0.0 when empty. No
    /// operands; result F64.
    NextEventKind = 242 => f64,
    /// Payload of the earliest pending calendar entry (f64 bit pattern), or 0.0
    /// when empty. No operands; result F64.
    NextEventPayload = 243 => f64,
    /// Pop (remove) the earliest pending calendar entry and yield its payload
    /// bit pattern as F64, or 0.0 when empty. No operands. Mutates the calendar.
    PopEvent = 244 => f64,
    /// Number of events emitted so far this step whose kind matches operand 0.
    EventSeenCount = 245 => f64,
    Return = 0x8000 => u64,
    /// Unconditional branch to an instruction index (block target). Single
    /// operand = target index.
    Br = 0x8001 => u64,
    /// Conditional branch: operand 0 = condition value id, operand 1 = true
    /// target index, operand 2 = false target index.
    CondBr = 0x8002 => u64,
    Trap = 0x8003 => u64,
    /// Marks a block that must not be reached (RFC-0021 UNREACHABLE). Traps.
    Unreachable = 0x8004 => u64,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub opcode: Opcode,
    pub result_id: u32,
    pub result_type: Option<ValueType>,
    pub operands: Vec<u32>,
    /// Inline constant payload for `Const`.
    pub constant: Option<Immediate>,
    /// Component access target for `ReadView`/`WriteView`.
    pub target: Option<ComponentRef>,
}

/// Addresses a scalar field within a component of an entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentRef {
    pub entity: u128,
    pub component: ComponentTypeId,
    /// Byte offset of the field within the component value.
    pub offset: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum Immediate {
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
}

/// A call frame's register file: a dense array indexed by SSA result id. The
/// interpreter used a `BTreeMap` here, which made every instruction a tree
/// operation; registers are small contiguous ids, so a dense array with a
/// presence bitmap is O(1) and allocation-light.
struct Regs {
    vals: Vec<Immediate>,
    def: Vec<bool>,
}

impl Regs {
    #[inline]
    fn new(cap: usize) -> Self {
        Self {
            vals: vec![Immediate::U64(0); cap],
            def: vec![false; cap],
        }
    }
    #[inline]
    fn get(&self, r: &u32) -> Option<&Immediate> {
        let i = *r as usize;
        if i < self.def.len() && self.def[i] {
            Some(&self.vals[i])
        } else {
            None
        }
    }
    #[inline]
    fn insert(&mut self, r: u32, v: Immediate) {
        let i = r as usize;
        if i >= self.vals.len() {
            self.vals.resize(i + 1, Immediate::U64(0));
            self.def.resize(i + 1, false);
        }
        self.vals[i] = v;
        self.def[i] = true;
    }
}

impl Immediate {
    fn ty(self) -> ValueType {
        match self {
            Immediate::I32(_) => ValueType::I32,
            Immediate::U32(_) => ValueType::U32,
            Immediate::I64(_) => ValueType::I64,
            Immediate::U64(_) => ValueType::U64,
            Immediate::F32(_) => ValueType::F32,
            Immediate::F64(_) => ValueType::F64,
            Immediate::Bool(_) => ValueType::Bool,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Function {
    pub id: u64,
    pub effect_mask: u32,
    /// Number of scalar arguments (SSA slots `1..=argument_count`), passed by a
    /// `CALL` (RFC-0021 per-function `argument_count`).
    pub argument_count: u32,
    pub instructions: Vec<Instruction>,
}

/// An ordered world write produced by the interpreter (RFC-0023 transaction
/// effect). Deterministic order follows instruction order. Addresses the scalar
/// field at `offset` within the `component` of `entity`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldWrite {
    pub entity: u128,
    pub component: ComponentTypeId,
    pub offset: u32,
    pub value: u64,
}

/// An ordered event produced by `EMIT_EVENT` (RFC-0023). Deterministic order
/// follows instruction order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmittedEvent {
    pub kind: u32,
    pub payload: u64,
    /// RFC-0048 slice B: monotonic insertion counter, so equal-time events keep
    /// a deterministic order across regions, snapshots, and backends.
    pub seq: u64,
}

/// A dynamically scheduled event: `(kind, payload)` to fire at simulation
/// time `time`. The queue is kept sorted by time (stable for ties), so due
/// events are delivered in deterministic order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub time: f64,
    pub kind: u32,
    pub payload: u64,
    /// RFC-0048 slice C1: queue discipline. The calendar is ordered by
    /// `(time, priority, seq)`: earlier time first, then lower priority, then
    /// insertion order. The plain `schedule(...)` uses priority 0, so its order
    /// is exactly the Slice-B `(time, seq)`.
    pub priority: i64,
    /// RFC-0048 slice B: monotonic insertion counter; ties in `time`/`priority`
    /// are broken by `seq` (never by float comparison), matching `EmittedEvent`.
    pub seq: u64,
}

/// RFC-0048 slice C2: the runtime state of one `resource` — its fixed capacity
/// and current busy count. Held in [`ExecEnv::resources`] keyed by the
/// compile-time resource id, exactly like the event calendar: execution-context
/// state, mutated only by `SeizeResource`/`ReleaseResource`, and compared
/// byte-for-byte by `step_cross` so the interpreter and JIT cannot diverge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceState {
    /// Maximum concurrent holders.
    pub capacity: i64,
    /// Current holders (`0..=capacity`).
    pub busy: i64,
}

/// A small deterministic PRNG (xorshift64*), seeded from a fixed domain constant
/// so that `RANDOM` is reproducible across runs (replay-stable) while remaining
/// a declared nondeterministic effect.
#[derive(Clone, Debug)]
pub struct SeededRng(u64);
impl SeededRng {
    pub const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
    pub fn new() -> Self {
        Self(Self::SEED)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
impl Default for SeededRng {
    fn default() -> Self {
        Self::new()
    }
}

/// The execution context for the nondeterministic opcodes (`TIME`, `RANDOM`,
/// `IO`, `EMIT_EVENT`). Defaults are deterministic: `time` is zero, the RNG is
/// fixed-seeded, I/O is a no-op, and events are collected into `events`.
#[derive(Clone, Debug)]
pub struct ExecEnv {
    /// Explicit simulation time read by `TIME`.
    pub time: f64,
    /// Deterministic source of randomness read by `RANDOM`.
    pub rng: SeededRng,
    /// Events emitted by `EMIT_EVENT`, in deterministic instruction order.
    pub events: Vec<EmittedEvent>,
    /// Values logged by `PRINT` (a debugging side-channel, in execution order).
    pub log: Vec<String>,
    /// Host-advanced step counter read by `STEP` (deterministic scheduling).
    pub step: u64,
    /// Host-set length of the current step's time window (seconds); the
    /// scheduled-event opcodes fire when `[time, time + step_dt)` covers a
    /// scheduled instant.
    pub step_dt: f64,
    /// Dynamic event queue (see [`ScheduledEvent`]); due events are moved into
    /// `events` at the start of each step, in time order.
    pub queue: Vec<ScheduledEvent>,
    /// Per-call-site history for the `deriv(E)` operator (site id -> previous
    /// (sub)step value). Persists across steps within a runtime; cleared on
    /// `reset`, so the first step's `deriv` is 0.
    pub hist: std::collections::BTreeMap<u64, f64>,
    /// RFC-0048: per-call-site timestamp of the most recent zero crossing
    /// (site id -> simulation time). Written by `CrossDown`/`RiseEdge`/
    /// `FallEdge` when they fire, read by `LastCross`.
    pub cross_time: std::collections::BTreeMap<u64, f64>,
    /// RFC-0048 slice B: monotonic insertion counter for the event calendar,
    /// so equal-time ties are ordered by insertion (`(time, seq)`).
    pub next_seq: u64,
    /// RFC-0048 slice C2: named resources (server capacity) keyed by the
    /// compile-time resource id. Execution-context state, like the calendar.
    pub resources: std::collections::BTreeMap<u64, ResourceState>,
}
impl Default for ExecEnv {
    fn default() -> Self {
        Self {
            time: 0.0,
            rng: SeededRng::new(),
            events: Vec::new(),
            log: Vec::new(),
            step: 0,
            step_dt: 0.0,
            queue: Vec::new(),
            hist: std::collections::BTreeMap::new(),
            cross_time: std::collections::BTreeMap::new(),
            next_seq: 0,
            resources: std::collections::BTreeMap::new(),
        }
    }
}

impl ExecEnv {
    /// The earliest pending calendar entry (the calendar is kept sorted by
    /// `(time, priority, seq)`), or `None` when empty.
    pub fn next_event(&self) -> Option<&ScheduledEvent> {
        self.queue.first()
    }

    /// Removes and returns the earliest pending calendar entry, or `None`.
    pub fn pop_next_event(&mut self) -> Option<ScheduledEvent> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }
}

/// Maximum CALL nesting depth. Exceeding it traps (guards against unbounded
/// or cyclic recursion).
pub const MAX_CALL_DEPTH: usize = 256;

/// Upper bound on retained `print(...)` log lines. The log persists across steps
/// (a debugging side-channel); older lines are dropped once the cap is reached
/// so a per-step `print` cannot grow memory without bound during a long run.
pub const MAX_LOG_LINES: usize = 65_536;

/// Moves every queued event whose time has been reached into `events`, in
/// time order. Called by the host at the start of each step (matching the
/// scheduled-event window semantics).
pub fn drain_due_events(env: &mut ExecEnv) {
    let now = env.time;
    let n = env.queue.partition_point(|e| e.time <= now);
    let due: Vec<EmittedEvent> = env
        .queue
        .drain(..n)
        .map(|e| EmittedEvent {
            kind: e.kind,
            payload: e.payload,
            seq: e.seq,
        })
        .collect();
    env.events.extend(due);
}

/// Byte stride of the canonical State component's scalar slots (fixed-width
/// f64 fields). `ReadSlotDyn`/`WriteSlotDyn` compute their runtime offset as
/// `index * STATE_SLOT_STRIDE`.
pub const STATE_SLOT_STRIDE: u32 = 8;

/// The world-access contract the interpreter executes against. A concrete
/// runtime supplies component field reads and accepts ordered writes; this is
/// how EIR drives high-level systems without bypassing the IR.
pub trait EirRuntime {
    /// Reads the scalar field at `ComponentRef::offset` of an entity's component,
    /// as a raw u64 (bits of an f64 or an integer). Sees any writes made earlier
    /// in the same interpretation.
    fn read_field(&self, target: ComponentRef) -> Result<u64>;
    /// Like [`read_field`](Self::read_field), but reads the **committed**
    /// (start-of-step) value, ignoring writes made earlier in the same
    /// interpretation. Cross-body reads (`nbody`, `@name`) use this so every
    /// body sees the same snapshot — preserving Newton's third law and
    /// momentum conservation. Defaults to `read_field` for runtimes that
    /// cannot distinguish the two.
    fn read_committed_field(&self, target: ComponentRef) -> Result<u64> {
        self.read_field(target)
    }
    /// Advances the committed (start-of-system) snapshot: all writes made so far
    /// in this interpretation become visible to subsequent committed reads. The
    /// engine emits a `Barrier` at each system boundary. Default: no-op.
    fn commit_barrier(&mut self) {}
    /// Applies a scalar write during interpretation so later reads observe it.
    fn write_field(&mut self, target: ComponentRef, value: u64);
    /// Counts the entities (other than `entity`) whose position lies within
    /// `radius` of `entity`'s position. Deterministic: entities are scanned in
    /// sorted id order, and positions observe in-interpretation writes.
    fn query_neighbor_count(&self, entity: u128, radius: f64) -> Result<u64>;
    /// Distance to the nearest entity other than `entity`; `f64::MAX` when
    /// there is no other entity. Same determinism as `query_neighbor_count`.
    fn query_nearest_dist(&self, entity: u128) -> Result<u64>;
    /// Mean of the State slot `slot` over the entities (other than `entity`)
    /// within `radius` of `entity`'s position; 0.0 when there are none. Same
    /// determinism as `query_neighbor_count`.
    fn query_neighbor_mean(&self, entity: u128, slot: u32, radius: f64) -> Result<f64>;
    /// `(nearest neighbor position - entity position)`; `(0, 0, 0)` when the
    /// entity is alone. Same determinism as `query_neighbor_count`.
    fn query_nearest_offset(&self, entity: u128) -> Result<(f64, f64, f64)>;
    /// Discrete Laplacian of a grid field cell (the Field's zero-flux
    /// stencil) for the field identified by `component`, at cell `(i, j)` on
    /// a grid of the given `width`. Deterministic.
    fn field_laplacian(
        &self,
        component: ComponentTypeId,
        i: f64,
        j: f64,
        width: f64,
    ) -> Result<f64>;
    /// RFC-0037: one Jacobi diffusion sweep `T += rate·∇²T` over the grid field
    /// `component`, in place. Runtimes that do not implement bulk sweeps return
    /// an error; the reference interpreter is the semantic oracle.
    fn field_diffuse(&mut self, _component: ComponentTypeId, _rate: f64) -> Result<()> {
        Err(error(Status::Internal, 0, 0))
    }
    /// RFC-0037: one leapfrog wave step over `component`, shifting `prev`.
    fn field_wave(
        &mut self,
        _component: ComponentTypeId,
        _prev: ComponentTypeId,
        _cfl: f64,
        _damping: f64,
        _absorb: f64,
        _absorb_width: f64,
    ) -> Result<()> {
        Err(error(Status::Internal, 0, 0))
    }
    /// RFC-0037: `iters` Gauss–Seidel sweeps of `∇²φ = ρ·scale` over `component`.
    fn field_poisson(
        &mut self,
        _component: ComponentTypeId,
        _source: Option<ComponentTypeId>,
        _iters: u32,
        _scale: f64,
    ) -> Result<()> {
        Err(error(Status::Internal, 0, 0))
    }
    /// RFC-0038: the lowest-id inactive slot in the pool `base..base+count`, or
    /// `0.0` when full. Deterministic ascending scan.
    fn find_free_slot(&self, _base: u128, _count: u32) -> Result<f64> {
        Err(error(Status::Internal, 0, 0))
    }
    /// RFC-0038: activate one free slot and copy `caller`'s state into it.
    /// Returns the slot id (`0.0` when full) and the ordered writes.
    fn spawn_into(
        &mut self,
        _base: u128,
        _count: u32,
        _caller: u128,
    ) -> Result<(f64, Vec<WorldWrite>)> {
        Err(error(Status::Internal, 0, 0))
    }
}

#[derive(Clone, Debug)]
pub struct EirModule {
    pub module_hash: Hash256,
    pub schema_set_hash: Hash256,
    pub domain_ir_hash: Hash256,
    pub target_kind: u16,
    pub functions: Vec<Function>,
}

/// Precomputed execution order and function-id -> index map for an
/// [`EirModule`] (see [`EirModule::prepare_index`]).
pub(crate) struct CallIndex {
    pub(crate) order: Vec<usize>,
    pub(crate) index_of: std::collections::HashMap<u64, usize>,
}

/// The SSA type of an instruction's result: an explicit annotation, else a
/// constant's type, else an opcode-inferred default (`Const` yields the
/// constant's type; comparisons and unknown ops default as before).
fn infer_instruction_type(i: &Instruction) -> Option<ValueType> {
    if let Some(t) = i.result_type {
        return Some(t);
    }
    if let Some(t) = i.constant.map(Immediate::ty) {
        return Some(t);
    }
    i.opcode.default_result_type()
}

/// All operands of an arithmetic instruction must share one numeric type.
fn numeric_type(
    reg_types: &[Option<ValueType>],
    operands: &[u32],
    index: usize,
) -> Result<ValueType> {
    if operands.is_empty() {
        return Err(error(Status::EirInvalid, 11, index));
    }
    let first = reg_types
        .get(operands[0] as usize)
        .copied()
        .flatten()
        .ok_or(error(Status::EirInvalid, 12, index))?;
    for &id in operands.iter().skip(1) {
        if reg_types.get(id as usize).copied().flatten() != Some(first) {
            return Err(error(Status::EirInvalid, 13, index));
        }
    }
    if !matches!(
        first,
        ValueType::I32
            | ValueType::U32
            | ValueType::I64
            | ValueType::U64
            | ValueType::F32
            | ValueType::F64
    ) {
        return Err(error(Status::EirInvalid, 14, index));
    }
    Ok(first)
}

impl EirModule {
    /// Validates SSA order, single definition, types, and declared effects.
    /// `pure` marks the module as deterministic (no time/random/io/device/
    /// network effects permitted).
    pub fn validate(&self, pure: bool) -> Result<()> {
        let fn_ids: std::collections::HashSet<u64> = self.functions.iter().map(|f| f.id).collect();
        for function in &self.functions {
            if pure && function.effect_mask & NONDETERMINISTIC != 0 {
                return Err(error(Status::EirInvalid, 1, function.id as usize));
            }
            self.validate_function(function, &fn_ids)?;
        }
        Ok(())
    }

    /// ORs each opcode's required effect bit into its function's `effect_mask`.
    /// Used by high-level lowering (e.g. the language) so that functions using
    /// `RANDOM`/`TIME`/`IO`/`ATOMIC`/`EMIT_EVENT` declare those effects without
    /// every lowering site tracking them by hand.
    pub fn apply_required_effects(&mut self) {
        for function in &mut self.functions {
            let mut required: u32 = 0;
            for ins in &function.instructions {
                required |= match ins.opcode {
                    Opcode::Atomic => EIR_EFFECT_ATOMIC,
                    Opcode::EmitEvent | Opcode::Io => EIR_EFFECT_IO,
                    Opcode::Time => EIR_EFFECT_TIME,
                    Opcode::Random => EIR_EFFECT_RANDOM,
                    _ => 0,
                };
            }
            function.effect_mask |= required;
        }
    }

    /// Runs the RFC-0021 block/dominance verifier over each function's
    /// straight-line instruction list (one block per function). This is the
    /// "one definition/value, dominance, block targets" gate that the AOT and
    /// JIT compile paths enforce before linking; the interpreter remains the
    /// semantic reference and is untouched.
    pub fn verify_linear_dominance(&self) -> Result<()> {
        for function in &self.functions {
            let blocks = blocks_from_instructions(&function.instructions);
            crate::dominance::verify_dominance_with_args(
                &function.instructions,
                &blocks,
                function.argument_count,
            )?;
        }
        Ok(())
    }

    /// RFC-0046: the **explicit basic blocks** of function `index` — a
    /// first-class view of the CFG (block ranges + terminators), derived
    /// deterministically from the instruction stream.
    pub fn blocks(&self, index: usize) -> Option<Vec<crate::dominance::Block>> {
        self.functions
            .get(index)
            .map(|f| blocks_from_instructions(&f.instructions))
    }

    /// RFC-0046: runs the explicit-CFG dominance gate for every function (the
    /// same check the AOT/JIT link paths enforce). Thin alias of
    /// [`EirModule::verify_linear_dominance`], which now uses real blocks.
    pub fn verify_cfg(&self) -> Result<()> {
        self.verify_linear_dominance()
    }

    fn validate_function(
        &self,
        function: &Function,
        fn_ids: &std::collections::HashSet<u64>,
    ) -> Result<()> {
        // Dense SSA tables indexed by register id (registers are small contiguous
        // ids), sized to the largest id referenced. Dense arrays keep validation
        // linear with a small constant (BTreeMap/HashMap tables were the dominant
        // compile cost for large unrolled systems).
        // Registers are result ids (and pre-defined argument slots); operand
        // ids always reference an earlier result, so sizing from result ids is
        // sufficient. (A `Call`'s first operand is a *function* id — up to
        // 0xF000_0000 — and must not inflate the table.)
        let mut cap = function.argument_count as usize + 1;
        for ins in &function.instructions {
            cap = cap.max(ins.result_id as usize + 1);
        }
        // defs[id] = (type, defined). reg_types[id] = inferable type.
        let mut defs: Vec<Option<(ValueType, bool)>> = vec![None; cap];
        let mut reg_types: Vec<Option<ValueType>> = vec![None; cap];
        let mut result_count = 0usize;
        // Required effect bits the function must declare (RFC-0021 effect mask).
        let mut required_effects: u32 = 0;
        // Function arguments are pre-defined SSA slots (typed F64 by default,
        // matching the reference's F64-centric component values).
        for slot in 1..=function.argument_count {
            let i = slot as usize;
            defs[i] = Some((ValueType::F64, true));
            reg_types[i] = Some(ValueType::F64);
        }
        for ins in &function.instructions {
            if ins.result_id != 0 {
                if let Some(t) = infer_instruction_type(ins) {
                    if let Some(slot) = reg_types.get_mut(ins.result_id as usize) {
                        *slot = Some(t);
                    }
                }
            }
        }

        for (index, instruction) in function.instructions.iter().enumerate() {
            let op: Opcode = instruction.opcode;
            required_effects |= match op {
                Opcode::Atomic => EIR_EFFECT_ATOMIC,
                Opcode::EmitEvent | Opcode::Io => EIR_EFFECT_IO,
                Opcode::Time => EIR_EFFECT_TIME,
                Opcode::Random => EIR_EFFECT_RANDOM,
                _ => 0,
            };
            let declared_type = match op {
                Opcode::Nop => None,
                Opcode::Const => Some(
                    instruction
                        .constant
                        .ok_or(error(Status::EirInvalid, 3, index))?
                        .ty(),
                ),
                Opcode::Store
                | Opcode::Return
                | Opcode::Trap
                | Opcode::Br
                | Opcode::CondBr
                | Opcode::Unreachable
                | Opcode::EmitEvent => None,
                Opcode::Call => {
                    // operand 0 = target function id (must exist), operands
                    // 1.. = argument value ids. Result type follows the callee's
                    // return value (the annotated type; U64 by default).
                    if instruction.operands.is_empty() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    let target_id = instruction.operands[0] as u64;
                    if !fn_ids.contains(&target_id) {
                        return Err(error(Status::EirInvalid, 37, index));
                    }
                    Some(instruction.result_type.unwrap_or(ValueType::U64))
                }
                Opcode::Atomic => {
                    // target component field + one rhs value id; yields old value.
                    if instruction.operands.len() != 1 || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::U64)
                }
                Opcode::Time | Opcode::Random | Opcode::Io => Some(ValueType::F64),
                Opcode::I64ToF64 => {
                    // RFC-0043: exactly one integer operand; result is f64.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    if reg_types
                        .get(instruction.operands[0] as usize)
                        .copied()
                        .flatten()
                        != Some(ValueType::I64)
                    {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::F64ToI64 => {
                    // RFC-0043: exactly one numeric operand; result is i64.
                    // (`F64ToI64` truncates toward zero and traps on a
                    // non-finite input; `numeric_type` rejects e.g. a Bool.)
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    numeric_type(&reg_types, &instruction.operands, index)?;
                    Some(ValueType::I64)
                }
                Opcode::Print => {
                    // `print(x)`: exactly one f64 operand; result is that value
                    // (transparent). Logs to the execution context.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    let t = reg_types
                        .get(instruction.operands[0] as usize)
                        .copied()
                        .flatten();
                    if t != Some(ValueType::F64) {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::Add
                | Opcode::Sub
                | Opcode::Mul
                | Opcode::Div
                | Opcode::Rem
                | Opcode::Pow => {
                    // arithmetic: operands must be a consistent numeric type
                    // (a mixed integer/float pair widens to f64).
                    let ty = numeric_type(&reg_types, &instruction.operands, index)?;
                    Some(ty)
                }
                Opcode::Fma => {
                    // fused multiply-add: exactly three f64 operands, result f64.
                    if instruction.operands.len() != 3 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    for &id in &instruction.operands {
                        if reg_types.get(id as usize).copied().flatten() != Some(ValueType::F64) {
                            return Err(error(Status::EirInvalid, 13, index));
                        }
                    }
                    Some(ValueType::F64)
                }
                Opcode::Atan2 | Opcode::Hypot => {
                    // binary f64 function: exactly two f64 operands, result f64.
                    if instruction.operands.len() != 2 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    let a = reg_types
                        .get(instruction.operands[0] as usize)
                        .copied()
                        .flatten();
                    let b = reg_types
                        .get(instruction.operands[1] as usize)
                        .copied()
                        .flatten();
                    if a != Some(ValueType::F64) || b != Some(ValueType::F64) {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::Sin
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
                | Opcode::Atan => {
                    // unary f64 function: exactly one f64 operand.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    if reg_types
                        .get(instruction.operands[0] as usize)
                        .copied()
                        .flatten()
                        != Some(ValueType::F64)
                    {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
                    numeric_type(&reg_types, &instruction.operands, index)?;
                    Some(ValueType::Bool)
                }
                Opcode::Load => {
                    // load: one address operand (integer), yields stored type.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::U64)
                }
                Opcode::ReadView | Opcode::ReadCommitted => Some(ValueType::F64),
                Opcode::NeighborCount | Opcode::NearestDist => {
                    // Spatial queries read the world via the runtime; the
                    // target entity is a compile-time component reference.
                    let want = usize::from(instruction.opcode == Opcode::NeighborCount);
                    if instruction.operands.len() != want {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::Step => {
                    if !instruction.operands.is_empty() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::ReadEvent => {
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::NeighborMean => {
                    if instruction.operands.len() != 2 || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::NearestOffsetX | Opcode::NearestOffsetY | Opcode::NearestOffsetZ => {
                    if !instruction.operands.is_empty() || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::FiredAt => {
                    if instruction.operands.len() != 1 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::FiredEvery => {
                    if instruction.operands.is_empty()
                        || instruction.operands.len() > 2
                        || instruction.target.is_some()
                    {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::ScheduleEvent => {
                    if instruction.operands.len() != 4 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    None
                }
                Opcode::ScheduleEventAt => {
                    if instruction.operands.len() != 5 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    None
                }
                Opcode::HistRead => {
                    if !instruction.operands.is_empty() || instruction.constant.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::HistWrite => {
                    if instruction.operands.len() != 1 || instruction.constant.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    None
                }
                Opcode::HistHas => {
                    if !instruction.operands.is_empty() || instruction.constant.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::CrossDown | Opcode::RiseEdge | Opcode::FallEdge => {
                    if instruction.operands.len() != 3 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    // The optional `constant` is the call site id (used to
                    // timestamp the crossing for `LastCross`).
                    if instruction.constant.is_some()
                        && !matches!(instruction.constant, Some(Immediate::U64(_)))
                    {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::LastCross => {
                    if !instruction.operands.is_empty() || instruction.constant.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::EventCount
                | Opcode::NextEventTime
                | Opcode::NextEventKind
                | Opcode::NextEventPayload
                | Opcode::NextEventPriority
                | Opcode::PopEvent => {
                    // Calendar reads/pops take no operands and read state from
                    // the execution context's pending queue.
                    if !instruction.operands.is_empty() || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::EventSeenCount => {
                    if instruction.operands.len() != 1 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::SeizeResource => {
                    // `seize(resource_id, capacity)`: operand 0 = capacity.
                    if instruction.operands.len() != 1
                        || instruction.target.is_some()
                        || instruction.constant.is_none()
                    {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::ReleaseResource | Opcode::ResourceBusy | Opcode::ResourceCapacity => {
                    if !instruction.operands.is_empty()
                        || instruction.target.is_some()
                        || instruction.constant.is_none()
                    {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::BoundsCheck => {
                    // RFC-0044: index, base, len — a runtime trap.
                    if instruction.operands.len() != 3 || instruction.target.is_some() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::ReadSlotDyn => {
                    if instruction.operands.len() != 1 || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::WriteSlotDyn => {
                    if instruction.operands.len() != 2 || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    None
                }
                Opcode::ReadFieldCell
                | Opcode::WriteFieldCell
                | Opcode::FieldLaplacian
                | Opcode::FieldDiffuse
                | Opcode::FieldWave
                | Opcode::FieldPoisson => {
                    // Grid field access: the field's width rides in the
                    // target's offset (compile-time, from the model). The bulk
                    // solver opcodes (RFC-0037) run the whole sweep natively and
                    // yield no SSA value.
                    let want = match instruction.opcode {
                        Opcode::WriteFieldCell => 3,
                        Opcode::FieldDiffuse => 1,
                        Opcode::FieldWave => 8,
                        Opcode::FieldPoisson => 6,
                        _ => 2,
                    };
                    if instruction.operands.len() != want || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    if matches!(
                        instruction.opcode,
                        Opcode::WriteFieldCell
                            | Opcode::FieldDiffuse
                            | Opcode::FieldWave
                            | Opcode::FieldPoisson
                    ) {
                        None
                    } else {
                        Some(ValueType::F64)
                    }
                }
                Opcode::FindFreeSlot | Opcode::SpawnInto => {
                    if instruction.operands.len() != 5 || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::WriteView => None,
                Opcode::Select => {
                    if instruction.operands.len() != 3 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    // Result type follows the selected operands.
                    let a = reg_types
                        .get(instruction.operands[1] as usize)
                        .copied()
                        .flatten();
                    let b = reg_types
                        .get(instruction.operands[2] as usize)
                        .copied()
                        .flatten();
                    if a != b || a.is_none() {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    a
                }
            };

            // Operands must be defined before use (program-order SSA monotonicity):
            // SSA IDs are monotonic, so rejecting use of a not-yet-defined id
            // and requiring operands be < current result_id enforces this.
            // Branch-target operands are instruction indices, not SSA value ids.
            let n = function.instructions.len();
            for (op_pos, &operand) in instruction.operands.iter().enumerate() {
                // Operand 0 is the null-result sentinel (e.g. a Store address),
                // not an SSA value use.
                if operand == 0 {
                    continue;
                }
                let is_target = match op {
                    Opcode::Br => true,
                    Opcode::CondBr => op_pos != 0,
                    Opcode::Call => op_pos == 0, // target function id, not a value
                    _ => false,
                };
                if is_target {
                    if op == Opcode::Call {
                        continue; // target id already range-checked by existence
                    }
                    if operand as usize >= n {
                        return Err(error(Status::EirInvalid, 31, index));
                    }
                    continue;
                }
                let defined = defs
                    .get(operand as usize)
                    .and_then(|o| *o)
                    .map(|(_, d)| d)
                    .unwrap_or(false);
                if !defined {
                    return Err(error(Status::EirInvalid, 5, index));
                }
            }

            // Record this instruction's result definition.
            if let Some(result_id) = non_zero(instruction.result_id) {
                let ty = declared_type.ok_or(error(Status::EirInvalid, 6, index))?;
                if let Some(Some((_, already))) = defs.get(result_id as usize) {
                    if *already {
                        return Err(error(Status::EirInvalid, 7, index));
                    }
                }
                // result_type annotation, if present, must agree with the
                // computed declared_type.
                if let Some(annotated) = instruction.result_type {
                    if annotated != ty {
                        return Err(error(Status::EirInvalid, 8, index));
                    }
                }
                if let Some(slot) = defs.get_mut(result_id as usize) {
                    *slot = Some((ty, true));
                }
                result_count += 1;
            }
        }

        // RFC-0021 effect mask: a function that uses ATOMIC/TIME/RANDOM/IO/
        // EMIT_EVENT must declare the corresponding effect bit, or it is invalid.
        if function.effect_mask & required_effects != required_effects {
            return Err(error(Status::EirInvalid, 38, function.id as usize));
        }

        // RFC-0021 control-flow gate: partition the instruction stream into basic
        // blocks at every branch target, then run the CFG dominance verifier.
        // This rejects unknown/out-of-order targets, terminator-less blocks,
        // and uses not dominated by their defining block.
        crate::dominance::verify_dominance_with_args(
            &function.instructions,
            &blocks_from_instructions(&function.instructions),
            function.argument_count,
        )?;

        let _ = result_count;
        Ok(())
    }

    /// Deterministic interpretation: executes each function in id order and
    /// returns ordered world writes. Uses a no-op runtime and a default
    /// deterministic execution context. Validates the module as pure
    /// (deterministic); modules using TIME/RANDOM/IO/EMIT_EVENT are rejected.
    pub fn interpret(&self, world: WorldId, version: WorldVersion) -> Result<Vec<WorldWrite>> {
        self.validate(true)?;
        let mut env = ExecEnv::default();
        self.execute(&mut NoopRuntime, &mut env, world, version)
    }

    /// Deterministic interpretation against a concrete `EirRuntime` with a
    /// default execution context. Validates as pure; rejects nondeterministic
    /// effect opcodes.
    pub fn interpret_with(
        &self,
        rt: &mut dyn EirRuntime,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        self.validate(true)?;
        let mut env = ExecEnv::default();
        self.execute(rt, &mut env, world, version)
    }

    /// Interpretation against a concrete `EirRuntime` and an explicit execution
    /// context (`TIME`/`RANDOM`/`IO`/`EMIT_EVENT` read or write `env`). Validates
    /// structurally (non-pure), so nondeterministic-opcode modules run; their
    /// reproducibility comes from the seeded, caller-supplied `env`.
    pub fn interpret_with_env(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        self.validate(false)?;
        self.execute(rt, env, world, version)
    }

    /// Executes the module without re-validating. The public entry points
    /// validate with the appropriate purity before delegating here. Callers
    /// that step the same immutable module repeatedly (the language runtime)
    /// validate once and then use this directly.
    /// The deterministic execution order (functions sorted by id) and a
    /// function-id -> index map, computed once so a steady-state step does not
    /// re-sort/re-hash the function table every call.
    pub(crate) fn prepare_index(&self) -> CallIndex {
        let mut order: Vec<usize> = (0..self.functions.len()).collect();
        order.sort_by_key(|&i| self.functions[i].id);
        let index_of: std::collections::HashMap<u64, usize> =
            order.iter().map(|&i| (self.functions[i].id, i)).collect();
        CallIndex { order, index_of }
    }

    pub(crate) fn execute(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        let _ = (world, version);
        let index = self.prepare_index();
        self.execute_with_index(rt, env, &index)
    }

    /// Executes all entry functions using a precomputed [`CallIndex`].
    pub(crate) fn execute_with_index(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        index: &CallIndex,
    ) -> Result<Vec<WorldWrite>> {
        let mut writes: Vec<WorldWrite> = Vec::new();
        for &entry in &index.order {
            // Only argument-less functions are entry points; functions that take
            // arguments are reached exclusively via `CALL`.
            if self.functions[entry].argument_count == 0 {
                // A system boundary: publish the previous systems' writes so this
                // system's committed (start-of-system) reads observe them.
                if self.functions[entry].effect_mask & EIR_EFFECT_BARRIER != 0 {
                    rt.commit_barrier();
                }
                self.run_call_tree(
                    rt,
                    env,
                    &self.functions,
                    &index.index_of,
                    entry,
                    &mut writes,
                )?;
            }
        }
        Ok(writes)
    }

    /// Executes a function (and, via `CALL`, its callee tree) starting at
    /// `entry`, appending writes/events. Uses a frame stack so nested calls
    /// return to their caller's return address; recursion is depth-capped.
    #[allow(clippy::too_many_arguments)]
    fn run_call_tree(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        functions: &[Function],
        index_of: &std::collections::HashMap<u64, usize>,
        entry: usize,
        writes: &mut Vec<WorldWrite>,
    ) -> Result<()> {
        let mut frames: Vec<usize> = vec![entry];
        let mut pcs: Vec<usize> = vec![0usize];
        // Register ids are dense; size the root frame to an upper bound.
        let root_cap =
            functions[entry].instructions.len() + functions[entry].argument_count as usize + 1;
        let mut stacks: Vec<Regs> = vec![Regs::new(root_cap)];
        loop {
            let depth = frames.len();
            let fi = frames[depth - 1];
            let pc = pcs[depth - 1];
            // Fell off the end of a function: treat as an implicit `Return`.
            if pc >= functions[fi].instructions.len() {
                return self.pop_return(&mut frames, &mut pcs, &mut stacks, functions);
            }
            let instruction = &functions[fi].instructions[pc];
            match instruction.opcode {
                Opcode::Const => {
                    stacks[depth - 1].insert(
                        instruction.result_id,
                        instruction
                            .constant
                            .ok_or(error(Status::EirInvalid, 15, 0))?,
                    );
                    pcs[depth - 1] += 1;
                }
                Opcode::Call => {
                    let target_id = instruction.operands[0] as u64;
                    let target =
                        *index_of
                            .get(&target_id)
                            .ok_or(error(Status::EirInvalid, 32, pc))?;
                    if depth + 1 > MAX_CALL_DEPTH {
                        return Err(error(Status::EirInvalid, 33, pc));
                    }
                    let callee_cap = functions[target].instructions.len()
                        + functions[target].argument_count as usize
                        + 1;
                    let mut callee = Regs::new(callee_cap);
                    {
                        let cur = &stacks[depth - 1];
                        for (slot, arg_id) in instruction.operands.iter().skip(1).enumerate() {
                            callee.insert(
                                (slot as u32) + 1,
                                *cur.get(arg_id).ok_or(error(Status::EirInvalid, 34, pc))?,
                            );
                        }
                    }
                    pcs[depth - 1] += 1; // return address
                    frames.push(target);
                    pcs.push(0);
                    stacks.push(callee);
                }
                Opcode::Return => {
                    let ret = if let Some(vid) = instruction.operands.first() {
                        stacks[depth - 1]
                            .get(vid)
                            .copied()
                            .unwrap_or(Immediate::U64(0))
                    } else {
                        Immediate::U64(0)
                    };
                    if depth == 1 {
                        return Ok(()); // top-level entry done
                    }
                    frames.pop();
                    pcs.pop();
                    stacks.pop();
                    let caller_fi = frames[depth - 2];
                    let caller_pc = pcs[depth - 2] - 1; // the CALL instruction
                    let caller_ins = &functions[caller_fi].instructions[caller_pc];
                    if let Some(rid) = non_zero(caller_ins.result_id) {
                        stacks[depth - 2].insert(rid, ret);
                    }
                }
                Opcode::Br => {
                    let target = instruction.operands[0] as usize;
                    pcs[depth - 1] = target;
                }
                Opcode::CondBr => {
                    let cond = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    pcs[depth - 1] = if as_u64(cond) != 0 {
                        instruction.operands[1] as usize
                    } else {
                        instruction.operands[2] as usize
                    };
                }
                Opcode::Trap | Opcode::Unreachable => return Ok(()),
                Opcode::Add
                | Opcode::Sub
                | Opcode::Mul
                | Opcode::Div
                | Opcode::Rem
                | Opcode::Eq
                | Opcode::Ne
                | Opcode::Lt
                | Opcode::Le
                | Opcode::Gt
                | Opcode::Ge
                | Opcode::Pow
                | Opcode::Select => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let b = stacks[depth - 1]
                        .get(&instruction.operands[1])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 17, 0))?;
                    let out = match instruction.opcode {
                        Opcode::Add | Opcode::Sub | Opcode::Mul => arith(instruction.opcode, a, b)
                            .ok_or(error(Status::EirInvalid, 18, 0))?,
                        Opcode::Div | Opcode::Rem => divrem(instruction.opcode, a, b)
                            .ok_or(error(Status::EirInvalid, 18, 0))?,
                        Opcode::Pow => Immediate::F64(as_f64(a).powf(as_f64(b))),
                        Opcode::Eq
                        | Opcode::Ne
                        | Opcode::Lt
                        | Opcode::Le
                        | Opcode::Gt
                        | Opcode::Ge => compare(instruction.opcode, a, b).ok_or(error(
                            Status::EirInvalid,
                            18,
                            0,
                        ))?,
                        Opcode::Select => {
                            let cond = a;
                            if as_u64(cond) != 0 {
                                b
                            } else {
                                let c = stacks[depth - 1]
                                    .get(&instruction.operands[2])
                                    .copied()
                                    .ok_or(error(Status::EirInvalid, 17, 0))?;
                                c
                            }
                        }
                        _ => unreachable!(),
                    };
                    stacks[depth - 1].insert(instruction.result_id, out);
                    pcs[depth - 1] += 1;
                }
                Opcode::I64ToF64 => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let out = Immediate::F64(as_i64(a) as f64);
                    stacks[depth - 1].insert(instruction.result_id, out);
                    pcs[depth - 1] += 1;
                }
                Opcode::F64ToI64 => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let x = as_f64(a);
                    if !x.is_finite() {
                        return Err(error(Status::EirInvalid, 18, 0));
                    }
                    stacks[depth - 1].insert(instruction.result_id, Immediate::I64(x as i64));
                    pcs[depth - 1] += 1;
                }
                Opcode::Fma => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let b = stacks[depth - 1]
                        .get(&instruction.operands[1])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 17, 0))?;
                    let c = stacks[depth - 1]
                        .get(&instruction.operands[2])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 18, 0))?;
                    let out = Immediate::F64(as_f64(a) * as_f64(b) + as_f64(c));
                    stacks[depth - 1].insert(instruction.result_id, out);
                    pcs[depth - 1] += 1;
                }
                Opcode::Sin
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
                | Opcode::Atan => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let x = as_f64(a);
                    let out = match instruction.opcode {
                        Opcode::Sin => x.sin(),
                        Opcode::Cos => x.cos(),
                        Opcode::Exp => x.exp(),
                        Opcode::Ln => x.ln(),
                        Opcode::Sqrt => x.sqrt(),
                        Opcode::Abs => x.abs(),
                        Opcode::Floor => x.floor(),
                        Opcode::Ceil => x.ceil(),
                        Opcode::Round => x.round(),
                        Opcode::Sign => x.signum(),
                        Opcode::Log10 => x.log10(),
                        Opcode::Log2 => x.log2(),
                        Opcode::Sinh => x.sinh(),
                        Opcode::Cosh => x.cosh(),
                        Opcode::Tanh => x.tanh(),
                        Opcode::Asin => x.asin(),
                        Opcode::Acos => x.acos(),
                        Opcode::Atan => x.atan(),
                        _ => unreachable!(),
                    };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(out));
                    pcs[depth - 1] += 1;
                }
                Opcode::Atan2 | Opcode::Hypot => {
                    let a = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let b = stacks[depth - 1]
                        .get(&instruction.operands[1])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 17, 0))?;
                    let out = match instruction.opcode {
                        Opcode::Atan2 => as_f64(a).atan2(as_f64(b)),
                        _ => as_f64(a).hypot(as_f64(b)),
                    };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(out));
                    pcs[depth - 1] += 1;
                }
                Opcode::Store => {
                    let address = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 19, 0))?;
                    let value = stacks[depth - 1]
                        .get(&instruction.operands[1])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 20, 0))?;
                    let component = component_from_addr(address);
                    writes.push(WorldWrite {
                        entity: 0,
                        component,
                        offset: 0,
                        value: as_u64(value),
                    });
                    pcs[depth - 1] += 1;
                }
                Opcode::ReadView => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let raw = rt.read_field(target)?;
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(f64::from_bits(raw)));
                    pcs[depth - 1] += 1;
                }
                Opcode::ReadCommitted => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let raw = rt.read_committed_field(target)?;
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(f64::from_bits(raw)));
                    pcs[depth - 1] += 1;
                }
                Opcode::WriteView => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 24, 0))?;
                    let value = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 25, 0))?;
                    rt.write_field(target, as_u64(value));
                    writes.push(WorldWrite {
                        entity: target.entity,
                        component: target.component,
                        offset: target.offset,
                        value: as_u64(value),
                    });
                    pcs[depth - 1] += 1;
                }
                Opcode::NeighborCount => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let radius = f64::from_bits(as_u64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ));
                    let n = rt.query_neighbor_count(target.entity, radius)?;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(n as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::NearestDist => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let d = f64::from_bits(rt.query_nearest_dist(target.entity)?);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(d));
                    pcs[depth - 1] += 1;
                }
                Opcode::Step => {
                    let s = env.step as f64;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(s));
                    pcs[depth - 1] += 1;
                }
                Opcode::NeighborMean => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let slot = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ) as u32;
                    let radius = f64::from_bits(as_u64(
                        stacks[depth - 1]
                            .get(&instruction.operands[1])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 17, 0))?,
                    ));
                    let m = rt.query_neighbor_mean(target.entity, slot, radius)?;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(m));
                    pcs[depth - 1] += 1;
                }
                Opcode::ScheduleEvent => {
                    let gate = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    if gate != 0.0 {
                        let delay = as_f64(
                            stacks[depth - 1]
                                .get(&instruction.operands[1])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 16, 0))?,
                        );
                        let kind = as_f64(
                            stacks[depth - 1]
                                .get(&instruction.operands[2])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 17, 0))?,
                        ) as u32;
                        let payload = as_u64(
                            stacks[depth - 1]
                                .get(&instruction.operands[3])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 18, 0))?,
                        );
                        let time = env.time + delay;
                        let seq = env.next_seq;
                        env.next_seq += 1;
                        // Keep the queue ordered by `(time, priority, seq)`:
                        // plain `schedule` uses priority 0, so equal-time
                        // entries land after every earlier insertion.
                        let pos = env
                            .queue
                            .partition_point(|e| (e.time, e.priority, e.seq) <= (time, 0, seq));
                        env.queue.insert(
                            pos,
                            ScheduledEvent {
                                time,
                                kind,
                                payload,
                                priority: 0,
                                seq,
                            },
                        );
                    }
                    pcs[depth - 1] += 1;
                }
                Opcode::ScheduleEventAt => {
                    let gate = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    if gate != 0.0 {
                        let delay = as_f64(
                            stacks[depth - 1]
                                .get(&instruction.operands[1])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 16, 0))?,
                        );
                        let kind = as_f64(
                            stacks[depth - 1]
                                .get(&instruction.operands[2])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 17, 0))?,
                        ) as u32;
                        let payload = as_u64(
                            stacks[depth - 1]
                                .get(&instruction.operands[3])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 18, 0))?,
                        );
                        let priority = as_i64(
                            stacks[depth - 1]
                                .get(&instruction.operands[4])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 19, 0))?,
                        );
                        let time = env.time + delay;
                        let seq = env.next_seq;
                        env.next_seq += 1;
                        let pos = env.queue.partition_point(|e| {
                            (e.time, e.priority, e.seq) <= (time, priority, seq)
                        });
                        env.queue.insert(
                            pos,
                            ScheduledEvent {
                                time,
                                kind,
                                payload,
                                priority,
                                seq,
                            },
                        );
                    }
                    pcs[depth - 1] += 1;
                }
                Opcode::FiredAt => {
                    let t = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let fired = env.time <= t && t < env.time + env.step_dt;
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(fired as u8 as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::FiredEvery => {
                    let period = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let phase = match instruction.operands.get(1) {
                        Some(op) => as_f64(stacks[depth - 1].get(op).copied().ok_or(error(
                            Status::EirInvalid,
                            17,
                            0,
                        ))?),
                        None => 0.0,
                    };
                    // The instant phase + k·period inside [time, time + step_dt),
                    // matching `FiredAt`'s half-open window exactly.
                    let fired = period > 0.0 && {
                        let k = ((env.time - phase) / period).ceil();
                        let instant = phase + k * period;
                        instant >= env.time && instant < env.time + env.step_dt
                    };
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(fired as u8 as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::NearestOffsetX | Opcode::NearestOffsetY | Opcode::NearestOffsetZ => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let (dx, dy, dz) = rt.query_nearest_offset(target.entity)?;
                    let v = match instruction.opcode {
                        Opcode::NearestOffsetX => dx,
                        Opcode::NearestOffsetY => dy,
                        _ => dz,
                    };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(v));
                    pcs[depth - 1] += 1;
                }
                Opcode::ReadEvent => {
                    let kind = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ) as u32;
                    // Deterministic reverse scan: the most recent event of this
                    // kind emitted so far in this interpretation; 0.0 if none.
                    let payload = env
                        .events
                        .iter()
                        .rev()
                        .find(|e| e.kind == kind)
                        .map(|e| f64::from_bits(e.payload))
                        .unwrap_or(0.0);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(payload));
                    pcs[depth - 1] += 1;
                }
                Opcode::EventCount => {
                    stacks[depth - 1].insert(
                        instruction.result_id,
                        Immediate::F64(env.queue.len() as f64),
                    );
                    pcs[depth - 1] += 1;
                }
                Opcode::NextEventTime => {
                    // `f64::MAX` (not `+inf`) when empty: a finite sentinel, so
                    // storing it in a state slot does not trip the detail-88
                    // non-finite check (matching `NearestDist`).
                    let t = env.queue.first().map(|e| e.time).unwrap_or(f64::MAX);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(t));
                    pcs[depth - 1] += 1;
                }
                Opcode::NextEventKind => {
                    let k = env.queue.first().map(|e| e.kind).unwrap_or(0) as f64;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(k));
                    pcs[depth - 1] += 1;
                }
                Opcode::NextEventPayload => {
                    let p = env
                        .queue
                        .first()
                        .map(|e| f64::from_bits(e.payload))
                        .unwrap_or(0.0);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(p));
                    pcs[depth - 1] += 1;
                }
                Opcode::NextEventPriority => {
                    let pr = env.queue.first().map(|e| e.priority).unwrap_or(0) as f64;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(pr));
                    pcs[depth - 1] += 1;
                }
                Opcode::SeizeResource => {
                    let rid = match instruction.constant {
                        Some(Immediate::U64(id)) => id,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let capacity = as_i64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    // The first seize fixes the capacity (the declaration
                    // value); later seizes reuse it, so a caller cannot widen
                    // a resource mid-run by passing a larger number.
                    let entry = env
                        .resources
                        .entry(rid)
                        .or_insert(ResourceState { capacity, busy: 0 });
                    let acquired = entry.busy < entry.capacity;
                    if acquired {
                        entry.busy += 1;
                    }
                    let r = if acquired { 1.0 } else { 0.0 };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(r));
                    pcs[depth - 1] += 1;
                }
                Opcode::ReleaseResource => {
                    let rid = match instruction.constant {
                        Some(Immediate::U64(id)) => id,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let busy = match env.resources.get_mut(&rid) {
                        Some(entry) => {
                            if entry.busy > 0 {
                                entry.busy -= 1;
                            }
                            entry.busy
                        }
                        None => 0,
                    };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(busy as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::ResourceBusy => {
                    let rid = match instruction.constant {
                        Some(Immediate::U64(id)) => id,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let busy = env.resources.get(&rid).map(|r| r.busy).unwrap_or(0);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(busy as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::ResourceCapacity => {
                    let rid = match instruction.constant {
                        Some(Immediate::U64(id)) => id,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let capacity = env.resources.get(&rid).map(|r| r.capacity).unwrap_or(0);
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(capacity as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::PopEvent => {
                    let p = env
                        .queue
                        .first()
                        .map(|e| f64::from_bits(e.payload))
                        .unwrap_or(0.0);
                    if !env.queue.is_empty() {
                        env.queue.remove(0);
                    }
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(p));
                    pcs[depth - 1] += 1;
                }
                Opcode::EventSeenCount => {
                    let kind = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ) as u32;
                    let n = env.events.iter().filter(|e| e.kind == kind).count() as f64;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(n));
                    pcs[depth - 1] += 1;
                }
                Opcode::HistRead => {
                    let site = match instruction.constant {
                        Some(Immediate::U64(site)) => site,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let v = env.hist.get(&site).copied().unwrap_or(0.0);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(v));
                    pcs[depth - 1] += 1;
                }
                Opcode::HistWrite => {
                    let site = match instruction.constant {
                        Some(Immediate::U64(site)) => site,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let value = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 17, 0))?,
                    );
                    env.hist.insert(site, value);
                    pcs[depth - 1] += 1;
                }
                Opcode::HistHas => {
                    let site = match instruction.constant {
                        Some(Immediate::U64(site)) => site,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let has = if env.hist.contains_key(&site) {
                        1.0
                    } else {
                        0.0
                    };
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(has));
                    pcs[depth - 1] += 1;
                }
                // RFC-0048 zero-crossing detection. Semantics are exact and
                // pure: the operands are compared with no epsilon. A crossing
                // timestamps the site for `LastCross`; NaN operands fail the
                // step (matches the language's NaN rejection elsewhere).
                Opcode::CrossDown | Opcode::RiseEdge | Opcode::FallEdge => {
                    let prev = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let cur = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[1])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 17, 0))?,
                    );
                    if prev.is_nan() || cur.is_nan() {
                        return Err(error(Status::EirInvalid, 18, 0));
                    }
                    let has = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[2])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 18, 0))?,
                    );
                    let fired = has != 0.0
                        && match instruction.opcode {
                            Opcode::CrossDown => {
                                prev != 0.0 && cur != 0.0 && (prev < 0.0) != (cur < 0.0)
                            }
                            Opcode::RiseEdge => prev <= 0.0 && cur > 0.0,
                            _ => prev >= 0.0 && cur < 0.0,
                        };
                    if fired {
                        if let Some(Immediate::U64(site)) = instruction.constant {
                            env.cross_time.insert(site, env.time);
                        }
                    }
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(fired as u8 as f64));
                    pcs[depth - 1] += 1;
                }
                Opcode::LastCross => {
                    let site = match instruction.constant {
                        Some(Immediate::U64(site)) => site,
                        _ => return Err(error(Status::EirInvalid, 23, 0)),
                    };
                    let t = env.cross_time.get(&site).copied().unwrap_or(0.0);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(t));
                    pcs[depth - 1] += 1;
                }
                Opcode::BoundsCheck => {
                    let idx = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let base = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[1])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 17, 0))?,
                    );
                    let len = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[2])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 18, 0))?,
                    );
                    // A non-finite or fractional index, or one outside the
                    // array's `[0, len)`, is a load-class trap (detail 18).
                    if !idx.is_finite() || idx.fract() != 0.0 || idx < 0.0 || idx >= len {
                        return Err(error(Status::EirInvalid, 18, pc));
                    }
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(base + idx));
                    pcs[depth - 1] += 1;
                }
                Opcode::ReadSlotDyn => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let idx = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ) as u64;
                    let dyn_target = ComponentRef {
                        entity: target.entity,
                        component: target.component,
                        offset: (idx as u32).wrapping_mul(STATE_SLOT_STRIDE),
                    };
                    let raw = rt.read_field(dyn_target)?;
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(f64::from_bits(raw)));
                    pcs[depth - 1] += 1;
                }
                Opcode::WriteSlotDyn => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 24, 0))?;
                    let idx = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    ) as u64;
                    let value = stacks[depth - 1]
                        .get(&instruction.operands[1])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 17, 0))?;
                    let dyn_target = ComponentRef {
                        entity: target.entity,
                        component: target.component,
                        offset: (idx as u32).wrapping_mul(STATE_SLOT_STRIDE),
                    };
                    rt.write_field(dyn_target, as_u64(value));
                    writes.push(WorldWrite {
                        entity: target.entity,
                        component: target.component,
                        offset: dyn_target.offset,
                        value: as_u64(value),
                    });
                    pcs[depth - 1] += 1;
                }
                Opcode::ReadFieldCell | Opcode::WriteFieldCell | Opcode::FieldLaplacian => {
                    // Grid field access: the field's width rides in the
                    // target's offset; the linear cell index is i + j·width.
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let i = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let j = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[1])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))?,
                    );
                    let width = target.offset as f64;
                    let linear = i + j * width;
                    let cell = ComponentRef {
                        entity: target.entity,
                        component: target.component,
                        offset: (linear as u32).wrapping_mul(STATE_SLOT_STRIDE),
                    };
                    match instruction.opcode {
                        Opcode::WriteFieldCell => {
                            let value = stacks[depth - 1]
                                .get(&instruction.operands[2])
                                .copied()
                                .ok_or(error(Status::EirInvalid, 17, 0))?;
                            rt.write_field(cell, as_u64(value));
                            writes.push(WorldWrite {
                                entity: target.entity,
                                component: target.component,
                                offset: cell.offset,
                                value: as_u64(value),
                            });
                        }
                        Opcode::FieldLaplacian => {
                            let v = rt.field_laplacian(target.component, i, j, width)?;
                            stacks[depth - 1].insert(instruction.result_id, Immediate::F64(v));
                        }
                        _ => {
                            let raw = rt.read_field(cell)?;
                            stacks[depth - 1]
                                .insert(instruction.result_id, Immediate::F64(f64::from_bits(raw)));
                        }
                    }
                    pcs[depth - 1] += 1;
                }
                Opcode::FieldDiffuse | Opcode::FieldWave | Opcode::FieldPoisson => {
                    // RFC-0037: the whole grid sweep runs in the runtime; the
                    // result lives in its dense field overlay, so no `WorldWrite`
                    // is emitted here. The `prev`/`source` field id arrives as
                    // four little-endian `u32` limbs (a 128-bit component id
                    // cannot fit an operand).
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let operand = |k: usize| -> Result<Immediate> {
                        stacks[depth - 1]
                            .get(&instruction.operands[k])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))
                    };
                    match instruction.opcode {
                        Opcode::FieldDiffuse => {
                            rt.field_diffuse(target.component, as_f64(operand(0)?))?;
                        }
                        Opcode::FieldWave => {
                            let prev = component_from_limbs(
                                as_u64(operand(0)?),
                                as_u64(operand(1)?),
                                as_u64(operand(2)?),
                                as_u64(operand(3)?),
                            );
                            rt.field_wave(
                                target.component,
                                prev,
                                as_f64(operand(4)?),
                                as_f64(operand(5)?),
                                as_f64(operand(6)?),
                                as_f64(operand(7)?),
                            )?;
                        }
                        _ => {
                            let sid = component_from_limbs(
                                as_u64(operand(0)?),
                                as_u64(operand(1)?),
                                as_u64(operand(2)?),
                                as_u64(operand(3)?),
                            );
                            let source = if sid.0 == [0u8; 16] { None } else { Some(sid) };
                            rt.field_poisson(
                                target.component,
                                source,
                                as_u64(operand(4)?) as u32,
                                as_f64(operand(5)?),
                            )?;
                        }
                    }
                    pcs[depth - 1] += 1;
                }
                Opcode::FindFreeSlot => {
                    // RFC-0038: pool scan (base entity id in four limbs, count).
                    let operand = |k: usize| -> Result<Immediate> {
                        stacks[depth - 1]
                            .get(&instruction.operands[k])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))
                    };
                    let base = u128_from_limbs(
                        as_u64(operand(0)?),
                        as_u64(operand(1)?),
                        as_u64(operand(2)?),
                        as_u64(operand(3)?),
                    );
                    let count = as_u64(operand(4)?) as u32;
                    let slot = rt.find_free_slot(base, count)?;
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(slot));
                    pcs[depth - 1] += 1;
                }
                Opcode::SpawnInto => {
                    // RFC-0038: pool spawn (base in four limbs, count); target
                    // carries the caller entity. The runtime emits the writes.
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 23, 0))?;
                    let operand = |k: usize| -> Result<Immediate> {
                        stacks[depth - 1]
                            .get(&instruction.operands[k])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 16, 0))
                    };
                    let base = u128_from_limbs(
                        as_u64(operand(0)?),
                        as_u64(operand(1)?),
                        as_u64(operand(2)?),
                        as_u64(operand(3)?),
                    );
                    let count = as_u64(operand(4)?) as u32;
                    let (slot, mut new_writes) = rt.spawn_into(base, count, target.entity)?;
                    writes.append(&mut new_writes);
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(slot));
                    pcs[depth - 1] += 1;
                }
                Opcode::Atomic => {
                    let target = instruction.target.ok_or(error(Status::EirInvalid, 35, 0))?;
                    let rhs = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let old = rt.read_field(target)?;
                    let sum = as_u64(rhs).wrapping_add(old);
                    rt.write_field(target, sum);
                    writes.push(WorldWrite {
                        entity: target.entity,
                        component: target.component,
                        offset: target.offset,
                        value: sum,
                    });
                    stacks[depth - 1].insert(instruction.result_id, Immediate::U64(old));
                    pcs[depth - 1] += 1;
                }
                Opcode::EmitEvent => {
                    // The kind is a numeric value (not an F64 bit pattern);
                    // otherwise every integer kind collapses to 0.
                    let kind = as_f64(
                        stacks[depth - 1]
                            .get(&instruction.operands[0])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 36, 0))?,
                    ) as u32;
                    let payload = as_u64(
                        stacks[depth - 1]
                            .get(&instruction.operands[1])
                            .copied()
                            .ok_or(error(Status::EirInvalid, 36, 0))?,
                    );
                    let seq = env.next_seq;
                    env.next_seq += 1;
                    env.events.push(EmittedEvent { kind, payload, seq });
                    pcs[depth - 1] += 1;
                }
                Opcode::Time => {
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(env.time));
                    pcs[depth - 1] += 1;
                }
                Opcode::Random => {
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::F64(env.rng.next_f64()));
                    pcs[depth - 1] += 1;
                }
                Opcode::Print => {
                    let v = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 16, 0))?;
                    let x = as_f64(v);
                    // `log` is a debugging side-channel that persists across
                    // steps; cap it so a program that prints every step cannot
                    // grow memory without bound in a long (live) run.
                    if env.log.len() >= MAX_LOG_LINES {
                        let drop = env.log.len() + 1 - MAX_LOG_LINES;
                        env.log.drain(0..drop);
                    }
                    env.log.push(format!("{x}"));
                    stacks[depth - 1].insert(instruction.result_id, Immediate::F64(x));
                    pcs[depth - 1] += 1;
                }
                Opcode::Io => {
                    stacks[depth - 1].insert(instruction.result_id, Immediate::U64(0));
                    pcs[depth - 1] += 1;
                }
                Opcode::Load => {
                    let address = stacks[depth - 1]
                        .get(&instruction.operands[0])
                        .copied()
                        .ok_or(error(Status::EirInvalid, 21, 0))?;
                    stacks[depth - 1]
                        .insert(instruction.result_id, Immediate::U64(as_u64(address)));
                    pcs[depth - 1] += 1;
                }
                Opcode::Nop => {
                    pcs[depth - 1] += 1;
                }
            }
        }
    }

    /// Pops one call frame (used when a function falls off its end).
    fn pop_return(
        &self,
        frames: &mut Vec<usize>,
        pcs: &mut Vec<usize>,
        stacks: &mut Vec<Regs>,
        functions: &[Function],
    ) -> Result<()> {
        if frames.len() == 1 {
            return Ok(());
        }
        frames.pop();
        pcs.pop();
        stacks.pop();
        let _ = functions;
        Ok(())
    }

    /// RFC-0021 binary module: envelope, directory, REQUIRED TYPES + FUNCTIONS
    /// sections. The module hash is SHA-256 over the file with its own hash
    /// bytes zeroed (RFC-0021 §module envelope).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let types = self.encode_types_section()?;
        let functions = self.encode_functions_section()?;
        let sections = [(KIND_TYPES, types), (KIND_FUNCTIONS, functions)];
        let mut offsets = Vec::with_capacity(sections.len());
        let mut cursor = HEADER_LEN + sections.len() * DIR_ENTRY_LEN;
        for (_, body) in &sections {
            cursor =
                cursor
                    .checked_add(7)
                    .map(|n| n & !7)
                    .ok_or(error(Status::Limit, 2, cursor))?;
            offsets.push(cursor);
            cursor = cursor
                .checked_add(body.len())
                .ok_or(error(Status::Limit, 3, cursor))?;
        }

        let mut out = Writer::new();
        out.bytes_raw(&EIR_MAGIC)?;
        out.u16(EIR_MAJOR)?;
        out.u16(EIR_MINOR)?;
        out.u32(0)?; // flags
        out.bytes_raw(&[0u8; 32])?; // module_hash placeholder (zeroed for hashing)
        out.bytes_raw(&self.schema_set_hash.0)?;
        out.bytes_raw(&self.domain_ir_hash.0)?;
        out.u16(self.target_kind)?;
        out.u16(0)?; // reserved
        out.u32(sections.len() as u32)?;
        for ((kind, body), offset) in sections.iter().zip(offsets.iter()) {
            let crc = crc32c(body);
            out.u16(*kind)?;
            out.u16(0)?; // dir flags
            out.u32(0)?;
            out.u64(*offset as u64)?;
            out.u64(body.len() as u64)?;
            out.u32(crc)?;
            out.u32(0)?;
        }
        let header_dir = out.finish();
        let mut out = Writer::new();
        out.bytes_raw(&header_dir)?;
        let mut written = header_dir.len();
        for ((_, body), offset) in sections.iter().zip(offsets.iter()) {
            out.bytes_raw(&vec![0u8; *offset - written])?;
            out.bytes_raw(body)?;
            written = *offset + body.len();
        }
        let mut bytes = out.finish();
        let hash = digest(&bytes);
        bytes[MODULE_HASH_OFFSET..MODULE_HASH_OFFSET + 32].copy_from_slice(&hash.0);
        Ok(bytes)
    }

    fn encode_types_section(&self) -> Result<Vec<u8>> {
        let mut types: Vec<u8> = self
            .functions
            .iter()
            .flat_map(|f| f.instructions.iter())
            .filter_map(|i| i.result_type.map(|t| t as u8))
            .collect();
        types.sort_unstable();
        types.dedup();
        let mut out = Writer::new();
        out.u32(types.len() as u32)?;
        for t in types {
            out.u8(t)?;
        }
        Ok(out.finish())
    }

    fn encode_functions_section(&self) -> Result<Vec<u8>> {
        let mut functions = self.functions.clone();
        functions.sort_by_key(|f| f.id);
        let mut out = Writer::new();
        out.u32(functions.len() as u32)?;
        for f in &functions {
            out.u64(f.id)?;
            out.u32(0)?; // signature_type
            out.u32(f.effect_mask)?;
            // RFC-0046: emit explicit basic blocks. A straight-line function
            // keeps the original single-block layout (byte-identical to legacy
            // artifacts); a branched function emits >1 block (block_count > 1),
            // which old readers reject and new readers decode by concatenation.
            let blocks = blocks_from_instructions(&f.instructions);
            if blocks.len() <= 1 {
                out.u32(1)?; // block_count
                out.u32(0)?; // block_id
                out.u32(f.argument_count)?;
                out.u32(f.instructions.len() as u32)?;
                for ins in &f.instructions {
                    encode_instruction(&mut out, ins)?;
                }
            } else {
                out.u32(blocks.len() as u32)?;
                out.u32(f.argument_count)?;
                for b in &blocks {
                    out.u32(b.id)?; // block_id
                    out.u32(0)?; // block params (reserved)
                    out.u32((b.end - b.start) as u32)?;
                    for ins in &f.instructions[b.start..b.end] {
                        encode_instruction(&mut out, ins)?;
                    }
                }
            }
        }
        Ok(out.finish())
    }

    /// RFC-0021 binary decoder. Validates envelope, directory (kind
    /// uniqueness, ordering, CRC, full consumption), required sections,
    /// target kind, and the module hash (computed over the file with its hash
    /// bytes zeroed). SSA/type/effect verification is a separate `validate`.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        if input.fixed(8)? != EIR_MAGIC {
            return Err(error(Status::Invalid, 1, 0));
        }
        let major = input.u16()?;
        let minor = input.u16()?;
        if major != EIR_MAJOR || minor != EIR_MINOR {
            return Err(error(Status::SchemaUnsupported, 2, 0));
        }
        let _flags = input.u32()?;
        let stored_hash = Hash256(input.fixed(32)?.try_into().expect("length-checked slice"));
        let schema_set_hash = Hash256(input.fixed(32)?.try_into().expect("length-checked slice"));
        let domain_ir_hash = Hash256(input.fixed(32)?.try_into().expect("length-checked slice"));
        let target_kind = input.u16()?;
        if target_kind > TARGET_NPU {
            return Err(error(Status::Invalid, 3, input.offset()));
        }
        if input.u16()? != 0 {
            return Err(error(Status::Invalid, 4, input.offset()));
        }
        let section_count = input.u32()?;
        if section_count == 0 || section_count > MAX_SECTIONS {
            return Err(error(Status::Limit, 1, input.offset()));
        }

        let mut directory = Vec::with_capacity(section_count as usize);
        for _ in 0..section_count {
            let kind = input.u16()?;
            let dir_flags = input.u16()?;
            let reserved0 = input.u32()?;
            let offset = input.u64()? as usize;
            let length = input.u64()? as usize;
            let crc = input.u32()?;
            let reserved1 = input.u32()?;
            if reserved0 != 0 || reserved1 != 0 || dir_flags != 0 {
                return Err(error(Status::Invalid, 5, input.offset()));
            }
            directory.push((kind, offset, length, crc));
        }

        let dir_end = input.offset();
        let mut previous_end = dir_end;
        let mut kinds_seen: BTreeMap<u16, (usize, usize)> = BTreeMap::new();
        for &(kind, offset, length, crc) in &directory {
            if kind == 0 || kind > KIND_MAX {
                return Err(error(Status::Invalid, 6, offset));
            }
            if kinds_seen.contains_key(&kind) {
                return Err(error(Status::Invalid, 7, offset));
            }
            if offset < dir_end || offset % 8 != 0 || offset < previous_end {
                return Err(error(Status::Invalid, 8, offset));
            }
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or(error(Status::Invalid, 9, offset))?;
            if end > MAX_DOCUMENT_BYTES {
                return Err(error(Status::Limit, 2, offset));
            }
            if crc32c(&bytes[offset..end]) != crc {
                return Err(error(Status::Invalid, 10, offset));
            }
            previous_end = end;
            kinds_seen.insert(kind, (offset, length));
        }
        if previous_end != bytes.len() {
            return Err(error(Status::Invalid, 11, previous_end));
        }
        for required in [KIND_TYPES, KIND_FUNCTIONS] {
            if !kinds_seen.contains_key(&required) {
                return Err(error(Status::Invalid, 12, 0));
            }
        }

        let (types_off, types_len) = kinds_seen[&KIND_TYPES];
        let (func_off, func_len) = kinds_seen[&KIND_FUNCTIONS];
        decode_types_section(&bytes[types_off..types_off + types_len])?;
        let functions = decode_functions_section(&bytes[func_off..func_off + func_len])?;

        // Module hash: SHA-256 over the file with its hash bytes zeroed.
        let mut copy = bytes.to_vec();
        copy[MODULE_HASH_OFFSET..MODULE_HASH_OFFSET + 32].fill(0);
        if digest(&copy) != stored_hash {
            return Err(error(Status::Invalid, 13, 0));
        }

        Ok(EirModule {
            module_hash: stored_hash,
            schema_set_hash,
            domain_ir_hash,
            target_kind,
            functions,
        })
    }
}

fn non_zero(id: u32) -> Option<u32> {
    if id == 0 {
        None
    } else {
        Some(id)
    }
}

/// Partitions an instruction stream into basic blocks. A new block starts at
/// index 0 and at every branch-target index. Each block's terminator is derived
/// from its final instruction; branch targets are the instruction indices the
/// terminator jumps to. Used by the RFC-0021 dominance gate.
fn blocks_from_instructions(instructions: &[Instruction]) -> Vec<crate::dominance::Block> {
    use crate::dominance::{Block, Terminator};
    // Degenerate input (an empty stream, or out-of-range branch targets) must not
    // panic here: `encode` and the CFG gate both call this. An empty function is
    // a single empty block; a target at/beyond the end is ignored (the dominance
    // verifier rejects it separately).
    if instructions.is_empty() {
        return vec![Block::new(0, 0, 0, Terminator::Return)];
    }
    let n = instructions.len();
    let mut starts: Vec<usize> = vec![0usize];
    for ins in instructions {
        match ins.opcode {
            Opcode::Br => {
                let t = ins.operands.first().copied().unwrap_or(0) as usize;
                if t < n {
                    starts.push(t);
                }
            }
            Opcode::CondBr => {
                for k in [1usize, 2] {
                    let t = ins.operands.get(k).copied().unwrap_or(0) as usize;
                    if t < n {
                        starts.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    starts.sort_unstable();
    starts.dedup();
    let mut blocks = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(instructions.len());
        let term = match instructions[e - 1].opcode {
            Opcode::Return => Terminator::Return,
            Opcode::Trap => Terminator::Trap,
            Opcode::Unreachable => Terminator::Unreachable,
            Opcode::Br => {
                Terminator::Br(instructions[e - 1].operands.first().copied().unwrap_or(0))
            }
            Opcode::CondBr => {
                let op = &instructions[e - 1].operands;
                Terminator::CondBr(
                    op.get(1).copied().unwrap_or(0),
                    op.get(2).copied().unwrap_or(0),
                )
            }
            _ => Terminator::Unreachable, // non-terminator tail; dominance rejects
        };
        blocks.push(Block::new(s as u32, s, e, term));
    }
    blocks
}

fn encode_instruction(out: &mut Writer, ins: &Instruction) -> Result<()> {
    let mut flags: u16 = 0;
    if ins.constant.is_some() {
        flags |= FLAG_CONSTANT;
    }
    if ins.target.is_some() {
        flags |= FLAG_TARGET;
    }
    out.u16(ins.opcode as u16)?;
    out.u16(flags)?;
    out.u32(ins.result_id)?;
    out.u32(ins.result_type.map(|t| t as u32).unwrap_or(0))?;
    out.u32(ins.operands.len() as u32)?;
    for op in &ins.operands {
        out.u32(*op)?;
    }
    if let Some(c) = ins.constant {
        encode_immediate(out, c)?;
    }
    if let Some(t) = ins.target {
        out.bytes_raw(&t.entity.to_le_bytes())?;
        out.bytes_raw(&t.component.0)?;
        out.u32(t.offset)?;
    }
    Ok(())
}

fn decode_instruction(input: &mut Reader<'_>) -> Result<Instruction> {
    let opcode = opcode_from_u16(input.u16()?)?;
    let flags = input.u16()?;
    let result_id = input.u32()?;
    let result_type = {
        let tag = input.u32()?;
        if tag == 0 {
            None
        } else {
            Some(value_type_from_u32(tag).ok_or(error(Status::EirInvalid, 26, input.offset()))?)
        }
    };
    let operand_count = input.u32()? as usize;
    let mut operands = Vec::with_capacity(operand_count);
    for _ in 0..operand_count {
        operands.push(input.u32()?);
    }
    let constant = if flags & FLAG_CONSTANT != 0 {
        Some(decode_immediate(input)?)
    } else {
        None
    };
    let target = if flags & FLAG_TARGET != 0 {
        let entity =
            u128::from_le_bytes(input.fixed(16)?.try_into().expect("length-checked slice"));
        let component = ComponentTypeId(input.fixed(16)?.try_into().expect("length-checked slice"));
        let offset = input.u32()?;
        Some(ComponentRef {
            entity,
            component,
            offset,
        })
    } else {
        None
    };
    Ok(Instruction {
        opcode,
        result_id,
        result_type,
        operands,
        constant,
        target,
    })
}

fn encode_immediate(out: &mut Writer, value: Immediate) -> Result<()> {
    let (kind, bits): (u8, u64) = match value {
        Immediate::I32(v) => (1, v as i64 as u64),
        Immediate::U32(v) => (2, v as u64),
        Immediate::I64(v) => (3, v as u64),
        Immediate::U64(v) => (4, v),
        Immediate::F32(v) => (5, v.to_bits() as u64),
        Immediate::F64(v) => (6, v.to_bits()),
        Immediate::Bool(v) => (7, v as u64),
    };
    out.u8(kind)?;
    out.u64(bits)?;
    Ok(())
}

fn decode_immediate(input: &mut Reader<'_>) -> Result<Immediate> {
    let kind = input.u8()?;
    let bits = input.u64()?;
    Ok(match kind {
        1 => Immediate::I32(bits as i64 as i32),
        2 => Immediate::U32(bits as u32),
        3 => Immediate::I64(bits as i64),
        4 => Immediate::U64(bits),
        5 => Immediate::F32(f32::from_bits(bits as u32)),
        6 => Immediate::F64(f64::from_bits(bits)),
        7 => Immediate::Bool(bits != 0),
        _ => return Err(error(Status::EirInvalid, 27, input.offset())),
    })
}

fn opcode_from_u16(raw: u16) -> Result<Opcode> {
    Opcode::from_u16(raw).ok_or(error(Status::EirInvalid, 28, 0))
}

fn value_type_from_u32(raw: u32) -> Option<ValueType> {
    match raw {
        x if x == ValueType::I32 as u32 => Some(ValueType::I32),
        x if x == ValueType::U32 as u32 => Some(ValueType::U32),
        x if x == ValueType::I64 as u32 => Some(ValueType::I64),
        x if x == ValueType::U64 as u32 => Some(ValueType::U64),
        x if x == ValueType::F32 as u32 => Some(ValueType::F32),
        x if x == ValueType::F64 as u32 => Some(ValueType::F64),
        x if x == ValueType::Bool as u32 => Some(ValueType::Bool),
        _ => None,
    }
}

fn decode_types_section(bytes: &[u8]) -> Result<()> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()? as usize;
    for _ in 0..count {
        let tag = input.u8()?;
        if value_type_from_u32(tag as u32).is_none() {
            return Err(error(Status::EirInvalid, 29, input.offset()));
        }
    }
    input.finish()
}

fn decode_functions_section(bytes: &[u8]) -> Result<Vec<Function>> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()? as usize;
    let mut functions = Vec::with_capacity(count);
    let mut prev_id: Option<u64> = None;
    for _ in 0..count {
        let id = input.u64()?;
        if prev_id.is_some_and(|prev| id <= prev) {
            return Err(error(Status::Invalid, 14, input.offset()));
        }
        prev_id = Some(id);
        let _signature_type = input.u32()?;
        let effect_mask = input.u32()?;
        let block_count = input.u32()?;
        let mut instructions: Vec<Instruction>;
        let argument_count: u32;
        if block_count == 1 {
            // Legacy / straight-line single-block layout.
            let _block_id = input.u32()?;
            argument_count = input.u32()?;
            let instruction_count = input.u32()? as usize;
            instructions = Vec::with_capacity(instruction_count);
            for _ in 0..instruction_count {
                instructions.push(decode_instruction(&mut input)?);
            }
        } else {
            // RFC-0046 explicit blocks: concatenating the blocks reproduces the
            // flat instruction stream deterministically.
            if block_count == 0 || block_count as usize > MAX_BLOCKS {
                return Err(error(Status::EirInvalid, 30, input.offset()));
            }
            argument_count = input.u32()?;
            instructions = Vec::new();
            for _ in 0..block_count {
                let _block_id = input.u32()?;
                let _params = input.u32()?;
                let ic = input.u32()? as usize;
                for _ in 0..ic {
                    instructions.push(decode_instruction(&mut input)?);
                }
            }
        }
        functions.push(Function {
            id,
            effect_mask,
            argument_count,
            instructions,
        });
    }
    input.finish()?;
    Ok(functions)
}

/// A runtime that returns 0 for every read (used when only write effects matter).
pub struct NoopRuntime;
impl EirRuntime for NoopRuntime {
    fn read_field(&self, _target: ComponentRef) -> Result<u64> {
        Ok(0)
    }
    fn write_field(&mut self, _target: ComponentRef, _value: u64) {}
    fn query_neighbor_count(&self, _entity: u128, _radius: f64) -> Result<u64> {
        Ok(0)
    }
    fn query_nearest_dist(&self, _entity: u128) -> Result<u64> {
        Ok(f64::MAX.to_bits())
    }
    fn query_neighbor_mean(&self, _entity: u128, _slot: u32, _radius: f64) -> Result<f64> {
        Ok(0.0)
    }
    fn query_nearest_offset(&self, _entity: u128) -> Result<(f64, f64, f64)> {
        Ok((0.0, 0.0, 0.0))
    }
    fn field_laplacian(
        &self,
        _component: ComponentTypeId,
        _i: f64,
        _j: f64,
        _width: f64,
    ) -> Result<f64> {
        Ok(0.0)
    }
}

impl EirModule {
    /// A dependency-free, semantics-preserving optimizing pass (Phase 3):
    /// **constant folding** and **Mul/Add -> Fma superinstruction fusion**.
    ///
    /// Every rewrite replaces an instruction in place, so instruction indices,
    /// branch targets and SSA dominance are unchanged, and `Fma` keeps the two
    /// roundings of `Mul;Add` — the optimized program is bit-identical to the
    /// input (verified by the differential `step_cross` path). The interpreter
    /// remains the semantic oracle.
    pub fn optimize(&self) -> EirModule {
        let mut out = self.clone();
        for f in &mut out.functions {
            fold_constants(&mut f.instructions);
            fuse_fma(&mut f.instructions);
        }
        out
    }
}

/// Folds instructions whose operands are compile-time constants into a single
/// `Const` (IEEE results are identical to the runtime op).
fn fold_constants(ins: &mut [Instruction]) {
    let mut vals: std::collections::HashMap<u32, Immediate> = Default::default();
    for i in ins.iter_mut() {
        if i.opcode == Opcode::Const {
            if let (Some(c), true) = (i.constant, i.result_id != 0) {
                vals.insert(i.result_id, c);
            }
            continue;
        }
        if i.result_id == 0 || !i.target.is_none() {
            continue;
        }
        let folded = eval_const(i, &vals);
        if let Some(c) = folded {
            let rid = i.result_id;
            *i = Instruction {
                opcode: Opcode::Const,
                result_id: rid,
                result_type: Some(c.ty()),
                operands: Vec::new(),
                constant: Some(c),
                target: None,
            };
            vals.insert(rid, c);
        }
    }
}

/// Evaluates a pure numeric instruction over known-constant operands.
fn eval_const(
    i: &Instruction,
    vals: &std::collections::HashMap<u32, Immediate>,
) -> Option<Immediate> {
    let op = i.opcode;
    let arg = |k: usize| -> Option<Immediate> { vals.get(&i.operands.get(k).copied()?).copied() };
    match op {
        Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Rem => {
            let a = arg(0)?;
            let b = arg(1)?;
            if matches!(op, Opcode::Div | Opcode::Rem) {
                divrem(op, a, b)
            } else {
                arith(op, a, b)
            }
        }
        Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
            compare(op, arg(0)?, arg(1)?)
        }
        Opcode::Fma => {
            let a = as_f64(arg(0)?);
            let b = as_f64(arg(1)?);
            let c = as_f64(arg(2)?);
            Some(Immediate::F64(a * b + c))
        }
        Opcode::Sin
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
        | Opcode::Atan => {
            let x = as_f64(arg(0)?);
            let v = match op {
                Opcode::Sin => x.sin(),
                Opcode::Cos => x.cos(),
                Opcode::Exp => x.exp(),
                Opcode::Ln => x.ln(),
                Opcode::Sqrt => x.sqrt(),
                Opcode::Abs => x.abs(),
                Opcode::Floor => x.floor(),
                Opcode::Ceil => x.ceil(),
                Opcode::Round => x.round(),
                Opcode::Sign => x.signum(),
                Opcode::Log10 => x.log10(),
                Opcode::Log2 => x.log2(),
                Opcode::Sinh => x.sinh(),
                Opcode::Cosh => x.cosh(),
                Opcode::Tanh => x.tanh(),
                Opcode::Asin => x.asin(),
                Opcode::Acos => x.acos(),
                _ => x.atan(),
            };
            Some(Immediate::F64(v))
        }
        Opcode::Select => {
            let cond = arg(0)?;
            if as_u64(cond) != 0 {
                arg(1)
            } else {
                arg(2)
            }
        }
        _ => None,
    }
}

/// Fuses `Add(x, y)` where `x` is a single-use `Mul(a, b)` into `Fma(a, b, y)`,
/// neutralising the now-dead `Mul`. Only f64 arithmetic is fused (integer math
/// stays as-is), and only when the `Mul` result has exactly one use.
fn fuse_fma(ins: &mut [Instruction]) {
    let mut uses: std::collections::HashMap<u32, usize> = Default::default();
    for i in ins.iter() {
        for &o in &i.operands {
            *uses.entry(o).or_default() += 1;
        }
    }
    let mul_idx: std::collections::HashMap<u32, usize> = ins
        .iter()
        .enumerate()
        .filter(|(_, i)| i.opcode == Opcode::Mul && i.result_id != 0)
        .map(|(k, i)| (i.result_id, k))
        .collect();
    for k in 0..ins.len() {
        if ins[k].opcode != Opcode::Add || ins[k].operands.len() != 2 {
            continue;
        }
        if infer_instruction_type(&ins[k]) != Some(ValueType::F64) {
            continue;
        }
        let (ra, rb) = (ins[k].operands[0], ins[k].operands[1]);
        for (want, other) in [(ra, rb), (rb, ra)] {
            if uses.get(&want).copied() != Some(1) {
                continue;
            }
            let Some(&mk) = mul_idx.get(&want) else {
                continue;
            };
            if mk == k || infer_instruction_type(&ins[mk]) != Some(ValueType::F64) {
                continue;
            }
            let m = &ins[mk];
            if m.opcode != Opcode::Mul || m.operands.len() != 2 {
                continue;
            }
            let (ma, mb) = (m.operands[0], m.operands[1]);
            let res = ins[k].result_id;
            ins[k] = Instruction {
                opcode: Opcode::Fma,
                result_id: res,
                result_type: Some(ValueType::F64),
                operands: vec![ma, mb, other],
                constant: None,
                target: None,
            };
            ins[mk] = Instruction {
                opcode: Opcode::Nop,
                result_id: 0,
                result_type: None,
                operands: Vec::new(),
                constant: None,
                target: None,
            };
            break;
        }
    }
}

fn arith(op: Opcode, a: Immediate, b: Immediate) -> Option<Immediate> {
    match (op, a, b) {
        (Opcode::Add, Immediate::I32(a), Immediate::I32(b)) => {
            Some(Immediate::I32(a.wrapping_add(b)))
        }
        (Opcode::Sub, Immediate::I32(a), Immediate::I32(b)) => {
            Some(Immediate::I32(a.wrapping_sub(b)))
        }
        (Opcode::Mul, Immediate::I32(a), Immediate::I32(b)) => {
            Some(Immediate::I32(a.wrapping_mul(b)))
        }
        (Opcode::Add, Immediate::U32(a), Immediate::U32(b)) => {
            Some(Immediate::U32(a.wrapping_add(b)))
        }
        (Opcode::Sub, Immediate::U32(a), Immediate::U32(b)) => {
            Some(Immediate::U32(a.wrapping_sub(b)))
        }
        (Opcode::Mul, Immediate::U32(a), Immediate::U32(b)) => {
            Some(Immediate::U32(a.wrapping_mul(b)))
        }
        // RFC-0043: exact 64-bit integer arithmetic.
        (Opcode::Add, Immediate::I64(a), Immediate::I64(b)) => {
            Some(Immediate::I64(a.wrapping_add(b)))
        }
        (Opcode::Sub, Immediate::I64(a), Immediate::I64(b)) => {
            Some(Immediate::I64(a.wrapping_sub(b)))
        }
        (Opcode::Mul, Immediate::I64(a), Immediate::I64(b)) => {
            Some(Immediate::I64(a.wrapping_mul(b)))
        }
        (Opcode::Add, Immediate::U64(a), Immediate::U64(b)) => {
            Some(Immediate::U64(a.wrapping_add(b)))
        }
        (Opcode::Sub, Immediate::U64(a), Immediate::U64(b)) => {
            Some(Immediate::U64(a.wrapping_sub(b)))
        }
        (Opcode::Mul, Immediate::U64(a), Immediate::U64(b)) => {
            Some(Immediate::U64(a.wrapping_mul(b)))
        }
        (Opcode::Add, Immediate::F32(a), Immediate::F32(b)) => Some(Immediate::F32(a + b)),
        (Opcode::Sub, Immediate::F32(a), Immediate::F32(b)) => Some(Immediate::F32(a - b)),
        (Opcode::Mul, Immediate::F32(a), Immediate::F32(b)) => Some(Immediate::F32(a * b)),
        (Opcode::Add, Immediate::F64(a), Immediate::F64(b)) => Some(Immediate::F64(a + b)),
        (Opcode::Sub, Immediate::F64(a), Immediate::F64(b)) => Some(Immediate::F64(a - b)),
        (Opcode::Mul, Immediate::F64(a), Immediate::F64(b)) => Some(Immediate::F64(a * b)),
        _ => None,
    }
}

/// Reassemble a 128-bit `ComponentTypeId` from four little-endian `u32` limbs
/// (the encoding of the bulk field opcodes' second field id, RFC-0037).
fn component_from_limbs(a: u64, b: u64, c: u64, d: u64) -> ComponentTypeId {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&(a as u32).to_le_bytes());
    bytes[4..8].copy_from_slice(&(b as u32).to_le_bytes());
    bytes[8..12].copy_from_slice(&(c as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(d as u32).to_le_bytes());
    ComponentTypeId(bytes)
}

/// The four little-endian `u32` limbs of a 128-bit entity id.
pub(crate) fn u128_limbs(id: u128) -> [u32; 4] {
    [
        id as u32,
        (id >> 32) as u32,
        (id >> 64) as u32,
        (id >> 96) as u32,
    ]
}

/// Reassemble a 128-bit entity id from four little-endian `u32` limbs.
fn u128_from_limbs(a: u64, b: u64, c: u64, d: u64) -> u128 {
    (a as u32 as u128)
        | ((b as u32 as u128) << 32)
        | ((c as u32 as u128) << 64)
        | ((d as u32 as u128) << 96)
}

/// The four little-endian `u32` limbs of a `ComponentTypeId` (inverse of
/// `component_from_limbs`), for the bulk field opcodes' lowering (RFC-0037).
pub(crate) fn component_limbs(id: ComponentTypeId) -> [u32; 4] {
    let b = id.0;
    [
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
    ]
}

fn as_u64(value: Immediate) -> u64 {
    match value {
        Immediate::I32(v) => v as u64,
        Immediate::U32(v) => v as u64,
        Immediate::I64(v) => v as u64,
        Immediate::U64(v) => v,
        Immediate::F32(v) => v.to_bits() as u64,
        Immediate::F64(v) => v.to_bits(),
        Immediate::Bool(v) => v as u64,
    }
}

fn as_f64(value: Immediate) -> f64 {
    match value {
        Immediate::F64(v) => v,
        Immediate::F32(v) => v as f64,
        Immediate::I32(v) => v as f64,
        Immediate::U32(v) => v as f64,
        Immediate::I64(v) => v as f64,
        Immediate::U64(v) => v as f64,
        Immediate::Bool(v) => v as u8 as f64,
    }
}

/// Reinterpret a value as an exact `i64` (RFC-0043 `I64ToF64`/integer ops).
/// An `F64`/`F32` is truncated toward zero; callers that must reject a
/// non-finite float do so explicitly (a nan/inf `as i64` saturates in Rust).
fn as_i64(value: Immediate) -> i64 {
    match value {
        Immediate::I32(v) => v as i64,
        Immediate::U32(v) => v as i64,
        Immediate::I64(v) => v,
        Immediate::U64(v) => v as i64,
        Immediate::F32(v) => v as i64,
        Immediate::F64(v) => v as i64,
        Immediate::Bool(v) => u8::from(v) as i64,
    }
}

/// Division and remainder with defined traps (RFC-0021): divide-by-zero and
/// signed overflow return `None`, which the interpreter maps to a trap.
fn divrem(op: Opcode, a: Immediate, b: Immediate) -> Option<Immediate> {
    use Immediate::*;
    match (op, a, b) {
        (Opcode::Div, I32(a), I32(b)) => {
            if b == 0 || (a == i32::MIN && b == -1) {
                None
            } else {
                Some(I32(a / b))
            }
        }
        (Opcode::Rem, I32(a), I32(b)) => {
            if b == 0 {
                None
            } else {
                Some(I32(a % b))
            }
        }
        (Opcode::Div, U32(a), U32(b)) => (b != 0).then(|| U32(a / b)),
        (Opcode::Rem, U32(a), U32(b)) => (b != 0).then(|| U32(a % b)),
        (Opcode::Div, U64(a), U64(b)) => (b != 0).then(|| U64(a / b)),
        (Opcode::Rem, U64(a), U64(b)) => (b != 0).then(|| U64(a % b)),
        (Opcode::Div, I64(a), I64(b)) => {
            if b == 0 || (a == i64::MIN && b == -1) {
                None
            } else {
                Some(I64(a / b))
            }
        }
        (Opcode::Rem, I64(a), I64(b)) => (b != 0).then(|| I64(a % b)),
        // RFC-0021: divide-by-zero is a **defined trap** for every width, so
        // F32/F64 division by (exactly) zero traps too — not silent inf/NaN.
        (Opcode::Div, F32(a), F32(b)) => (b != 0.0).then_some(F32(a / b)),
        (Opcode::Div, F64(a), F64(b)) => (b != 0.0).then_some(F64(a / b)),
        // fmod semantics; exact for integer-valued f64 (step % n).
        (Opcode::Rem, F32(a), F32(b)) => (b != 0.0).then_some(F32(a % b)),
        (Opcode::Rem, F64(a), F64(b)) => (b != 0.0).then_some(F64(a % b)),
        _ => None,
    }
}

/// Ordered comparisons produce a Bool immediate.
fn compare(op: Opcode, a: Immediate, b: Immediate) -> Option<Immediate> {
    use Immediate::*;
    let ordering: Option<std::cmp::Ordering> = match (a, b) {
        (I32(a), I32(b)) => Some(a.cmp(&b)),
        (U32(a), U32(b)) => Some(a.cmp(&b)),
        (I64(a), I64(b)) => Some(a.cmp(&b)),
        (U64(a), U64(b)) => Some(a.cmp(&b)),
        (F32(a), F32(b)) => a.partial_cmp(&b),
        (F64(a), F64(b)) => a.partial_cmp(&b),
        _ => None,
    };
    let ordering = ordering?;
    let result = match op {
        Opcode::Eq => ordering == std::cmp::Ordering::Equal,
        Opcode::Ne => ordering != std::cmp::Ordering::Equal,
        Opcode::Lt => ordering == std::cmp::Ordering::Less,
        Opcode::Le => ordering != std::cmp::Ordering::Greater,
        Opcode::Gt => ordering == std::cmp::Ordering::Greater,
        Opcode::Ge => ordering != std::cmp::Ordering::Less,
        _ => return None,
    };
    Some(Bool(result))
}

fn component_from_addr(address: Immediate) -> ComponentTypeId {
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&as_u64(address).to_le_bytes());
    ComponentTypeId(id)
}

// ---------------------------------------------------------------------------
// Opt-in threaded dispatch (Phase 3).
//
// A second execution strategy for the interpreter: instead of a `match` on the
// opcode (jump table), each instruction is dispatched through a static
// function-pointer table indexed by opcode (`handlers()[opcode]`). This is a
// genuine threaded interpreter. It is **opt-in** because measurements show the
// jump table is ~8% faster on this platform; it is kept as a selectable backend
// and is differentially verified against the match interpreter.
//
// Coverage: functions whose opcodes are all in the supported subset run
// threaded; any other module falls back to the match interpreter (the caller
// checks `EirModule::threaded_supported`).
// ---------------------------------------------------------------------------

/// Mutable threaded execution state (mirrors the match interpreter's locals).
struct ThMachine<'a> {
    rt: &'a mut dyn EirRuntime,
    writes: &'a mut Vec<WorldWrite>,
    frames: Vec<usize>,
    pcs: Vec<usize>,
    stacks: Vec<Regs>,
    halt: bool,
    /// Index of the instruction currently executing (for error offsets, so
    /// threaded diagnostics match the jump table, which reports `pc`).
    pc: usize,
}

/// Immutable program data (kept separate so instruction borrows don't conflict
/// with the mutable machine).
struct ThProg<'p> {
    functions: &'p [Function],
    index_of: &'p std::collections::HashMap<u64, usize>,
}

type Handler = fn(&mut ThMachine<'_>, &Instruction, &ThProg<'_>) -> Result<()>;

fn h_unsupported(_: &mut ThMachine<'_>, _: &Instruction, _: &ThProg<'_>) -> Result<()> {
    Err(error(Status::EirInvalid, 3, 0))
}

fn th_get(m: &ThMachine<'_>, id: u32) -> Result<Immediate> {
    th_get_d(m, id, 16)
}

/// Operand fetch with an explicit error detail, matching the jump-table
/// interpreter's per-operand details (`16` for operand 0, `17` for operand 1,
/// `18` for operand 2, `25` for `WriteView`'s value, ...).
fn th_get_d(m: &ThMachine<'_>, id: u32, detail: u32) -> Result<Immediate> {
    m.stacks[m.frames.len() - 1]
        .get(&id)
        .copied()
        .ok_or(error(Status::EirInvalid, detail, 0))
}

fn th_set(m: &mut ThMachine<'_>, id: u32, v: Immediate) {
    let d = m.frames.len() - 1;
    m.stacks[d].insert(id, v);
}

fn h_nop(m: &mut ThMachine<'_>, _: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_const(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let c = ins.constant.ok_or(error(Status::EirInvalid, 15, 0))?;
    th_set(m, ins.result_id, c);
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_arith(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let a = th_get_d(m, ins.operands[0], 16)?;
    let b = th_get_d(m, ins.operands[1], 17)?;
    let out = match ins.opcode {
        Opcode::Add | Opcode::Sub | Opcode::Mul => arith(ins.opcode, a, b),
        Opcode::Div | Opcode::Rem => divrem(ins.opcode, a, b),
        Opcode::Pow => Some(Immediate::F64(as_f64(a).powf(as_f64(b)))),
        _ => None,
    }
    .ok_or(error(Status::EirInvalid, 18, 0))?;
    th_set(m, ins.result_id, out);
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_fma(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let a = as_f64(th_get_d(m, ins.operands[0], 16)?);
    let b = as_f64(th_get_d(m, ins.operands[1], 17)?);
    let c = as_f64(th_get_d(m, ins.operands[2], 18)?);
    th_set(m, ins.result_id, Immediate::F64(a * b + c));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

/// RFC-0043 `I64ToF64`: exact widen of an integer to f64.
fn h_i64_to_f64(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let a = as_i64(th_get_d(m, ins.operands[0], 16)?);
    th_set(m, ins.result_id, Immediate::F64(a as f64));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

/// RFC-0043 `F64ToI64`: truncate toward zero; trap (detail 18) on non-finite.
fn h_f64_to_i64(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let x = as_f64(th_get_d(m, ins.operands[0], 16)?);
    if !x.is_finite() {
        return Err(error(Status::EirInvalid, 18, 0));
    }
    th_set(m, ins.result_id, Immediate::I64(x as i64));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_cmp(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let a = th_get_d(m, ins.operands[0], 16)?;
    let b = th_get_d(m, ins.operands[1], 17)?;
    let out = compare(ins.opcode, a, b).ok_or(error(Status::EirInvalid, 18, 0))?;
    th_set(m, ins.result_id, out);
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_select(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let cond = th_get_d(m, ins.operands[0], 16)?;
    let out = if as_u64(cond) != 0 {
        th_get_d(m, ins.operands[1], 17)?
    } else {
        th_get_d(m, ins.operands[2], 17)?
    };
    th_set(m, ins.result_id, out);
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_unary(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let x = as_f64(th_get(m, ins.operands[0])?);
    let v = match ins.opcode {
        Opcode::Sin => x.sin(),
        Opcode::Cos => x.cos(),
        Opcode::Exp => x.exp(),
        Opcode::Ln => x.ln(),
        Opcode::Sqrt => x.sqrt(),
        Opcode::Abs => x.abs(),
        Opcode::Floor => x.floor(),
        Opcode::Ceil => x.ceil(),
        Opcode::Round => x.round(),
        Opcode::Sign => x.signum(),
        Opcode::Log10 => x.log10(),
        Opcode::Log2 => x.log2(),
        Opcode::Sinh => x.sinh(),
        Opcode::Cosh => x.cosh(),
        Opcode::Tanh => x.tanh(),
        Opcode::Asin => x.asin(),
        Opcode::Acos => x.acos(),
        _ => x.atan(),
    };
    th_set(m, ins.result_id, Immediate::F64(v));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_atan2_hypot(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let a = as_f64(th_get_d(m, ins.operands[0], 16)?);
    let b = as_f64(th_get_d(m, ins.operands[1], 17)?);
    let v = if ins.opcode == Opcode::Atan2 {
        a.atan2(b)
    } else {
        a.hypot(b)
    };
    th_set(m, ins.result_id, Immediate::F64(v));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_read_view(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let target = ins.target.ok_or(error(Status::EirInvalid, 23, 0))?;
    let raw = m.rt.read_field(target)?;
    th_set(m, ins.result_id, Immediate::F64(f64::from_bits(raw)));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_read_committed(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let target = ins.target.ok_or(error(Status::EirInvalid, 23, 0))?;
    let raw = m.rt.read_committed_field(target)?;
    th_set(m, ins.result_id, Immediate::F64(f64::from_bits(raw)));
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_write_view(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let target = ins.target.ok_or(error(Status::EirInvalid, 24, 0))?;
    let value = as_u64(th_get_d(m, ins.operands[0], 25)?);
    m.rt.write_field(target, value);
    m.writes.push(WorldWrite {
        entity: target.entity,
        component: target.component,
        offset: target.offset,
        value,
    });
    let d = m.frames.len() - 1;
    m.pcs[d] += 1;
    Ok(())
}

fn h_br(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let d = m.frames.len() - 1;
    m.pcs[d] = ins.operands[0] as usize;
    Ok(())
}

fn h_condbr(m: &mut ThMachine<'_>, ins: &Instruction, _: &ThProg<'_>) -> Result<()> {
    let cond = th_get(m, ins.operands[0])?;
    let d = m.frames.len() - 1;
    m.pcs[d] = if as_u64(cond) != 0 {
        ins.operands[1] as usize
    } else {
        ins.operands[2] as usize
    };
    Ok(())
}

fn h_call(m: &mut ThMachine<'_>, ins: &Instruction, p: &ThProg<'_>) -> Result<()> {
    let d = m.frames.len();
    let target_id = ins.operands[0] as u64;
    let target = *p
        .index_of
        .get(&target_id)
        .ok_or(error(Status::EirInvalid, 32, m.pc))?;
    if d + 1 > MAX_CALL_DEPTH {
        return Err(error(Status::EirInvalid, 33, m.pc));
    }
    let callee_cap =
        p.functions[target].instructions.len() + p.functions[target].argument_count as usize + 1;
    let mut callee = Regs::new(callee_cap);
    {
        let cur = &m.stacks[d - 1];
        for (slot, arg_id) in ins.operands.iter().skip(1).enumerate() {
            callee.insert(
                (slot as u32) + 1,
                *cur.get(arg_id).ok_or(error(Status::EirInvalid, 34, m.pc))?,
            );
        }
    }
    m.pcs[d - 1] += 1;
    m.frames.push(target);
    m.pcs.push(0);
    m.stacks.push(callee);
    Ok(())
}

fn h_return(m: &mut ThMachine<'_>, ins: &Instruction, p: &ThProg<'_>) -> Result<()> {
    let d = m.frames.len();
    let ret = if let Some(vid) = ins.operands.first() {
        m.stacks[d - 1]
            .get(vid)
            .copied()
            .unwrap_or(Immediate::U64(0))
    } else {
        Immediate::U64(0)
    };
    if d == 1 {
        m.halt = true;
        return Ok(());
    }
    m.frames.pop();
    m.pcs.pop();
    m.stacks.pop();
    let caller_fi = m.frames[d - 2];
    let caller_pc = m.pcs[d - 2] - 1;
    if let Some(rid) = non_zero(p.functions[caller_fi].instructions[caller_pc].result_id) {
        m.stacks[d - 2].insert(rid, ret);
    }
    Ok(())
}

fn h_trap(m: &mut ThMachine<'_>, _: &Instruction, _: &ThProg<'_>) -> Result<()> {
    m.halt = true;
    Ok(())
}

/// Pops a frame when execution falls off the end of a function.
fn th_pop_return(m: &mut ThMachine<'_>) {
    if m.frames.len() == 1 {
        m.halt = true;
        return;
    }
    m.frames.pop();
    m.pcs.pop();
    m.stacks.pop();
}

/// Opcodes with the highest wire values (`Trap`/`Unreachable`) are ~0x8003, so
/// the dispatch table is dense up to there and built once.
const HANDLER_TABLE_LEN: usize = 0x8005;

fn handlers() -> &'static Vec<Handler> {
    static TABLE: std::sync::OnceLock<Vec<Handler>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t: Vec<Handler> = vec![h_unsupported; HANDLER_TABLE_LEN];
        let mut set = |op: Opcode, h: Handler| t[op as usize] = h;
        set(Opcode::Nop, h_nop);
        set(Opcode::Const, h_const);
        for op in [
            Opcode::Add,
            Opcode::Sub,
            Opcode::Mul,
            Opcode::Div,
            Opcode::Rem,
            Opcode::Pow,
        ] {
            set(op, h_arith);
        }
        set(Opcode::Fma, h_fma);
        set(Opcode::I64ToF64, h_i64_to_f64);
        set(Opcode::F64ToI64, h_f64_to_i64);
        for op in [
            Opcode::Eq,
            Opcode::Ne,
            Opcode::Lt,
            Opcode::Le,
            Opcode::Gt,
            Opcode::Ge,
        ] {
            set(op, h_cmp);
        }
        set(Opcode::Select, h_select);
        for op in [
            Opcode::Sin,
            Opcode::Cos,
            Opcode::Exp,
            Opcode::Ln,
            Opcode::Sqrt,
            Opcode::Abs,
            Opcode::Floor,
            Opcode::Ceil,
            Opcode::Round,
            Opcode::Sign,
            Opcode::Log10,
            Opcode::Log2,
            Opcode::Sinh,
            Opcode::Cosh,
            Opcode::Tanh,
            Opcode::Asin,
            Opcode::Acos,
            Opcode::Atan,
        ] {
            set(op, h_unary);
        }
        set(Opcode::Atan2, h_atan2_hypot);
        set(Opcode::Hypot, h_atan2_hypot);
        set(Opcode::ReadView, h_read_view);
        set(Opcode::ReadCommitted, h_read_committed);
        set(Opcode::WriteView, h_write_view);
        set(Opcode::Br, h_br);
        set(Opcode::CondBr, h_condbr);
        set(Opcode::Call, h_call);
        set(Opcode::Return, h_return);
        set(Opcode::Trap, h_trap);
        set(Opcode::Unreachable, h_trap);
        t
    })
}

fn is_threaded_op(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::Nop
            | Opcode::Const
            | Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Div
            | Opcode::Rem
            | Opcode::Pow
            | Opcode::Fma
            | Opcode::I64ToF64
            | Opcode::F64ToI64
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
            | Opcode::ReadView
            | Opcode::ReadCommitted
            | Opcode::WriteView
            | Opcode::Br
            | Opcode::CondBr
            | Opcode::Call
            | Opcode::Return
            | Opcode::Trap
            | Opcode::Unreachable
    )
}

impl EirModule {
    /// Whether every function uses only opcodes the threaded dispatcher covers.
    pub fn threaded_supported(&self) -> bool {
        self.functions
            .iter()
            .all(|f| f.instructions.iter().all(|i| is_threaded_op(i.opcode)))
    }

    /// Executes the module with **threaded dispatch** (static handler table).
    /// Requires [`EirModule::threaded_supported`]; otherwise use `execute_with_index`.
    pub(crate) fn execute_threaded_with_index(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        index: &CallIndex,
    ) -> Result<Vec<WorldWrite>> {
        let prog = ThProg {
            functions: &self.functions,
            index_of: &index.index_of,
        };
        let table = handlers();
        let mut writes: Vec<WorldWrite> = Vec::new();
        let _ = env;
        let mut m = ThMachine {
            rt,
            writes: &mut writes,
            frames: Vec::new(),
            pcs: Vec::new(),
            stacks: Vec::new(),
            halt: false,
            pc: 0,
        };
        for &entry in &index.order {
            let f = &self.functions[entry];
            if f.argument_count != 0 {
                continue;
            }
            if f.effect_mask & EIR_EFFECT_BARRIER != 0 {
                m.rt.commit_barrier();
            }
            let root_cap = f.instructions.len() + f.argument_count as usize + 1;
            m.frames = vec![entry];
            m.pcs = vec![0];
            m.stacks = vec![Regs::new(root_cap)];
            m.halt = false;
            loop {
                if m.halt {
                    break;
                }
                let d = m.frames.len();
                let fi = m.frames[d - 1];
                let pc = m.pcs[d - 1];
                if pc >= prog.functions[fi].instructions.len() {
                    th_pop_return(&mut m);
                    continue;
                }
                let ins = &prog.functions[fi].instructions[pc];
                m.pc = pc;
                (table[ins.opcode as usize])(&mut m, ins, &prog)?;
            }
        }
        Ok(writes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_table_is_consistent() {
        // Every opcode round-trips through the wire encoding, has a mnemonic,
        // and has a unique wire value.
        let mut seen = std::collections::BTreeSet::new();
        for &op in Opcode::ALL {
            assert_eq!(Opcode::from_u16(op as u16), Some(op), "{}", op.name());
            assert!(!op.name().is_empty());
            assert!(
                seen.insert(op as u16),
                "duplicate wire value: {}",
                op.name()
            );
        }
        assert!(Opcode::from_u16(0x7fff).is_none());
    }

    fn add(_a: Immediate, _b: Immediate, result: u32, ty: ValueType) -> Instruction {
        Instruction {
            opcode: Opcode::Add,
            result_id: result,
            result_type: Some(ty),
            operands: vec![1, 2],
            constant: None,
            target: None,
        }
    }

    fn const_i32(id: u32, value: i32) -> Instruction {
        Instruction {
            opcode: Opcode::Const,
            result_id: id,
            result_type: Some(ValueType::I32),
            operands: vec![],
            constant: Some(Immediate::I32(value)),
            target: None,
        }
    }

    fn ret() -> Instruction {
        Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        }
    }

    #[test]
    fn eir_valid_function_round_trips_interpretation() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 10),
                    const_i32(2, 5),
                    add(Immediate::I32(0), Immediate::I32(0), 3, ValueType::I32),
                    ret(),
                ],
            }],
        };
        assert!(module.validate(true).is_ok());
    }

    #[test]
    fn eir_rejects_use_before_def() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    add(Immediate::I32(0), Immediate::I32(0), 3, ValueType::I32),
                    ret(),
                ],
            }],
        };
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn eir_rejects_type_mismatch() {
        // Annotate result type differently from computed type.
        let mut ins = add(Immediate::I32(0), Immediate::I32(0), 3, ValueType::I32);
        // operands poorly typed is caught elsewhere; simulate mismatch by
        // constructing an Add whose result_type disagrees with operand types.
        ins.operands = vec![1, 2];
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 1),
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 2,
                        result_type: Some(ValueType::F64),
                        operands: vec![],
                        constant: Some(Immediate::F64(1.0)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Add,
                        result_id: 3,
                        result_type: Some(ValueType::I32),
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn eir_rejects_nondeterministic_effect_in_pure_module() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_TIME,
                argument_count: 0,
                instructions: vec![const_i32(1, 1), ret()],
            }],
        };
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn eir_interpreter_produces_ordered_writes() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 0),
                    const_i32(2, 42),
                    Instruction {
                        opcode: Opcode::Store,
                        result_id: 0,
                        result_type: None,
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    const_i32(4, 1),
                    const_i32(5, 7),
                    Instruction {
                        opcode: Opcode::Store,
                        result_id: 0,
                        result_type: None,
                        operands: vec![4, 5],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        let writes = module.interpret(WorldId(0), WorldVersion(0)).unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].value, 42);
        assert_eq!(writes[1].value, 7);
    }

    #[test]
    fn eir_binary_round_trip_is_byte_identical_and_recomputes_hash() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([7; 32]),
            domain_ir_hash: Hash256([8; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 5,
                effect_mask: EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 10),
                    const_i32(2, 5),
                    add(Immediate::I32(0), Immediate::I32(0), 3, ValueType::I32),
                    Instruction {
                        opcode: Opcode::WriteView,
                        result_id: 0,
                        result_type: None,
                        operands: vec![3],
                        constant: None,
                        target: Some(ComponentRef {
                            entity: 9,
                            component: ComponentTypeId([1; 16]),
                            offset: 4,
                        }),
                    },
                    ret(),
                ],
            }],
        };
        let bytes = module.encode().unwrap();
        assert_eq!(&bytes[..8], b"PWEEIR2\0");
        let decoded = EirModule::decode(&bytes).unwrap();
        assert_eq!(decoded.schema_set_hash, module.schema_set_hash);
        assert_eq!(decoded.domain_ir_hash, module.domain_ir_hash);
        assert_eq!(decoded.functions.len(), 1);
        assert_eq!(decoded.functions[0].id, 5);
        assert_eq!(decoded.functions[0].instructions.len(), 5);
        // Module hash is populated and recomputed on re-encode.
        assert_ne!(decoded.module_hash, Hash256([0; 32]));
        assert_eq!(decoded.encode().unwrap(), bytes);
        assert!(decoded.validate(true).is_ok());
    }

    #[test]
    fn eir_binary_rejects_tampered_module_hash() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([7; 32]),
            domain_ir_hash: Hash256([8; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![const_i32(1, 1), ret()],
            }],
        };
        let mut bytes = module.encode().unwrap();
        // Flip a byte in a section body; the module hash no longer matches.
        bytes[HEADER_LEN + DIR_ENTRY_LEN * 2 + 8] ^= 0xff;
        assert_eq!(
            EirModule::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn eir_binary_rejects_duplicate_section_kind() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([7; 32]),
            domain_ir_hash: Hash256([8; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![const_i32(1, 1), ret()],
            }],
        };
        let mut bytes = module.encode().unwrap();
        // First directory entry kind (u16 LE at byte 120) -> FUNCTIONS (4),
        // duplicating the second entry.
        bytes[120] = KIND_FUNCTIONS as u8;
        assert_eq!(
            EirModule::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
    }

    fn branch_inst(opcode: Opcode, result: u32, operands: Vec<u32>) -> Instruction {
        Instruction {
            opcode,
            result_id: result,
            result_type: None,
            operands,
            constant: None,
            target: None,
        }
    }

    #[test]
    fn eir_branch_control_flow_validates_round_trips_and_interprets() {
        // if %1 { %2 = 10 } else { %3 = 20 }; return
        // Instruction-index targets. CondBr cond=%1, true->2, false->4.
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 1), // %1 = 1 (condition)
                    branch_inst(Opcode::CondBr, 0, vec![1, 2, 4]),
                    const_i32(2, 10), // true path
                    branch_inst(Opcode::Br, 0, vec![6]),
                    const_i32(3, 20), // false path
                    branch_inst(Opcode::Br, 0, vec![6]),
                    ret(), // merge
                ],
            }],
        };
        // The branchy CFG is valid (real terminators + dominance).
        assert!(module.validate(true).is_ok());
        // It round-trips byte-identically through the binary codec.
        let bytes = module.encode().unwrap();
        let decoded = EirModule::decode(&bytes).unwrap();
        assert_eq!(decoded.encode().unwrap(), bytes);
        // The interpreter executes the taken path (cond nonzero -> true) and
        // returns without trapping or erroring.
        assert!(decoded.interpret(WorldId(1), WorldVersion(0)).is_ok());
    }

    #[test]
    fn eir_out_of_range_branch_target_is_rejected() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    const_i32(1, 1),
                    branch_inst(Opcode::CondBr, 0, vec![1, 2, 99]), // target 99 out of range
                    ret(),
                ],
            }],
        };
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn eir_unreachable_terminator_is_accepted_as_block_end() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![branch_inst(Opcode::Unreachable, 0, vec![])],
            }],
        };
        // A function whose only block ends in UNREACHABLE is a valid terminator.
        assert!(module.validate(true).is_ok());
    }

    #[test]
    fn eir_zero_crossing_operators_fire_and_timestamp() {
        // RFC-0048: the three edge operators share a per-site history. Run one
        // function once per step against ONE `ExecEnv` so the history persists,
        // and let it write the edge flag to a tracked component field.
        fn const_f64(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        let hist = |id: u32| Instruction {
            opcode: Opcode::HistRead,
            result_id: id,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: Some(Immediate::U64(0)),
            target: None,
        };
        let has = |id: u32| Instruction {
            opcode: Opcode::HistHas,
            result_id: id,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: Some(Immediate::U64(0)),
            target: None,
        };
        let write = |v: u32| Instruction {
            opcode: Opcode::HistWrite,
            result_id: 0,
            result_type: None,
            operands: vec![v],
            constant: Some(Immediate::U64(0)),
            target: None,
        };
        let edge = |id: u32, op: Opcode, prev: u32, cur: u32, has: u32| Instruction {
            opcode: op,
            result_id: id,
            result_type: Some(ValueType::F64),
            operands: vec![prev, cur, has],
            constant: Some(Immediate::U64(0)),
            target: None,
        };
        // The body: v = value; prev = hist[0]; has = histhas[0]; histwrite(0,v);
        // raw = edge(prev, v); flag = raw * has; write the flag to a field.
        let mk = |op: Opcode, value: f64| EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    const_f64(1, value),
                    hist(2),
                    has(3),
                    write(1),
                    edge(4, op, 2, 1, 3),
                    Instruction {
                        opcode: Opcode::WriteView,
                        result_id: 0,
                        result_type: None,
                        operands: vec![4],
                        constant: None,
                        target: Some(ComponentRef {
                            entity: 1,
                            component: ComponentTypeId([7; 16]),
                            offset: 0,
                        }),
                    },
                    ret(),
                ],
            }],
        };
        // A runtime whose only interesting behavior is capturing the write.
        #[derive(Default)]
        struct Sink {
            last: f64,
        }
        impl EirRuntime for Sink {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0.0f64.to_bits())
            }
            fn write_field(&mut self, _t: ComponentRef, value: u64) {
                self.last = f64::from_bits(value);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _component: ComponentTypeId,
                _i: f64,
                _j: f64,
                _width: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let values = [0.5f64, -0.5, -0.5, 0.5];
        let want_cross = [0.0, 1.0, 0.0, 1.0];
        let want_rise = [0.0, 0.0, 0.0, 1.0];
        let want_fall = [0.0, 1.0, 0.0, 0.0];
        for (op, want, want_last_t) in [
            (Opcode::CrossDown, want_cross, 3.0),
            (Opcode::RiseEdge, want_rise, 3.0),
            (Opcode::FallEdge, want_fall, 1.0),
        ] {
            let mut rt = Sink::default();
            let mut env = ExecEnv::default();
            for (k, value) in values.iter().enumerate() {
                env.time = k as f64;
                mk(op, *value)
                    .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
                    .unwrap();
                assert_eq!(rt.last, want[k], "{} step {k} (value {value})", op.name());
            }
            assert_eq!(
                env.cross_time.get(&0).copied().unwrap_or(-1.0),
                want_last_t,
                "{} timestamps its most recent crossing at t={want_last_t}",
                op.name()
            );
            assert_eq!(env.hist.get(&0), Some(&0.5));
        }
    }

    fn const_u64(id: u32, value: u64) -> Instruction {
        Instruction {
            opcode: Opcode::Const,
            result_id: id,
            result_type: Some(ValueType::U64),
            operands: vec![],
            constant: Some(Immediate::U64(value)),
            target: None,
        }
    }
    fn ret_value(value_id: u32) -> Instruction {
        Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![value_id],
            constant: None,
            target: None,
        }
    }

    #[test]
    fn eir_call_returns_callee_value_and_recursion_guard_traps() {
        // fn 1: %1=const 3; %2 = call 2(%1); return %2
        // fn 2: %arg1(1)=5; %2 = %1+%1; return %2  (double the argument)
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![
                Function {
                    id: 1,
                    effect_mask: 0,
                    argument_count: 0,
                    instructions: vec![
                        const_u64(1, 3),
                        Instruction {
                            opcode: Opcode::Call,
                            result_id: 2,
                            result_type: Some(ValueType::U64),
                            operands: vec![2, 1],
                            constant: None,
                            target: None,
                        },
                        ret_value(2),
                    ],
                },
                Function {
                    id: 2,
                    effect_mask: 0,
                    argument_count: 1,
                    // argument lives in slot %1 (passed by caller).
                    instructions: vec![
                        Instruction {
                            opcode: Opcode::Add,
                            result_id: 2,
                            result_type: Some(ValueType::F64),
                            operands: vec![1, 1],
                            constant: None,
                            target: None,
                        },
                        ret_value(2),
                    ],
                },
            ],
        };
        // Valid: both functions have a terminator and the call target exists.
        assert!(module.validate(true).is_ok());
        // Interpreting yields no writes (no Store), but must not error.
        assert!(module.interpret(WorldId(1), WorldVersion(0)).is_ok());
        // Binary round-trips byte-identically.
        let bytes = module.encode().unwrap();
        assert_eq!(EirModule::decode(&bytes).unwrap().encode().unwrap(), bytes);
        // Effect-mask requirement: a function using CALL with a callee effect is
        // still valid here (callee has no effects); verify a call to a missing
        // function id is rejected.
        let mut bad = module.clone();
        bad.functions[0].instructions[1].operands[0] = 99;
        assert_eq!(bad.validate(true).unwrap_err().status, Status::EirInvalid);
    }

    #[test]
    fn eir_recursion_depth_is_capped() {
        // fn 0: %1 = call 0() (infinite self-recursion). validate is fine; the
        // interpreter must trap (Err) once the depth cap is hit, not overflow.
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Call,
                        result_id: 1,
                        result_type: Some(ValueType::U64),
                        operands: vec![0],
                        constant: None,
                        target: None,
                    },
                    ret_value(1),
                ],
            }],
        };
        assert!(module.validate(true).is_ok());
        assert!(module.interpret(WorldId(1), WorldVersion(0)).is_err());
    }

    #[test]
    fn eir_random_is_deterministic_across_runs() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_RANDOM,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Random,
                        result_id: 1,
                        result_type: Some(ValueType::F64),
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Random,
                        result_id: 2,
                        result_type: Some(ValueType::F64),
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        // Must be rejected in a pure (deterministic) module.
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
        // But accepted when validation is not strict-deterministic, and the RNG
        // is seeded: two runs produce the same event stream / writes.
        let mut env = ExecEnv::default();
        let a = module
            .interpret_with_env(&mut NoopRuntime, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        let mut env2 = ExecEnv::default();
        let b = module
            .interpret_with_env(&mut NoopRuntime, &mut env2, WorldId(1), WorldVersion(0))
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn eir_time_random_emit_require_declared_effect() {
        // A function using TIME but not declaring EIR_EFFECT_TIME is invalid.
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0, // missing TIME effect
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Time,
                        result_id: 1,
                        result_type: Some(ValueType::F64),
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        assert_eq!(
            module.validate(true).unwrap_err().status,
            Status::EirInvalid
        );
        // With the effect declared, it validates and reads env.time.
        let ok = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_TIME,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Time,
                        result_id: 1,
                        result_type: Some(ValueType::F64),
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        assert!(ok.validate(false).is_ok());
        // But TIME is nondeterministic: rejected in a pure module even with the
        // effect declared.
        assert_eq!(ok.validate(true).unwrap_err().status, Status::EirInvalid);
    }

    #[test]
    fn eir_emit_event_produces_ordered_events() {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_IO,
                argument_count: 0,
                instructions: vec![
                    const_u64(1, 7),
                    const_u64(2, 99),
                    Instruction {
                        opcode: Opcode::EmitEvent,
                        result_id: 0,
                        result_type: None,
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        let mut env = ExecEnv::default();
        module
            .interpret_with_env(&mut NoopRuntime, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        assert_eq!(
            env.events,
            vec![EmittedEvent {
                kind: 7,
                payload: 99,
                seq: 0,
            }]
        );
    }

    /// The canonical EIR codec must round-trip every opcode: the decode table
    /// once drifted (newly added opcodes decoded to `EirInvalid 28`, silently
    /// breaking the artifact path). Keep this list covering the extended space;
    /// the exhaustive opcode match in `tests/prop.rs` guards the encode side.
    #[test]
    fn extended_opcodes_round_trip_through_the_codec() {
        let opcodes = [
            Opcode::NeighborCount,
            Opcode::NearestDist,
            Opcode::Step,
            Opcode::ReadSlotDyn,
            Opcode::WriteSlotDyn,
            Opcode::ReadFieldCell,
            Opcode::WriteFieldCell,
            Opcode::FieldLaplacian,
            Opcode::ReadEvent,
            Opcode::NeighborMean,
            Opcode::NearestOffsetX,
            Opcode::NearestOffsetY,
            Opcode::NearestOffsetZ,
            Opcode::FiredAt,
            Opcode::FiredEvery,
            Opcode::ScheduleEvent,
            Opcode::CrossDown,
            Opcode::RiseEdge,
            Opcode::FallEdge,
            Opcode::LastCross,
            Opcode::EventCount,
            Opcode::NextEventTime,
            Opcode::NextEventKind,
            Opcode::NextEventPayload,
            Opcode::PopEvent,
            Opcode::EventSeenCount,
            Opcode::ScheduleEventAt,
            Opcode::NextEventPriority,
            Opcode::SeizeResource,
            Opcode::ReleaseResource,
            Opcode::ResourceBusy,
            Opcode::ResourceCapacity,
            Opcode::BoundsCheck,
        ];
        let mut instructions: Vec<Instruction> = opcodes
            .iter()
            .enumerate()
            .map(|(k, &opcode)| Instruction {
                opcode,
                result_id: (k as u32) + 1,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: None,
                target: None,
            })
            .collect();
        instructions.push(Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        });
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: 0,
                argument_count: 0,
                instructions: instructions.clone(),
            }],
        };
        let bytes = module.encode().expect("encode");
        let decoded = EirModule::decode(&bytes).expect("decode");
        let got = &decoded.functions[0].instructions;
        assert_eq!(got.len(), instructions.len());
        for (k, want) in opcodes.iter().enumerate() {
            assert_eq!(
                got[k].opcode, *want,
                "{want:?} did not survive the EIR codec round-trip"
            );
        }
    }

    /// RFC-0048 slice B: the calendar opcodes read/pop the pending queue in
    /// `(time, seq)` order; equal-time ties follow insertion order.
    #[test]
    fn eir_calendar_reads_and_pop_in_time_seq_order() {
        fn cst(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        fn put(gate: u32, delay: u32, kind: u32, payload: u32) -> Instruction {
            Instruction {
                opcode: Opcode::ScheduleEvent,
                result_id: 0,
                result_type: None,
                operands: vec![gate, delay, kind, payload],
                constant: None,
                target: None,
            }
        }
        fn cal(id: u32, op: Opcode) -> Instruction {
            Instruction {
                opcode: op,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: None,
                target: None,
            }
        }
        fn wr(v: u32) -> Instruction {
            Instruction {
                opcode: Opcode::WriteView,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: 1,
                    component: ComponentTypeId([7; 16]),
                    offset: 0,
                }),
            }
        }
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    // Both schedules fire at t=0 + delay, so with delays 2 and 3
                    // they enqueue at t=2 and t=3; the gate is a plain 1.0.
                    cst(1, 1.0),
                    cst(2, 2.0),
                    cst(3, 5.0),
                    cst(4, 50.0),
                    put(1, 2, 3, 4),
                    cst(5, 3.0),
                    cst(6, 6.0),
                    cst(7, 60.0),
                    put(1, 5, 6, 7),
                    cal(8, Opcode::EventCount),
                    cal(9, Opcode::NextEventTime),
                    cal(10, Opcode::NextEventKind),
                    cal(11, Opcode::NextEventPayload),
                    cal(12, Opcode::PopEvent),
                    cal(13, Opcode::EventCount),
                    cal(14, Opcode::NextEventTime),
                    wr(8),
                    wr(9),
                    wr(10),
                    wr(11),
                    wr(12),
                    wr(13),
                    wr(14),
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        #[derive(Default)]
        struct Track {
            writes: Vec<u64>,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0)
            }
            fn write_field(&mut self, _t: ComponentRef, v: u64) {
                self.writes.push(v);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _c: ComponentTypeId,
                _i: f64,
                _j: f64,
                _w: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let mut rt = Track::default();
        let mut env = ExecEnv {
            time: 0.0,
            step_dt: 1.0,
            ..ExecEnv::default()
        };
        module
            .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        let got: Vec<f64> = rt.writes.iter().map(|b| f64::from_bits(*b)).collect();
        assert_eq!(
            got,
            vec![2.0, 2.0, 5.0, 50.0, 50.0, 1.0, 3.0],
            "count, time, kind, payload, pop payload, remaining count, remaining time"
        );
        assert_eq!(env.queue.len(), 1);
        assert_eq!(env.queue[0].kind, 6, "the t=3 entry remains after the pop");
    }

    /// RFC-0048 slice C1: `ScheduleEventAt` orders the calendar by
    /// `(time, priority, seq)`.
    #[test]
    fn eir_calendar_priority_orders_within_a_time() {
        fn cst(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        fn put_at(gate: u32, delay: u32, kind: u32, payload: u32, prio: u32) -> Instruction {
            Instruction {
                opcode: Opcode::ScheduleEventAt,
                result_id: 0,
                result_type: None,
                operands: vec![gate, delay, kind, payload, prio],
                constant: None,
                target: None,
            }
        }
        fn cal(id: u32, op: Opcode) -> Instruction {
            Instruction {
                opcode: op,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: None,
                target: None,
            }
        }
        fn wr(v: u32) -> Instruction {
            Instruction {
                opcode: Opcode::WriteView,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: 1,
                    component: ComponentTypeId([7; 16]),
                    offset: 0,
                }),
            }
        }
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    cst(1, 1.0),
                    cst(2, 2.0),
                    // kind 5 at priority 10
                    cst(3, 5.0),
                    cst(4, 50.0),
                    cst(5, 10.0),
                    put_at(1, 2, 3, 4, 5),
                    // kind 6 at priority 1 (earlier)
                    cst(6, 6.0),
                    cst(7, 60.0),
                    cst(8, 1.0),
                    put_at(1, 2, 6, 7, 8),
                    cal(9, Opcode::NextEventKind),
                    cal(10, Opcode::NextEventPriority),
                    cal(11, Opcode::PopEvent),
                    cal(12, Opcode::NextEventKind),
                    cal(13, Opcode::PopEvent),
                    wr(9),
                    wr(10),
                    wr(11),
                    wr(12),
                    wr(13),
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        #[derive(Default)]
        struct Track {
            writes: Vec<u64>,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0)
            }
            fn write_field(&mut self, _t: ComponentRef, v: u64) {
                self.writes.push(v);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _c: ComponentTypeId,
                _i: f64,
                _j: f64,
                _w: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let mut rt = Track::default();
        let mut env = ExecEnv {
            time: 0.0,
            step_dt: 1.0,
            ..ExecEnv::default()
        };
        module
            .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        let got: Vec<f64> = rt.writes.iter().map(|b| f64::from_bits(*b)).collect();
        assert_eq!(
            got,
            vec![6.0, 1.0, 60.0, 5.0, 50.0],
            "priority 1 before priority 10, then each pops once"
        );
        assert!(env.queue.is_empty());
    }

    /// RFC-0044: `BoundsCheck(index, base, len)` yields the absolute slot
    /// `base + index` for an in-range index and traps (detail 18) otherwise.
    #[test]
    fn eir_bounds_check_maps_and_traps() {
        fn cst(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        fn bounds(id: u32, idx: u32, base: u32, len: u32) -> Instruction {
            Instruction {
                opcode: Opcode::BoundsCheck,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![idx, base, len],
                constant: None,
                target: None,
            }
        }
        fn wr(v: u32) -> Instruction {
            Instruction {
                opcode: Opcode::WriteView,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: 1,
                    component: ComponentTypeId([9; 16]),
                    offset: 0,
                }),
            }
        }
        #[derive(Default)]
        struct Track {
            writes: Vec<u64>,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0)
            }
            fn write_field(&mut self, _t: ComponentRef, v: u64) {
                self.writes.push(v);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _c: ComponentTypeId,
                _i: f64,
                _j: f64,
                _w: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let mk = |idx: f64| EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    cst(1, idx),
                    cst(2, 2.0),
                    cst(3, 3.0),
                    bounds(4, 1, 2, 3),
                    wr(4),
                    ret(),
                ],
            }],
        };
        // In range: 2 + 1 == 3.
        let mut rt = Track::default();
        let mut env = ExecEnv::default();
        mk(1.0)
            .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        assert_eq!(
            rt.writes
                .iter()
                .map(|b| f64::from_bits(*b))
                .collect::<Vec<_>>(),
            vec![3.0]
        );
        // Out of range, negative, and fractional indices all trap (detail 18).
        for bad in [3.0, -1.0, 0.5, f64::NAN, f64::INFINITY] {
            let mut rt = Track::default();
            let mut env = ExecEnv::default();
            let e = mk(bad)
                .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
                .unwrap_err();
            assert_eq!(e.detail, 18, "index {bad} must trap");
        }
    }

    /// RFC-0048 slice C2: `SeizeResource`/`ReleaseResource` guard on capacity;
    /// the busy/capacity reads reflect the execution context's resource table.
    #[test]
    fn eir_resource_seize_release_respects_capacity() {
        fn cst(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        fn res(id: u32, op: Opcode, rid: u64, operand: Option<u32>) -> Instruction {
            Instruction {
                opcode: op,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: operand.into_iter().collect(),
                constant: Some(Immediate::U64(rid)),
                target: None,
            }
        }
        fn wr(v: u32) -> Instruction {
            Instruction {
                opcode: Opcode::WriteView,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: 1,
                    component: ComponentTypeId([7; 16]),
                    offset: 0,
                }),
            }
        }
        let rid = 0x8000_0000_0000_0042;
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    cst(1, 2.0),
                    res(2, Opcode::SeizeResource, rid, Some(1)),
                    res(3, Opcode::SeizeResource, rid, Some(1)),
                    res(4, Opcode::SeizeResource, rid, Some(1)),
                    res(5, Opcode::ResourceBusy, rid, None),
                    res(6, Opcode::ResourceCapacity, rid, None),
                    res(7, Opcode::ReleaseResource, rid, None),
                    res(8, Opcode::SeizeResource, rid, Some(1)),
                    wr(2),
                    wr(3),
                    wr(4),
                    wr(5),
                    wr(6),
                    wr(7),
                    wr(8),
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        #[derive(Default)]
        struct Track {
            writes: Vec<u64>,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0)
            }
            fn write_field(&mut self, _t: ComponentRef, v: u64) {
                self.writes.push(v);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _c: ComponentTypeId,
                _i: f64,
                _j: f64,
                _w: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let mut rt = Track::default();
        let mut env = ExecEnv::default();
        module
            .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        let got: Vec<f64> = rt.writes.iter().map(|b| f64::from_bits(*b)).collect();
        assert_eq!(
            got,
            vec![1.0, 1.0, 0.0, 2.0, 2.0, 1.0, 1.0],
            "two seizes succeed, the third is refused, then a release frees one"
        );
        assert_eq!(env.resources.get(&rid).map(|r| r.busy), Some(2));
    }

    /// RFC-0048 slice B: equal-time entries pop in insertion order (`seq`).
    #[test]
    fn eir_calendar_equal_time_ties_use_insertion_order() {
        fn cst(id: u32, v: f64) -> Instruction {
            Instruction {
                opcode: Opcode::Const,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(v)),
                target: None,
            }
        }
        fn put(gate: u32, delay: u32, kind: u32, payload: u32) -> Instruction {
            Instruction {
                opcode: Opcode::ScheduleEvent,
                result_id: 0,
                result_type: None,
                operands: vec![gate, delay, kind, payload],
                constant: None,
                target: None,
            }
        }
        fn pop(id: u32) -> Instruction {
            Instruction {
                opcode: Opcode::PopEvent,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: None,
                target: None,
            }
        }
        fn wr(v: u32) -> Instruction {
            Instruction {
                opcode: Opcode::WriteView,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: 1,
                    component: ComponentTypeId([7; 16]),
                    offset: 0,
                }),
            }
        }
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: vec![
                    cst(1, 1.0),
                    cst(2, 2.0),
                    cst(3, 5.0),
                    cst(4, 50.0),
                    put(1, 2, 3, 4),
                    cst(5, 6.0),
                    cst(6, 60.0),
                    put(1, 2, 5, 6),
                    pop(7),
                    pop(8),
                    wr(7),
                    wr(8),
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        #[derive(Default)]
        struct Track {
            writes: Vec<u64>,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(0)
            }
            fn write_field(&mut self, _t: ComponentRef, v: u64) {
                self.writes.push(v);
            }
            fn query_neighbor_count(&self, _e: u128, _r: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _e: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _e: u128, _s: u32, _r: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _e: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _c: ComponentTypeId,
                _i: f64,
                _j: f64,
                _w: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let mut rt = Track::default();
        let mut env = ExecEnv {
            time: 0.0,
            step_dt: 1.0,
            ..ExecEnv::default()
        };
        module
            .interpret_with_env(&mut rt, &mut env, WorldId(1), WorldVersion(0))
            .unwrap();
        let got: Vec<f64> = rt.writes.iter().map(|b| f64::from_bits(*b)).collect();
        assert_eq!(
            got,
            vec![50.0, 60.0],
            "the first-inserted tied entry pops first"
        );
        assert!(env.queue.is_empty());
    }

    #[test]
    fn eir_atomic_read_modify_write_returns_old_and_writes_sum() {
        // Atomic on a component field with rhs = 5, initial field = 10 via a
        // tracking runtime. Result (old) = 10, write value = 15.
        struct Track {
            field: u64,
            last_write: u64,
        }
        impl EirRuntime for Track {
            fn read_field(&self, _t: ComponentRef) -> Result<u64> {
                Ok(self.field)
            }
            fn write_field(&mut self, _t: ComponentRef, value: u64) {
                self.field = value;
                self.last_write = value;
            }
            fn query_neighbor_count(&self, _entity: u128, _radius: f64) -> Result<u64> {
                Ok(0)
            }
            fn query_nearest_dist(&self, _entity: u128) -> Result<u64> {
                Ok(f64::MAX.to_bits())
            }
            fn query_neighbor_mean(&self, _entity: u128, _slot: u32, _radius: f64) -> Result<f64> {
                Ok(0.0)
            }
            fn query_nearest_offset(&self, _entity: u128) -> Result<(f64, f64, f64)> {
                Ok((0.0, 0.0, 0.0))
            }
            fn field_laplacian(
                &self,
                _component: ComponentTypeId,
                _i: f64,
                _j: f64,
                _width: f64,
            ) -> Result<f64> {
                Ok(0.0)
            }
        }
        let target = ComponentRef {
            entity: 1,
            component: ComponentTypeId([0; 16]),
            offset: 0,
        };
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: EIR_EFFECT_ATOMIC,
                argument_count: 0,
                instructions: vec![
                    const_u64(1, 5), // rhs
                    Instruction {
                        opcode: Opcode::Atomic,
                        result_id: 2,
                        result_type: Some(ValueType::U64),
                        operands: vec![1],
                        constant: None,
                        target: Some(target),
                    },
                    ret(),
                ],
            }],
        };
        assert!(module.validate(false).is_ok());
        let mut rt = Track {
            field: 10,
            last_write: 0,
        };
        let writes = module
            .interpret_with(&mut rt, WorldId(1), WorldVersion(0))
            .unwrap();
        assert_eq!(rt.field, 15); // 10 + 5
        assert_eq!(rt.last_write, 15);
        // The write is emitted as a WorldWrite with the summed value.
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].value, 15);
    }

    #[test]
    fn extended_math_opcodes_compute_correctly() {
        // Write the result of a new math op to a component field and check it.
        fn unary(op: Opcode, value: f64) -> Vec<Instruction> {
            let target = ComponentRef {
                entity: 1,
                component: ComponentTypeId([0; 16]),
                offset: 0,
            };
            vec![
                Instruction {
                    opcode: Opcode::Const,
                    result_id: 1,
                    result_type: Some(ValueType::F64),
                    operands: vec![],
                    constant: Some(Immediate::F64(value)),
                    target: None,
                },
                Instruction {
                    opcode: op,
                    result_id: 2,
                    result_type: Some(ValueType::F64),
                    operands: vec![1],
                    constant: None,
                    target: None,
                },
                Instruction {
                    opcode: Opcode::WriteView,
                    result_id: 0,
                    result_type: None,
                    operands: vec![2],
                    constant: None,
                    target: Some(target),
                },
            ]
        }
        let module = |op: Opcode, value: f64| EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 0,
                effect_mask: 0,
                argument_count: 0,
                instructions: {
                    let mut v = unary(op, value);
                    v.push(Instruction {
                        opcode: Opcode::Return,
                        result_id: 0,
                        result_type: None,
                        operands: vec![],
                        constant: None,
                        target: None,
                    });
                    v
                },
            }],
        };
        let run = |op: Opcode, value: f64| -> f64 {
            let writes = module(op, value)
                .interpret(WorldId(1), WorldVersion(0))
                .unwrap();
            f64::from_bits(writes[0].value)
        };
        assert_eq!(run(Opcode::Abs, -3.0), 3.0);
        assert_eq!(run(Opcode::Floor, 3.7), 3.0);
        assert_eq!(run(Opcode::Ceil, 2.1), 3.0);
        assert_eq!(run(Opcode::Round, 2.6), 3.0);
        assert_eq!(run(Opcode::Sign, -5.0), -1.0);
        assert_eq!(run(Opcode::Log10, 100.0), 2.0);
        assert_eq!(run(Opcode::Log2, 8.0), 3.0);
        assert_eq!(run(Opcode::Tanh, 0.0), 0.0);
        assert_eq!(run(Opcode::Sinh, 0.0), 0.0);
    }
}

#[test]
fn eir_cfg_blocks_round_trip() {
    // RFC-0046: a branched function encodes as explicit basic blocks
    // (block_count > 1) and round-trips byte-identically.
    let ins = |opcode, result_id, operands: Vec<u32>, constant| Instruction {
        opcode,
        result_id,
        result_type: None,
        operands,
        constant,
        target: None,
    };
    let module = EirModule {
        module_hash: Hash256([0; 32]),
        schema_set_hash: Hash256([0; 32]),
        domain_ir_hash: Hash256([0; 32]),
        target_kind: 0,
        functions: vec![Function {
            id: 1,
            effect_mask: 0,
            argument_count: 0,
            instructions: vec![
                ins(Opcode::Const, 1, vec![], Some(Immediate::F64(1.0))),
                ins(Opcode::CondBr, 0, vec![1, 3, 5], None),
                ins(Opcode::Return, 0, vec![], None),
                ins(Opcode::Const, 2, vec![], Some(Immediate::F64(2.0))),
                ins(Opcode::Return, 0, vec![2], None),
                ins(Opcode::Const, 3, vec![], Some(Immediate::F64(3.0))),
                ins(Opcode::Return, 0, vec![3], None),
            ],
        }],
    };
    module.validate(true).unwrap();
    assert!(module.verify_cfg().is_ok());
    let blocks = module.blocks(0).unwrap();
    assert!(blocks.len() >= 3, "a branch should create multiple blocks");
    let encoded = module.encode().unwrap();
    let decoded = EirModule::decode(&encoded).unwrap();
    decoded.validate(true).unwrap();
    assert_eq!(decoded.functions[0].instructions.len(), 7);
    // Deterministic: re-encoding the decoded module is byte-identical.
    assert_eq!(decoded.encode().unwrap(), encoded);
}

#[test]
fn eir_encode_does_not_panic_on_degenerate_input() {
    // #73: `encode` partitions via `blocks_from_instructions`; an empty stream
    // and an out-of-range branch target must not panic there.
    let base = || EirModule {
        module_hash: Hash256([0; 32]),
        schema_set_hash: Hash256([0; 32]),
        domain_ir_hash: Hash256([0; 32]),
        target_kind: 0,
        functions: vec![],
    };
    // Empty function body.
    let mut m = base();
    m.functions.push(Function {
        id: 1,
        effect_mask: 0,
        argument_count: 0,
        instructions: vec![],
    });
    assert!(m.encode().is_ok());

    // Out-of-range CondBr target (a hand-built, unvalidated module).
    let mut m = base();
    m.functions.push(Function {
        id: 1,
        effect_mask: 0,
        argument_count: 0,
        instructions: vec![
            Instruction {
                opcode: Opcode::Const,
                result_id: 1,
                result_type: None,
                operands: vec![],
                constant: Some(Immediate::F64(1.0)),
                target: None,
            },
            Instruction {
                opcode: Opcode::CondBr,
                result_id: 0,
                result_type: None,
                operands: vec![1, 99, 100],
                constant: None,
                target: None,
            },
        ],
    });
    assert!(
        m.encode().is_ok(),
        "out-of-range targets must not panic encode"
    );
}
