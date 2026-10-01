//! Abstract syntax: the parsed program (`ParsedProgram`), system declarations,
//! rules/statements (`UpdateStmt`/`LetStmt`), scalar expressions (`Expr`), and
//! cross-entity property paths (`PropKind`).
use super::*;

/// A parsed system declaration: `kind { key = value; ... }`.
#[derive(Clone, Debug)]
pub struct SystemDecl {
    pub kind: String,
    pub params: std::collections::BTreeMap<String, f64>,
    /// Vector-valued params (e.g. `linear` rows `row0 = (a, b, c)`).
    pub vec_params: std::collections::BTreeMap<String, Vec<f64>>,
    /// ODE derivative rules (`inte slot = rate`): integrated as slot += dt·rate.
    pub update: std::collections::BTreeMap<String, String>,
    /// Assignment rules (`slot = expr`): written each step.
    pub assigns: std::collections::BTreeMap<String, String>,
    /// `let name = expr` local bindings in the `update` system, in order.
    pub update_stmts: Vec<UpdateStmt>,
    /// Ident-valued params (e.g. `chan = ping`).
    pub string_params: std::collections::BTreeMap<String, String>,
    /// Declared units for scalar params (e.g. `dt = 0.01 s`).
    pub param_units: std::collections::BTreeMap<String, crate::units::Dim>,
    /// Byte offset of this system's opening brace in the source (for
    /// diagnostics).
    pub byte_offset: usize,
    /// The module namespace this system came from ("" for the root program);
    /// unqualified function/parameter references resolve within it first.
    pub namespace: String,
    /// Reaction channels of a `gillespie` system, in source order.
    pub channels: Vec<ChannelDecl>,
}
/// One stochastic reaction channel of a `gillespie` system:
/// `channel <name> = <propensity> => (<slot> = <new value>, …)`.
#[derive(Clone, Debug)]
pub struct ChannelDecl {
    /// Channel name (`r0`, `bind`, …); unique within the system.
    pub name: String,
    /// Propensity `a_k` as source text (parsed at compile time).
    pub prop: String,
    /// State changes applied when this channel fires, as raw
    /// `(slot name, new-value expression)` pairs in source order.
    pub set: Vec<(String, String)>,
}
/// A statement in an `update` rule body. Loops keep their structure (count /
/// range + nested body) so lowering can unroll them with per-loop
/// break/continue gating; `break`/`continue` only occur inside loop bodies
/// (enforced by the grammar).
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStmt {
    /// `let name[: type] = expr` — a reusable local computed before the slot
    /// rules run. The third field is the optional annotation: `Some(true)` for
    /// `: bool`, `Some(false)` for a numeric type, `None` when unannotated.
    Let(String, String, Option<bool>),
    /// `repeat n { … }` / `repeat n until (cond) { … }` — unrolled at lowering
    /// time, bounded.
    Repeat(usize, Vec<UpdateStmt>),
    /// `for i in lo..hi { … }` — unrolled with the index bound per iteration.
    For(String, f64, f64, Vec<UpdateStmt>),
    /// `break` / `break if (cond)` inside a loop body.
    Break(Option<String>),
    /// `continue` / `continue if (cond)` inside a loop body.
    Continue(Option<String>),
    /// `if cond { return a } [else { return b }]` in a function body: a
    /// control-flow branch (lazy) so function calls nest/recursively on the
    /// call stack, unlike the eager `if(c,a,b)` expression.
    If(String, String, Option<String>),
}
/// A lowered (resolved) statement tree: the `Expr` form of [`UpdateStmt`],
/// stored per system and unrolled with gates during EIR lowering.
#[derive(Clone, Debug)]
pub enum LetStmt {
    Let(String, Expr),
    Repeat(usize, Vec<LetStmt>),
    For(String, f64, f64, Vec<LetStmt>),
    Break(Option<Expr>),
    Continue(Option<Expr>),
    /// `if cond { return a } [else { return b }]` (control-flow branch).
    If(Expr, Expr, Option<Expr>),
    /// An integer-annotated `let` (RFC-0043): the value is coerced to an exact
    /// `I64` (wrapping the RHS in a `F64ToI64` when it is not already integer).
    LetInt(String, Expr),
}
/// A parsed scalar expression over state slots (`s0`, `s1`, …).
#[derive(Clone, Debug)]
pub enum Expr {
    /// A numeric literal. `int = true` is an exact integer literal (no decimal
    /// point / exponent); lowering emits `Immediate::I64` and integer
    /// arithmetic (RFC-0043). The `f64` is the literal's value (also its exact
    /// coercion target where a number is required).
    Const(f64),
    /// An exact integer literal (RFC-0043): `7`, not `7.0`.
    Int(i64),
    Slot(usize),
    /// A dynamic slot read `s[i]`: the State slot at a runtime index.
    SlotDyn(Box<Expr>),
    /// A typed-array element access `name[i]` (RFC-0044): a named, length-aware
    /// run of consecutive State slots. Resolved at lowering against the entity's
    /// layout (base slot + compile-time length) into a static slot or a
    /// runtime-indexed `ReadSlotDyn`.
    Index(String, Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    /// Remainder `a % b` (fmod semantics).
    Rem(Box<Expr>, Box<Expr>),
    /// A comparison `a <op> b`, yielding 1.0 / 0.0.
    Cmp(&'static str, Box<Expr>, Box<Expr>),
    /// Logical conjunction `a and b`: 1.0 iff both operands are nonzero.
    And(Box<Expr>, Box<Expr>),
    /// Logical disjunction `a or b`: 1.0 iff either operand is nonzero.
    Or(Box<Expr>, Box<Expr>),
    /// Logical negation `not a`: 1.0 iff the operand is zero.
    Not(Box<Expr>),
    /// Unary negation `-x`.
    Neg(Box<Expr>),
    /// A built-in math function call: `sin(x)`, `pow(a, b)`, `if(c,a,b)`, …
    Call(&'static str, Vec<Expr>),
    /// A reference to another entity's state slot: `@name.sN`.
    Ref(String, usize),
    /// A named state slot reference (`x`), resolved against the model's state
    /// layout. Requires named `state = (x = 0, …)` declarations.
    Name(String),
    /// A cross-entity property reference: `@name.mass`, `@name.position.x`, …
    PropRef(String, PropKind),
    /// The global simulation clock: `t`.
    Time,
}
/// The index half of a runtime-indexed **write** target (update-system
/// `dyn_rules` / `dyn_assigns`).
///
/// A write target must keep the array *name* until lowering, because the base
/// slot differs per entity layout: `v[j] = …` on a system that may target
/// several bodies resolves `base` against each target's own `state_names`.
/// A raw `s[expr]` index is already absolute and stays a `Slot`.
#[derive(Clone, Debug)]
pub enum DynIndex {
    /// `s[expr]` — an absolute runtime slot index.
    Slot(Expr),
    /// `name[j]` — RFC-0044 array write; lowered to `base(sn) + j`.
    Array(String, Box<Expr>),
}
impl DynIndex {
    /// The index as a checkable expression (`check_array_index`): an
    /// `Array(name, j)` validates exactly like the read `name[j]`.
    pub fn as_expr(&self) -> Expr {
        match self {
            DynIndex::Slot(e) => e.clone(),
            DynIndex::Array(name, j) => Expr::Index(name.clone(), j.clone()),
        }
    }
    /// Resolves the absolute slot index against the target entity's layout.
    /// `None` when the layout declares no such array (a detail-109 diagnostic
    /// is recorded); the write is then skipped for that entity.
    pub fn resolve(&self, sn: &std::collections::BTreeMap<String, usize>) -> Option<Expr> {
        match self {
            DynIndex::Slot(e) => Some(e.clone()),
            DynIndex::Array(name, j) => {
                let base = sn.get(&format!("{name}.0")).copied();
                match base {
                    Some(base) => Some(Expr::Add(Box::new(Expr::Const(base as f64)), j.clone())),
                    None => {
                        super::diagnostics::push_diag(
                            109,
                            0,
                            format!("array `{name}` is not in this entity's state — write skipped"),
                        );
                        None
                    }
                }
            }
        }
    }
    /// The index expression used for slot-span bookkeeping.
    pub fn span_expr(&self) -> Expr {
        match self {
            DynIndex::Slot(e) => e.clone(),
            DynIndex::Array(name, j) => Expr::Index(name.clone(), j.clone()),
        }
    }
}
/// A cross-entity property path (`@name.<kind>`), lowering to a read of the
/// corresponding physics component field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum PropKind {
    Mass,
    IsDynamic,
    PositionX,
    PositionY,
    PositionZ,
    VelocityX,
    VelocityY,
    VelocityZ,
    State(usize),
}
/// The parsed program before lowering: a world model plus system declarations.
#[derive(Clone, Debug)]
pub struct ParsedProgram {
    pub model: WorldModel,
    pub systems: Vec<SystemDecl>,
    /// User-defined pure functions (params referenced as `s0`, `s1`, … in the
    /// body), lowered to EIR `CALL` functions.
    pub funcs: Vec<FuncDecl>,
}
/// A user-defined pure function: a name, its parameters (referenced as slots
/// `s0..s_{n-1}` in the body), statements computed before the return (lets and
/// bounded loops), and the return-value expression.
#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: String,
    /// The module namespace this function belongs to ("" for the root); its
    /// body resolves unqualified parameter/slot names within it first.
    pub namespace: String,
    pub params: Vec<String>,
    /// Declared unit of each parameter (aligned with `params`), if annotated.
    pub param_units: Vec<Option<crate::units::Dim>>,
    /// Declared unit of the return value, if annotated (`: [unit]`).
    pub ret_unit: Option<crate::units::Dim>,
    /// Statements before the `return` (lets / loops), in order.
    pub stmts: Vec<UpdateStmt>,
    pub body: Expr,
}
