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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Opcode {
    Nop = 0,
    Const = 1,
    Add = 16,
    Sub = 17,
    Mul = 18,
    Div = 19,
    Rem = 20,
    /// Unary transcendental / elementary functions (single f64 operand).
    Sin = 128,
    Cos = 129,
    Exp = 130,
    Ln = 131,
    Sqrt = 132,
    /// `pow(base, exponent)`.
    Pow = 133,
    Eq = 32,
    Ne = 33,
    Lt = 34,
    Le = 35,
    Gt = 36,
    Ge = 37,
    /// Select: if cond (operand 0) is nonzero, result = operand 1 else operand 2.
    Select = 40,
    /// Intra-module call: operand 0 = target function id, operands 1.. = argument
    /// value ids. Result = the callee's return value (its `Return` operand 0).
    Call = 48,
    /// Read one scalar field of a component into the value stack (field offset
    /// is a compile-time operand). RFC-0021 `READ_VIEW=64`. Yields the field's
    /// f64/u64 value.
    ReadView = 64,
    /// Write one scalar field of a component from the value stack.
    /// RFC-0021 `WRITE_VIEW=65`.
    WriteView = 65,
    Load = 66,
    Store = 67,
    /// Atomic read-modify-write on a component field (add). Reads the field,
    /// writes field + rhs, yields the old value. Effect `ATOMIC`.
    Atomic = 68,
    /// Emit an ordered event `(kind, payload)` (RFC-0023). Operands:
    /// `kind`, `payload`. Effect `IO`.
    EmitEvent = 69,
    /// Read the explicit sim time from the execution context. Effect `TIME`.
    Time = 70,
    /// Draw a (deterministically seeded) random value. Effect `RANDOM`.
    Random = 71,
    /// External input/output through the execution context. Effect `IO`.
    Io = 72,
    // -- extended math (unary f64 -> f64) --
    Abs = 192,
    Floor = 193,
    Ceil = 194,
    Round = 195,
    Sign = 196,
    Log10 = 197,
    Log2 = 198,
    Sinh = 199,
    Cosh = 200,
    Tanh = 201,
    Asin = 202,
    Acos = 203,
    Atan = 204,
    // -- extended math (binary f64 x f64 -> f64) --
    Atan2 = 205,
    Hypot = 206,
    /// Debugging `print`: logs operand 0 to the execution context and yields it
    /// back unchanged (semantically transparent). No effect bit — a debug
    /// side-channel that never changes world state.
    Print = 207,
    // -- spatial queries (read-only, deterministic; served by the EirRuntime) --
    /// Count the entities (other than the target entity) whose position lies
    /// within `radius` of the target entity's position. Operand 0 = radius
    /// value id; target = the querying entity. Yields the count as f64.
    NeighborCount = 208,
    /// Distance to the nearest entity other than the target entity; `f64::MAX`
    /// when the target entity is alone. Target = the querying entity. Yields
    /// the distance as f64.
    NearestDist = 209,
    /// Read the host-advanced step counter from the execution context.
    /// Deterministic (advances by 1 per step); no effect bit.
    Step = 210,
    // -- dynamic slot access (runtime index into the State component) --
    /// Read the State slot at a runtime index. Operand 0 = index value id;
    /// target = the owning entity (component = the State component). Yields
    /// the slot's f64 value. The byte offset is `index * STATE_SLOT_STRIDE`.
    ReadSlotDyn = 211,
    /// Write the State slot at a runtime index. Operand 0 = index value id,
    /// operand 1 = value id; target = the owning entity.
    WriteSlotDyn = 212,
    // -- grid field access (the PDE substrate; runtime cell coordinates) --
    /// Read a grid field cell. Operand 0 = i value id, operand 1 = j value id;
    /// target = ComponentRef whose component is the field's canonical id and
    /// whose offset is the field's width (compile-time, from the model).
    /// Yields the cell's f64 value.
    ReadFieldCell = 213,
    /// Write a grid field cell. Operand 0 = i, operand 1 = j, operand 2 =
    /// value id; target as for `ReadFieldCell`.
    WriteFieldCell = 214,
    /// Discrete Laplacian of a grid field cell (the Field's zero-flux stencil).
    /// Operand 0 = i, operand 1 = j; target as for `ReadFieldCell`. Yields f64.
    FieldLaplacian = 215,
    /// Read the payload of the most recent event with a given kind, from the
    /// events emitted so far in this interpretation. Operand 0 = kind value
    /// id. Yields the payload as f64, or 0.0 when no event of that kind has
    /// been emitted (deterministic reverse scan).
    ReadEvent = 216,
    Return = 0x8000,
    /// Unconditional branch to an instruction index (block target). Single
    /// operand = target index.
    Br = 0x8001,
    /// Conditional branch: operand 0 = condition value id, operand 1 = true
    /// target index, operand 2 = false target index.
    CondBr = 0x8002,
    Trap = 0x8003,
    /// Marks a block that must not be reached (RFC-0021 UNREACHABLE). Traps.
    Unreachable = 0x8004,
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
}
impl Default for ExecEnv {
    fn default() -> Self {
        Self {
            time: 0.0,
            rng: SeededRng::new(),
            events: Vec::new(),
            log: Vec::new(),
            step: 0,
        }
    }
}

/// Maximum CALL nesting depth. Exceeding it traps (guards against unbounded
/// or cyclic recursion).
pub const MAX_CALL_DEPTH: usize = 256;

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
    /// Applies a scalar write during interpretation so later reads observe it.
    fn write_field(&mut self, target: ComponentRef, value: u64);
    /// Counts the entities (other than `entity`) whose position lies within
    /// `radius` of `entity`'s position. Deterministic: entities are scanned in
    /// sorted id order, and positions observe in-interpretation writes.
    fn query_neighbor_count(&self, entity: u128, radius: f64) -> Result<u64>;
    /// Distance to the nearest entity other than `entity`; `f64::MAX` when
    /// there is no other entity. Same determinism as `query_neighbor_count`.
    fn query_nearest_dist(&self, entity: u128) -> Result<u64>;
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
}

#[derive(Clone, Debug)]
pub struct EirModule {
    pub module_hash: Hash256,
    pub schema_set_hash: Hash256,
    pub domain_ir_hash: Hash256,
    pub target_kind: u16,
    pub functions: Vec<Function>,
}

impl EirModule {
    /// Validates SSA order, single definition, types, and declared effects.
    /// `pure` marks the module as deterministic (no time/random/io/device/
    /// network effects permitted).
    pub fn validate(&self, pure: bool) -> Result<()> {
        for function in &self.functions {
            if pure && function.effect_mask & NONDETERMINISTIC != 0 {
                return Err(error(Status::EirInvalid, 1, function.id as usize));
            }
            self.validate_function(function)?;
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
            let terminator = match function.instructions.last().map(|i| i.opcode) {
                Some(Opcode::Trap) => crate::dominance::Terminator::Trap,
                _ => crate::dominance::Terminator::Return,
            };
            let block = crate::dominance::Block::new(0, 0, function.instructions.len(), terminator);
            crate::dominance::verify_dominance_with_args(
                &function.instructions,
                &[block],
                function.argument_count,
            )?;
        }
        Ok(())
    }

    fn validate_function(&self, function: &Function) -> Result<()> {
        // SSA value table: id -> (defining instruction index, type, defined flag).
        let mut defs: BTreeMap<u32, (ValueType, bool)> = BTreeMap::new();
        let mut result_count = 0usize;
        // Required effect bits the function must declare (RFC-0021 effect mask).
        let mut required_effects: u32 = 0;
        // Function arguments are pre-defined SSA slots (typed F64 by default, matching
        // the reference's F64-centric component values).
        for slot in 1..=function.argument_count {
            defs.insert(slot, (ValueType::F64, true));
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
                    if !self.functions.iter().any(|f| f.id == target_id) {
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
                Opcode::Print => {
                    // `print(x)`: exactly one f64 operand; result is that value
                    // (transparent). Logs to the execution context.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    let t = self.lookup_type(function, instruction.operands[0]);
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
                    // arithmetic: operands must be a consistent numeric type.
                    let ty = self.numeric_type(function, &instruction.operands, index)?;
                    Some(ty)
                }
                Opcode::Atan2 | Opcode::Hypot => {
                    // binary f64 function: exactly two f64 operands, result f64.
                    if instruction.operands.len() != 2 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    let a = self.lookup_type(function, instruction.operands[0]);
                    let b = self.lookup_type(function, instruction.operands[1]);
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
                    if self.lookup_type(function, instruction.operands[0]) != Some(ValueType::F64) {
                        return Err(error(Status::EirInvalid, 13, index));
                    }
                    Some(ValueType::F64)
                }
                Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
                    self.numeric_type(function, &instruction.operands, index)?;
                    Some(ValueType::Bool)
                }
                Opcode::Load => {
                    // load: one address operand (integer), yields stored type.
                    if instruction.operands.len() != 1 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    Some(ValueType::U64)
                }
                Opcode::ReadView => Some(ValueType::F64),
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
                Opcode::ReadFieldCell | Opcode::WriteFieldCell | Opcode::FieldLaplacian => {
                    // Grid field access: the field's width rides in the
                    // target's offset (compile-time, from the model).
                    let want = match instruction.opcode {
                        Opcode::WriteFieldCell => 3,
                        Opcode::FieldLaplacian => 2,
                        _ => 2,
                    };
                    if instruction.operands.len() != want || instruction.target.is_none() {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    if instruction.opcode == Opcode::WriteFieldCell {
                        None
                    } else {
                        Some(ValueType::F64)
                    }
                }
                Opcode::WriteView => None,
                Opcode::Select => {
                    if instruction.operands.len() != 3 {
                        return Err(error(Status::EirInvalid, 4, index));
                    }
                    // Result type follows the selected operands.
                    let a = self.lookup_type(function, instruction.operands[1]);
                    let b = self.lookup_type(function, instruction.operands[2]);
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
                let defined = defs.get(&operand).is_some_and(|(_, d)| *d);
                if !defined {
                    return Err(error(Status::EirInvalid, 5, index));
                }
            }

            // Record this instruction's result definition.
            if let Some(result_id) = non_zero(instruction.result_id) {
                let ty = declared_type.ok_or(error(Status::EirInvalid, 6, index))?;
                if let Some((_, already)) = defs.get(&result_id) {
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
                defs.insert(result_id, (ty, true));
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

    fn numeric_type(
        &self,
        function: &Function,
        operands: &[u32],
        index: usize,
    ) -> Result<ValueType> {
        if operands.is_empty() {
            return Err(error(Status::EirInvalid, 11, index));
        }
        // All arithmetic operands must resolve to the same numeric type.
        // We look types up from the collected defs (built during validation);
        // here we re-scan earlier instructions for each operand type.
        let types: Vec<Option<ValueType>> = operands
            .iter()
            .map(|id| self.lookup_type(function, *id))
            .collect();
        let first = types[0].ok_or(error(Status::EirInvalid, 12, index))?;
        for ty in types.iter().skip(1) {
            if *ty != Some(first) {
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

    fn lookup_type(&self, function: &Function, id: u32) -> Option<ValueType> {
        if id != 0 && id <= function.argument_count {
            return Some(ValueType::F64); // function argument slot
        }
        function.instructions.iter().find_map(|i| {
            if i.result_id == id {
                // Prefer an explicit annotation (present on all lowered ops),
                // then a constant, then opcode-inferred defaults.
                if let Some(t) = i.result_type {
                    return Some(t);
                }
                i.constant.map(Immediate::ty).or(match i.opcode {
                    Opcode::Eq | Opcode::Ne | Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge => {
                        Some(ValueType::Bool)
                    }
                    Opcode::ReadView | Opcode::NeighborCount | Opcode::NearestDist => {
                        Some(ValueType::F64)
                    }
                    Opcode::Step => Some(ValueType::F64),
                    Opcode::ReadSlotDyn => Some(ValueType::F64),
                    Opcode::ReadFieldCell | Opcode::FieldLaplacian | Opcode::ReadEvent => {
                        Some(ValueType::F64)
                    }
                    Opcode::Const => i.constant.map(Immediate::ty),
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
                    | Opcode::Atan
                    | Opcode::Atan2
                    | Opcode::Hypot
                    | Opcode::Print
                    | Opcode::Pow
                    | Opcode::Add
                    | Opcode::Sub
                    | Opcode::Mul
                    | Opcode::Div
                    | Opcode::Rem
                    | Opcode::Time
                    | Opcode::Random
                    | Opcode::Io => Some(ValueType::F64),
                    _ => Some(ValueType::U64),
                })
            } else {
                None
            }
        })
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
    /// validate with the appropriate purity before delegating here.
    fn execute(
        &self,
        rt: &mut dyn EirRuntime,
        env: &mut ExecEnv,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        let _ = (world, version);
        let mut writes: Vec<WorldWrite> = Vec::new();
        let mut functions = self.functions.clone();
        functions.sort_by_key(|f| f.id);
        let index_of: BTreeMap<u64, usize> = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        for entry in 0..functions.len() {
            // Only argument-less functions are entry points; functions that take
            // arguments are reached exclusively via `CALL`.
            if functions[entry].argument_count == 0 {
                self.run_call_tree(rt, env, &functions, &index_of, entry, &mut writes)?;
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
        index_of: &BTreeMap<u64, usize>,
        entry: usize,
        writes: &mut Vec<WorldWrite>,
    ) -> Result<()> {
        let mut frames: Vec<usize> = vec![entry];
        let mut pcs: Vec<usize> = vec![0usize];
        let mut stacks: Vec<BTreeMap<u32, Immediate>> = vec![BTreeMap::new()];
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
                    let mut callee: BTreeMap<u32, Immediate> = BTreeMap::new();
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
                    env.events.push(EmittedEvent { kind, payload });
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
        stacks: &mut Vec<BTreeMap<u32, Immediate>>,
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
            out.u32(1)?; // block_count
            out.u32(0)?; // block_id
            out.u32(f.argument_count)?;
            out.u32(f.instructions.len() as u32)?;
            for ins in &f.instructions {
                encode_instruction(&mut out, ins)?;
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
        let stored_hash = Hash256(input.fixed(32)?.try_into().unwrap());
        let schema_set_hash = Hash256(input.fixed(32)?.try_into().unwrap());
        let domain_ir_hash = Hash256(input.fixed(32)?.try_into().unwrap());
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
    let mut starts: Vec<usize> = vec![0usize];
    for ins in instructions {
        match ins.opcode {
            Opcode::Br => starts.push(ins.operands.first().copied().unwrap_or(0) as usize),
            Opcode::CondBr => {
                starts.push(ins.operands.get(1).copied().unwrap_or(0) as usize);
                starts.push(ins.operands.get(2).copied().unwrap_or(0) as usize);
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
        let entity = u128::from_le_bytes(input.fixed(16)?.try_into().unwrap());
        let component = ComponentTypeId(input.fixed(16)?.try_into().unwrap());
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
    Ok(match raw {
        x if x == Opcode::Nop as u16 => Opcode::Nop,
        x if x == Opcode::Const as u16 => Opcode::Const,
        x if x == Opcode::Add as u16 => Opcode::Add,
        x if x == Opcode::Sub as u16 => Opcode::Sub,
        x if x == Opcode::Mul as u16 => Opcode::Mul,
        x if x == Opcode::Div as u16 => Opcode::Div,
        x if x == Opcode::Rem as u16 => Opcode::Rem,
        x if x == Opcode::Eq as u16 => Opcode::Eq,
        x if x == Opcode::Ne as u16 => Opcode::Ne,
        x if x == Opcode::Lt as u16 => Opcode::Lt,
        x if x == Opcode::Le as u16 => Opcode::Le,
        x if x == Opcode::Gt as u16 => Opcode::Gt,
        x if x == Opcode::Ge as u16 => Opcode::Ge,
        x if x == Opcode::Select as u16 => Opcode::Select,
        x if x == Opcode::Call as u16 => Opcode::Call,
        x if x == Opcode::ReadView as u16 => Opcode::ReadView,
        x if x == Opcode::WriteView as u16 => Opcode::WriteView,
        x if x == Opcode::Load as u16 => Opcode::Load,
        x if x == Opcode::Store as u16 => Opcode::Store,
        x if x == Opcode::Atomic as u16 => Opcode::Atomic,
        x if x == Opcode::EmitEvent as u16 => Opcode::EmitEvent,
        x if x == Opcode::Time as u16 => Opcode::Time,
        x if x == Opcode::Random as u16 => Opcode::Random,
        x if x == Opcode::Io as u16 => Opcode::Io,
        x if x == Opcode::Return as u16 => Opcode::Return,
        x if x == Opcode::Br as u16 => Opcode::Br,
        x if x == Opcode::CondBr as u16 => Opcode::CondBr,
        x if x == Opcode::Trap as u16 => Opcode::Trap,
        x if x == Opcode::Unreachable as u16 => Opcode::Unreachable,
        x if x == Opcode::Sin as u16 => Opcode::Sin,
        x if x == Opcode::Cos as u16 => Opcode::Cos,
        x if x == Opcode::Exp as u16 => Opcode::Exp,
        x if x == Opcode::Ln as u16 => Opcode::Ln,
        x if x == Opcode::Sqrt as u16 => Opcode::Sqrt,
        x if x == Opcode::Pow as u16 => Opcode::Pow,
        x if x == Opcode::Abs as u16 => Opcode::Abs,
        x if x == Opcode::Floor as u16 => Opcode::Floor,
        x if x == Opcode::Ceil as u16 => Opcode::Ceil,
        x if x == Opcode::Round as u16 => Opcode::Round,
        x if x == Opcode::Sign as u16 => Opcode::Sign,
        x if x == Opcode::Log10 as u16 => Opcode::Log10,
        x if x == Opcode::Log2 as u16 => Opcode::Log2,
        x if x == Opcode::Sinh as u16 => Opcode::Sinh,
        x if x == Opcode::Cosh as u16 => Opcode::Cosh,
        x if x == Opcode::Tanh as u16 => Opcode::Tanh,
        x if x == Opcode::Asin as u16 => Opcode::Asin,
        x if x == Opcode::Acos as u16 => Opcode::Acos,
        x if x == Opcode::Atan as u16 => Opcode::Atan,
        x if x == Opcode::Atan2 as u16 => Opcode::Atan2,
        x if x == Opcode::Hypot as u16 => Opcode::Hypot,
        x if x == Opcode::Print as u16 => Opcode::Print,
        _ => return Err(error(Status::EirInvalid, 28, 0)),
    })
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
        if block_count != 1 {
            return Err(error(Status::EirInvalid, 30, input.offset()));
        }
        let _block_id = input.u32()?;
        let argument_count = input.u32()?;
        let instruction_count = input.u32()? as usize;
        let mut instructions = Vec::with_capacity(instruction_count);
        for _ in 0..instruction_count {
            instructions.push(decode_instruction(&mut input)?);
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
        (Opcode::Div, F32(a), F32(b)) => Some(F32(a / b)),
        (Opcode::Div, F64(a), F64(b)) => Some(F64(a / b)),
        // fmod semantics; exact for integer-valued f64 (step % n).
        (Opcode::Rem, F32(a), F32(b)) => Some(F32(a % b)),
        (Opcode::Rem, F64(a), F64(b)) => Some(F64(a % b)),
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

#[cfg(test)]
mod tests {
    use super::*;

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
                payload: 99
            }]
        );
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
