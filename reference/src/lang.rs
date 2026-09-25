//! The PWE source language — a textual front end that compiles to low-level
//! EIR and executes cross-backend (interpreter + JIT) through a runtime.
//!
//! The front end is a declarative **PEST grammar** (this module), so the
//! language is easy to extend: add a rule here and a lowering arm in
//! `build_systems` / `WorldModel::build_scene`.
//!
//! Pipeline mirrors Erlang↔BEAM:
//!
//! ```text
//! PWE source  ──parse──▶ WorldModel + system decls  ──lower──▶ EIR (low-level IR)
//! EIR ──interpret──▶ writes      (semantic oracle)
//! EIR ──compile/publish──▶ CPU JIT ──execute──▶ writes   (differential: identical)
//! ```
//!
//! A `PweSource` is compiled once to an `EirModule`. `LangRuntime` then runs the
//! same module through the interpreter and the JIT and asserts the two backends
//! produce byte-identical writes — the "run cross" guarantee of the reference.
//!
//! Example source:
//!
//! ```text
//! world {
//!   gravity = (0, -9.81, 0)
//!   entity vehicle {
//!     position = (0, 8, 0); velocity = (4, 0, 0)
//!     mass = 4; dynamic = true; box = (1, 0.5, 0.7)
//!   }
//!   entity ground {
//!     position = (0, -5, 0); dynamic = false; box = (50, 5, 50)
//!   }
//! }
//! systems {
//!   gravity { gravity_y = -9.81; dt = 1 / 60 }
//!   integrate { dt = 1 / 60 }
//!   ground_contact { restitution = 0.6 }
//! }
//! ```

use crate::channel::{ChannelAddr, ChannelId, ChannelRouter};
use crate::dsl::{ColliderDecl, EntityDecl, WorldModel};
use crate::eir::{EirModule, Function, WorldWrite};
use crate::jit::{profile_hash, CodeCacheKey, CpuJit, JitAssumption};
use crate::math::Vec3;
use crate::physics_eir::{
    apply_writes, DampingSystem, EirSystem, ForceSystem, GravitySystem, GroundContactSystem,
    IntegrateSystem, LinearSystem, PhysicsProgram, SceneRuntime, WallSystem,
};
use crate::scene::Scene;
use pwe_api::{Access, EntityId, Error, Hash256, RegionId, Result, Status, WorldId, WorldVersion};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

// ---------------------------------------------------------------------------
// Compile diagnostics
// ---------------------------------------------------------------------------

/// A single compile diagnostic: a human message, a detail code, and a byte
/// offset into the source (`0` when no precise position is available).
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub detail: u32,
    pub message: String,
    pub byte_offset: usize,
}

thread_local! {
    /// Diagnostics accumulated by the most recent compile on this thread.
    static DIAGNOSTICS: std::cell::RefCell<Vec<Diagnostic>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Clears the accumulated diagnostics for the current thread.
pub fn clear_diagnostics() {
    DIAGNOSTICS.with(|d| d.borrow_mut().clear());
}

/// Returns the diagnostics accumulated since the last `clear_diagnostics`,
/// clearing the log. The most recent entry is last.
pub fn take_diagnostics() -> Vec<Diagnostic> {
    DIAGNOSTICS.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

fn push_diag(detail: u32, byte_offset: usize, message: impl Into<String>) {
    DIAGNOSTICS.with(|d| {
        d.borrow_mut().push(Diagnostic {
            detail,
            byte_offset,
            message: message.into(),
        })
    });
}

/// An error with a human message and a byte offset, recorded as a diagnostic.
fn error_at(status: Status, detail: u32, byte_offset: usize, message: impl Into<String>) -> Error {
    push_diag(detail, byte_offset, message);
    Error {
        status,
        detail,
        byte_offset: byte_offset as u64,
    }
}

/// Maps a detail code to a short human phrase (used when no richer diagnostic
/// was recorded). Keep in sync with the detail-code table in `docs/lang-usage`.
pub fn detail_name(detail: u32) -> &'static str {
    match detail {
        48 => "missing required system parameter",
        49 => "unknown system kind",
        51 => "convex hull needs at least 4 points",
        52 => "state slot index out of range (0..=15)",
        53 => "missing linear system row",
        54 => "linear row length must equal slots + 1",
        55 => "invalid function body / empty update rule / bad slot lhs",
        56 => "expression parse failure",
        57 => "number parse failure",
        58 => "slot or reference parse failure",
        59 => "call arity mismatch",
        60 => "program parse failure",
        62 => "unknown entity or channel name",
        63 => "nbody system has no dynamic bodies",
        64 => "invalid color literal",
        65 => "invalid loop count (integer in 1..=1000 required)",
        66 => "loop unrolls beyond the 10000-statement limit",
        67 => "let name shadows a reserved token (t / pi / e / sN)",
        68 => "for range must be ascending integers",
        69 => "invariant violated at step boundary",
        70 => "spatial query outside a system rule (no entity context)",
        71 => "every must be an integer ≥ 1",
        72 => "substeps must be an integer in 1..=1000",
        73 => "dynamic slot LHS is update-only (rk4 stages need compile-time slots)",
        75 => "field needs width and height ≥ 1",
        76 => "import failed (missing file, bad directive, or cycle)",
        77 => "dimension mismatch (see declared units)",
        _ => "unspecified compile error",
    }
}

/// Renders a diagnostic as a readable multi-line message with the offending
/// source line and a caret. `source` is the original program text.
pub fn render_diagnostic(source: &str, diag: &Diagnostic) -> String {
    let mut out = format!("error {}: {}", diag.detail, diag.message);
    let off = diag.byte_offset;
    if off > 0 && off <= source.len() {
        let before = &source[..off];
        let line = before.matches('\n').count() + 1;
        let col = before.rfind('\n').map(|i| off - i).unwrap_or(off + 1);
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = source[off..]
            .find('\n')
            .map(|i| off + i)
            .unwrap_or(source.len());
        out.push_str(&format!("\n  --> line {line}, column {col}\n"));
        out.push_str(&format!(
            "    |\n{line:>4} | {}\n    | ",
            &source[line_start..line_end]
        ));
        for _ in 0..(col.saturating_sub(1)) {
            out.push(' ');
        }
        out.push('^');
    }
    out
}

/// Builds a human-readable diagnostic for a failed compile against `source`,
/// preferring the most recent recorded diagnostic for `err`'s detail code, then
/// falling back to the code's canonical phrase.
pub fn diagnose(source: &str, err: &Error) -> String {
    let diags = take_diagnostics();
    let found = diags.iter().rev().find(|d| d.detail == err.detail);
    match found {
        Some(d) => render_diagnostic(source, d),
        None => {
            let msg = format!(
                "{:?} ({}): {}",
                err.status,
                err.detail,
                detail_name(err.detail)
            );
            if err.byte_offset > 0 {
                let pseudo = Diagnostic {
                    detail: err.detail,
                    message: detail_name(err.detail).to_string(),
                    byte_offset: err.byte_offset as usize,
                };
                render_diagnostic(source, &pseudo)
            } else {
                msg
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PEST grammar and parser
// ---------------------------------------------------------------------------

use pest::iterators::Pair;
use pest::Parser;

/// The PWE language grammar, expressed declaratively with PEST (see
/// [`grammar`](lang.pest)). Keeping the grammar as data (not hand-written
/// tokenizer code) makes it easy to extend with new phenomena: add a rule to
/// `lang.pest` and a lowering arm below.
#[derive(pest_derive::Parser)]
#[grammar = "src/lang.pest"]
struct LangParser;

/// Reads a `value` pair: a single number, or a fraction `a / b`.
fn parse_value(pair: Pair<'_, Rule>) -> f64 {
    let nums: Vec<f64> = pair
        .into_inner()
        .map(|n| n.as_str().parse::<f64>().unwrap_or(0.0))
        .collect();
    match nums.as_slice() {
        [a, b] => a / b,
        [a] => *a,
        _ => 0.0,
    }
}

/// Reads a `vec3` pair into a `Vec3`.
fn parse_vec3(pair: Pair<'_, Rule>) -> Vec3 {
    let mut v = pair.into_inner().map(parse_value);
    Vec3::new(
        v.next().unwrap_or(0.0),
        v.next().unwrap_or(0.0),
        v.next().unwrap_or(0.0),
    )
}

/// A parsed system declaration: `kind { key = value; ... }`.
#[derive(Clone, Debug)]
pub struct SystemDecl {
    pub kind: String,
    pub params: std::collections::BTreeMap<String, f64>,
    /// Vector-valued params (e.g. `linear` rows `row0 = (a, b, c)`).
    pub vec_params: std::collections::BTreeMap<String, Vec<f64>>,
    /// Scalar expression rules (the nonlinear `update` system): slot → expr text.
    pub update: std::collections::BTreeMap<String, String>,
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
}

/// A statement in an `update` rule body. Loops keep their structure (count /
/// range + nested body) so lowering can unroll them with per-loop
/// break/continue gating; `break`/`continue` only occur inside loop bodies
/// (enforced by the grammar).
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStmt {
    /// `let name = expr` — a reusable local computed before the slot rules run.
    Let(String, String),
    /// `repeat n { … }` / `repeat n until (cond) { … }` — unrolled at lowering
    /// time, bounded.
    Repeat(usize, Vec<UpdateStmt>),
    /// `for i in lo..hi { … }` — unrolled with the index bound per iteration.
    For(String, f64, f64, Vec<UpdateStmt>),
    /// `break` / `break if (cond)` inside a loop body.
    Break(Option<String>),
    /// `continue` / `continue if (cond)` inside a loop body.
    Continue(Option<String>),
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
}

/// A parsed scalar expression over state slots (`s0`, `s1`, …).
#[derive(Clone, Debug)]
pub enum Expr {
    Const(f64),
    Slot(usize),
    /// A dynamic slot read `s[i]`: the State slot at a runtime index.
    SlotDyn(Box<Expr>),
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

/// Parses an `ident = rhs` parameter pair and stores it in the appropriate
/// bucket (scalar / vector / expression) of a `SystemDecl`.
fn store_param(param: Pair<'_, Rule>, decl: &mut SystemDecl) -> Result<()> {
    let mut inner = param.into_inner();
    let first = inner.next().unwrap();
    // `let name = expr` is a local binding; otherwise `ident = rhs`.
    if first.as_rule() == Rule::let_stmt {
        // The literal `let` is transparent; children are [ident(name), expr].
        let mut li = first.into_inner();
        let name = li.next().unwrap().as_str().to_string();
        check_let_name(&name, decl.byte_offset)?;
        let expr = li.next().unwrap().as_str().trim().to_string();
        decl.update_stmts.push(UpdateStmt::Let(name, expr));
        return Ok(());
    }
    // `repeat` / `for` loops keep their tree structure for gated lowering.
    if matches!(first.as_rule(), Rule::repeat_stmt | Rule::for_stmt) {
        let stmt = build_loop_stmt(first, decl.byte_offset)?;
        decl.update_stmts.push(stmt);
        return Ok(());
    }
    // A bare call statement (`fset(field, i, j, v)`) runs for its side
    // effect; bound to the `_` local and discarded.
    if first.as_rule() == Rule::call {
        let text = first.as_str().trim().to_string();
        decl.update_stmts
            .push(UpdateStmt::Let("_".to_string(), text));
        return Ok(());
    }
    // Dynamic slot LHS: `s[expr] = expr` writes the State slot at a runtime
    // index; the raw LHS text is resolved per-entity during lowering.
    if first.as_rule() == Rule::slot_lhs {
        let key = first.as_str().to_string();
        let rhs = inner.next().unwrap();
        if rhs.as_rule() != Rule::expr {
            return Err(error(Status::Invalid, 55));
        }
        decl.update.insert(key, rhs.as_str().trim().to_string());
        return Ok(());
    }
    let key = first.as_str().to_string();
    let rhs = inner.next().unwrap();
    match rhs.as_rule() {
        Rule::vecN => {
            let vals: Vec<f64> = rhs.into_inner().map(parse_value).collect();
            decl.vec_params.insert(key, vals);
        }
        Rule::expr => {
            let text = rhs.as_str().trim().to_string();
            // String-valued params (`chan`, `on`, `when`) keep the raw text.
            if matches!(
                key.as_str(),
                "chan" | "on" | "when" | "field" | "source" | "prev" | "pool"
            ) {
                decl.string_params.insert(key, text);
            } else if let Some(v) = parse_scalar_number(&text) {
                decl.params.insert(key, v);
            } else {
                decl.update.insert(key, text);
            }
        }
        Rule::ident => {
            decl.string_params.insert(key, rhs.as_str().to_string());
        }
        _ => {}
    }
    // Optional trailing unit annotation (`dt = 0.01 s`): compile-time only.
    if let Some(unit) = inner.next() {
        if unit.as_rule() == Rule::unit_expr {
            if let Ok(d) = unit
                .as_str()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<crate::units::Dim>()
            {
                decl.param_units.insert(first.as_str().to_string(), d);
            }
        }
    }
    Ok(())
}

/// Maximum iteration count of one loop.
const MAX_REPEAT_COUNT: usize = 1_000;
/// Maximum number of statements one loop may unroll into.
/// The numeric slot index of an `sN` token, or `None` for any other name
/// (including a named slot like `smooth` that merely starts with `s`).
fn numeric_slot(key: &str) -> Option<usize> {
    key.strip_prefix('s').and_then(|d| {
        if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
            d.parse().ok()
        } else {
            None
        }
    })
}

const MAX_UNROLLED_STMTS: usize = 10_000;

/// Rejects `let` names that can never be read back: the grammar resolves
/// `t` (time), `pi`/`e` (constants), and `sN` (slot) before bare idents, so a
/// binding with such a name is silently unreachable.
fn check_let_name(name: &str, offset: usize) -> Result<()> {
    let reserved = name == "t"
        || name == "pi"
        || name == "e"
        || (name.len() > 1
            && name.starts_with('s')
            && name[1..].bytes().all(|b| b.is_ascii_digit()));
    if reserved {
        return Err(error_at(
            Status::Invalid,
            67,
            offset,
            format!(
                "let name `{name}` shadows a reserved token (t, pi, e, s0..) and can never be read"
            ),
        ));
    }
    Ok(())
}

/// Parses an integer bound (repeat count / for-range end), rejecting
/// non-integers.
fn parse_int_bound(pair: Pair<'_, Rule>, offset: usize, what: &str) -> Result<f64> {
    let raw = pair
        .as_str()
        .parse::<f64>()
        .map_err(|_| error(Status::Invalid, 57))?;
    if raw.fract() != 0.0 {
        return Err(error_at(
            Status::Invalid,
            65,
            offset,
            format!("{what} must be an integer, got `{}`", pair.as_str()),
        ));
    }
    Ok(raw)
}

/// Number of statements a statement tree unrolls into (loops multiply; `for`
/// adds one index binding per iteration).
fn unrolled_size(stmts: &[UpdateStmt]) -> usize {
    stmts
        .iter()
        .map(|s| match s {
            UpdateStmt::Let(..) | UpdateStmt::Break(_) | UpdateStmt::Continue(_) => 1,
            UpdateStmt::Repeat(n, body) => n * unrolled_size(body),
            UpdateStmt::For(_, lo, hi, body) => {
                ((*hi - *lo).max(0.0) as usize) * (unrolled_size(body) + 1)
            }
        })
        .sum()
}

/// Builds a `Repeat`/`For` statement tree from a parsed loop pair. The body
/// keeps its structure (nested loops, `break`/`continue`) so lowering can
/// unroll with per-loop gating. Size caps are enforced here.
fn build_loop_stmt(pair: Pair<'_, Rule>, offset: usize) -> Result<UpdateStmt> {
    match pair.as_rule() {
        Rule::repeat_stmt => {
            let mut it = pair.into_inner();
            let count = it.next().ok_or(error(Status::Invalid, 56))?;
            let count_text = count.as_str().to_string();
            let raw = parse_int_bound(count, offset, "repeat count")?;
            if raw < 1.0 || raw > MAX_REPEAT_COUNT as f64 {
                return Err(error_at(
                    Status::Invalid,
                    65,
                    offset,
                    format!(
                        "repeat count must be an integer in 1..={MAX_REPEAT_COUNT}, got `{count_text}`"
                    ),
                ));
            }
            let n = raw as usize;
            // Optional `until (cond)` / `while (cond)` early-exit clause.
            let mut gate: Option<(bool, String)> = None; // (is_while, cond text)
            let mut items: Vec<Pair<'_, Rule>> = Vec::new();
            for child in it {
                if child.as_rule() == Rule::repeat_gate {
                    let mut ci = child.into_inner();
                    let kw = ci.next().unwrap().as_str().to_string();
                    let cond = ci.next().unwrap().as_str().trim().to_string();
                    gate = Some((kw == "while", cond));
                } else {
                    items.push(child);
                }
            }
            let mut body = build_loop_body(items.into_iter(), offset, n)?;
            match &gate {
                // `while (cond)`: check before each iteration — break when
                // the condition is false.
                Some((true, cond)) => {
                    body.insert(0, UpdateStmt::Break(Some(format!("not ({cond})"))));
                }
                // `until (cond)`: check after each iteration — break when
                // the condition is true.
                Some((false, cond)) => body.push(UpdateStmt::Break(Some(cond.clone()))),
                None => {}
            }
            Ok(UpdateStmt::Repeat(n, body))
        }
        Rule::for_stmt => {
            let mut it = pair.into_inner();
            let name = it
                .next()
                .ok_or(error(Status::Invalid, 56))?
                .as_str()
                .to_string();
            check_let_name(&name, offset)?;
            let range = it.next().ok_or(error(Status::Invalid, 56))?;
            let mut ri = range.into_inner();
            let lo_pair = ri.next().ok_or(error(Status::Invalid, 56))?;
            let hi_pair = ri.next().ok_or(error(Status::Invalid, 56))?;
            let lo = parse_int_bound(lo_pair, offset, "for range start")?;
            let hi = parse_int_bound(hi_pair, offset, "for range end")?;
            if hi < lo {
                return Err(error_at(
                    Status::Invalid,
                    68,
                    offset,
                    format!("for range must be ascending integers, got {}..{}", lo, hi),
                ));
            }
            if (hi - lo) as usize > MAX_REPEAT_COUNT {
                return Err(error_at(
                    Status::Invalid,
                    65,
                    offset,
                    format!("for range wider than {MAX_REPEAT_COUNT} iterations"),
                ));
            }
            let body = build_loop_body(it, offset, (hi - lo) as usize)?;
            Ok(UpdateStmt::For(name, lo, hi, body))
        }
        _ => Err(error(Status::Invalid, 56)),
    }
}

/// Converts parsed update statements (expr texts) into their resolved `Expr`
/// form, keeping the loop / break / continue structure for gated lowering.
fn to_let_stmts(stmts: &[UpdateStmt]) -> Result<Vec<LetStmt>> {
    stmts
        .iter()
        .map(|s| match s {
            UpdateStmt::Let(name, text) => Ok(LetStmt::Let(name.clone(), parse_expr_str(text)?)),
            UpdateStmt::Repeat(n, body) => Ok(LetStmt::Repeat(*n, to_let_stmts(body)?)),
            UpdateStmt::For(name, lo, hi, body) => {
                Ok(LetStmt::For(name.clone(), *lo, *hi, to_let_stmts(body)?))
            }
            UpdateStmt::Break(cond) => Ok(LetStmt::Break(match cond {
                Some(text) => Some(parse_expr_str(text)?),
                None => None,
            })),
            UpdateStmt::Continue(cond) => Ok(LetStmt::Continue(match cond {
                Some(text) => Some(parse_expr_str(text)?),
                None => None,
            })),
        })
        .collect()
}

/// Builds the body of a loop from its parsed items. Only `let`, nested loops,
/// and `break`/`continue` are allowed (the grammar enforces this); slot rules
/// stay outside the loop.
fn build_loop_body<'a>(
    items: impl Iterator<Item = Pair<'a, Rule>>,
    offset: usize,
    iterations: usize,
) -> Result<Vec<UpdateStmt>> {
    let mut body: Vec<UpdateStmt> = Vec::new();
    for item in items {
        let inner = item.into_inner().next().ok_or(error(Status::Invalid, 56))?;
        match inner.as_rule() {
            Rule::let_stmt => {
                let mut li = inner.into_inner();
                let name = li.next().unwrap().as_str().to_string();
                check_let_name(&name, offset)?;
                let expr = li.next().unwrap().as_str().trim().to_string();
                body.push(UpdateStmt::Let(name, expr));
            }
            Rule::repeat_stmt | Rule::for_stmt => body.push(build_loop_stmt(inner, offset)?),
            Rule::break_stmt | Rule::continue_stmt => {
                // Children: [kw] or [kw, if_kw, expr].
                let is_break = inner.as_rule() == Rule::break_stmt;
                let children: Vec<Pair<'_, Rule>> = inner.into_inner().collect();
                let cond = if children.len() == 3 {
                    Some(children[2].as_str().trim().to_string())
                } else {
                    None
                };
                if is_break {
                    body.push(UpdateStmt::Break(cond));
                } else {
                    body.push(UpdateStmt::Continue(cond));
                }
            }
            _ => return Err(error(Status::Invalid, 56)),
        }
    }
    if unrolled_size(&body) * iterations.max(1) > MAX_UNROLLED_STMTS {
        return Err(error_at(
            Status::Invalid,
            66,
            offset,
            format!("loop unrolls to more than {MAX_UNROLLED_STMTS} statements"),
        ));
    }
    Ok(body)
}

/// Truncates a scalar fragment at the first trailing comment (`#` or `//` to
/// end of line) and trims whitespace. The `expr` grammar consumes trailing
/// comments as whitespace, so a value like `0.0005\n # note` must be cleaned
/// before it is read as a plain number.
fn strip_trailing_comment(text: &str) -> &str {
    let cut = text
        .find('#')
        .or_else(|| text.find("//"))
        .unwrap_or(text.len());
    text[..cut].trim()
}

/// A plain numeric scalar: a single number or an `a / b` fraction. Any other
/// text (operators, state-slot identifiers) is treated as an expression.
fn parse_scalar_number(text: &str) -> Option<f64> {
    let t = strip_trailing_comment(text);
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    if let Some((a, b)) = t.split_once('/') {
        if let (Ok(an), Ok(bn)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            if bn != 0.0 {
                return Some(an / bn);
            }
        }
    }
    None
}

/// Parses a scalar expression string (from the `update` system) into an `Expr`.
pub fn parse_expr_str(text: &str) -> Result<Expr> {
    let mut pairs = LangParser::parse(Rule::expr, text)
        .map_err(|e| error_at(Status::Invalid, 56, 0, format!("invalid expression: {e}")))?;
    let expr = pairs.next().ok_or(error(Status::Invalid, 56))?;
    build_expr(expr)
}

fn build_expr(pair: Pair<'_, Rule>) -> Result<Expr> {
    // `expr` wraps a single `logical_or` chain.
    let inner = pair.into_inner().next().ok_or(error(Status::Invalid, 56))?;
    build_logical_or(inner)
}

/// `a or b or c` — left-associative logical disjunction.
fn build_logical_or(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_logical_and(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(_op) = it.next() {
        let rhs = build_logical_and(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = Expr::Or(Box::new(acc), Box::new(rhs));
    }
    Ok(acc)
}

/// `a and b and c` — left-associative logical conjunction.
fn build_logical_and(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_comparison(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(_op) = it.next() {
        let rhs = build_comparison(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = Expr::And(Box::new(acc), Box::new(rhs));
    }
    Ok(acc)
}

/// `a <op> b` comparisons over additive operands, left-associative.
fn build_comparison(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_additive(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(op) = it.next() {
        let rhs = build_additive(it.next().ok_or(error(Status::Invalid, 56))?)?;
        let static_op = match op.as_str() {
            "<" => "<",
            "<=" => "<=",
            ">" => ">",
            ">=" => ">=",
            "==" => "==",
            "!=" => "!=",
            _ => return Err(error(Status::Invalid, 56)),
        };
        acc = Expr::Cmp(static_op, Box::new(acc), Box::new(rhs));
    }
    Ok(acc)
}

fn build_additive(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_term(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(op) = it.next() {
        let rhs = build_term(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = match op.as_str() {
            "+" => Expr::Add(Box::new(acc), Box::new(rhs)),
            "-" => Expr::Sub(Box::new(acc), Box::new(rhs)),
            _ => acc,
        };
    }
    Ok(acc)
}

fn build_term(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_factor(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(op) = it.next() {
        let rhs = build_factor(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = match op.as_str() {
            "*" => Expr::Mul(Box::new(acc), Box::new(rhs)),
            "/" => Expr::Div(Box::new(acc), Box::new(rhs)),
            "%" => Expr::Rem(Box::new(acc), Box::new(rhs)),
            _ => acc,
        };
    }
    Ok(acc)
}

fn build_factor(pair: Pair<'_, Rule>) -> Result<Expr> {
    // A `factor` pair wraps a single `number` / `ident` / `(expr)` / `call` /
    // unary-`-`.
    let inner = pair.into_inner().next().ok_or(error(Status::Invalid, 56))?;
    match inner.as_rule() {
        Rule::unary => {
            let children: Vec<Pair<'_, Rule>> = inner.into_inner().collect();
            if children.len() == 2 && children[0].as_rule() == Rule::not_op {
                Ok(Expr::Not(Box::new(build_factor(children[1].clone())?)))
            } else {
                let child = children
                    .into_iter()
                    .next()
                    .ok_or(error(Status::Invalid, 56))?;
                Ok(Expr::Neg(Box::new(build_factor(child)?)))
            }
        }
        Rule::number => inner
            .as_str()
            .parse::<f64>()
            .map(Expr::Const)
            .map_err(|_| error(Status::Invalid, 57)),
        Rule::slot => {
            let idx: usize = inner
                .as_str()
                .trim_start_matches('s')
                .parse()
                .map_err(|_| error(Status::Invalid, 58))?;
            Ok(Expr::Slot(idx))
        }
        Rule::slot_dyn => {
            // `s[expr]`: the State slot at a runtime index.
            let idx = inner
                .into_inner()
                .next()
                .ok_or(error(Status::Invalid, 58))?;
            Ok(Expr::SlotDyn(Box::new(build_expr(idx)?)))
        }
        Rule::entity_ref => build_ref(inner),
        Rule::time => Ok(Expr::Time),
        Rule::constant => {
            let c = if inner.as_str() == "pi" {
                std::f64::consts::PI
            } else {
                std::f64::consts::E
            };
            Ok(Expr::Const(c))
        }
        Rule::state_name => Ok(Expr::Name(inner.as_str().to_string())),
        Rule::namespaced => Ok(Expr::Name(inner.as_str().to_string())),
        Rule::expr => build_expr(inner),
        Rule::call => build_call(inner),
        _ => Err(error(Status::Invalid, 56)),
    }
}

fn build_call(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let name = it.next().unwrap().as_str().to_string();
    let mut args = Vec::new();
    for a in it {
        args.push(build_expr(a)?);
    }
    let static_name = match name.as_str() {
        "sin" => "sin",
        "cos" => "cos",
        "exp" => "exp",
        "ln" => "ln",
        "sqrt" => "sqrt",
        "pow" => "pow",
        "min" | "max" => {
            if args.len() != 2 {
                return Err(error(Status::Invalid, 59));
            }
            if name == "min" {
                "min"
            } else {
                "max"
            }
        }
        "if" => {
            if args.len() != 3 {
                return Err(error(Status::Invalid, 59));
            }
            "if"
        }
        "random" => {
            if !args.is_empty() {
                return Err(error(Status::Invalid, 59));
            }
            "random"
        }
        "print" => {
            if args.len() != 1 {
                return Err(error(Status::Invalid, 59));
            }
            "print"
        }
        "emit" => {
            if args.len() != 2 {
                return Err(error(Status::Invalid, 59));
            }
            "emit"
        }
        // RFC-0038: the current entity's activation flag.
        "active" => {
            if !args.is_empty() {
                return Err(error(Status::Invalid, 59));
            }
            "active"
        }
        // Spatial queries: `neighbor_count(radius)` / `nearest_dist()`.
        "neighbor_count" => {
            if args.len() != 1 {
                return Err(error(Status::Invalid, 59));
            }
            "neighbor_count"
        }
        "nearest_dist" => {
            if !args.is_empty() {
                return Err(error(Status::Invalid, 59));
            }
            "nearest_dist"
        }
        // Scheduled events (discrete-event scheduling on the step grid).
        "schedule" => {
            if args.len() != 4 {
                return Err(error(Status::Invalid, 59));
            }
            "schedule"
        }
        "at" => {
            if args.len() != 1 {
                return Err(error(Status::Invalid, 59));
            }
            "at"
        }
        "periodic" => {
            if args.is_empty() || args.len() > 2 {
                return Err(error(Status::Invalid, 59));
            }
            "periodic"
        }
        // Neighborhood aggregates / directional sensing.
        "neighbor_mean" => {
            if args.len() != 2 {
                return Err(error(Status::Invalid, 59));
            }
            "neighbor_mean"
        }
        "nearest_dx" | "nearest_dy" | "nearest_dz" => {
            if !args.is_empty() {
                return Err(error(Status::Invalid, 59));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Statistical noise: `noise()` (standard normal, Box–Muller).
        "noise" => {
            if !args.is_empty() {
                return Err(error(Status::Invalid, 59));
            }
            "noise"
        }
        // Vector helpers over scalar components.
        "vlen" => {
            if args.len() != 3 {
                return Err(error(Status::Invalid, 59));
            }
            "vlen"
        }
        "vdot" | "vdist" => {
            if args.len() != 6 {
                return Err(error(Status::Invalid, 59));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Event consumption: `last_event(kind)` reads the most recent event
        // of that kind emitted so far in this step.
        "last_event" => {
            if args.len() != 1 {
                return Err(error(Status::Invalid, 59));
            }
            "last_event"
        }
        // Grid field access: `fget(f, i, j)` / `flap(f, i, j)` / `fset(f, i, j, v)`;
        // the first argument must be a literal field name.
        "fget" | "flap" => {
            // 2D `fget(f, i, j)` or 3D `fget(f, i, j, k)`.
            if !(args.len() == 3 || args.len() == 4) || !matches!(args.first(), Some(Expr::Name(_)))
            {
                return Err(error(Status::Invalid, 59));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        "fset" => {
            // 2D `fset(f, i, j, v)` or 3D `fset(f, i, j, k, v)`.
            if !(args.len() == 4 || args.len() == 5) || !matches!(args.first(), Some(Expr::Name(_)))
            {
                return Err(error(Status::Invalid, 59));
            }
            "fset"
        }
        // Extended unary math builtins (1 arg).
        "abs" | "floor" | "ceil" | "round" | "sign" | "log10" | "log2" | "sinh" | "cosh"
        | "tanh" | "asin" | "acos" | "atan" => {
            if args.len() != 1 {
                return Err(error(Status::Invalid, 59));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Extended binary math builtins (2 args).
        "atan2" | "hypot" => {
            if args.len() != 2 {
                return Err(error(Status::Invalid, 59));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Any other name is a user-defined function (resolved at lowering); its
        // existence and arity are validated there.
        _ => Box::leak(name.clone().into_boxed_str()),
    };
    Ok(Expr::Call(static_name, args))
}

fn build_ref(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let name = it.next().unwrap().as_str().to_string();
    let path = it.next().unwrap();
    match path.as_rule() {
        Rule::slot => {
            let idx: usize = path
                .as_str()
                .trim_start_matches('s')
                .parse()
                .map_err(|_| error(Status::Invalid, 58))?;
            Ok(Expr::Ref(name, idx))
        }
        Rule::prop_path => {
            let segs: Vec<String> = path.into_inner().map(|p| p.as_str().to_string()).collect();
            let kind = match segs.len() {
                // `@name.state.<vec>.<idx>` names a vec-state component:
                // `pos.1` -> the named slot `pos.1`.
                3 if segs[0] == "state" => {
                    let expr_name = if name == "self" {
                        format!("{}.{}", segs[1], segs[2])
                    } else {
                        format!("{name}.state.{}.{}", segs[1], segs[2])
                    };
                    return Ok(Expr::Name(expr_name));
                }
                _ => {
                    let first = segs.first().map(String::as_str).unwrap_or("");
                    let second = segs.get(1).cloned();
                    match second {
                        None => match first {
                            "mass" => PropKind::Mass,
                            "is_dynamic" => PropKind::IsDynamic,
                            other => {
                                // `@name.<named-state-slot>`; `@self.<name>` = own.
                                let expr_name = if name == "self" {
                                    other.to_string()
                                } else {
                                    format!("{name}.{other}")
                                };
                                return Ok(Expr::Name(expr_name));
                            }
                        },
                        Some(sub) => match (first, sub.as_str()) {
                            ("position", "x") => PropKind::PositionX,
                            ("position", "y") => PropKind::PositionY,
                            ("position", "z") => PropKind::PositionZ,
                            ("velocity", "x") => PropKind::VelocityX,
                            ("velocity", "y") => PropKind::VelocityY,
                            ("velocity", "z") => PropKind::VelocityZ,
                            ("state", s) if s.starts_with('s') => {
                                let idx: usize =
                                    s[1..].parse().map_err(|_| error(Status::Invalid, 58))?;
                                PropKind::State(idx)
                            }
                            ("state", other) => {
                                // `@name.state.<named-slot>`; `@self.state.<name>` = own.
                                let expr_name = if name == "self" {
                                    other.to_string()
                                } else {
                                    format!("{name}.state.{other}")
                                };
                                return Ok(Expr::Name(expr_name));
                            }
                            _ => return Err(error(Status::Invalid, 58)),
                        },
                    }
                }
            };
            Ok(Expr::PropRef(name, kind))
        }
        _ => Err(error(Status::Invalid, 58)),
    }
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
    /// Statements before the `return` (lets / loops), in order.
    pub stmts: Vec<UpdateStmt>,
    pub body: Expr,
}

/// Parses PWE source text into a world model plus system declarations, using
/// the PEST grammar above.
pub fn parse(source: &str) -> Result<ParsedProgram> {
    let pairs = LangParser::parse(Rule::program, source).map_err(|e| {
        error_at(
            Status::Invalid,
            60,
            0,
            format!("failed to parse program: {e}"),
        )
    })?;
    let mut model = WorldModel::default();
    let mut systems = Vec::new();
    let mut funcs = Vec::new();

    for section in pairs {
        match section.as_rule() {
            Rule::world_section => {
                for item in section.into_inner() {
                    match item.as_rule() {
                        Rule::gravity_stmt => {
                            let vec3 = item.into_inner().next().unwrap();
                            model.gravity = parse_vec3(vec3);
                        }
                        Rule::title_stmt => {
                            let inner = item.into_inner().next().unwrap();
                            let text = inner.as_str().trim();
                            // Strip the surrounding quotes.
                            model.title = Some(
                                text.trim_start_matches('"')
                                    .trim_end_matches('"')
                                    .to_string(),
                            );
                        }
                        Rule::chan_stmt => {
                            let mut inner = item.into_inner();
                            let name = inner.next().unwrap().as_str().to_string();
                            let value = inner.next().map(parse_value).unwrap_or(0.0);
                            model.channels.push(crate::dsl::ChanDecl { name, value });
                        }
                        Rule::shape_stmt => {
                            // `shape <name> { part <kind> = <params> [at (x,y,z)]; }`
                            let mut it = item.into_inner();
                            let name = it.next().unwrap().as_str().to_string();
                            let mut parts: Vec<crate::components::ShapePart> = Vec::new();
                            for part in it {
                                let mut pi = part.into_inner();
                                let kind = match pi.next().unwrap().as_str() {
                                    "sphere" => 1u8,
                                    "box" => 2,
                                    "capsule" => 3,
                                    "svg" => 4,
                                    "hull" => 5,
                                    "poly" => 6,
                                    _ => 0,
                                };
                                let val = pi.next().unwrap();
                                let mut path: Option<String> = None;
                                let mut points: Vec<(f64, f64, f64)> = Vec::new();
                                let (mut a, b, c) = if val.as_rule() == Rule::string {
                                    path = Some(
                                        val.as_str()
                                            .trim_start_matches('"')
                                            .trim_end_matches('"')
                                            .to_string(),
                                    );
                                    (0.0, 0.0, 0.0)
                                } else if val.as_rule() == Rule::hull_list {
                                    for p3 in val.into_inner() {
                                        let v = parse_vec3(p3);
                                        points.push((v.x, v.y, v.z));
                                    }
                                    (0.0, 0.0, 0.0)
                                } else if val.as_rule() == Rule::vec3 {
                                    let v = parse_vec3(val);
                                    (v.x, v.y, v.z)
                                } else {
                                    let x = parse_value(val);
                                    (x, x, x)
                                };
                                let mut faces: Vec<Vec<u32>> = Vec::new();
                                let mut scale = 1.0f64;
                                let mut offset = (0.0, 0.0, 0.0);
                                for opt in pi {
                                    let rule = opt.as_rule();
                                    let inner = opt.into_inner().next().unwrap();
                                    match rule {
                                        Rule::at_opt => {
                                            let o = parse_vec3(inner);
                                            offset = (o.x, o.y, o.z);
                                        }
                                        Rule::depth_opt => a = parse_value(inner),
                                        Rule::scale_opt => scale = parse_value(inner),
                                        Rule::faces_opt => {
                                            for f in inner.into_inner() {
                                                let idx: Vec<u32> = f
                                                    .as_str()
                                                    .trim_matches(|ch| ch == '[' || ch == ']')
                                                    .split(',')
                                                    .filter_map(|t| t.trim().parse().ok())
                                                    .collect();
                                                if idx.len() >= 3 {
                                                    faces.push(idx);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                if kind != 0 {
                                    parts.push(crate::components::ShapePart {
                                        kind,
                                        a,
                                        b,
                                        c,
                                        offset,
                                        path,
                                        points,
                                        faces,
                                        scale,
                                    });
                                }
                            }
                            model.shapes.insert(name, parts);
                        }
                        Rule::field_stmt => {
                            let mut inner = item.into_inner();
                            let name = inner.next().unwrap().as_str().to_string();
                            let mut params: std::collections::BTreeMap<String, f64> =
                                Default::default();
                            for p in inner {
                                let mut pi = p.into_inner();
                                let key = pi.next().unwrap().as_str().to_string();
                                let rhs = pi.next().unwrap();
                                if let Some(v) = parse_scalar_number(rhs.as_str().trim()) {
                                    params.insert(key, v);
                                }
                            }
                            let width = params.get("width").copied().unwrap_or(0.0) as usize;
                            let height = params.get("height").copied().unwrap_or(0.0) as usize;
                            // `depth` is optional: a 2D field is depth 1.
                            let depth =
                                params.get("depth").copied().unwrap_or(1.0).max(1.0) as usize;
                            if width == 0 || height == 0 {
                                return Err(error(Status::Invalid, 75));
                            }
                            let dx = params.get("dx").copied().unwrap_or(1.0);
                            model.fields.push(crate::dsl::FieldDecl {
                                name,
                                width,
                                height,
                                depth,
                                dx,
                            });
                        }
                        Rule::params_stmt => {
                            // The repetition flattens to `ident`, `value`,
                            // `unit_expr`?, `ident`, `value`, … Tokens are
                            // classified by rule kind rather than position.
                            let mut pending_key: Option<String> = None;
                            for pair in item.into_inner() {
                                match pair.as_rule() {
                                    Rule::ident => pending_key = Some(pair.as_str().to_string()),
                                    Rule::value => {
                                        if let (Some(k), Some(v)) = (
                                            pending_key.take(),
                                            parse_scalar_number(pair.as_str().trim()),
                                        ) {
                                            model.params.insert(k, v);
                                        }
                                    }
                                    Rule::unit_expr => {
                                        if let (Some(k), Ok(d)) = (
                                            pending_key.clone(),
                                            pair.as_str()
                                                .trim_start_matches('[')
                                                .trim_end_matches(']')
                                                .parse::<crate::units::Dim>(),
                                        ) {
                                            model.param_units.insert(k, d);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Rule::entity_stmt => {
                            let mut inner = item.into_inner();
                            let name = inner.next().unwrap().as_str().to_string();
                            let mut decl = EntityDecl::named(&name);
                            for field in inner {
                                apply_entity_field(field, &mut decl)?;
                            }
                            model.entities.push(decl);
                        }
                        Rule::pool_stmt => {
                            let mut inner = item.into_inner();
                            let name = inner.next().unwrap().as_str().to_string();
                            let count = inner
                                .next()
                                .map(|n| n.as_str().parse::<u32>().unwrap_or(0))
                                .unwrap_or(0);
                            let mut decl = EntityDecl::named(&name);
                            for field in inner {
                                apply_entity_field(field, &mut decl)?;
                            }
                            model.pools.push(crate::dsl::PoolDecl { name, count, decl });
                        }
                        _ => {}
                    }
                }
            }
            Rule::systems_section => {
                for sys in section.into_inner() {
                    let rule = sys.as_rule();
                    let sys_off = sys.as_span().start();
                    let mut inner = sys.into_inner();
                    // send_system / recv_system imply their kind; a generic
                    // system's kind is its leading ident.
                    let kind = match rule {
                        Rule::send_system => "send".to_string(),
                        Rule::recv_system => "recv".to_string(),
                        _ => inner.next().unwrap().as_str().to_string(),
                    };
                    let mut decl = SystemDecl {
                        kind,
                        params: std::collections::BTreeMap::new(),
                        vec_params: std::collections::BTreeMap::new(),
                        update: std::collections::BTreeMap::new(),
                        update_stmts: Vec::new(),
                        param_units: std::collections::BTreeMap::new(),
                        namespace: String::new(),
                        string_params: std::collections::BTreeMap::new(),
                        byte_offset: sys_off,
                    };
                    for param in inner {
                        store_param(param, &mut decl)?;
                    }
                    systems.push(decl);
                }
            }
            Rule::funcs_section => {
                for func in section.into_inner() {
                    let mut inner = func.into_inner();
                    let name = inner.next().unwrap().as_str().to_string();
                    let mut params = Vec::new();
                    let mut body_child: Option<Pair<'_, Rule>> = None;
                    for child in inner {
                        match child.as_rule() {
                            Rule::param_list => {
                                params =
                                    child.into_inner().map(|p| p.as_str().to_string()).collect();
                            }
                            Rule::func_stmts | Rule::expr => body_child = Some(child),
                            _ => {}
                        }
                    }
                    let body_pair = body_child.ok_or(error(Status::Invalid, 55))?;
                    // Body: a bare expression, or statements ending with an
                    // explicit `return expr`. `func_body` is transparent, so
                    // its child (`func_stmts` or `expr`) appears directly.
                    let (stmts, body) = match body_pair.as_rule() {
                        Rule::expr => (Vec::new(), build_expr(body_pair)?),
                        Rule::func_stmts => {
                            let mut stmts: Vec<UpdateStmt> = Vec::new();
                            let mut body: Option<Expr> = None;
                            // `func_item` and `func_return` are transparent:
                            // children are [let_stmt|repeat_stmt|for_stmt…,
                            // return_kw, expr].
                            let children: Vec<Pair<'_, Rule>> = body_pair.into_inner().collect();
                            let mut i = 0;
                            while i < children.len() {
                                match children[i].as_rule() {
                                    Rule::let_stmt => {
                                        let mut li = children[i].clone().into_inner();
                                        let lname = li.next().unwrap().as_str().to_string();
                                        check_let_name(&lname, 0)?;
                                        let text = li.next().unwrap().as_str().trim().to_string();
                                        stmts.push(UpdateStmt::Let(lname, text));
                                        i += 1;
                                    }
                                    Rule::repeat_stmt | Rule::for_stmt => {
                                        stmts.push(build_loop_stmt(children[i].clone(), 0)?);
                                        i += 1;
                                    }
                                    Rule::return_kw => {
                                        let e = children
                                            .get(i + 1)
                                            .ok_or(error(Status::Invalid, 55))?;
                                        body = Some(build_expr(e.clone())?);
                                        i += 2;
                                    }
                                    _ => {
                                        i += 1;
                                    }
                                }
                            }
                            (stmts, body.ok_or(error(Status::Invalid, 55))?)
                        }
                        _ => return Err(error(Status::Invalid, 55)),
                    };
                    funcs.push(FuncDecl {
                        name,
                        namespace: String::new(),
                        params,
                        stmts,
                        body,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(ParsedProgram {
        model,
        systems,
        funcs,
    })
}

// ---------------------------------------------------------------------------
// Lower to low-level IR (EIR)
// ---------------------------------------------------------------------------

/// A user-defined nonlinear dynamical system over generic state slots, expressed
/// as per-slot scalar rules: `state[i] += dt · expr(state)`. Rules are parsed
/// scalar expressions (constants, state slots `s0…`, `+ - * /`, parentheses,
/// transcendental functions, and `@name.sN` cross-entity references), so it
/// expresses nonlinear phenomena — logistic growth, predator–prey, coupled
/// oscillators, N-body gravity — entirely from source. All referenced slots
/// (own and other entities') are read first (simultaneous, explicit Euler).
pub struct UpdateSystem {
    /// Slot rules, with raw LHS (`sN` or a named slot) resolved per-entity.
    pub rules: Vec<(String, Expr)>,
    /// Dynamic slot rules `s[idx] = expr`: the State slot at a runtime index,
    /// written via `ReadSlotDyn`/`WriteSlotDyn`.
    pub dyn_rules: Vec<(Expr, Expr)>,
    /// `let name = expr` local bindings, computed sequentially before the rules.
    pub lets: Vec<LetStmt>,
    /// Fallback slot count (max positional `sN` index + 1) when no named layout.
    pub slots_hint: usize,
    pub dt: f64,
    /// Run only when `step % every == 0` (scheduling; None = every step).
    pub every: Option<u64>,
    /// Integration substeps per step (the rules run `substeps` times with
    /// dt/substeps each; default 1).
    pub substeps: usize,
    /// Entity name -> id, for resolving `@name.sN` cross-entity references.
    pub entity_map: std::collections::BTreeMap<String, u128>,
    /// Optional set of entity ids this rule applies to (empty = all bodies).
    /// Lets a rule target only the Moon, for example.
    pub only: Option<std::collections::BTreeSet<u128>>,
    /// Optional gate: every rule's write is predicated on it (mode semantics;
    /// the state is untouched when it evaluates to 0).
    pub when: Option<Expr>,
    /// User-defined function name -> EIR function id (for `CALL`).
    pub func_ids: std::collections::BTreeMap<String, u64>,
    /// Grid field name -> width (compile-time, from the model).
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    /// Module namespace of this system's rules.
    pub namespace: String,
    /// Every declared parameter name (qualified), for namespace fallback.
    pub param_names: std::collections::BTreeSet<String>,
    /// Per-entity named state slot -> index (from `state = (x = 0, …)`).
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
}
impl EirSystem for UpdateSystem {
    fn name(&self) -> &'static str {
        "physics.update"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        if let Some(only) = &self.only {
            if !only.contains(&entity) {
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                return;
            }
        }
        // `every = n`: run only when step % n == 0. A single conditional
        // branch to the function's final Return (patched after the body).
        let mut gate: Option<(u32, usize)> = None;
        if let Some(n) = self.every {
            let next0 = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            let step_reg = next0;
            let mut next_id = next0 + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Step,
                step_reg,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                None,
            ));
            let n_reg = const_reg(n as f64, &mut next_id, out);
            let rem = binary(crate::eir::Opcode::Rem, step_reg, n_reg, &mut next_id, out);
            let zero = const_reg(0.0, &mut next_id, out);
            let skip = next_id;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Ne,
                skip,
                Some(crate::eir::ValueType::Bool),
                vec![rem, zero],
                None,
                None,
            ));
            let gi = out.len();
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::CondBr,
                0,
                None,
                vec![skip, 0, 0],
                None,
                None,
            ));
            gate = Some((skip, gi));
        }
        // Per-entity named state layout; resolve each rule's raw LHS (`sN` or a
        // named slot) to a slot index against this entity's own layout.
        let sn: std::collections::BTreeMap<String, usize> = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        let mut resolved: Vec<(usize, &Expr)> = Vec::new();
        let mut slots = self.slots_hint;
        for (lhs, expr) in &self.rules {
            let idx: Option<usize> = numeric_slot(lhs).or_else(|| sn.get(lhs).copied());
            if let Some(idx) = idx {
                slots = slots.max(idx + 1);
                resolved.push((idx, expr));
            }
        }
        // `let` locals may also read own slots (e.g. `temp`) that are not rule
        // LHS; cover them too so their reads bind real registers.
        let mut let_max: usize = 0;
        let mut let_any = false;
        let_stmts_slot_span(&self.lets, &sn, &mut let_max, &mut let_any);
        if let Some(w) = &self.when {
            expr_slot_span(w, &sn, &mut let_max, &mut let_any);
        }
        for (idx_expr, _) in &self.dyn_rules {
            expr_slot_span(idx_expr, &sn, &mut let_max, &mut let_any);
        }
        // A rule RHS may read an own slot that no rule writes (e.g. `x = vx`
        // reads `vx`); cover those reads too, or they would lower to register 0.
        for (_, expr) in &self.rules {
            expr_slot_span(expr, &sn, &mut let_max, &mut let_any);
        }
        if let_any {
            slots = slots.max(let_max + 1);
        }
        let _ = &sn;
        // Collect every distinct cross-entity reference, property reference, and
        // named-state reference used across all rules.
        let mut refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        let mut prop_refs: std::collections::BTreeSet<(String, PropKind)> = Default::default();
        let mut named_refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        for (_, expr) in &self.rules {
            collect_refs(
                expr,
                &mut refs,
                &mut prop_refs,
                &mut named_refs,
                &self.entity_map,
                &self.state_names_by_id,
            );
        }
        if let Some(w) = &self.when {
            collect_refs(
                w,
                &mut refs,
                &mut prop_refs,
                &mut named_refs,
                &self.entity_map,
                &self.state_names_by_id,
            );
        }
        collect_let_refs(
            &self.lets,
            &mut refs,
            &mut prop_refs,
            &mut named_refs,
            &self.entity_map,
            &self.state_names_by_id,
        );
        // Resolve refs to entity ids up front (unknown name -> no coupling).
        let mut ref_ids: Vec<(u128, usize)> = refs
            .iter()
            .chain(named_refs.iter())
            .filter_map(|(name, slot)| self.entity_map.get(name).map(|id| (*id, *slot)))
            .collect();
        ref_ids.sort_unstable();
        ref_ids.dedup();
        let mut prop_ids: Vec<(u128, PropKind)> = prop_refs
            .iter()
            .filter_map(|(name, kind)| self.entity_map.get(name).map(|id| (*id, *kind)))
            .collect();
        prop_ids.sort_unstable();
        prop_ids.dedup();

        // Read each distinct referenced (entity, slot) once.
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    rid,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(rslot),
                )),
            ));
            ref_regs.insert((rid, rslot), next);
        }
        // Read each distinct cross-entity property once.
        let mut prop_regs: std::collections::BTreeMap<(u128, PropKind), u32> = Default::default();
        for (pid, kind) in prop_ids {
            let (cid, off) = prop_component(kind);
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(pid, cid, off)),
            ));
            prop_regs.insert((pid, kind), next);
        }
        // Substeps: repeat the integration `substeps` times with dt/n; each
        // substep re-reads the own state (the previous substep's writes are
        // visible) and recomputes the `let` locals. Cross-entity references
        // and properties are sampled once per step.
        let dt_sub = self.dt / self.substeps as f64;
        for _ in 0..self.substeps {
            // Read every own slot once into registers (fresh per substep).
            let mut slot_regs: Vec<u32> = Vec::with_capacity(slots);
            for i in 0..slots {
                let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::ReadView,
                    next,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(i),
                    )),
                ));
                slot_regs.push(next);
            }
            // Compute `let` local bindings sequentially; each may use state slots
            // and earlier locals, and loops unroll with break/continue gating.
            // The results feed the slot rules below.
            let mut locals: std::collections::BTreeMap<String, u32> = Default::default();
            let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            let parts = LowerParts {
                slot_regs: &slot_regs,
                ref_regs: &ref_regs,
                prop_regs: &prop_regs,
                entity_map: &self.entity_map,
                state_names: &sn,
                state_names_by_id: &self.state_names_by_id,
                func_ids: &self.func_ids,
                namespace: &self.namespace,
                params: &self.param_names,
                field_dims: &self.field_dims,
                current_entity: entity,
            };
            lower_let_block(&self.lets, None, &mut next_id, out, &mut locals, &parts);
            // Rebuild the ctx so the slot rules can see the locals.
            let ctx = LowerCtx {
                slot_regs: &slot_regs,
                ref_regs: &ref_regs,
                prop_regs: &prop_regs,
                entity_map: &self.entity_map,
                state_names: &sn,
                state_names_by_id: &self.state_names_by_id,
                locals: &locals,
                func_ids: &self.func_ids,
                namespace: &self.namespace,
                params: &self.param_names,
                field_dims: &self.field_dims,
                current_entity: entity,
            };
            // `when = expr` gates every rule's write: `new = state + gate·delta` —
            // the state is untouched when the gate is 0 (mode semantics).
            let gate_reg = match &self.when {
                Some(w) => {
                    let rw = lower_expr(w, &ctx, &mut next_id, out);
                    Some(truthy(rw, &mut next_id, out))
                }
                None => None,
            };
            for (i, expr) in &resolved {
                if *i >= slots {
                    continue;
                }
                // delta = expr(state)
                let expr_reg = lower_expr(expr, &ctx, &mut next_id, out);
                // delta *= dt
                let dt_reg = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Const,
                    dt_reg,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    Some(crate::eir::Immediate::F64(dt_sub)),
                    None,
                ));
                let scaled = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Mul,
                    scaled,
                    Some(crate::eir::ValueType::F64),
                    vec![expr_reg, dt_reg],
                    None,
                    None,
                ));
                let scaled = match gate_reg {
                    Some(g) => binary(crate::eir::Opcode::Mul, scaled, g, &mut next_id, out),
                    None => scaled,
                };
                // new = state[i] + delta
                let new = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Add,
                    new,
                    Some(crate::eir::ValueType::F64),
                    vec![slot_regs[*i], scaled],
                    None,
                    None,
                ));
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteView,
                    0,
                    None,
                    vec![new],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(*i),
                    )),
                ));
            }
            // Dynamic slot rules: `s[idx] += dt·expr` via a runtime read and a
            // runtime-index write; the index may use slots and locals.
            for (idx_expr, expr) in &self.dyn_rules {
                let expr_reg = lower_expr(expr, &ctx, &mut next_id, out);
                let dt_reg = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Const,
                    dt_reg,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    Some(crate::eir::Immediate::F64(dt_sub)),
                    None,
                ));
                let scaled = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Mul,
                    scaled,
                    Some(crate::eir::ValueType::F64),
                    vec![expr_reg, dt_reg],
                    None,
                    None,
                ));
                let scaled = match gate_reg {
                    Some(g) => binary(crate::eir::Opcode::Mul, scaled, g, &mut next_id, out),
                    None => scaled,
                };
                let ri = lower_expr(idx_expr, &ctx, &mut next_id, out);
                let cur = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::ReadSlotDyn,
                    cur,
                    Some(crate::eir::ValueType::F64),
                    vec![ri],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        0,
                    )),
                ));
                let new = binary(crate::eir::Opcode::Add, cur, scaled, &mut next_id, out);
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteSlotDyn,
                    0,
                    None,
                    vec![ri, new],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        0,
                    )),
                ));
            }
        }
        match gate {
            Some((skip, gi)) => {
                // The body block must end in a terminator (the dominance
                // gate): append the body's own Return, then the skip target
                // the CondBr jumps to.
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                let ret_idx = out.len();
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                if let Some(ins) = out.get_mut(gi) {
                    ins.operands = vec![skip, ret_idx as u32, (gi + 1) as u32];
                }
            }
            None => out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            )),
        }
    }
}

/// A user-defined dynamical system integrated with the **classic 4th-order
/// Runge–Kutta** method (`rk4`). Rules are identical in form to the `update`
/// system — `slot = expr(state)` — but instead of explicit Euler
/// (`state += dt·expr`), each step evaluates the derivative four times and
/// combines them, giving much tighter accuracy for oscillators and nonlinear
/// ODEs at the same `dt`. Like `update`, all reads happen first (simultaneous);
/// cross-entity references and properties are sampled once per step.
pub struct Rk4System {
    /// Slot rules, raw LHS (`sN` or a named slot) resolved per-entity.
    pub rules: Vec<(String, Expr)>,
    /// `let name = expr` local bindings, recomputed per RK4 stage.
    pub lets: Vec<LetStmt>,
    /// Fallback slot count (max positional `sN` index + 1) when no named layout.
    pub slots_hint: usize,
    pub dt: f64,
    /// Run only when `step % every == 0` (scheduling; None = every step).
    pub every: Option<u64>,
    /// Optional gate on the final state write (mode semantics).
    pub when: Option<Expr>,
    /// Integration substeps per step (the rules run `substeps` times with
    /// dt/substeps each; default 1).
    pub substeps: usize,
    pub entity_map: std::collections::BTreeMap<String, u128>,
    pub only: Option<std::collections::BTreeSet<u128>>,
    pub func_ids: std::collections::BTreeMap<String, u64>,
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    pub namespace: String,
    pub param_names: std::collections::BTreeSet<String>,
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
}
impl EirSystem for Rk4System {
    fn name(&self) -> &'static str {
        "physics.rk4"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        if let Some(only) = &self.only {
            if !only.contains(&entity) {
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                return;
            }
        }
        // `every = n`: run only when step % n == 0. A single conditional
        // branch to the function's final Return (patched after the body).
        let mut gate: Option<(u32, usize)> = None;
        if let Some(n) = self.every {
            let next0 = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            let step_reg = next0;
            let mut next_id = next0 + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Step,
                step_reg,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                None,
            ));
            let n_reg = const_reg(n as f64, &mut next_id, out);
            let rem = binary(crate::eir::Opcode::Rem, step_reg, n_reg, &mut next_id, out);
            let zero = const_reg(0.0, &mut next_id, out);
            let skip = next_id;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Ne,
                skip,
                Some(crate::eir::ValueType::Bool),
                vec![rem, zero],
                None,
                None,
            ));
            let gi = out.len();
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::CondBr,
                0,
                None,
                vec![skip, 0, 0],
                None,
                None,
            ));
            gate = Some((skip, gi));
        }
        let sn: std::collections::BTreeMap<String, usize> = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        let mut resolved: Vec<(usize, &Expr)> = Vec::new();
        let mut slots = self.slots_hint;
        for (lhs, expr) in &self.rules {
            let idx: Option<usize> = numeric_slot(lhs).or_else(|| sn.get(lhs).copied());
            if let Some(idx) = idx {
                slots = slots.max(idx + 1);
                resolved.push((idx, expr));
            }
        }
        // `let` locals may also read own slots (e.g. `temp`) that are not rule
        // LHS; cover them too so their reads bind real registers.
        let mut let_max: usize = 0;
        let mut let_any = false;
        let_stmts_slot_span(&self.lets, &sn, &mut let_max, &mut let_any);
        if let Some(w) = &self.when {
            expr_slot_span(w, &sn, &mut let_max, &mut let_any);
        }
        if let_any {
            slots = slots.max(let_max + 1);
        }
        // Collect every distinct cross-entity / property / named reference.
        let mut refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        let mut prop_refs: std::collections::BTreeSet<(String, PropKind)> = Default::default();
        let mut named_refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        for (_, expr) in self.rules.iter() {
            collect_refs(
                expr,
                &mut refs,
                &mut prop_refs,
                &mut named_refs,
                &self.entity_map,
                &self.state_names_by_id,
            );
        }
        if let Some(w) = &self.when {
            collect_refs(
                w,
                &mut refs,
                &mut prop_refs,
                &mut named_refs,
                &self.entity_map,
                &self.state_names_by_id,
            );
        }
        collect_let_refs(
            &self.lets,
            &mut refs,
            &mut prop_refs,
            &mut named_refs,
            &self.entity_map,
            &self.state_names_by_id,
        );
        let mut ref_ids: Vec<(u128, usize)> = refs
            .iter()
            .chain(named_refs.iter())
            .filter_map(|(name, slot)| self.entity_map.get(name).map(|id| (*id, *slot)))
            .collect();
        ref_ids.sort_unstable();
        ref_ids.dedup();
        let mut prop_ids: Vec<(u128, PropKind)> = prop_refs
            .iter()
            .filter_map(|(name, kind)| self.entity_map.get(name).map(|id| (*id, *kind)))
            .collect();
        prop_ids.sort_unstable();
        prop_ids.dedup();

        // Cross-entity references and properties are sampled once per step.
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    rid,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(rslot),
                )),
            ));
            ref_regs.insert((rid, rslot), next);
        }
        let mut prop_regs: std::collections::BTreeMap<(u128, PropKind), u32> = Default::default();
        for (pid, kind) in prop_ids {
            let (cid, off) = prop_component(kind);
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(pid, cid, off)),
            ));
            prop_regs.insert((pid, kind), next);
        }
        // Substeps: repeat the whole RK4 step (base reads, stages, combine)
        // with dt/n; each substep re-reads the state.
        let dt_sub = self.dt / self.substeps as f64;
        for _ in 0..self.substeps {
            // Read the base (start-of-step) own state into `y`.
            let mut y: Vec<u32> = Vec::with_capacity(slots);
            for i in 0..slots {
                let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::ReadView,
                    next,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(i),
                    )),
                ));
                y.push(next);
            }
            // Evaluate the derivative four times (k1..k4), each against a working
            // state `y + shift·k_prev`. `None` marks a slot with no rule (k = 0).
            let mut stage_k: Vec<Vec<Option<u32>>> = Vec::with_capacity(4);
            let mut prev_k: Option<&Vec<Option<u32>>> = None;
            for stage in 0..4 {
                let mut work: Vec<u32> = Vec::with_capacity(slots);
                match prev_k {
                    None => {
                        // k1: evaluate at the base state.
                        work.extend_from_slice(&y);
                    }
                    Some(kp) => {
                        let shift = if stage < 3 { dt_sub / 2.0 } else { dt_sub };
                        for (i, base) in y.iter().copied().enumerate() {
                            let k = match kp.get(i) {
                                Some(Some(r)) => {
                                    let sreg =
                                        out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                                    out.push(crate::physics_eir::instr(
                                        crate::eir::Opcode::Const,
                                        sreg,
                                        Some(crate::eir::ValueType::F64),
                                        vec![],
                                        Some(crate::eir::Immediate::F64(shift)),
                                        None,
                                    ));
                                    let mut sid =
                                        out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                                    binary(crate::eir::Opcode::Mul, sreg, *r, &mut sid, out)
                                }
                                // No rule for this slot: derivative is 0, keep y.
                                _ => {
                                    let zreg =
                                        out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                                    out.push(crate::physics_eir::instr(
                                        crate::eir::Opcode::Const,
                                        zreg,
                                        Some(crate::eir::ValueType::F64),
                                        vec![],
                                        Some(crate::eir::Immediate::F64(0.0)),
                                        None,
                                    ));
                                    zreg
                                }
                            };
                            let mut next_id =
                                out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                            work.push(binary(crate::eir::Opcode::Add, base, k, &mut next_id, out));
                        }
                    }
                }
                // Recompute `let` locals against this stage's working state; loops
                // unroll with break/continue gating per stage.
                let mut locals: std::collections::BTreeMap<String, u32> = Default::default();
                let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                let parts = LowerParts {
                    slot_regs: &work,
                    ref_regs: &ref_regs,
                    prop_regs: &prop_regs,
                    entity_map: &self.entity_map,
                    state_names: &sn,
                    state_names_by_id: &self.state_names_by_id,
                    func_ids: &self.func_ids,
                    namespace: &self.namespace,
                    params: &self.param_names,
                    field_dims: &self.field_dims,
                    current_entity: entity,
                };
                lower_let_block(&self.lets, None, &mut next_id, out, &mut locals, &parts);
                let ctx = LowerCtx {
                    slot_regs: &work,
                    ref_regs: &ref_regs,
                    prop_regs: &prop_regs,
                    entity_map: &self.entity_map,
                    state_names: &sn,
                    state_names_by_id: &self.state_names_by_id,
                    locals: &locals,
                    func_ids: &self.func_ids,
                    namespace: &self.namespace,
                    params: &self.param_names,
                    field_dims: &self.field_dims,
                    current_entity: entity,
                };
                let mut k: Vec<Option<u32>> = vec![None; slots];
                for (i, expr) in &resolved {
                    k[*i] = Some(lower_expr(expr, &ctx, &mut next_id, out));
                }
                stage_k.push(k);
                prev_k = stage_k.last();
            }
            // Combine: y' = y + (dt/6)·(k1 + 2·k2 + 2·k3 + k4), write only resolved.
            // `when = expr` gates the final write: the gate is evaluated against
            // the step's input state (stages don't write state until the combine).
            let gate_reg = self.when.as_ref().map(|w| {
                let mut gmax = 0usize;
                let mut gany = false;
                expr_slot_span(w, &sn, &mut gmax, &mut gany);
                let gslots = if gany { gmax + 1 } else { 0 };
                let mut gregs: Vec<u32> = Vec::with_capacity(gslots);
                for i in 0..gslots {
                    let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::ReadView,
                        next,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        Some(crate::physics_eir::cr(
                            entity,
                            crate::physics_eir::state_id(),
                            crate::physics_eir::field::state_slot(i),
                        )),
                    ));
                    gregs.push(next);
                }
                let glocals: std::collections::BTreeMap<String, u32> = Default::default();
                let gctx = LowerCtx {
                    slot_regs: &gregs,
                    ref_regs: &ref_regs,
                    prop_regs: &prop_regs,
                    entity_map: &self.entity_map,
                    state_names: &sn,
                    state_names_by_id: &self.state_names_by_id,
                    locals: &glocals,
                    func_ids: &self.func_ids,
                    namespace: &self.namespace,
                    params: &self.param_names,
                    field_dims: &self.field_dims,
                    current_entity: entity,
                };
                let mut gnext = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                let rw = lower_expr(w, &gctx, &mut gnext, out);
                truthy(rw, &mut gnext, out)
            });
            for (i, _) in &resolved {
                let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                let k1 = stage_k[0][*i].unwrap();
                let k2 = stage_k[1][*i].unwrap();
                let k3 = stage_k[2][*i].unwrap();
                let k4 = stage_k[3][*i].unwrap();
                // 2*k2
                let two = {
                    let sreg = next_id;
                    next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Const,
                        sreg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        Some(crate::eir::Immediate::F64(2.0)),
                        None,
                    ));
                    binary(crate::eir::Opcode::Mul, sreg, k2, &mut next_id, out)
                };
                let two_k3 = {
                    let sreg = next_id;
                    next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Const,
                        sreg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        Some(crate::eir::Immediate::F64(2.0)),
                        None,
                    ));
                    binary(crate::eir::Opcode::Mul, sreg, k3, &mut next_id, out)
                };
                let sum = {
                    let a = binary(crate::eir::Opcode::Add, k1, two, &mut next_id, out);
                    let b = binary(crate::eir::Opcode::Add, two_k3, k4, &mut next_id, out);
                    binary(crate::eir::Opcode::Add, a, b, &mut next_id, out)
                };
                let dtsix = {
                    let sreg = next_id;
                    next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Const,
                        sreg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        Some(crate::eir::Immediate::F64(dt_sub / 6.0)),
                        None,
                    ));
                    binary(crate::eir::Opcode::Mul, sreg, sum, &mut next_id, out)
                };
                // Gate the delta (not the sum): the state is untouched when 0.
                let dtsix = match gate_reg {
                    Some(g) => binary(crate::eir::Opcode::Mul, dtsix, g, &mut next_id, out),
                    None => dtsix,
                };
                let new = binary(crate::eir::Opcode::Add, y[*i], dtsix, &mut next_id, out);
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteView,
                    0,
                    None,
                    vec![new],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(*i),
                    )),
                ));
            }
        }
        match gate {
            Some((skip, gi)) => {
                // The body block must end in a terminator (the dominance
                // gate): append the body's own Return, then the skip target
                // the CondBr jumps to.
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                let ret_idx = out.len();
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                if let Some(ins) = out.get_mut(gi) {
                    ins.operands = vec![skip, ret_idx as u32, (gi + 1) as u32];
                }
            }
            None => out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            )),
        }
    }
}

/// Lowers an `Expr` into EIR instructions, returning the result register id.
/// Lowering context: registers for own slots and resolved cross-entity refs.
struct LowerCtx<'a> {
    slot_regs: &'a [u32],
    ref_regs: &'a std::collections::BTreeMap<(u128, usize), u32>,
    prop_regs: &'a std::collections::BTreeMap<(u128, PropKind), u32>,
    entity_map: &'a std::collections::BTreeMap<String, u128>,
    /// Named state slot -> index (from `state = (x = 0, …)`).
    state_names: &'a std::collections::BTreeMap<String, usize>,
    /// Per-entity named state layouts, for resolving `@name.x` cross-entity.
    state_names_by_id:
        &'a std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    /// `let` local name -> register id.
    locals: &'a std::collections::BTreeMap<String, u32>,
    /// User-defined function name -> EIR function id (for `CALL`).
    func_ids: &'a std::collections::BTreeMap<String, u64>,
    /// Grid field name -> width (compile-time, from the model), for
    /// `fget`/`fset`/`flap` cell access.
    field_dims: &'a std::collections::BTreeMap<String, (u32, u32)>,
    /// The module namespace of the system being lowered; unqualified function
    /// and parameter references resolve within it first.
    namespace: &'a str,
    /// Every declared parameter name (qualified), for `namespace` fallback.
    params: &'a std::collections::BTreeSet<String>,
    /// The entity whose rule is being lowered; spatial queries
    /// (`neighbor_count`/`nearest_dist`) target it. 0 in function bodies,
    /// where queries are rejected at compile time.
    current_entity: u128,
}

/// Lowers an `Expr` into EIR instructions, returning the result register id.
fn lower_expr(
    expr: &Expr,
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    match expr {
        Expr::Const(c) => {
            let r = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Const,
                r,
                Some(crate::eir::ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(*c)),
                None,
            ));
            r
        }
        Expr::Slot(i) => ctx.slot_regs.get(*i).copied().unwrap_or(0),
        Expr::SlotDyn(idx) => {
            // `s[i]`: read the State slot at a runtime index via the EIR.
            let ri = lower_expr(idx, ctx, next_id, out);
            let out_reg = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadSlotDyn,
                out_reg,
                Some(crate::eir::ValueType::F64),
                vec![ri],
                None,
                Some(crate::physics_eir::cr(
                    ctx.current_entity,
                    crate::physics_eir::state_id(),
                    0,
                )),
            ));
            out_reg
        }
        Expr::Neg(x) => {
            let rx = lower_expr(x, ctx, next_id, out);
            let neg_one = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Const,
                neg_one,
                Some(crate::eir::ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(-1.0)),
                None,
            ));
            binary(crate::eir::Opcode::Mul, rx, neg_one, next_id, out)
        }
        Expr::Time => {
            // Read the global simulation clock (entity id is ignored by the runtime).
            let r = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                r,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    0,
                    crate::physics_eir::sim_time_id(),
                    crate::physics_eir::field::SIM_TIME,
                )),
            ));
            r
        }
        Expr::Ref(name, slot) => {
            let id = ctx.entity_map.get(name).copied().unwrap_or(u128::MAX);
            ctx.ref_regs.get(&(id, *slot)).copied().unwrap_or(0)
        }
        Expr::PropRef(name, kind) => {
            let id = ctx.entity_map.get(name).copied().unwrap_or(u128::MAX);
            ctx.prop_regs.get(&(id, *kind)).copied().unwrap_or(0)
        }
        Expr::Name(name) => {
            // Resolution order:
            //   1. a `let` local (bare names only),
            //   2. an own named state slot (bare names only),
            //   3. a model parameter — bare (`G`) or module-qualified (`mod.G`),
            //   4. a cross-entity named state slot (`@name.x` / `@name.state.x`).
            if !name.contains('.') {
                if let Some(&reg) = ctx.locals.get(name.as_str()) {
                    return reg;
                }
                if let Some(slot) = ctx.state_names.get(name.as_str()).copied() {
                    return ctx.slot_regs.get(slot).copied().unwrap_or(0);
                }
            }
            // A parameter: the exact name (module-qualified) or the system's
            // module namespace applied to a bare name, else globally bare.
            let param_canonical: Option<&str> = if ctx.params.contains(name.as_str()) {
                Some(name.as_str())
            } else {
                None
            };
            let qualified = if param_canonical.is_none() && !ctx.namespace.is_empty() {
                let q = format!("{}.{}", ctx.namespace, name);
                if ctx.params.contains(&q) {
                    Some(q)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(canonical) = param_canonical.or(qualified.as_deref()) {
                let r = *next_id;
                *next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::ReadView,
                    r,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    None,
                    Some(crate::physics_eir::cr(
                        0,
                        crate::physics_eir::param_component_id(canonical),
                        0,
                    )),
                ));
                return r;
            }
            // Cross-entity named state slot.
            if let Some((ent, rest)) = name.split_once('.') {
                let slot_name = rest.strip_prefix("state.").unwrap_or(rest);
                let id = ctx.entity_map.get(ent).copied().unwrap_or(u128::MAX);
                let slot = ctx
                    .state_names_by_id
                    .get(&id)
                    .and_then(|m| m.get(slot_name))
                    .copied()
                    .unwrap_or(0);
                return ctx.ref_regs.get(&(id, slot)).copied().unwrap_or(0);
            }
            // An undeclared bare name: read 0.0 (via an unset parameter).
            let r = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                r,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    0,
                    crate::physics_eir::param_component_id(name),
                    0,
                )),
            ));
            r
        }
        Expr::Add(a, b) => {
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            binary(crate::eir::Opcode::Add, ra, rb, next_id, out)
        }
        Expr::Sub(a, b) => {
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            binary(crate::eir::Opcode::Sub, ra, rb, next_id, out)
        }
        Expr::Mul(a, b) => {
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            binary(crate::eir::Opcode::Mul, ra, rb, next_id, out)
        }
        Expr::Div(a, b) => {
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            binary(crate::eir::Opcode::Div, ra, rb, next_id, out)
        }
        Expr::Rem(a, b) => {
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            binary(crate::eir::Opcode::Rem, ra, rb, next_id, out)
        }
        Expr::Cmp(op, a, b) => {
            // Compute the boolean comparison, then Select(cond, 1.0, 0.0).
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            let cmp_op = match *op {
                "<" => crate::eir::Opcode::Lt,
                "<=" => crate::eir::Opcode::Le,
                ">" => crate::eir::Opcode::Gt,
                ">=" => crate::eir::Opcode::Ge,
                "==" => crate::eir::Opcode::Eq,
                "!=" => crate::eir::Opcode::Ne,
                _ => crate::eir::Opcode::Nop,
            };
            let bool_reg = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                cmp_op,
                bool_reg,
                Some(crate::eir::ValueType::Bool),
                vec![ra, rb],
                None,
                None,
            ));
            // Select(cond, 1.0, 0.0)
            let one = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Const,
                one,
                Some(crate::eir::ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(1.0)),
                None,
            ));
            let zero = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Const,
                zero,
                Some(crate::eir::ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(0.0)),
                None,
            ));
            let r = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Select,
                r,
                Some(crate::eir::ValueType::F64),
                vec![bool_reg, one, zero],
                None,
                None,
            ));
            r
        }
        Expr::And(a, b) => {
            // bool(a) AND bool(b): normalize each operand to 1.0 / 0.0, then
            // multiply. Any nonzero operand counts as true.
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            let na = truthy(ra, next_id, out);
            let nb = truthy(rb, next_id, out);
            binary(crate::eir::Opcode::Mul, na, nb, next_id, out)
        }
        Expr::Or(a, b) => {
            // bool(a) OR bool(b) via de Morgan: 1 - (1-na)*(1-nb).
            let ra = lower_expr(a, ctx, next_id, out);
            let rb = lower_expr(b, ctx, next_id, out);
            let na = truthy(ra, next_id, out);
            let nb = truthy(rb, next_id, out);
            let one = const_reg(1.0, next_id, out);
            let not_a = binary(crate::eir::Opcode::Sub, one, na, next_id, out);
            let not_b = binary(crate::eir::Opcode::Sub, one, nb, next_id, out);
            let both_false = binary(crate::eir::Opcode::Mul, not_a, not_b, next_id, out);
            binary(crate::eir::Opcode::Sub, one, both_false, next_id, out)
        }
        Expr::Not(a) => {
            // NOT bool(a): Eq(a, 0) yields 1.0 exactly when `a` is zero.
            let ra = lower_expr(a, ctx, next_id, out);
            let zero = const_reg(0.0, next_id, out);
            let bool_reg = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Eq,
                bool_reg,
                Some(crate::eir::ValueType::Bool),
                vec![ra, zero],
                None,
                None,
            ));
            select_bool(bool_reg, next_id, out)
        }
        Expr::Call(name, args) => {
            // A user-defined function (not a builtin) lowers to an EIR `CALL`.
            // Unqualified names resolve within the system's module first.
            let qualified = if ctx.namespace.is_empty() {
                None
            } else {
                ctx.func_ids
                    .get(&format!("{}.{}", ctx.namespace, name))
                    .copied()
            };
            if let Some(target) = qualified.or_else(|| ctx.func_ids.get(*name).copied()) {
                let mut operands = vec![target as u32];
                for a in args {
                    operands.push(lower_expr(a, ctx, next_id, out));
                }
                let out_reg = *next_id;
                *next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Call,
                    out_reg,
                    Some(crate::eir::ValueType::F64),
                    operands,
                    None,
                    None,
                ));
                return out_reg;
            }
            // `if` / `min` / `max` lower via comparisons + Select; the rest are
            // elementary functions.
            let (op, needs_bool): (crate::eir::Opcode, bool) = match *name {
                "sin" => (crate::eir::Opcode::Sin, false),
                "cos" => (crate::eir::Opcode::Cos, false),
                "exp" => (crate::eir::Opcode::Exp, false),
                "ln" => (crate::eir::Opcode::Ln, false),
                "sqrt" => (crate::eir::Opcode::Sqrt, false),
                "pow" => (crate::eir::Opcode::Pow, false),
                "abs" => (crate::eir::Opcode::Abs, false),
                "floor" => (crate::eir::Opcode::Floor, false),
                "ceil" => (crate::eir::Opcode::Ceil, false),
                "round" => (crate::eir::Opcode::Round, false),
                "sign" => (crate::eir::Opcode::Sign, false),
                "log10" => (crate::eir::Opcode::Log10, false),
                "log2" => (crate::eir::Opcode::Log2, false),
                "sinh" => (crate::eir::Opcode::Sinh, false),
                "cosh" => (crate::eir::Opcode::Cosh, false),
                "tanh" => (crate::eir::Opcode::Tanh, false),
                "asin" => (crate::eir::Opcode::Asin, false),
                "acos" => (crate::eir::Opcode::Acos, false),
                "atan" => (crate::eir::Opcode::Atan, false),
                "atan2" => (crate::eir::Opcode::Atan2, false),
                "hypot" => (crate::eir::Opcode::Hypot, false),
                "print" => (crate::eir::Opcode::Print, false),
                "last_event" => (crate::eir::Opcode::ReadEvent, false),
                _ => (crate::eir::Opcode::Nop, false),
            };
            let r = match *name {
                "if" => {
                    let cond = lower_expr(&args[0], ctx, next_id, out);
                    let a = lower_expr(&args[1], ctx, next_id, out);
                    let b = lower_expr(&args[2], ctx, next_id, out);
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Select,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![cond, a, b],
                        None,
                        None,
                    ));
                    out_reg
                }
                "min" | "max" => {
                    let a = lower_expr(&args[0], ctx, next_id, out);
                    let b = lower_expr(&args[1], ctx, next_id, out);
                    let cmp = if *name == "min" {
                        crate::eir::Opcode::Lt
                    } else {
                        crate::eir::Opcode::Gt
                    };
                    let bool_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        cmp,
                        bool_reg,
                        Some(crate::eir::ValueType::Bool),
                        vec![a, b],
                        None,
                        None,
                    ));
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Select,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![bool_reg, a, b],
                        None,
                        None,
                    ));
                    out_reg
                }
                "random" => {
                    // Emit a (seeded, reproducible) random draw in [0,1).
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Random,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        None,
                    ));
                    out_reg
                }
                "active" => {
                    // RFC-0038: read the current entity's activation flag.
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::ReadView,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        Some(crate::physics_eir::cr(
                            ctx.current_entity,
                            crate::physics_eir::active_id(),
                            0,
                        )),
                    ));
                    out_reg
                }
                "emit" => {
                    // Emit an ordered event (kind, payload); yields 0.0.
                    let kind = lower_expr(&args[0], ctx, next_id, out);
                    let payload = lower_expr(&args[1], ctx, next_id, out);
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::EmitEvent,
                        0,
                        None,
                        vec![kind, payload],
                        None,
                        None,
                    ));
                    // Return 0.0 as a constant expression value.
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Const,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        Some(crate::eir::Immediate::F64(0.0)),
                        None,
                    ));
                    out_reg
                }
                "at" | "periodic" | "schedule" => {
                    // Scheduled events: `at`/`periodic` probe the step's time
                    // window; `schedule(gate, delay, kind, payload)` pushes an
                    // event into the queue when `gate` is nonzero.
                    let op = match *name {
                        "at" => crate::eir::Opcode::FiredAt,
                        "periodic" => crate::eir::Opcode::FiredEvery,
                        _ => crate::eir::Opcode::ScheduleEvent,
                    };
                    let operands: Vec<u32> = args
                        .iter()
                        .map(|a| lower_expr(a, ctx, next_id, out))
                        .collect();
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        op,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        operands,
                        None,
                        None,
                    ));
                    out_reg
                }
                "neighbor_count" | "nearest_dist" | "neighbor_mean" | "nearest_dx"
                | "nearest_dy" | "nearest_dz" => {
                    // Spatial queries read the world via the EirRuntime: the
                    // target is the rule's own entity (compile-time), the
                    // radius a runtime value. Rejected in function bodies.
                    let op = match *name {
                        "neighbor_count" => crate::eir::Opcode::NeighborCount,
                        "nearest_dist" => crate::eir::Opcode::NearestDist,
                        "neighbor_mean" => crate::eir::Opcode::NeighborMean,
                        "nearest_dx" => crate::eir::Opcode::NearestOffsetX,
                        "nearest_dy" => crate::eir::Opcode::NearestOffsetY,
                        _ => crate::eir::Opcode::NearestOffsetZ,
                    };
                    let operands: Vec<u32> = args
                        .iter()
                        .map(|a| lower_expr(a, ctx, next_id, out))
                        .collect();
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        op,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        operands,
                        None,
                        Some(crate::physics_eir::cr(
                            ctx.current_entity,
                            crate::physics_eir::transform_id(),
                            0,
                        )),
                    ));
                    out_reg
                }
                "noise" => {
                    // Box–Muller standard normal from two seeded draws:
                    // sqrt(-2·ln(1-u1)) · cos(2π·u2); 1-u1 ∈ (0,1] keeps the
                    // log finite, so the magnitude is never NaN/inf.
                    let u1 = random_reg(next_id, out);
                    let u2 = random_reg(next_id, out);
                    let one = const_reg(1.0, next_id, out);
                    let zero = const_reg(0.0, next_id, out);
                    let two = const_reg(2.0, next_id, out);
                    let two_pi = const_reg(2.0 * std::f64::consts::PI, next_id, out);
                    let m1 = binary(crate::eir::Opcode::Sub, one, u1, next_id, out);
                    let l = unary(crate::eir::Opcode::Ln, m1, next_id, out);
                    let two_l = binary(crate::eir::Opcode::Mul, two, l, next_id, out);
                    let pos = binary(crate::eir::Opcode::Sub, zero, two_l, next_id, out);
                    let mag = unary(crate::eir::Opcode::Sqrt, pos, next_id, out);
                    let ang = binary(crate::eir::Opcode::Mul, two_pi, u2, next_id, out);
                    let c = unary(crate::eir::Opcode::Cos, ang, next_id, out);
                    binary(crate::eir::Opcode::Mul, mag, c, next_id, out)
                }
                "vlen" | "vdot" | "vdist" => {
                    // Vector helpers over scalar components, pure arithmetic:
                    // vlen(x,y,z) = √(x²+y²+z²); vdot = Σ component products;
                    // vdist = length of the component difference.
                    let comps: Vec<u32> = args
                        .iter()
                        .map(|a| lower_expr(a, ctx, next_id, out))
                        .collect();
                    let squares: Vec<u32> = match *name {
                        "vlen" => (0..3)
                            .map(|i| {
                                binary(crate::eir::Opcode::Mul, comps[i], comps[i], next_id, out)
                            })
                            .collect(),
                        "vdot" => (0..3)
                            .map(|i| {
                                binary(
                                    crate::eir::Opcode::Mul,
                                    comps[i],
                                    comps[i + 3],
                                    next_id,
                                    out,
                                )
                            })
                            .collect(),
                        _ => (0..3)
                            .map(|i| {
                                let d = binary(
                                    crate::eir::Opcode::Sub,
                                    comps[i],
                                    comps[i + 3],
                                    next_id,
                                    out,
                                );
                                binary(crate::eir::Opcode::Mul, d, d, next_id, out)
                            })
                            .collect(),
                    };
                    let sum = binary(
                        crate::eir::Opcode::Add,
                        squares[0],
                        squares[1],
                        next_id,
                        out,
                    );
                    let total = binary(crate::eir::Opcode::Add, sum, squares[2], next_id, out);
                    if *name == "vdot" {
                        total
                    } else {
                        unary(crate::eir::Opcode::Sqrt, total, next_id, out)
                    }
                }
                "fget" | "flap" | "fset" => {
                    // Grid field access: the field's canonical component id and
                    // width ride in the target (compile-time from the model);
                    // the cell coordinates are runtime values.
                    let Expr::Name(fname) = &args[0] else {
                        // The parse requires a literal field name.
                        return 0;
                    };
                    let (width, height) = ctx
                        .field_dims
                        .get(fname.as_str())
                        .copied()
                        .unwrap_or((0, 0));
                    let target = crate::physics_eir::cr(
                        0,
                        crate::physics_eir::field_component_id(fname),
                        width,
                    );
                    let is_fset = *name == "fset";
                    // The value (fset only) is the last argument; the index
                    // arguments are `(i, j)` in 2D and `(i, j, k)` in 3D. The
                    // runtime packs `(j, k)` as `j + k·height` — the same
                    // combination the linear `[k][j][i]` index uses — so the
                    // existing field opcodes address a 3D grid unchanged.
                    let idx_args = if is_fset { args.len() - 1 } else { args.len() };
                    let three_d = idx_args - 1 == 3;
                    let ri = lower_expr(&args[1], ctx, next_id, out);
                    let rj = lower_expr(&args[2], ctx, next_id, out);
                    let rj = if three_d {
                        let rk = lower_expr(&args[3], ctx, next_id, out);
                        let h = const_reg(height as f64, next_id, out);
                        let kt = binary(crate::eir::Opcode::Mul, rk, h, next_id, out);
                        binary(crate::eir::Opcode::Add, rj, kt, next_id, out)
                    } else {
                        rj
                    };
                    if is_fset {
                        let rv = lower_expr(&args[idx_args], ctx, next_id, out);
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::WriteFieldCell,
                            0,
                            None,
                            vec![ri, rj, rv],
                            None,
                            Some(target),
                        ));
                        let out_reg = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Const,
                            out_reg,
                            Some(crate::eir::ValueType::F64),
                            vec![],
                            Some(crate::eir::Immediate::F64(0.0)),
                            None,
                        ));
                        out_reg
                    } else {
                        let op = if *name == "fget" {
                            crate::eir::Opcode::ReadFieldCell
                        } else {
                            crate::eir::Opcode::FieldLaplacian
                        };
                        let out_reg = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            op,
                            out_reg,
                            Some(crate::eir::ValueType::F64),
                            vec![ri, rj],
                            None,
                            Some(target),
                        ));
                        out_reg
                    }
                }
                _ => {
                    let mut operands = Vec::with_capacity(args.len());
                    for a in args {
                        operands.push(lower_expr(a, ctx, next_id, out));
                    }
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        op,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        operands,
                        None,
                        None,
                    ));
                    out_reg
                }
            };
            let _ = needs_bool;
            r
        }
    }
}

/// A declarative **invariant check** (`invariant`): an expression that must
/// hold — be non-zero — for the checked entities at every step boundary. The
/// check lowers to plain EIR: the expression is evaluated per entity and its
/// 0/1 verdict is written to a dedicated hidden `pwe.lang.check` component
/// field, so both backends execute it identically. A zero value fails the step
/// (`step_*` returns `Err`) **before any write is applied**: the scene is left
/// untouched, never silently proceeding past a broken state. A NaN value fails
/// the step the same way — the EIR's own comparison rejection (NaN never
/// compares) surfaces first, with the scene untouched either way.
pub struct InvariantSystem {
    /// The invariant expression: holds when truthy (non-zero, non-NaN).
    pub expr: Expr,
    /// `let name = expr` local bindings, computed sequentially before the check.
    pub lets: Vec<LetStmt>,
    /// Byte offset of this invariant's verdict within the check component.
    /// Each `invariant` system owns one offset so several coexist.
    pub check_offset: u32,
    /// Entity name -> id, for resolving `@name.sN` cross-entity references.
    pub entity_map: std::collections::BTreeMap<String, u128>,
    /// Optional set of entity ids this invariant applies to (empty = all bodies).
    pub only: Option<std::collections::BTreeSet<u128>>,
    /// User-defined function name -> EIR function id (for `CALL`).
    pub func_ids: std::collections::BTreeMap<String, u64>,
    /// Grid field name -> width (compile-time, from the model).
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    /// Module namespace of this system's rules.
    pub namespace: String,
    /// Every declared parameter name (qualified), for namespace fallback.
    pub param_names: std::collections::BTreeSet<String>,
    /// Per-entity named state slot -> index (from `state = (x = 0, …)`).
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
}
impl EirSystem for InvariantSystem {
    fn name(&self) -> &'static str {
        "lang.invariant"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        if let Some(only) = &self.only {
            if !only.contains(&entity) {
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                return;
            }
        }
        // Per-entity named state layout for bare-name slot resolution.
        let sn: std::collections::BTreeMap<String, usize> = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        // Collect cross-entity references and properties, and the highest own
        // state slot the check reads (`sN` or a named slot).
        let mut refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        let mut prop_refs: std::collections::BTreeSet<(String, PropKind)> = Default::default();
        let mut named_refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        collect_refs(
            &self.expr,
            &mut refs,
            &mut prop_refs,
            &mut named_refs,
            &self.entity_map,
            &self.state_names_by_id,
        );
        collect_let_refs(
            &self.lets,
            &mut refs,
            &mut prop_refs,
            &mut named_refs,
            &self.entity_map,
            &self.state_names_by_id,
        );
        let mut ref_ids: Vec<(u128, usize)> = refs
            .iter()
            .chain(named_refs.iter())
            .filter_map(|(name, slot)| self.entity_map.get(name).map(|id| (*id, *slot)))
            .collect();
        ref_ids.sort_unstable();
        ref_ids.dedup();
        let mut prop_ids: Vec<(u128, PropKind)> = prop_refs
            .iter()
            .filter_map(|(name, kind)| self.entity_map.get(name).map(|id| (*id, *kind)))
            .collect();
        prop_ids.sort_unstable();
        prop_ids.dedup();
        let mut max_slot: usize = 0;
        let mut any_own = false;
        expr_slot_span(&self.expr, &sn, &mut max_slot, &mut any_own);
        let_stmts_slot_span(&self.lets, &sn, &mut max_slot, &mut any_own);
        // Read every own slot once into registers (the check reads the step's
        // input state, like every other system).
        let slots = if any_own { max_slot + 1 } else { 0 };
        let mut slot_regs: Vec<u32> = Vec::with_capacity(slots);
        for i in 0..slots {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    entity,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(i),
                )),
            ));
            slot_regs.push(next);
        }
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    rid,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(rslot),
                )),
            ));
            ref_regs.insert((rid, rslot), next);
        }
        let mut prop_regs: std::collections::BTreeMap<(u128, PropKind), u32> = Default::default();
        for (pid, kind) in prop_ids {
            let (cid, off) = prop_component(kind);
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(pid, cid, off)),
            ));
            prop_regs.insert((pid, kind), next);
        }
        // Compute `let` locals, then the expression, then the 0/1 verdict:
        // violated iff the value is zero. A NaN value fails the step too — the
        // EIR's own comparison rejection (NaN never compares equal) surfaces
        // before the verdict, with the scene untouched either way.
        let mut locals: std::collections::BTreeMap<String, u32> = Default::default();
        let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let parts = LowerParts {
            slot_regs: &slot_regs,
            ref_regs: &ref_regs,
            prop_regs: &prop_regs,
            entity_map: &self.entity_map,
            state_names: &sn,
            state_names_by_id: &self.state_names_by_id,
            func_ids: &self.func_ids,
            namespace: &self.namespace,
            params: &self.param_names,
            field_dims: &self.field_dims,
            current_entity: entity,
        };
        lower_let_block(&self.lets, None, &mut next_id, out, &mut locals, &parts);
        let ctx = parts.ctx(&locals);
        let expr_reg = lower_expr(&self.expr, &ctx, &mut next_id, out);
        let zero = const_reg(0.0, &mut next_id, out);
        let one = const_reg(1.0, &mut next_id, out);
        let eq0 = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Eq,
            eq0,
            Some(crate::eir::ValueType::Bool),
            vec![expr_reg, zero],
            None,
            None,
        ));
        let viol = next_id;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Select,
            viol,
            Some(crate::eir::ValueType::F64),
            vec![eq0, one, zero],
            None,
            None,
        ));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::WriteView,
            0,
            None,
            vec![viol],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::check_id(),
                self.check_offset,
            )),
        ));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Return,
            0,
            None,
            vec![],
            None,
            None,
        ));
    }
}

/// A declarative **zero-crossing watch** (`watch`): detects when a watched
/// expression changes sign between consecutive steps and raises a 0/1 flag.
/// The previous value lives in a user-dedicated state slot (`mem`), so the
/// watch's cross-step memory is ordinary world state — persisted by the step's
/// write application, deterministic, no hidden schema. The flag lands in
/// `into`; the entity's own rules read it and react (bounce, reinit, switch).
/// NaN values fail the step via the EIR's own comparison rejection.
pub struct WatchSystem {
    /// The watched expression.
    pub expr: Expr,
    /// State slot index holding the previous step's value (the watch memory).
    pub mem: usize,
    /// State slot index receiving the crossing flag (0/1).
    pub into: usize,
    /// Entity name -> id, for resolving `@name.sN` cross-entity references.
    pub entity_map: std::collections::BTreeMap<String, u128>,
    /// Optional set of entity ids this watch applies to (empty = all bodies).
    pub only: Option<std::collections::BTreeSet<u128>>,
    /// User-defined function name -> EIR function id (for `CALL`).
    pub func_ids: std::collections::BTreeMap<String, u64>,
    /// Grid field name -> width (compile-time, from the model).
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    /// Module namespace of this system's rules.
    pub namespace: String,
    /// Every declared parameter name (qualified), for namespace fallback.
    pub param_names: std::collections::BTreeSet<String>,
    /// Per-entity named state slot -> index (from `state = (x = 0, …)`).
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
}
impl EirSystem for WatchSystem {
    fn name(&self) -> &'static str {
        "lang.watch"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        if let Some(only) = &self.only {
            if !only.contains(&entity) {
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Return,
                    0,
                    None,
                    vec![],
                    None,
                    None,
                ));
                return;
            }
        }
        let sn: std::collections::BTreeMap<String, usize> = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        let mut refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        let mut prop_refs: std::collections::BTreeSet<(String, PropKind)> = Default::default();
        let mut named_refs: std::collections::BTreeSet<(String, usize)> = Default::default();
        collect_refs(
            &self.expr,
            &mut refs,
            &mut prop_refs,
            &mut named_refs,
            &self.entity_map,
            &self.state_names_by_id,
        );
        let mut ref_ids: Vec<(u128, usize)> = refs
            .iter()
            .chain(named_refs.iter())
            .filter_map(|(name, slot)| self.entity_map.get(name).map(|id| (*id, *slot)))
            .collect();
        ref_ids.sort_unstable();
        ref_ids.dedup();
        let mut prop_ids: Vec<(u128, PropKind)> = prop_refs
            .iter()
            .filter_map(|(name, kind)| self.entity_map.get(name).map(|id| (*id, *kind)))
            .collect();
        prop_ids.sort_unstable();
        prop_ids.dedup();
        // Own slots: the expression's reads plus the watch's memory and flag.
        let mut max_slot: usize = 0;
        let mut any_own = false;
        expr_slot_span(&self.expr, &sn, &mut max_slot, &mut any_own);
        max_slot = max_slot.max(self.mem).max(self.into);
        let slots = max_slot + 1;
        let mut slot_regs: Vec<u32> = Vec::with_capacity(slots);
        for i in 0..slots {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    entity,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(i),
                )),
            ));
            slot_regs.push(next);
        }
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    rid,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(rslot),
                )),
            ));
            ref_regs.insert((rid, rslot), next);
        }
        let mut prop_regs: std::collections::BTreeMap<(u128, PropKind), u32> = Default::default();
        for (pid, kind) in prop_ids {
            let (cid, off) = prop_component(kind);
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(pid, cid, off)),
            ));
            prop_regs.insert((pid, kind), next);
        }
        let locals: std::collections::BTreeMap<String, u32> = Default::default();
        let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let parts = LowerParts {
            slot_regs: &slot_regs,
            ref_regs: &ref_regs,
            prop_regs: &prop_regs,
            entity_map: &self.entity_map,
            state_names: &sn,
            state_names_by_id: &self.state_names_by_id,
            func_ids: &self.func_ids,
            namespace: &self.namespace,
            params: &self.param_names,
            field_dims: &self.field_dims,
            current_entity: entity,
        };
        let ctx = parts.ctx(&locals);
        // v = expr(state); prev = mem
        let v = lower_expr(&self.expr, &ctx, &mut next_id, out);
        let prev = slot_regs[self.mem];
        // crossing = prev·v < 0 (a strict sign change; a zero memory is the
        // initial state and never counts as a crossing).
        let prod = binary(crate::eir::Opcode::Mul, prev, v, &mut next_id, out);
        let zero = const_reg(0.0, &mut next_id, out);
        let lt = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Lt,
            lt,
            Some(crate::eir::ValueType::Bool),
            vec![prod, zero],
            None,
            None,
        ));
        let crossing = select_bool(lt, &mut next_id, out);
        // Flag (raw) and memory update (raw) — assignments, not dt-integrated.
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::WriteView,
            0,
            None,
            vec![crossing],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::state_id(),
                crate::physics_eir::field::state_slot(self.into),
            )),
        ));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::WriteView,
            0,
            None,
            vec![v],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::state_id(),
                crate::physics_eir::field::state_slot(self.mem),
            )),
        ));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Return,
            0,
            None,
            vec![],
            None,
            None,
        ));
    }
}

/// A grid-field **diffusion** solver (`diffuse { field = heat; rate = r }`):
/// every step advances each cell by `T += rate·∇²T` (Gauss–Seidel: later cells
/// in a step see earlier cells' fresh writes). The step runs once per step
/// (only the first dynamic entity emits the sweep). `rate ≤ 1/4` in 2D for
/// explicit stability.
/// RFC-0038: `spawn { on = <caller>; pool = <name> }`. One `SpawnInto` opcode
/// per caller/step activates the lowest free slot and copies the caller's state.
pub struct SpawnSystem {
    pub on: u128,
    pub base: u128,
    /// Pool size (the runtime scan range).
    pub count: u32,
    pub limbs: [u32; 4],
    /// Slots to activate per emission (batch), default 1.
    pub per_step: u32,
    /// When set, emit only on steps where `step % every == phase` (phase).
    pub every: Option<u64>,
    pub phase: u64,
}
impl EirSystem for SpawnSystem {
    fn name(&self) -> &'static str {
        "physics.spawn"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if entity != self.on {
            ret(out);
            return;
        }
        let mut next_id = 1u32;
        // Optional phase gate: emit only when `step % every == phase`.
        let mut gate: Option<(u32, usize)> = None;
        if let Some(n) = self.every {
            let step_reg = next_id;
            next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Step,
                step_reg,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                None,
            ));
            let n_reg = const_reg(n as f64, &mut next_id, out);
            let rem = binary(crate::eir::Opcode::Rem, step_reg, n_reg, &mut next_id, out);
            let phase_reg = const_reg(self.phase as f64, &mut next_id, out);
            let skip = next_id;
            next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Ne,
                skip,
                Some(crate::eir::ValueType::Bool),
                vec![rem, phase_reg],
                None,
                None,
            ));
            let gi = out.len();
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::CondBr,
                0,
                None,
                vec![skip, 0, 0],
                None,
                None,
            ));
            gate = Some((skip, gi));
        }
        // Fixed operands: pool base limbs + pool size; emitted `per_step` times.
        let mut operands = Vec::with_capacity(5);
        for limb in self.limbs {
            operands.push(const_u64_reg(limb as u64, &mut next_id, out));
        }
        operands.push(const_u64_reg(self.count as u64, &mut next_id, out));
        for _ in 0..self.per_step.max(1) {
            let slot_reg = next_id;
            next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::SpawnInto,
                slot_reg,
                Some(crate::eir::ValueType::F64),
                operands.clone(),
                None,
                Some(crate::physics_eir::cr(
                    entity,
                    crate::physics_eir::state_id(),
                    0,
                )),
            ));
        }
        match gate {
            Some((skip, gi)) => {
                // Body block ends in a terminator; the CondBr skips to a second
                // Return (the same idiom the `update` gate uses).
                ret(out);
                let ret_idx = out.len();
                ret(out);
                if let Some(ins) = out.get_mut(gi) {
                    ins.operands = vec![skip, ret_idx as u32, (gi + 1) as u32];
                }
            }
            None => ret(out),
        }
    }
}

/// RFC-0038: `despawn { on = <pool>; when = <expr> }`. For every slot, the
/// activation flag is cleared when it is active and `when` holds:
/// `active = Select(active && when, 0, active)`.
pub struct DespawnSystem {
    pub pool_slots: Vec<u128>,
    pub when: Expr,
    pub entity_map: std::collections::BTreeMap<String, u128>,
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    pub func_ids: std::collections::BTreeMap<String, u64>,
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    pub namespace: String,
    pub param_names: std::collections::BTreeSet<String>,
}
impl EirSystem for DespawnSystem {
    fn name(&self) -> &'static str {
        "physics.despawn"
    }
    fn guards_pool_slots(&self) -> bool {
        // `despawn` must still run on slots it is about to clear.
        false
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if !self.pool_slots.contains(&entity) {
            ret(out);
            return;
        }
        let sn = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        let mut max_slot = 0usize;
        let mut any = false;
        expr_slot_span(&self.when, &sn, &mut max_slot, &mut any);
        let slots = if any { max_slot + 1 } else { 0 };
        let mut next_id = 1u32;
        let mut slot_regs = Vec::with_capacity(slots);
        for i in 0..slots {
            let r = next_id;
            next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                r,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    entity,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(i),
                )),
            ));
            slot_regs.push(r);
        }
        let empty_refs = std::collections::BTreeMap::new();
        let empty_props = std::collections::BTreeMap::new();
        let locals = std::collections::BTreeMap::new();
        let ctx = LowerCtx {
            slot_regs: &slot_regs,
            ref_regs: &empty_refs,
            prop_regs: &empty_props,
            entity_map: &self.entity_map,
            state_names: &sn,
            state_names_by_id: &self.state_names_by_id,
            locals: &locals,
            func_ids: &self.func_ids,
            field_dims: &self.field_dims,
            namespace: &self.namespace,
            params: &self.param_names,
            current_entity: entity,
        };
        let active_reg = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::ReadView,
            active_reg,
            Some(crate::eir::ValueType::F64),
            vec![],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::active_id(),
                0,
            )),
        ));
        let w = lower_expr(&self.when, &ctx, &mut next_id, out);
        let zero = const_reg(0.0, &mut next_id, out);
        let one = const_reg(1.0, &mut next_id, out);
        let is_active = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Ne,
            is_active,
            Some(crate::eir::ValueType::Bool),
            vec![active_reg, zero],
            None,
            None,
        ));
        let active_f = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Select,
            active_f,
            Some(crate::eir::ValueType::F64),
            vec![is_active, one, zero],
            None,
            None,
        ));
        let w_true = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Ne,
            w_true,
            Some(crate::eir::ValueType::Bool),
            vec![w, zero],
            None,
            None,
        ));
        // Mul needs numeric operands: materialize the `when` truth as 0.0/1.0.
        let w_num = next_id;
        next_id += 1;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Select,
            w_num,
            Some(crate::eir::ValueType::F64),
            vec![w_true, one, zero],
            None,
            None,
        ));
        let both = binary(crate::eir::Opcode::Mul, active_f, w_num, &mut next_id, out);
        let value = next_id;
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Select,
            value,
            Some(crate::eir::ValueType::F64),
            vec![both, zero, active_f],
            None,
            None,
        ));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::WriteView,
            0,
            None,
            vec![value],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::active_id(),
                0,
            )),
        ));
        ret(out);
    }
}

pub struct DiffuseSystem {
    pub field: String,
    pub rate: f64,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// The single entity whose function carries the sweep.
    pub run_on: u128,
}
impl EirSystem for DiffuseSystem {
    fn name(&self) -> &'static str {
        "physics.diffuse"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if entity != self.run_on {
            ret(out);
            return;
        }
        // RFC-0037: one bulk opcode runs the whole grid sweep natively.
        let target = crate::physics_eir::cr(
            0,
            crate::physics_eir::field_component_id(&self.field),
            self.width,
        );
        let mut next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let rate = const_reg(self.rate, &mut next, out);
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::FieldDiffuse,
            0,
            None,
            vec![rate],
            None,
            Some(target),
        ));
        ret(out);
    }
}

/// A grid-field **Poisson/Laplace relaxation** solver
/// (`poisson { field = phi; source = rho?; iters = n; scale = s }`): each step
/// runs `n` Gauss–Seidel sweeps of `∇²φ = ρ·scale` over the field's interior
/// cells; boundary cells are held (fixed potentials). With no `source`, solves
/// the Laplace equation.
pub struct PoissonSystem {
    pub field: String,
    pub source: Option<String>,
    pub iters: u32,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub dx: f64,
    pub run_on: u128,
}
impl EirSystem for PoissonSystem {
    fn name(&self) -> &'static str {
        "physics.poisson"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if entity != self.run_on {
            ret(out);
            return;
        }
        // RFC-0037: `iters` Gauss-Seidel sweeps run natively in one opcode; the
        // runtime derives `div` and `dx^2 * scale` from the field and `scale`.
        let target = crate::physics_eir::cr(
            0,
            crate::physics_eir::field_component_id(&self.field),
            self.width,
        );
        let mut next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let limbs = self
            .source
            .as_ref()
            .map(|src| crate::eir::component_limbs(crate::physics_eir::field_component_id(src)))
            .unwrap_or([0u32; 4]);
        let mut operands = Vec::with_capacity(6);
        for limb in limbs {
            operands.push(const_u64_reg(limb as u64, &mut next, out));
        }
        operands.push(const_u64_reg(self.iters as u64, &mut next, out));
        operands.push(const_reg(self.scale, &mut next, out));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::FieldPoisson,
            0,
            None,
            operands,
            None,
            Some(target),
        ));
        ret(out);
    }
}

/// A grid-field **wave equation** solver
/// (`wave { field = u; prev = u_prev; velocity = c; dt = h }`): a second-order
/// leapfrog `u_tt = c²∇²u`, integrated as
/// `u(t+h) = 2u(t) − u(t−h) + (c·h/dx)²·∇²u(t)`. Reads every cell and its
/// Laplacian from one snapshot (Jacobi), then shifts `prev ← u` and
/// `u ← u(t+h)`. Stability: the Courant number `c·h/dx ≤ 1/√2` in 2D.
pub struct WaveSystem {
    pub field: String,
    pub prev: String,
    pub velocity: f64,
    pub dt: f64,
    /// Per-step amplitude retention on the temporal term (`1.0` = lossless).
    pub damping: f64,
    /// Sponge absorption at the boundary (`0.0` = reflecting, `1.0` = fully
    /// damped at the edge) over `absorb_width` cells.
    pub absorb: f64,
    pub absorb_width: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub dx: f64,
    pub run_on: u128,
}
impl EirSystem for WaveSystem {
    fn name(&self) -> &'static str {
        "physics.wave"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if entity != self.run_on {
            ret(out);
            return;
        }
        // RFC-0037: one bulk opcode runs the leapfrog step and the `prev <- u`
        // shift natively. The `prev` field id rides as four little-endian `u32`
        // limbs; `cfl` is the squared Courant number.
        let target = crate::physics_eir::cr(
            0,
            crate::physics_eir::field_component_id(&self.field),
            self.width,
        );
        let mut next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let pl = crate::eir::component_limbs(crate::physics_eir::field_component_id(&self.prev));
        let mut operands = Vec::with_capacity(8);
        for limb in pl {
            operands.push(const_u64_reg(limb as u64, &mut next, out));
        }
        let cfl = self.velocity * self.dt / self.dx;
        operands.push(const_reg(cfl * cfl, &mut next, out));
        operands.push(const_reg(self.damping, &mut next, out));
        operands.push(const_reg(self.absorb, &mut next, out));
        operands.push(const_reg(self.absorb_width as f64, &mut next, out));
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::FieldWave,
            0,
            None,
            operands,
            None,
            Some(target),
        ));
        ret(out);
    }
}

/// A Go-like channel send/recv system in the language. A channel is an entity
/// (declared `chan <name> { value = v }`) holding its latest value in `state[0]`.
///
/// * `send`: each dynamic entity evaluates `value` and writes it to the channel
///   entity (cross-entity `WriteView`) — the last sender wins.
/// * `recv`: each dynamic entity reads the channel's value into its own `slot`.
///
/// Because both directions are plain cross-entity component reads/writes, they
/// are EIR instructions shared by the interpreter and the JIT byte-for-byte.
pub struct ChanSystem {
    pub op: ChanOp,
    /// The channel entity id (`state[0]` holds the value).
    pub channel_entity: u128,
    /// For `send`: the expression to publish.
    pub value: Option<Expr>,
    /// For `recv`: the local state slot to receive into.
    pub slot: usize,
    pub slots: usize,
    pub entity_map: std::collections::BTreeMap<String, u128>,
    pub func_ids: std::collections::BTreeMap<String, u64>,
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    pub namespace: String,
    pub param_names: std::collections::BTreeSet<String>,
    pub state_names: std::collections::BTreeMap<String, usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChanOp {
    Send,
    Recv,
}

impl EirSystem for ChanSystem {
    fn name(&self) -> &'static str {
        "physics.chan"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        // Read all self state slots into registers (for a send value expr).
        let mut slot_regs: Vec<u32> = Vec::with_capacity(self.slots);
        for i in 0..self.slots {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadView,
                next,
                Some(crate::eir::ValueType::F64),
                vec![],
                None,
                Some(crate::physics_eir::cr(
                    entity,
                    crate::physics_eir::state_id(),
                    crate::physics_eir::field::state_slot(i),
                )),
            ));
            slot_regs.push(next);
        }
        let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        match self.op {
            ChanOp::Send => {
                // value -> channel.state[0]
                let expr = self.value.as_ref().unwrap();
                let mut refs: std::collections::BTreeSet<(String, usize)> = Default::default();
                let mut props = std::collections::BTreeSet::new();
                let mut named = std::collections::BTreeSet::new();
                collect_refs(
                    expr,
                    &mut refs,
                    &mut props,
                    &mut named,
                    &self.entity_map,
                    &std::collections::BTreeMap::new(),
                );
                let mut ref_ids: Vec<(u128, usize)> = refs
                    .iter()
                    .filter_map(|(n, s)| self.entity_map.get(n).map(|id| (*id, *s)))
                    .collect();
                ref_ids.sort_unstable();
                let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> =
                    Default::default();
                for (rid, rslot) in ref_ids {
                    let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::ReadView,
                        next,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        Some(crate::physics_eir::cr(
                            rid,
                            crate::physics_eir::state_id(),
                            crate::physics_eir::field::state_slot(rslot),
                        )),
                    ));
                    ref_regs.insert((rid, rslot), next);
                }
                let ctx = LowerCtx {
                    slot_regs: &slot_regs,
                    ref_regs: &ref_regs,
                    prop_regs: &std::collections::BTreeMap::new(),
                    entity_map: &self.entity_map,
                    state_names: &self.state_names,
                    state_names_by_id: &std::collections::BTreeMap::new(),
                    locals: &std::collections::BTreeMap::new(),
                    func_ids: &self.func_ids,
                    namespace: &self.namespace,
                    params: &self.param_names,
                    field_dims: &self.field_dims,
                    current_entity: entity,
                };
                let value_reg = lower_expr(expr, &ctx, &mut next_id, out);
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteView,
                    0,
                    None,
                    vec![value_reg],
                    None,
                    Some(crate::physics_eir::cr(
                        self.channel_entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(0),
                    )),
                ));
            }
            ChanOp::Recv => {
                // channel.state[0] -> self.state[slot]
                let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::ReadView,
                    next,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    None,
                    Some(crate::physics_eir::cr(
                        self.channel_entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(0),
                    )),
                ));
                let val = next;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteView,
                    0,
                    None,
                    vec![val],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(self.slot),
                    )),
                ));
            }
        }
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Return,
            0,
            None,
            vec![],
            None,
            None,
        ));
    }
}

/// A first-class multi-body interaction system for micro particles and macro
/// celestial bodies.
///
/// Every dynamic body carries `state = (px, py, pz, vx, vy, vz, m)` — position,
/// velocity, and mass (or charge). `nbody { G; dt }` computes the mutual
/// inverse-square force between every pair:
///
/// ```text
///   a_i = Σ_{j≠i}  G·m_j·(p_j − p_i) / |p_j − p_i|³
/// ```
///
/// The sign of `G` selects gravity (`G > 0`, attractive — planets) or Coulomb
/// repulsion/attraction (`G < 0` or mixed signs — charged micro particles). All
/// forces are computed in EIR (interpreter == JIT), with a small epsilon guard
/// against coincident-particle blowup.
pub struct NbodySystem {
    pub bodies: Vec<u128>,
    pub g: f64,
    pub dt: f64,
}

impl EirSystem for NbodySystem {
    fn name(&self) -> &'static str {
        "physics.nbody"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        // A body excluded from nbody (e.g. a Moon) gets a no-op function.
        if !self.bodies.contains(&entity) {
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
            return;
        }
        let eps = 1e-12;
        let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;

        // Self state: px,py,pz,vx,vy,vz,m = slots 0..6.
        let px = nb_read(out, &mut next_id, entity, 0);
        let py = nb_read(out, &mut next_id, entity, 1);
        let pz = nb_read(out, &mut next_id, entity, 2);
        let vx = nb_read(out, &mut next_id, entity, 3);
        let vy = nb_read(out, &mut next_id, entity, 4);
        let vz = nb_read(out, &mut next_id, entity, 5);

        // ax = ay = az = 0
        let mut ax = nb_const(out, &mut next_id, 0.0);
        let mut ay = nb_const(out, &mut next_id, 0.0);
        let mut az = nb_const(out, &mut next_id, 0.0);

        for &j in &self.bodies {
            if j == entity {
                continue;
            }
            let jx = nb_read(out, &mut next_id, j, 0);
            let jy = nb_read(out, &mut next_id, j, 1);
            let jz = nb_read(out, &mut next_id, j, 2);
            let jm = nb_read(out, &mut next_id, j, 6);
            let dx = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, jx, px);
            let dy = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, jy, py);
            let dz = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, jz, pz);
            let dx2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dx, dx);
            let dy2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dy, dy);
            let dz2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dz, dz);
            let sxy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, dx2, dy2);
            let r2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, sxy, dz2);
            let epsc = nb_const(out, &mut next_id, eps);
            let r2eps = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, r2, epsc);
            let r = nb_un(crate::eir::Opcode::Sqrt, out, &mut next_id, r2eps);
            let r3 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, r2eps, r);
            let gc = nb_const(out, &mut next_id, self.g);
            let gm = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, gc, jm);
            let scale = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, gm, r3);
            let tx = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, scale, dx);
            let ty = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, scale, dy);
            let tz = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, scale, dz);
            ax = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, ax, tx);
            ay = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, ay, ty);
            az = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, az, tz);
        }

        // v += a*dt ; p += v*dt
        let dtc = nb_const(out, &mut next_id, self.dt);
        let dax = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, ax, dtc);
        let day = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, ay, dtc);
        let daz = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, az, dtc);
        let nvx = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vx, dax);
        let nvy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vy, day);
        let nvz = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vz, daz);
        let dtc2 = nb_const(out, &mut next_id, self.dt);
        let dxp = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, nvx, dtc2);
        let dyp = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, nvy, dtc2);
        let dzp = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, nvz, dtc2);
        let npx = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, px, dxp);
        let npy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, py, dyp);
        let npz = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, pz, dzp);
        nb_write(out, entity, 3, nvx);
        nb_write(out, entity, 4, nvy);
        nb_write(out, entity, 5, nvz);
        nb_write(out, entity, 0, npx);
        nb_write(out, entity, 1, npy);
        nb_write(out, entity, 2, npz);

        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::Return,
            0,
            None,
            vec![],
            None,
            None,
        ));
    }
}

fn nb_arith(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    op: crate::eir::Opcode,
    a: u32,
    b: u32,
) -> u32 {
    let r = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        op,
        r,
        Some(crate::eir::ValueType::F64),
        vec![a, b],
        None,
        None,
    ));
    r
}

fn nb_un(
    op: crate::eir::Opcode,
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    a: u32,
) -> u32 {
    let r = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        op,
        r,
        Some(crate::eir::ValueType::F64),
        vec![a],
        None,
        None,
    ));
    r
}

fn nb_const(out: &mut Vec<crate::eir::Instruction>, next: &mut u32, v: f64) -> u32 {
    let r = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Const,
        r,
        Some(crate::eir::ValueType::F64),
        vec![],
        Some(crate::eir::Immediate::F64(v)),
        None,
    ));
    r
}

fn nb_read(out: &mut Vec<crate::eir::Instruction>, next: &mut u32, e: u128, slot: usize) -> u32 {
    let r = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::ReadView,
        r,
        Some(crate::eir::ValueType::F64),
        vec![],
        None,
        Some(crate::physics_eir::cr(
            e,
            crate::physics_eir::state_id(),
            crate::physics_eir::field::state_slot(slot),
        )),
    ));
    r
}

fn nb_write(out: &mut Vec<crate::eir::Instruction>, e: u128, slot: usize, v: u32) {
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::WriteView,
        0,
        None,
        vec![v],
        None,
        Some(crate::physics_eir::cr(
            e,
            crate::physics_eir::state_id(),
            crate::physics_eir::field::state_slot(slot),
        )),
    ));
}

/// Maps a cross-entity property to its (component id, byte offset).
fn prop_component(kind: PropKind) -> (pwe_api::ComponentTypeId, u32) {
    use crate::physics_eir::{field, rigid_body_id, transform_id, velocity_id};
    match kind {
        PropKind::Mass => (rigid_body_id(), field::MASS),
        PropKind::IsDynamic => (rigid_body_id(), field::IS_DYNAMIC),
        PropKind::PositionX => (transform_id(), field::POS_X),
        PropKind::PositionY => (transform_id(), field::POS_Y),
        PropKind::PositionZ => (transform_id(), field::POS_Z),
        PropKind::VelocityX => (velocity_id(), field::VEL_X),
        PropKind::VelocityY => (velocity_id(), field::VEL_Y),
        PropKind::VelocityZ => (velocity_id(), field::VEL_Z),
        PropKind::State(i) => (
            crate::physics_eir::state_id(),
            crate::physics_eir::field::state_slot(i),
        ),
    }
}

/// Collects every `@name.sN` cross-entity reference, cross-entity property
/// reference (`@name.mass`, `@name.position.x`, …), and named state reference
/// in an expression tree.
fn collect_refs(
    expr: &Expr,
    out: &mut std::collections::BTreeSet<(String, usize)>,
    props: &mut std::collections::BTreeSet<(String, PropKind)>,
    named_refs: &mut std::collections::BTreeSet<(String, usize)>,
    entity_map: &std::collections::BTreeMap<String, u128>,
    state_names_by_id: &std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
) {
    match expr {
        Expr::Ref(name, slot) => {
            out.insert((name.clone(), *slot));
        }
        Expr::PropRef(name, kind) => {
            props.insert((name.clone(), *kind));
        }
        Expr::Name(name) => {
            // Cross-entity named refs (`@name.x` / `@name.state.x`) need a
            // pre-read of that entity's named slot, resolved against ITS layout.
            if let Some((ent, rest)) = name.split_once('.') {
                let slot_name = rest.strip_prefix("state.").unwrap_or(rest);
                let id = entity_map.get(ent).copied();
                if let Some(id) = id {
                    if let Some(slot) = state_names_by_id.get(&id).and_then(|m| m.get(slot_name)) {
                        named_refs.insert((ent.to_string(), *slot));
                    }
                }
            }
        }
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Div(a, b)
        | Expr::Rem(a, b)
        | Expr::Cmp(_, a, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => {
            collect_refs(a, out, props, named_refs, entity_map, state_names_by_id);
            collect_refs(b, out, props, named_refs, entity_map, state_names_by_id);
        }
        Expr::Call(_, args) => {
            for a in args {
                collect_refs(a, out, props, named_refs, entity_map, state_names_by_id);
            }
        }
        Expr::Neg(x) | Expr::Not(x) => {
            collect_refs(x, out, props, named_refs, entity_map, state_names_by_id)
        }
        Expr::SlotDyn(idx) => {
            collect_refs(idx, out, props, named_refs, entity_map, state_names_by_id)
        }
        Expr::Const(_) | Expr::Slot(_) | Expr::Time => {}
    }
}

/// Emits a `Const` instruction and returns its result register.
fn const_reg(value: f64, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Const,
        r,
        Some(crate::eir::ValueType::F64),
        vec![],
        Some(crate::eir::Immediate::F64(value)),
        None,
    ));
    r
}

/// A `U64` constant register (bulk-opcode field ids / iteration counts).
fn const_u64_reg(value: u64, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Const,
        r,
        Some(crate::eir::ValueType::U64),
        vec![],
        Some(crate::eir::Immediate::U64(value)),
        None,
    ));
    r
}

/// Emits a single-operand f64 instruction, returning its register.
fn unary(
    op: crate::eir::Opcode,
    a: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        op,
        r,
        Some(crate::eir::ValueType::F64),
        vec![a],
        None,
        None,
    ));
    r
}

/// Emits a seeded random draw instruction, returning its register.
fn random_reg(next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Random,
        r,
        Some(crate::eir::ValueType::F64),
        vec![],
        None,
        None,
    ));
    r
}

/// Normalizes a value to a strict 1.0 / 0.0 boolean: `Ne(x, 0)` then
/// `Select(cond, 1.0, 0.0)`. Any nonzero input yields 1.0.
fn truthy(reg: u32, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let zero = const_reg(0.0, next_id, out);
    let bool_reg = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Ne,
        bool_reg,
        Some(crate::eir::ValueType::Bool),
        vec![reg, zero],
        None,
        None,
    ));
    select_bool(bool_reg, next_id, out)
}

/// Emits `Select(cond, 1.0, 0.0)`, turning a Bool register into an F64 1/0.
fn select_bool(cond: u32, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let one = const_reg(1.0, next_id, out);
    let zero = const_reg(0.0, next_id, out);
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Select,
        r,
        Some(crate::eir::ValueType::F64),
        vec![cond, one, zero],
        None,
        None,
    ));
    r
}

/// Collects references from a resolved let-tree: `let` bindings and the
/// conditions of `break`/`continue` all read state, so their cross-entity /
/// property refs need pre-reads too.
/// Parses an optional `substeps = n` parameter (integer in 1..=1000).
fn parse_substeps(
    params: &std::collections::BTreeMap<String, f64>,
    byte_offset: usize,
) -> Result<usize> {
    match params.get("substeps") {
        None => Ok(1),
        Some(&v) => {
            if v < 1.0 || v.fract() != 0.0 || v > 1000.0 {
                return Err(error_at(
                    Status::Invalid,
                    72,
                    byte_offset,
                    "substeps must be an integer in 1..=1000".to_string(),
                ));
            }
            Ok(v as usize)
        }
    }
}

/// Parses an optional `every = n` scheduling parameter (integer ≥ 1).
fn parse_every(
    params: &std::collections::BTreeMap<String, f64>,
    byte_offset: usize,
) -> Result<Option<u64>> {
    match params.get("every") {
        None => Ok(None),
        Some(&v) => {
            if v < 1.0 || v.fract() != 0.0 || v > 1_000_000.0 {
                return Err(error_at(
                    Status::Invalid,
                    71,
                    byte_offset,
                    "every must be an integer ≥ 1".to_string(),
                ));
            }
            Ok(Some(v as u64))
        }
    }
}

/// Walks an expression tree recording the highest own state slot it reads
/// directly (`sN`) or by bare name, resolved against the entity's layout.
/// Cross-entity references and properties do not touch own slots.
fn expr_slot_span(
    expr: &Expr,
    sn: &std::collections::BTreeMap<String, usize>,
    max: &mut usize,
    any: &mut bool,
) {
    match expr {
        Expr::Const(_) | Expr::Time => {}
        Expr::Slot(i) => {
            *max = (*max).max(*i);
            *any = true;
        }
        Expr::SlotDyn(idx) => expr_slot_span(idx, sn, max, any),
        Expr::Name(n) if !n.contains('.') => {
            if let Some(&s) = sn.get(n.as_str()) {
                *max = (*max).max(s);
                *any = true;
            }
        }
        Expr::Name(_) | Expr::Ref(..) | Expr::PropRef(..) => {}
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Div(a, b)
        | Expr::Rem(a, b)
        | Expr::Cmp(_, a, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => {
            expr_slot_span(a, sn, max, any);
            expr_slot_span(b, sn, max, any);
        }
        Expr::Not(a) | Expr::Neg(a) => expr_slot_span(a, sn, max, any),
        Expr::Call(_, args) => {
            for a in args {
                expr_slot_span(a, sn, max, any);
            }
        }
    }
}

/// Walks a statement tree recording the highest own state slot its
/// expressions read (`sN` or a bare name), resolved against the entity's
/// layout. Mirrors `expr_slot_span` for `let` statement trees.
fn let_stmts_slot_span(
    stmts: &[LetStmt],
    sn: &std::collections::BTreeMap<String, usize>,
    max: &mut usize,
    any: &mut bool,
) {
    for s in stmts {
        match s {
            LetStmt::Let(_, e) => expr_slot_span(e, sn, max, any),
            LetStmt::Break(c) | LetStmt::Continue(c) => {
                if let Some(e) = c {
                    expr_slot_span(e, sn, max, any);
                }
            }
            LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => {
                let_stmts_slot_span(body, sn, max, any);
            }
        }
    }
}

fn collect_let_refs(
    stmts: &[LetStmt],
    out: &mut std::collections::BTreeSet<(String, usize)>,
    props: &mut std::collections::BTreeSet<(String, PropKind)>,
    named_refs: &mut std::collections::BTreeSet<(String, usize)>,
    entity_map: &std::collections::BTreeMap<String, u128>,
    state_names_by_id: &std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
) {
    for s in stmts {
        match s {
            LetStmt::Let(_, e) => {
                collect_refs(e, out, props, named_refs, entity_map, state_names_by_id);
            }
            LetStmt::Break(c) | LetStmt::Continue(c) => {
                if let Some(e) = c {
                    collect_refs(e, out, props, named_refs, entity_map, state_names_by_id);
                }
            }
            LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => {
                collect_let_refs(body, out, props, named_refs, entity_map, state_names_by_id);
            }
        }
    }
}

/// Captures the immutable lowering context pieces so `LowerCtx` can be
/// rebuilt per `let` binding (the locals map mutates between bindings).
struct LowerParts<'a> {
    slot_regs: &'a [u32],
    ref_regs: &'a std::collections::BTreeMap<(u128, usize), u32>,
    prop_regs: &'a std::collections::BTreeMap<(u128, PropKind), u32>,
    entity_map: &'a std::collections::BTreeMap<String, u128>,
    state_names: &'a std::collections::BTreeMap<String, usize>,
    state_names_by_id:
        &'a std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    func_ids: &'a std::collections::BTreeMap<String, u64>,
    field_dims: &'a std::collections::BTreeMap<String, (u32, u32)>,
    namespace: &'a str,
    params: &'a std::collections::BTreeSet<String>,
    current_entity: u128,
}

impl<'a> LowerParts<'a> {
    fn ctx<'l>(&self, locals: &'l std::collections::BTreeMap<String, u32>) -> LowerCtx<'l>
    where
        'a: 'l,
    {
        LowerCtx {
            slot_regs: self.slot_regs,
            ref_regs: self.ref_regs,
            prop_regs: self.prop_regs,
            entity_map: self.entity_map,
            state_names: self.state_names,
            state_names_by_id: self.state_names_by_id,
            locals,
            func_ids: self.func_ids,
            field_dims: self.field_dims,
            namespace: self.namespace,
            params: self.params,
            current_entity: self.current_entity,
        }
    }
}

/// `a or b` over F64 0/1 gate registers: `a + b*(1-a)`.
fn or_gate(
    a: Option<u32>,
    b: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    match a {
        None => b,
        Some(x) => {
            let one = const_reg(1.0, next_id, out);
            let nx = binary(crate::eir::Opcode::Sub, one, x, next_id, out);
            let add = binary(crate::eir::Opcode::Mul, b, nx, next_id, out);
            binary(crate::eir::Opcode::Add, x, add, next_id, out)
        }
    }
}

/// Whether a statement tree contains `break`/`continue` anywhere. Loops
/// without control flow lower to plain unrolled bindings (no gate
/// instructions); loops with control flow get run/skip predication.
fn has_control(stmts: &[LetStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LetStmt::Break(_) | LetStmt::Continue(_) => true,
        LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => has_control(body),
        LetStmt::Let(..) => false,
    })
}

/// Whether an expression contains a spatial query (`neighbor_count` /
/// `nearest_dist`). Queries need a per-entity lowering context, which
/// function bodies do not have.
fn expr_has_query(expr: &Expr) -> bool {
    match expr {
        Expr::Call(name, args) => {
            if matches!(
                *name,
                "neighbor_count"
                    | "nearest_dist"
                    | "neighbor_mean"
                    | "nearest_dx"
                    | "nearest_dy"
                    | "nearest_dz"
            ) {
                return true;
            }
            args.iter().any(expr_has_query)
        }
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Div(a, b)
        | Expr::Rem(a, b)
        | Expr::Cmp(_, a, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => expr_has_query(a) || expr_has_query(b),
        Expr::Not(a) | Expr::Neg(a) | Expr::SlotDyn(a) => expr_has_query(a),
        _ => false,
    }
}

/// Whether a statement tree contains a spatial query anywhere.
fn stmts_have_query(stmts: &[LetStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LetStmt::Let(_, e) => expr_has_query(e),
        LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => stmts_have_query(body),
        LetStmt::Break(g) | LetStmt::Continue(g) => g.as_ref().map(expr_has_query).unwrap_or(false),
    })
}

/// Binds a `for`-loop index to a constant value, predicated on the iteration's
/// run gate when the loop is gated (`new = prev + run*(lit - prev)`).
#[allow(clippy::too_many_arguments)]
fn bind_loop_index(
    name: &str,
    value: f64,
    run: Option<u32>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
    locals: &mut std::collections::BTreeMap<String, u32>,
    parts: &LowerParts<'_>,
) {
    let lit = const_reg(value, next_id, out);
    match run {
        None => {
            locals.insert(name.to_string(), lit);
        }
        Some(r) => {
            let ctx = parts.ctx(locals);
            let prev = lower_expr(&Expr::Name(name.to_string()), &ctx, next_id, out);
            let diff = binary(crate::eir::Opcode::Sub, lit, prev, next_id, out);
            let scaled = binary(crate::eir::Opcode::Mul, r, diff, next_id, out);
            let new = binary(crate::eir::Opcode::Add, prev, scaled, next_id, out);
            locals.insert(name.to_string(), new);
        }
    }
}

/// Lowers a resolved let-tree block sequentially.
///
/// `run` is the block's execution gate: `None` for the top level (lets bind
/// directly, identical to the pre-control-flow lowering) or `Some(reg)` inside
/// a loop whose body contains `break`/`continue` (reg holds 1.0/0.0 — whether
/// this iteration executes).
///
/// Guarded expressions are evaluated unconditionally and their results
/// discarded by the gate (`new = prev + gate*(computed - prev)`, gate ∈
/// {0,1}): IEEE f64 evaluation has no traps, so this is safe in straight-line
/// EIR without back-edges.
///
/// Returns the block's break register (`Some(1.0)` if a `break` fired within
/// it) so an enclosing loop can stop iterating. A `break` exits only the
/// innermost loop; it does not propagate to enclosing blocks.
#[allow(clippy::too_many_arguments)]
fn lower_let_block(
    stmts: &[LetStmt],
    run: Option<u32>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
    locals: &mut std::collections::BTreeMap<String, u32>,
    parts: &LowerParts<'_>,
) -> Option<u32> {
    let mut skip: Option<u32> = None; // block-local continue gate
    let mut broke: Option<u32> = None; // block-local break accumulator
    for stmt in stmts {
        match stmt {
            LetStmt::Let(name, expr) => {
                let ctx = parts.ctx(locals);
                let computed = lower_expr(expr, &ctx, next_id, out);
                match run {
                    None => {
                        locals.insert(name.clone(), computed);
                    }
                    Some(r) => {
                        // gate = run * (1 - skip)
                        let gate = match skip {
                            None => r,
                            Some(s) => {
                                let one = const_reg(1.0, next_id, out);
                                let ns = binary(crate::eir::Opcode::Sub, one, s, next_id, out);
                                binary(crate::eir::Opcode::Mul, r, ns, next_id, out)
                            }
                        };
                        // new = prev + gate*(computed - prev)
                        let prev = lower_expr(&Expr::Name(name.clone()), &ctx, next_id, out);
                        let diff = binary(crate::eir::Opcode::Sub, computed, prev, next_id, out);
                        let scaled = binary(crate::eir::Opcode::Mul, gate, diff, next_id, out);
                        let new = binary(crate::eir::Opcode::Add, prev, scaled, next_id, out);
                        locals.insert(name.clone(), new);
                    }
                }
            }
            LetStmt::Break(cond) | LetStmt::Continue(cond) => {
                let ctx = parts.ctx(locals);
                let c = match cond {
                    Some(e) => truthy(lower_expr(e, &ctx, next_id, out), next_id, out),
                    None => const_reg(1.0, next_id, out),
                };
                let one = const_reg(1.0, next_id, out);
                let r = run.unwrap_or_else(|| const_reg(1.0, next_id, out));
                // fired = run * cond * (1 - skip)
                let mut fired = binary(crate::eir::Opcode::Mul, r, c, next_id, out);
                if let Some(s) = skip {
                    let ns = binary(crate::eir::Opcode::Sub, one, s, next_id, out);
                    fired = binary(crate::eir::Opcode::Mul, fired, ns, next_id, out);
                }
                // A break also skips the rest of this iteration.
                skip = Some(or_gate(skip, fired, next_id, out));
                if matches!(stmt, LetStmt::Break(_)) {
                    broke = Some(or_gate(broke, fired, next_id, out));
                }
            }
            LetStmt::Repeat(n, body) => {
                let controlled = has_control(body);
                let context_gated = run.is_some() || skip.is_some();
                // Gate entering this loop: run * (1 - skip-so-far).
                let enter: Option<u32> = match (run, skip) {
                    (None, None) => None,
                    (Some(r), None) => Some(r),
                    (r, Some(s)) => {
                        let one = const_reg(1.0, next_id, out);
                        let ns = binary(crate::eir::Opcode::Sub, one, s, next_id, out);
                        Some(match r {
                            None => ns,
                            Some(rr) => binary(crate::eir::Opcode::Mul, rr, ns, next_id, out),
                        })
                    }
                };
                if !controlled {
                    // Plain inner loop: direct lets unless the enclosing
                    // context gates them.
                    let inner_run = if context_gated { enter } else { None };
                    for _ in 0..*n {
                        lower_let_block(body, inner_run, next_id, out, locals, parts);
                    }
                } else {
                    // Controlled inner loop: iteration run chains on breaks.
                    let mut run_prev = enter.unwrap_or_else(|| const_reg(1.0, next_id, out));
                    let mut broke_inner: Option<u32> = None;
                    for _ in 0..*n {
                        let run_k = match broke_inner {
                            None => run_prev,
                            Some(b) => {
                                let one = const_reg(1.0, next_id, out);
                                let nb = binary(crate::eir::Opcode::Sub, one, b, next_id, out);
                                binary(crate::eir::Opcode::Mul, run_prev, nb, next_id, out)
                            }
                        };
                        let b = lower_let_block(body, Some(run_k), next_id, out, locals, parts);
                        if let Some(bb) = b {
                            broke_inner = Some(or_gate(broke_inner, bb, next_id, out));
                        }
                        run_prev = run_k;
                    }
                }
            }
            LetStmt::For(name, lo, hi, body) => {
                let controlled = has_control(body);
                let context_gated = run.is_some() || skip.is_some();
                let enter: Option<u32> = match (run, skip) {
                    (None, None) => None,
                    (Some(r), None) => Some(r),
                    (r, Some(s)) => {
                        let one = const_reg(1.0, next_id, out);
                        let ns = binary(crate::eir::Opcode::Sub, one, s, next_id, out);
                        Some(match r {
                            None => ns,
                            Some(rr) => binary(crate::eir::Opcode::Mul, rr, ns, next_id, out),
                        })
                    }
                };
                if !controlled {
                    let inner_run = if context_gated { enter } else { None };
                    for k in (*lo as i64)..(*hi as i64) {
                        bind_loop_index(name, k as f64, inner_run, next_id, out, locals, parts);
                        lower_let_block(body, inner_run, next_id, out, locals, parts);
                    }
                } else {
                    let mut run_prev = enter.unwrap_or_else(|| const_reg(1.0, next_id, out));
                    let mut broke_inner: Option<u32> = None;
                    for k in (*lo as i64)..(*hi as i64) {
                        let run_k = match broke_inner {
                            None => run_prev,
                            Some(b) => {
                                let one = const_reg(1.0, next_id, out);
                                let nb = binary(crate::eir::Opcode::Sub, one, b, next_id, out);
                                binary(crate::eir::Opcode::Mul, run_prev, nb, next_id, out)
                            }
                        };
                        bind_loop_index(name, k as f64, Some(run_k), next_id, out, locals, parts);
                        let b = lower_let_block(body, Some(run_k), next_id, out, locals, parts);
                        if let Some(bb) = b {
                            broke_inner = Some(or_gate(broke_inner, bb, next_id, out));
                        }
                        run_prev = run_k;
                    }
                }
            }
        }
    }
    broke
}

fn binary(
    op: crate::eir::Opcode,
    a: u32,
    b: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        op,
        r,
        Some(crate::eir::ValueType::F64),
        vec![a, b],
        None,
        None,
    ));
    r
}

/// Builds the concrete `EirSystem` list from parsed system declarations.
#[allow(clippy::too_many_arguments)]
pub fn build_systems(
    systems: &[SystemDecl],
    entity_ids: &std::collections::BTreeMap<String, u128>,
    nbody_bodies: &[u128],
    func_ids: &std::collections::BTreeMap<String, u64>,
    state_names_by_id: &std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    field_dims: &std::collections::BTreeMap<String, (u32, u32)>,
    param_names: &std::collections::BTreeSet<String>,
    field_info: &std::collections::BTreeMap<String, (u32, u32, u32, f64)>,
    dynamic: &[u128],
) -> Result<Vec<Box<dyn EirSystem>>> {
    // ChanSystem uses the first entity's named-state layout for its send value.
    let state_names = state_names_by_id
        .values()
        .next()
        .cloned()
        .unwrap_or_default();
    fn param(
        params: &std::collections::BTreeMap<String, f64>,
        key: &str,
        sys_off: usize,
        sys_kind: &str,
    ) -> Result<f64> {
        params.get(key).copied().ok_or_else(|| {
            error_at(
                Status::Invalid,
                48,
                sys_off,
                format!("system '{sys_kind}' is missing required parameter '{key}'"),
            )
        })
    }
    let mut out: Vec<Box<dyn EirSystem>> = Vec::new();
    let mut invariant_count: usize = 0;
    for s in systems {
        match s.kind.as_str() {
            "gravity" => out.push(Box::new(GravitySystem {
                gravity_y: param(&s.params, "gravity_y", s.byte_offset, &s.kind)?,
                dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
            })),
            "integrate" => out.push(Box::new(IntegrateSystem {
                dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
            })),
            "damping" => out.push(Box::new(DampingSystem {
                factor: param(&s.params, "factor", s.byte_offset, &s.kind)?,
            })),
            "ground_contact" => out.push(Box::new(GroundContactSystem {
                restitution: param(&s.params, "restitution", s.byte_offset, &s.kind)?,
            })),
            "wall" => out.push(Box::new(WallSystem {
                x: param(&s.params, "x", s.byte_offset, &s.kind)?,
                z: param(&s.params, "z", s.byte_offset, &s.kind)?,
                y_min: s.params.get("y_min").copied().unwrap_or(f64::NEG_INFINITY),
                restitution: s.params.get("restitution").copied().unwrap_or(0.5),
            })),
            "force" => out.push(Box::new(ForceSystem {
                ax: param(&s.params, "ax", s.byte_offset, &s.kind)?,
                ay: param(&s.params, "ay", s.byte_offset, &s.kind)?,
                az: param(&s.params, "az", s.byte_offset, &s.kind)?,
                dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
            })),
            "linear" => {
                let slots = param(&s.params, "slots", s.byte_offset, &s.kind)? as usize;
                if slots == 0 || slots > crate::components::State::MAX_STATE_SLOTS {
                    return Err(error(Status::Invalid, 52));
                }
                let mut rows: Vec<Vec<f64>> = Vec::with_capacity(slots);
                for i in 0..slots {
                    let row = s
                        .vec_params
                        .get(&format!("row{i}"))
                        .cloned()
                        .ok_or(error(Status::Invalid, 53))?;
                    if row.len() != slots + 1 {
                        return Err(error(Status::Invalid, 54));
                    }
                    rows.push(row);
                }
                out.push(Box::new(LinearSystem {
                    rows,
                    slots,
                    dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
                }));
            }
            "nbody" => {
                if nbody_bodies.is_empty() {
                    return Err(error(Status::Invalid, 63));
                }
                out.push(Box::new(NbodySystem {
                    bodies: nbody_bodies.to_vec(),
                    g: param(&s.params, "G", s.byte_offset, &s.kind)?,
                    dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
                }));
            }
            "send" | "recv" => {
                let op = if s.kind == "send" {
                    ChanOp::Send
                } else {
                    ChanOp::Recv
                };
                let chan_name = s
                    .string_params
                    .get("chan")
                    .cloned()
                    .ok_or(error(Status::Invalid, 62))?;
                let channel_entity = entity_ids
                    .get(&chan_name)
                    .copied()
                    .ok_or(error(Status::Invalid, 62))?;
                let mut chan = ChanSystem {
                    op,
                    channel_entity,
                    value: None,
                    slot: 0,
                    slots: crate::components::State::MAX_STATE_SLOTS,
                    entity_map: entity_ids.clone(),
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names: state_names.clone(),
                };
                match op {
                    ChanOp::Send => {
                        let value = if let Some(t) = s.update.get("value") {
                            parse_expr_str(t)?
                        } else {
                            Expr::Const(param(&s.params, "value", s.byte_offset, &s.kind)?)
                        };
                        chan.value = Some(value);
                    }
                    ChanOp::Recv => {
                        chan.slot = param(&s.params, "slot", s.byte_offset, &s.kind)? as usize;
                        if chan.slot >= crate::components::State::MAX_STATE_SLOTS {
                            return Err(error(Status::Invalid, 52));
                        }
                    }
                }
                out.push(Box::new(chan));
            }
            "update" => {
                let mut rules = Vec::new();
                for (key, text) in &s.update {
                    // Keep the raw LHS (`sN` or a named slot); it is resolved to a
                    // slot index per-entity during lowering.
                    // Only an all-digit `sN` suffix is a numeric slot token; any
                    // other `s...` name is a named slot (resolved in lowering).
                    let idx: usize = match key.strip_prefix('s') {
                        Some(digits)
                            if !digits.is_empty()
                                && !key.contains('[')
                                && digits.bytes().all(|b| b.is_ascii_digit()) =>
                        {
                            digits.parse().map_err(|_| error(Status::Invalid, 55))?
                        }
                        _ => continue, // named slot or dynamic LHS; handled below
                    };
                    if idx >= crate::components::State::MAX_STATE_SLOTS {
                        return Err(error(Status::Invalid, 52));
                    }
                    let expr = parse_expr_str(text)?;
                    rules.push((key.clone(), expr));
                }
                // Named-slot LHS rules (not `sN`) are kept raw for
                // per-entity resolution; dynamic LHS rules (`s[i]`) get their
                // index expressions parsed now.
                let mut dyn_rules = Vec::new();
                let numeric_slot = |key: &str| {
                    key.strip_prefix('s')
                        .map(|d| {
                            !d.is_empty()
                                && !d.contains('[')
                                && d.bytes().all(|b| b.is_ascii_digit())
                        })
                        .unwrap_or(false)
                };
                for (key, text) in &s.update {
                    if key.starts_with('s') && key.contains('[') {
                        let inner = key
                            .trim_start_matches('s')
                            .trim_start_matches('[')
                            .trim_end_matches(']');
                        let idx = parse_expr_str(inner)?;
                        let expr = parse_expr_str(text)?;
                        dyn_rules.push((idx, expr));
                    } else if !numeric_slot(key) {
                        let expr = parse_expr_str(text)?;
                        rules.push((key.clone(), expr));
                    }
                }
                if rules.is_empty() && dyn_rules.is_empty() {
                    return Err(error_at(
                        Status::Invalid,
                        55,
                        s.byte_offset,
                        "update system has no slot rules (a pure-number RHS such as `s0 = 1.0` \
becomes a scalar parameter — write `s0 = 0.0 + 1.0` instead)"
                            .to_string(),
                    ));
                }
                // `let name = expr` local bindings, in order.
                let lets = to_let_stmts(&s.update_stmts)?;
                // Optional `on = <name>` restricts the rule to one entity.
                let only = match s.string_params.get("on") {
                    Some(name) => {
                        Some(resolve_on(entity_ids, name).ok_or(error(Status::Invalid, 62))?)
                    }
                    None => None,
                };
                let every = parse_every(&s.params, s.byte_offset)?;
                let when = match s.string_params.get("when") {
                    Some(text) => Some(parse_expr_str(text)?),
                    None => None,
                };
                out.push(Box::new(UpdateSystem {
                    rules,
                    dyn_rules,
                    lets,
                    slots_hint: 0,
                    dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
                    every,
                    when,
                    substeps: parse_substeps(&s.params, s.byte_offset)?,
                    entity_map: entity_ids.clone(),
                    only,
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                }));
            }
            "rk4" => {
                let mut rules = Vec::new();
                for (key, text) in &s.update {
                    if let Some(idx) = numeric_slot(key) {
                        if idx >= crate::components::State::MAX_STATE_SLOTS {
                            return Err(error(Status::Invalid, 52));
                        }
                    } else if key.starts_with('s') && key.contains('[') {
                        // RK4's working states are compile-time register
                        // chains; a runtime-index LHS cannot feed them.
                        return Err(error_at(
                            Status::Invalid,
                            73,
                            s.byte_offset,
                            "dynamic slot LHS is update-only (rk4 stages need compile-time slots)"
                                .to_string(),
                        ));
                    }
                    rules.push((key.clone(), parse_expr_str(text)?));
                }
                if rules.is_empty() {
                    return Err(error_at(
                        Status::Invalid,
                        55,
                        s.byte_offset,
                        "rk4 system has no slot rules (a pure-number RHS such as `s0 = 1.0` \
becomes a scalar parameter — write `s0 = 0.0 + 1.0` instead)"
                            .to_string(),
                    ));
                }
                let lets = to_let_stmts(&s.update_stmts)?;
                let only = match s.string_params.get("on") {
                    Some(name) => {
                        Some(resolve_on(entity_ids, name).ok_or(error(Status::Invalid, 62))?)
                    }
                    None => None,
                };
                let every = parse_every(&s.params, s.byte_offset)?;
                let when = match s.string_params.get("when") {
                    Some(text) => Some(parse_expr_str(text)?),
                    None => None,
                };
                out.push(Box::new(Rk4System {
                    rules,
                    lets,
                    slots_hint: 0,
                    dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
                    every,
                    when,
                    substeps: parse_substeps(&s.params, s.byte_offset)?,
                    entity_map: entity_ids.clone(),
                    only,
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                }));
            }
            "invariant" => {
                let expr_text = s.update.get("expr").cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "invariant is missing required parameter 'expr'",
                    )
                })?;
                let expr = parse_expr_str(&expr_text)?;
                let lets = to_let_stmts(&s.update_stmts)?;
                let only = match s.string_params.get("on") {
                    Some(name) => {
                        Some(resolve_on(entity_ids, name).ok_or(error(Status::Invalid, 62))?)
                    }
                    None => None,
                };
                // Each invariant owns one 8-byte verdict field in the hidden
                // check component, in source order.
                let check_offset =
                    (invariant_count as u32) * crate::physics_eir::field::STATE_SLOT_BYTES;
                invariant_count += 1;
                out.push(Box::new(InvariantSystem {
                    expr,
                    lets,
                    check_offset,
                    entity_map: entity_ids.clone(),
                    only,
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                }));
            }
            "watch" => {
                let expr_text = s.update.get("expr").cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "watch is missing required parameter 'expr'".to_string(),
                    )
                })?;
                let expr = parse_expr_str(&expr_text)?;
                let mem = param(&s.params, "mem", s.byte_offset, &s.kind)? as usize;
                let into = param(&s.params, "into", s.byte_offset, &s.kind)? as usize;
                if mem >= crate::components::State::MAX_STATE_SLOTS
                    || into >= crate::components::State::MAX_STATE_SLOTS
                {
                    return Err(error(Status::Invalid, 52));
                }
                let only = match s.string_params.get("on") {
                    Some(name) => {
                        Some(resolve_on(entity_ids, name).ok_or(error(Status::Invalid, 62))?)
                    }
                    None => None,
                };
                out.push(Box::new(WatchSystem {
                    expr,
                    mem,
                    into,
                    entity_map: entity_ids.clone(),
                    only,
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                }));
            }
            "diffuse" => {
                let field = s
                    .string_params
                    .get("field")
                    .cloned()
                    .ok_or(error(Status::Invalid, 62))?;
                let (w, h, d, _) = *field_info.get(&field).ok_or(error(Status::Invalid, 62))?;
                out.push(Box::new(DiffuseSystem {
                    field,
                    rate: param(&s.params, "rate", s.byte_offset, &s.kind)?,
                    width: w,
                    height: h,
                    depth: d,
                    run_on: dynamic.first().copied().unwrap_or(0),
                }));
            }
            "poisson" => {
                let field = s
                    .string_params
                    .get("field")
                    .cloned()
                    .ok_or(error(Status::Invalid, 62))?;
                let (w, h, d, dx) = *field_info.get(&field).ok_or(error(Status::Invalid, 62))?;
                let source = s.string_params.get("source").cloned();
                if let Some(src) = &source {
                    if !field_info.contains_key(src) {
                        return Err(error(Status::Invalid, 62));
                    }
                }
                out.push(Box::new(PoissonSystem {
                    field,
                    source,
                    iters: param(&s.params, "iters", s.byte_offset, &s.kind)? as u32,
                    scale: s.params.get("scale").copied().unwrap_or(1.0),
                    width: w,
                    height: h,
                    depth: d,
                    dx,
                    run_on: dynamic.first().copied().unwrap_or(0),
                }));
            }
            "wave" => {
                let field = s
                    .string_params
                    .get("field")
                    .cloned()
                    .ok_or(error(Status::Invalid, 62))?;
                let prev = s
                    .string_params
                    .get("prev")
                    .cloned()
                    .ok_or(error(Status::Invalid, 62))?;
                let (w, h, d, dx) = *field_info.get(&field).ok_or(error(Status::Invalid, 62))?;
                if !field_info.contains_key(&prev) {
                    return Err(error(Status::Invalid, 62));
                }
                out.push(Box::new(WaveSystem {
                    field,
                    prev,
                    velocity: param(&s.params, "velocity", s.byte_offset, &s.kind)?,
                    dt: param(&s.params, "dt", s.byte_offset, &s.kind)?,
                    // Optional per-step retention (1.0 = lossless); a small
                    // damping keeps reflected waves in a closed box legible.
                    damping: s.params.get("damping").copied().unwrap_or(1.0),
                    // Optional sponge layer: outgoing waves are absorbed near
                    // the boundary instead of reflecting off it.
                    absorb: s.params.get("absorb").copied().unwrap_or(0.0),
                    absorb_width: s.params.get("absorb_width").copied().unwrap_or(0.0) as u32,
                    width: w,
                    height: h,
                    depth: d,
                    dx,
                    run_on: dynamic.first().copied().unwrap_or(0),
                }));
            }
            // RFC-0038: `spawn { on = <caller>; pool = <name> }` — activate one
            // free slot per caller/step and copy the caller's state into it.
            "spawn" => {
                let caller_name = s.string_params.get("on").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "spawn requires `on = <entity>`".to_string(),
                    )
                })?;
                let caller = entity_ids
                    .get(caller_name)
                    .copied()
                    .ok_or(error(Status::Invalid, 62))?;
                let pool = s.string_params.get("pool").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "spawn requires `pool = <name>`".to_string(),
                    )
                })?;
                let base = entity_ids
                    .get(&format!("{pool}#0"))
                    .copied()
                    .ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            78,
                            s.byte_offset,
                            format!("spawn references unknown pool '{pool}'"),
                        )
                    })?;
                let mut count = 0u32;
                while entity_ids.contains_key(&format!("{pool}#{count}")) {
                    count += 1;
                }
                let per_step = match s.params.get("count").copied() {
                    Some(v) if (1.0..=1024.0).contains(&v) && v.fract() == 0.0 => v as u32,
                    Some(_) => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "spawn `count` must be an integer in 1..=1024".to_string(),
                        ))
                    }
                    None => 1,
                };
                let every = parse_every(&s.params, s.byte_offset)?;
                let phase = match s.params.get("phase").copied() {
                    Some(v) if v.fract() == 0.0 && v >= 0.0 => v as u64,
                    Some(_) => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "spawn `phase` must be a non-negative integer".to_string(),
                        ))
                    }
                    None => 0,
                };
                out.push(Box::new(SpawnSystem {
                    on: caller,
                    base,
                    count,
                    limbs: crate::eir::u128_limbs(base),
                    per_step,
                    every,
                    phase,
                }));
            }
            // RFC-0038: `despawn { on = <pool>; when = <expr> }` — deactivate
            // every matching active slot.
            "despawn" => {
                let pool = s.string_params.get("on").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "despawn requires `on = <pool>`".to_string(),
                    )
                })?;
                let mut slots = Vec::new();
                let mut k = 0u32;
                while let Some(&id) = entity_ids.get(&format!("{pool}#{k}")) {
                    slots.push(id);
                    k += 1;
                }
                if slots.is_empty() {
                    return Err(error_at(
                        Status::Invalid,
                        78,
                        s.byte_offset,
                        format!("despawn references unknown pool '{pool}'"),
                    ));
                }
                let when = match s.string_params.get("when") {
                    Some(text) => parse_expr_str(text)?,
                    None => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "despawn requires `when = <expr>`".to_string(),
                        ))
                    }
                };
                out.push(Box::new(DespawnSystem {
                    pool_slots: slots,
                    when,
                    entity_map: entity_ids.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                }));
            }
            _ => return Err(error(Status::Invalid, 49)),
        }
    }
    Ok(out)
}

/// The dynamic physics bodies a system program acts on: entities that are not
/// explicitly static and are not cameras.
/// RFC-0038: resolve an `on = <name>` target to its entity set — a single
/// declared entity, or every slot of a pool (`<name>#0`, `<name>#1`, …).
fn resolve_on(
    entity_ids: &std::collections::BTreeMap<String, u128>,
    name: &str,
) -> Option<std::collections::BTreeSet<u128>> {
    if let Some(&id) = entity_ids.get(name) {
        return Some(std::collections::BTreeSet::from([id]));
    }
    if entity_ids.contains_key(&format!("{name}#0")) {
        let mut set = std::collections::BTreeSet::new();
        let mut k = 0u32;
        while let Some(&id) = entity_ids.get(&format!("{name}#{k}")) {
            set.insert(id);
            k += 1;
        }
        return Some(set);
    }
    None
}

fn dynamic_entity_ids(model: &WorldModel) -> Vec<u128> {
    model
        .entities
        .iter()
        .enumerate()
        .filter(|(_, d)| d.dynamic != Some(false) && d.camera != Some(true))
        .map(|(index, _)| (index as u128) + 1)
        .collect()
}

/// A compiled program: the parsed model plus the low-level EIR module.
pub struct CompiledProgram {
    pub parsed: ParsedProgram,
    pub program: PhysicsProgram,
    pub eir: EirModule,
}

/// Compiles PWE source end-to-end: parse → build systems → lower to EIR.
/// A Python-style import directive:
/// `import "pkg/mod"`, `import "pkg/mod" as m`, or `from "pkg/mod" import a, b`.
#[derive(Clone, Debug)]
struct ImportDirective {
    path: String,
    alias: Option<String>,
    /// `Some(names)` for `from … import …` (bind these names unqualified).
    names: Option<Vec<String>>,
}

/// One loaded module: its namespace, path, import-stripped source, parsed
/// program, and its own import directives (with resolved child paths).
struct ModuleInfo {
    /// The namespace used by this module's own rules (its first alias).
    ns: String,
    /// Every namespace this module has been imported under.
    aliases: Vec<String>,
    path: std::path::PathBuf,
    source: String,
    parsed: ParsedProgram,
    imports: Vec<(ImportDirective, std::path::PathBuf)>,
}

fn ident_tokens(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let t = part.trim();
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        out.push(t.to_string());
    }
    Some(out)
}

/// Parses a Python-style import directive at the start of a line, returning it
/// and the remainder of the line (preserved so `world { import "x" }` keeps
/// its brace).
fn parse_import_line(line: &str) -> Option<(ImportDirective, String)> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("import") {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        let path = rest[..end].to_string();
        let mut tail = rest[end + 1..].to_string();
        let trimmed = tail.trim_start();
        let alias = if let Some(a) = trimmed.strip_prefix("as") {
            let a = a.trim_start();
            let name: String = a
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                return None;
            }
            tail = a[name.len()..].to_string();
            Some(name)
        } else {
            None
        };
        return Some((
            ImportDirective {
                path,
                alias,
                names: None,
            },
            tail,
        ));
    }
    if let Some(rest) = t.strip_prefix("from") {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        let path = rest[..end].to_string();
        let rest = rest[end + 1..].trim_start();
        let rest = rest.strip_prefix("import")?;
        let rest = rest.trim_start();
        // The name list runs to end of line (trailing `;`, `}`, or comment).
        let list_end = rest.find([';', '}', '#']).unwrap_or(rest.len());
        let names = ident_tokens(&rest[..list_end])?;
        let tail = rest[list_end..].to_string();
        return Some((
            ImportDirective {
                path,
                alias: None,
                names: Some(names),
            },
            tail,
        ));
    }
    None
}

/// Resolves an import spec against a directory: `pkg/mod` -> `pkg/mod.pwe`;
/// a directory `pkg` -> `pkg/__init__.pwe` (a package).
fn resolve_module_path(dir: &std::path::Path, spec: &str) -> std::path::PathBuf {
    let joined = dir.join(spec);
    if joined.is_dir() {
        joined.join("__init__.pwe")
    } else if joined.extension().is_some() {
        joined
    } else {
        joined.with_extension("pwe")
    }
}

/// The default namespace of an import spec: its last path segment.
fn module_stem(spec: &str) -> String {
    let last = spec.rsplit('/').next().unwrap_or(spec);
    if last == "__init__" {
        spec.rsplit('/').nth(1).unwrap_or(spec).to_string()
    } else {
        last.trim_end_matches(".pwe").to_string()
    }
}

/// Recursively loads a module and its imports (deduplicated by canonical path;
/// a module keeps the namespace of the first import that reached it).
fn collect_module(
    path: &std::path::Path,
    ns: &str,
    seen: &mut std::collections::BTreeMap<std::path::PathBuf, usize>,
    out: &mut Vec<ModuleInfo>,
) -> Result<()> {
    let canon = path.canonicalize().map_err(|e| {
        error_at(
            Status::Invalid,
            76,
            0,
            format!("cannot import {}: {e}", path.display()),
        )
    })?;
    // A module already loaded (e.g. a cycle, or another alias) only gains an
    // alias; its definitions are merged once and reachable by every alias.
    if let Some(&idx) = seen.get(&canon) {
        if !out[idx].aliases.contains(&ns.to_string()) {
            out[idx].aliases.push(ns.to_string());
        }
        return Ok(());
    }
    seen.insert(canon, out.len());
    let raw = std::fs::read_to_string(path).map_err(|e| {
        error_at(
            Status::Invalid,
            76,
            0,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut stripped = String::new();
    let mut imports = Vec::new();
    for line in raw.lines() {
        match parse_import_line(line) {
            Some((d, tail)) => {
                let child = resolve_module_path(&dir, &d.path);
                imports.push((d, child));
                stripped.push_str(&tail);
                stripped.push('\n');
            }
            None => {
                stripped.push_str(line);
                stripped.push('\n');
            }
        }
    }
    let parsed = parse(&stripped)?;
    let children: Vec<(ImportDirective, std::path::PathBuf)> = imports.clone();
    out.push(ModuleInfo {
        ns: ns.to_string(),
        aliases: vec![ns.to_string()],
        path: path.to_path_buf(),
        source: stripped,
        parsed,
        imports,
    });
    for (d, child) in children {
        let default_ns = d.alias.clone().unwrap_or_else(|| module_stem(&d.path));
        collect_module(&child, &default_ns, seen, out)?;
    }
    Ok(())
}

/// A program's sources: the root source plus every imported module (namespace,
/// display path, import-stripped source) and the resolved `from`-import aliases.
/// Stored in the compiled artifact so it stays self-contained.
#[derive(Clone, Debug, Default)]
pub struct ProgramSources {
    pub root: String,
    /// Per module: namespace, display path, source, and every alias.
    pub modules: Vec<(String, String, String, Vec<String>)>,
    /// `from … import …` bindings: (bare name, qualified name).
    pub aliases: Vec<(String, String)>,
}

/// Parses each stored source and merges them (in-memory; no filesystem).
pub fn merge_sources(src: &ProgramSources) -> Result<ParsedProgram> {
    let mut modules = Vec::new();
    let root_parsed = parse(&src.root)?;
    modules.push(ModuleInfo {
        ns: String::new(),
        aliases: vec![String::new()],
        path: std::path::PathBuf::from("<root>"),
        source: src.root.clone(),
        parsed: root_parsed,
        imports: Vec::new(),
    });
    for (ns, path, source, aliases) in &src.modules {
        let parsed = parse(source)?;
        modules.push(ModuleInfo {
            ns: ns.clone(),
            aliases: aliases.clone(),
            path: std::path::PathBuf::from(path),
            source: source.clone(),
            parsed,
            imports: Vec::new(),
        });
    }
    merge_modules(modules, src.aliases.clone())
}

/// Loads a program file and its module imports, returning both the merged
/// program and the sources needed to rebuild it without the filesystem.
pub fn load_program_sources(path: &std::path::Path) -> Result<(ParsedProgram, ProgramSources)> {
    let mut modules = Vec::new();
    let mut seen: std::collections::BTreeMap<std::path::PathBuf, usize> = Default::default();
    collect_module(path, "", &mut seen, &mut modules)?;
    // Resolve `from … import …` alias requests against the child's namespace.
    let mut aliases: Vec<(String, String)> = Vec::new();
    for m in &modules {
        for (d, child) in &m.imports {
            if let Some(names) = &d.names {
                let child_ns = child
                    .canonicalize()
                    .ok()
                    .and_then(|c| seen.get(&c).copied())
                    .map(|i| modules[i].ns.clone())
                    .unwrap_or_else(|| module_stem(&d.path));
                for n in names {
                    aliases.push((n.clone(), format!("{child_ns}.{n}")));
                }
            }
        }
    }
    let root = modules
        .first()
        .map(|m| m.source.clone())
        .unwrap_or_default();
    let exported: Vec<(String, String, String, Vec<String>)> = modules
        .iter()
        .skip(1)
        .map(|m| {
            (
                m.ns.clone(),
                m.path.display().to_string(),
                m.source.clone(),
                m.aliases.clone(),
            )
        })
        .collect();
    let sources = ProgramSources {
        root,
        modules: exported,
        aliases: aliases.clone(),
    };
    let parsed = merge_modules(modules, aliases)?;
    Ok((parsed, sources))
}

/// Loads a program file and its module imports into one merged program.
pub fn load_program(path: &std::path::Path) -> Result<ParsedProgram> {
    Ok(load_program_sources(path)?.0)
}

/// Compiles a program file with its module imports resolved.
pub fn compile_file(path: &std::path::Path) -> Result<CompiledProgram> {
    let (parsed, _) = load_program_sources(path)?;
    compile_program(parsed)
}

/// Merges loaded modules into one program. Entities, channels, fields, and
/// systems are world content and merge flatly (duplicate names are an error);
/// functions and parameters are namespaced (`module::name`), with `from`
/// imports additionally bound bare.
fn merge_modules(
    modules: Vec<ModuleInfo>,
    aliases: Vec<(String, String)>,
) -> Result<ParsedProgram> {
    use std::collections::BTreeMap;
    let mut model = crate::dsl::WorldModel::default();
    let mut systems: Vec<SystemDecl> = Vec::new();
    let mut funcs: Vec<FuncDecl> = Vec::new();
    let mut entity_seen: BTreeMap<String, String> = Default::default();
    let mut field_seen: BTreeMap<String, String> = Default::default();
    let mut channel_seen: BTreeMap<String, String> = Default::default();
    let mut gravity_set = false;
    for m in &modules {
        let q = |n: &str| -> String {
            if m.ns.is_empty() {
                n.to_string()
            } else {
                format!("{}.{}", m.ns, n)
            }
        };
        let who = m.path.display().to_string();
        if !gravity_set {
            model.gravity = m.parsed.model.gravity;
            gravity_set = true;
        }
        if model.title.is_none() {
            model.title = m.parsed.model.title.clone();
        }
        for e in &m.parsed.model.entities {
            if let Some(prev) = entity_seen.get(&e.name) {
                return Err(error_at(
                    Status::Invalid,
                    76,
                    0,
                    format!("entity `{}` defined in both `{prev}` and `{who}`", e.name),
                ));
            }
            entity_seen.insert(e.name.clone(), who.clone());
            model.entities.push(e.clone());
        }
        // RFC-0038: pools merge like entities, with a duplicate-name check.
        for p in &m.parsed.model.pools {
            if entity_seen.contains_key(&p.name) {
                return Err(error_at(
                    Status::Invalid,
                    76,
                    0,
                    format!("pool `{}` defined twice", p.name),
                ));
            }
            entity_seen.insert(p.name.clone(), who.clone());
            model.pools.push(p.clone());
        }
        // User-defined custom shapes merge by name (later definitions win).
        for (name, parts) in &m.parsed.model.shapes {
            model.shapes.insert(name.clone(), parts.clone());
        }
        for c in &m.parsed.model.channels {
            if channel_seen.contains_key(&c.name) {
                return Err(error_at(
                    Status::Invalid,
                    76,
                    0,
                    format!("channel `{}` defined twice", c.name),
                ));
            }
            channel_seen.insert(c.name.clone(), who.clone());
            model.channels.push(c.clone());
        }
        for f in &m.parsed.model.fields {
            if field_seen.contains_key(&f.name) {
                return Err(error_at(
                    Status::Invalid,
                    76,
                    0,
                    format!("field `{}` defined twice", f.name),
                ));
            }
            field_seen.insert(f.name.clone(), who.clone());
            model.fields.push(f.clone());
        }
        // Parameters: one canonical key (the module's primary namespace) plus
        // an alias entry per other namespace, so `--param` can find every form.
        let canonical = |n: &str| q(n);
        for (k, v) in &m.parsed.model.params {
            let canon = canonical(k);
            for a in &m.aliases {
                let key = if a.is_empty() {
                    k.clone()
                } else {
                    format!("{a}.{k}")
                };
                model.params.entry(key.clone()).or_insert(*v);
                model.param_alias.insert(key, canon.clone());
            }
            model.params.entry(canon.clone()).or_insert(*v);
            model.param_alias.insert(canon.clone(), canon.clone());
        }
        for (k, v) in &m.parsed.model.param_units {
            for a in &m.aliases {
                let key = if a.is_empty() {
                    k.clone()
                } else {
                    format!("{a}.{k}")
                };
                model.param_units.insert(key, *v);
            }
            model.param_units.insert(canonical(k), *v);
        }
        // Functions: one declaration per alias (its body is small and the
        // duplication keeps every alias callable).
        for fd in &m.parsed.funcs {
            for a in &m.aliases {
                let mut fd = fd.clone();
                fd.name = if a.is_empty() {
                    fd.name.clone()
                } else {
                    format!("{a}.{}", fd.name)
                };
                fd.namespace = a.clone();
                funcs.push(fd);
            }
            // The canonical namespace may already be among the aliases.
            let canon = canonical(&fd.name);
            if !funcs.iter().any(|f| f.name == canon) {
                let mut fd = fd.clone();
                fd.namespace = m.ns.clone();
                fd.name = canon;
                funcs.push(fd);
            }
        }
        for sys in &m.parsed.systems {
            let mut sys = sys.clone();
            sys.namespace = m.ns.clone();
            systems.push(sys);
        }
    }
    for (bare, qualified) in aliases {
        // A function import binds the bare name to a copy of the function.
        if !funcs.iter().any(|f| f.name == bare) {
            if let Some(src) = funcs.iter().find(|f| f.name == qualified).cloned() {
                let mut fd = src;
                fd.name = bare.clone();
                funcs.push(fd);
            }
        }
        // A parameter import binds the bare name (alias of the canonical key).
        if let Some(canon) = model.param_alias.get(&qualified).cloned() {
            if let Some(v) = model.params.get(&qualified).copied() {
                model.params.insert(bare.clone(), v);
                model.param_alias.insert(bare, canon);
            }
        }
    }
    let mut dup = BTreeMap::new();
    for f in &funcs {
        if dup.insert(f.name.clone(), ()).is_some() {
            return Err(error_at(
                Status::Invalid,
                76,
                0,
                format!("function `{}` defined twice", f.name),
            ));
        }
    }
    Ok(ParsedProgram {
        model,
        systems,
        funcs,
    })
}

/// Dimensional-analysis environment: declared slot/parameter units and the
/// inferred units of `let` locals.
struct DimEnv<'a> {
    slot_dims: &'a [crate::units::MaybeDim],
    name_to_slot: &'a std::collections::BTreeMap<String, usize>,
    params: &'a std::collections::BTreeMap<String, crate::units::Dim>,
    locals: std::collections::BTreeMap<String, crate::units::MaybeDim>,
}

impl DimEnv<'_> {
    fn of_expr(&self, expr: &Expr) -> Result<crate::units::MaybeDim> {
        use crate::units::{div, mul, unify, Dim, MaybeDim};
        let err = || error(Status::Invalid, 77);
        Ok(match expr {
            Expr::Const(_) => None,
            Expr::Time => Some(Dim::seconds()),
            Expr::Slot(i) => self.slot_dims.get(*i).copied().flatten(),
            Expr::SlotDyn(_) => None,
            Expr::Name(n) => {
                if let Some(d) = self.locals.get(n.as_str()) {
                    *d
                } else if let Some(slot) = self.name_to_slot.get(n.as_str()) {
                    self.slot_dims.get(*slot).copied().flatten()
                } else {
                    self.params.get(n.as_str()).copied()
                }
            }
            Expr::Ref(..) | Expr::PropRef(..) => None,
            Expr::Neg(a) => self.of_expr(a)?,
            Expr::Add(a, b) | Expr::Sub(a, b) => {
                unify(self.of_expr(a)?, self.of_expr(b)?).map_err(|_| err())?
            }
            Expr::Mul(a, b) => mul(self.of_expr(a)?, self.of_expr(b)?),
            Expr::Div(a, b) => div(self.of_expr(a)?, self.of_expr(b)?),
            // Remainder / comparison require compatible operands.
            Expr::Rem(a, b) => unify(self.of_expr(a)?, self.of_expr(b)?).map_err(|_| err())?,
            Expr::Cmp(_, a, b) => {
                unify(self.of_expr(a)?, self.of_expr(b)?).map_err(|_| err())?;
                None
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                unify(self.of_expr(a)?, Some(Dim::ZERO)).map_err(|_| err())?;
                unify(self.of_expr(b)?, Some(Dim::ZERO)).map_err(|_| err())?;
                None
            }
            Expr::Not(a) => {
                unify(self.of_expr(a)?, Some(Dim::ZERO)).map_err(|_| err())?;
                None
            }
            Expr::Call(name, args) => {
                let arg = |i: usize| -> Result<MaybeDim> {
                    args.get(i).map(|e| self.of_expr(e)).unwrap_or(Ok(None))
                };
                let dimensionless = |d: MaybeDim| -> Result<()> {
                    unify(d, Some(Dim::ZERO)).map_err(|_| err()).map(|_| ())
                };
                match *name {
                    // Transcendental functions require a dimensionless argument.
                    "sin" | "cos" | "exp" | "ln" | "log10" | "log2" | "sinh" | "cosh" | "tanh"
                    | "asin" | "acos" | "atan" => {
                        dimensionless(arg(0)?)?;
                        None
                    }
                    // Dim-preserving unary functions.
                    "abs" | "floor" | "ceil" | "round" | "sign" => arg(0)?,
                    "sqrt" => match arg(0)? {
                        Some(d) => Some(d.sqrt().ok_or_else(err)?),
                        None => None,
                    },
                    "pow" => {
                        dimensionless(arg(1)?)?;
                        None
                    }
                    "hypot" => unify(arg(0)?, arg(1)?).map_err(|_| err())?,
                    "min" | "max" => unify(arg(0)?, arg(1)?).map_err(|_| err())?,
                    "if" => {
                        dimensionless(arg(0)?)?;
                        unify(arg(1)?, arg(2)?).map_err(|_| err())?
                    }
                    "print" => arg(0)?,
                    _ => None,
                }
            }
        })
    }

    fn of_lets(&mut self, stmts: &[LetStmt]) -> Result<()> {
        for s in stmts {
            match s {
                LetStmt::Let(name, e) => {
                    let d = self.of_expr(e)?;
                    self.locals.insert(name.clone(), d);
                }
                LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => self.of_lets(body)?,
                LetStmt::Break(g) | LetStmt::Continue(g) => {
                    if let Some(e) = g {
                        self.of_expr(e)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Checks `update`/`rk4` rules against declared units. Graded: a value with no
/// declared unit is a wildcard and never errors, so unit-free models (and
/// unannotated parts of annotated models) pass. Assumes `dt` is in seconds.
fn check_dimensions(parsed: &ParsedProgram) -> Result<()> {
    use crate::units::{unify, MaybeDim};
    // Merged slot dimensions by index (declarations across entities agree in
    // practice; the first non-None wins) and the named layout of the first
    // entity that declares names.
    let mut slot_dims: Vec<MaybeDim> = Vec::new();
    let mut name_to_slot: std::collections::BTreeMap<String, usize> = Default::default();
    let mut any_units = false;
    for e in &parsed.model.entities {
        if let Some(names) = &e.state_names {
            for (slot, n) in names.iter().enumerate() {
                if let Some(n) = n {
                    name_to_slot.entry(n.clone()).or_insert(slot);
                }
            }
        }
        if let Some(units) = &e.state_units {
            if units.len() > slot_dims.len() {
                slot_dims.resize(units.len(), None);
            }
            for (slot, u) in units.iter().enumerate() {
                if u.is_some() {
                    any_units = true;
                    if slot_dims[slot].is_none() {
                        slot_dims[slot] = *u;
                    }
                }
            }
        }
    }
    if !parsed.model.param_units.is_empty() {
        any_units = true;
    }
    if !any_units {
        return Ok(());
    }
    for sys in &parsed.systems {
        // Check every declared expression for internal consistency: rule bodies
        // and `let`s (update/rk4), the `when` gate, `invariant`/`watch` exprs.
        {
            let env = DimEnv {
                slot_dims: &slot_dims,
                name_to_slot: &name_to_slot,
                params: &parsed.model.param_units,
                locals: Default::default(),
            };
            if let Some(w) = sys.string_params.get("when") {
                if let Ok(e) = parse_expr_str(w) {
                    let d = env.of_expr(&e)?;
                    if unify(d, Some(crate::units::Dim::ZERO)).is_err() {
                        return Err(error_at(
                            Status::Invalid,
                            77,
                            sys.byte_offset,
                            "`when` gate must be dimensionless".to_string(),
                        ));
                    }
                }
            }
            if matches!(sys.kind.as_str(), "invariant" | "watch") {
                if let Some(text) = sys.update.get("expr") {
                    if let Ok(e) = parse_expr_str(text) {
                        env.of_expr(&e)?;
                    }
                }
            }
        }
        if sys.kind != "update" && sys.kind != "rk4" {
            continue;
        }
        // `dt` is the step's time scale; use its declared unit, else seconds.
        let dt_dim = sys
            .param_units
            .get("dt")
            .copied()
            .unwrap_or_else(crate::units::Dim::seconds);
        let lets = to_let_stmts(&sys.update_stmts)?;
        // Rule LHS -> slot index (sN or a named slot).
        let mut rules: Vec<(usize, &str)> = Vec::new();
        for (key, text) in &sys.update {
            let idx = if key.starts_with('s') && key.contains('[') {
                continue;
            } else if let Some(i) = numeric_slot(key) {
                i
            } else {
                match name_to_slot.get(key) {
                    Some(i) => *i,
                    None => continue,
                }
            };
            rules.push((idx, text.as_str()));
        }
        for (idx, text) in rules {
            let lhs = slot_dims.get(idx).copied().flatten();
            let expr = match parse_expr_str(text) {
                Ok(e) => e,
                Err(_) => continue, // rule parse errors surface elsewhere
            };
            let mut env = DimEnv {
                slot_dims: &slot_dims,
                name_to_slot: &name_to_slot,
                params: &parsed.model.param_units,
                locals: Default::default(),
            };
            env.of_lets(&lets)?;
            let rhs = env.of_expr(&expr)?;
            // `slot += dt · expr` with dt in seconds.
            let scaled: MaybeDim = rhs.map(|d| d.times(dt_dim));
            if unify(lhs, scaled).is_err() {
                let lhs_name = match lhs {
                    Some(d) => d.name(),
                    None => "1".to_string(),
                };
                let rhs_name = match scaled {
                    Some(d) => d.name(),
                    None => "1".to_string(),
                };
                return Err(error_at(
                    Status::Invalid,
                    77,
                    sys.byte_offset,
                    format!(
                        "rule `{text}` is not dimensionally consistent: `{lhs_name}` expected, \
right-hand side (`dt·expr`) has `{rhs_name}`"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Compiles PWE source end-to-end: parse → build systems → lower to EIR.
pub fn compile(source: &str) -> Result<CompiledProgram> {
    compile_program(parse(source)?)
}

/// Compiles an already-parsed (and merged) program to EIR.
pub fn compile_program(parsed: ParsedProgram) -> Result<CompiledProgram> {
    check_dimensions(&parsed)?;
    // Entity name -> scene id (ids are 1-based model order, matching build_scene).
    let mut entity_ids: std::collections::BTreeMap<String, u128> = parsed
        .model
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.clone(), (i as u128) + 1))
        .collect();
    // Channels follow the bodies (matching build_scene's id assignment).
    let body_count = parsed.model.entities.len() as u128;
    for (i, c) in parsed.model.channels.iter().enumerate() {
        entity_ids.insert(c.name.clone(), body_count + (i as u128) + 1);
    }
    // RFC-0038: pool slots follow the channels; `<name>#<k>` names each slot.
    let pools = parsed.model.pool_ranges();
    for (name, base, count) in &pools {
        for k in 0..*count {
            entity_ids.insert(format!("{name}#{k}"), base + k as u128);
        }
    }
    let mut entities = dynamic_entity_ids(&parsed.model);
    for (_, base, count) in &pools {
        for k in 0..*count {
            entities.push(base + k as u128);
        }
    }
    // Bodies eligible for mutual `nbody`: dynamic bodies not explicitly
    // excluded (`nbody = false`, e.g. a Moon driven by a targeted update rule).
    let nbody_entities: Vec<u128> = entities
        .iter()
        .copied()
        .filter(|&id| {
            // RFC-0038: pool slots are never mutual-n-body bodies.
            id <= body_count
                && parsed
                    .model
                    .entities
                    .get((id - 1) as usize)
                    .map(|e| e.nbody != Some(false))
                    .unwrap_or(true)
        })
        .collect();
    // User-defined functions get reserved high EIR ids (no collision with the
    // per-system/per-entity function ids that start at 1).
    let mut func_ids: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for (i, f) in parsed.funcs.iter().enumerate() {
        func_ids.insert(f.name.clone(), 0xF000_0000 + i as u64);
    }
    // Named state slots: each entity's `state = (x = 0, …)` layout, keyed by
    // entity id (ids are 1-based model order, matching build_scene).
    let mut state_names_by_id: std::collections::BTreeMap<
        u128,
        std::collections::BTreeMap<String, usize>,
    > = std::collections::BTreeMap::new();
    for (i, e) in parsed.model.entities.iter().enumerate() {
        if let Some(names) = &e.state_names {
            let mut map = std::collections::BTreeMap::new();
            for (slot, n) in names.iter().enumerate() {
                if let Some(n) = n {
                    map.insert(n.clone(), slot);
                }
            }
            if !map.is_empty() {
                state_names_by_id.insert((i as u128) + 1, map);
            }
        }
    }
    // RFC-0038: pool slots share their pool's named-state layout.
    {
        let mut next = body_count + parsed.model.channels.len() as u128 + 1;
        for p in &parsed.model.pools {
            if let Some(names) = &p.decl.state_names {
                let mut map = std::collections::BTreeMap::new();
                for (slot, n) in names.iter().enumerate() {
                    if let Some(n) = n {
                        map.insert(n.clone(), slot);
                    }
                }
                if !map.is_empty() {
                    for k in 0..p.count {
                        state_names_by_id.insert(next + k as u128, map.clone());
                    }
                }
            }
            next += p.count as u128;
        }
    }
    // Grid field dimensions (name -> (width, height, dx)) for the field solvers.
    let field_info: std::collections::BTreeMap<String, (u32, u32, u32, f64)> = parsed
        .model
        .fields
        .iter()
        .map(|f| {
            (
                f.name.clone(),
                (f.width as u32, f.height as u32, f.depth as u32, f.dx),
            )
        })
        .collect();
    // Every declared parameter name (qualified), for namespace fallback.
    let param_names: std::collections::BTreeSet<String> =
        parsed.model.params.keys().cloned().collect();
    // Grid field dimensions (compile-time, from the model) for field cell
    // access: name -> (width, height). Height splits the packed 3D `j` index.
    let field_dims: std::collections::BTreeMap<String, (u32, u32)> = parsed
        .model
        .fields
        .iter()
        .map(|f| (f.name.clone(), (f.width as u32, f.height as u32)))
        .collect();
    let systems = build_systems(
        &parsed.systems,
        &entity_ids,
        &nbody_entities,
        &func_ids,
        &state_names_by_id,
        &field_dims,
        &param_names,
        &field_info,
        &entities,
    )?;
    let pool_slots: std::collections::BTreeSet<u128> = pools
        .iter()
        .flat_map(|(_, base, count)| (0..*count).map(move |k| *base + k as u128))
        .collect();
    let program = PhysicsProgram::build_with_guards(systems, entities, &pool_slots);
    let mut module = program.module.clone();
    // Append user-defined functions as EIR CALL targets (reserved high ids).
    for (i, f) in parsed.funcs.iter().enumerate() {
        let id = 0xF000_0000 + i as u64;
        let arg_count = f.params.len() as u32;
        let slot_regs: Vec<u32> = (0..arg_count).map(|p| p + 1).collect();
        let empty_refs = std::collections::BTreeMap::new();
        let empty_props = std::collections::BTreeMap::new();
        let empty_ents = std::collections::BTreeMap::new();
        let empty_names = std::collections::BTreeMap::new();
        let empty_dims = std::collections::BTreeMap::new();
        let empty_by_id: std::collections::BTreeMap<
            u128,
            std::collections::BTreeMap<String, usize>,
        > = std::collections::BTreeMap::new();
        let parts = LowerParts {
            slot_regs: &slot_regs,
            ref_regs: &empty_refs,
            prop_regs: &empty_props,
            entity_map: &empty_ents,
            state_names: &empty_names,
            state_names_by_id: &empty_by_id,
            func_ids: &func_ids,
            field_dims: &empty_dims,
            namespace: &f.namespace,
            params: &param_names,
            current_entity: 0,
        };
        let mut next_id = arg_count + 1;
        let mut instrs = Vec::new();
        // Statements before the return: lets and bounded loops, sequentially.
        // Parameter names are bound as locals so bodies can read `a`/`b`
        // directly as well as `s0`/`s1`.
        let mut locals: std::collections::BTreeMap<String, u32> = f
            .params
            .iter()
            .cloned()
            .zip(slot_regs.iter().copied())
            .collect();
        let lets = to_let_stmts(&f.stmts)?;
        // Spatial queries need a per-entity lowering context, which function
        // bodies do not have; reject them at compile time.
        if expr_has_query(&f.body) || stmts_have_query(&lets) {
            return Err(error(Status::Invalid, 70));
        }
        lower_let_block(&lets, None, &mut next_id, &mut instrs, &mut locals, &parts);
        let ctx = parts.ctx(&locals);
        let ret_reg = lower_expr(&f.body, &ctx, &mut next_id, &mut instrs);
        instrs.push(crate::physics_eir::instr(
            crate::eir::Opcode::Return,
            0,
            None,
            vec![ret_reg],
            None,
            None,
        ));
        module.functions.push(Function {
            id,
            effect_mask: 0,
            argument_count: arg_count,
            instructions: instrs,
        });
    }
    // Fold RANDOM/TIME/IO/ATOMIC/EMIT_EVENT effect bits from the emitted opcodes.
    module.apply_required_effects();
    module.validate(false)?;
    let eir = module;
    Ok(CompiledProgram {
        parsed,
        program,
        eir,
    })
}

// ---------------------------------------------------------------------------
// Cross-backend runtime
// ---------------------------------------------------------------------------

/// Runs a compiled program's low-level IR across the interpreter and the CPU
/// JIT, verifying they produce byte-identical writes (the "run cross" contract).
pub struct LangRuntime {
    pub scene: Scene,
    pub program: PhysicsProgram,
    pub module: EirModule,
    pub clock: u64,
    /// Seconds advanced per step (the `update` system's dt, else 1/60).
    pub sim_dt: f64,
    /// This runtime's region id (for cross-runtime channel routing).
    pub region: pwe_api::RegionId,
    jit: CpuJit,
    jit_key: CodeCacheKey,
    /// Cross-runtime channel router backing the language `chan`s.
    router: crate::channel::ChannelRouter,
    /// The scene entity ids that are channels (their `state[0]` is the value).
    channel_ids: Vec<u128>,
    /// Entity id -> name, for presentation labels.
    entity_names: std::collections::BTreeMap<u128, String>,
    /// Field names that are solver-internal (`wave`'s `prev` time-shift buffer)
    /// and are not presented as physical fields.
    hidden_fields: std::collections::BTreeSet<String>,
    /// Optional peer region: when set, `send` also routes to the peer's channel.
    peer_region: Option<pwe_api::RegionId>,
    /// Execution context for `time`/`random`/`emit` (seeded → reproducible).
    env: crate::eir::ExecEnv,
    /// Raw invariant expressions in source order; verdict field `i` of the
    /// hidden check component belongs to `invariant_exprs[i]` (diagnostics).
    invariant_exprs: Vec<String>,
}

impl LangRuntime {
    /// Compiles source and boots a runtime with an executable scene.
    pub fn compile(source: &str) -> Result<Self> {
        let compiled = compile(source)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Compiles a program file with its `import` fragments resolved.
    pub fn compile_file(path: &std::path::Path) -> Result<Self> {
        let compiled = compile_file(path)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Compiles source into a runtime belonging to `region` (for cross-runtime
    /// channel routing).
    pub fn compile_in_region(source: &str, region: RegionId) -> Result<Self> {
        let compiled = compile(source)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, region)
    }

    /// Boots a runtime from an already-compiled program and a scene.
    pub fn from_compiled(compiled: CompiledProgram, scene: Scene) -> Result<Self> {
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Boots a runtime from a compiled program, scene, and owning region.
    pub fn from_compiled_region(
        compiled: CompiledProgram,
        scene: Scene,
        region: RegionId,
    ) -> Result<Self> {
        let module = compiled.eir.clone();
        let program = compiled.program;
        // The simulation clock advances by the `update` system's dt so `t`
        // tracks integration time; fall back to 1/60.
        let sim_dt = compiled
            .parsed
            .systems
            .iter()
            .find(|s| s.kind == "update")
            .and_then(|s| s.params.get("dt").copied())
            .unwrap_or(1.0 / 60.0);

        // Prepare a CPU JIT over the same low-level IR.
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"pwe-lang");
        let code = jit.compile(&module, 0, profile, vec![JitAssumption::World(WorldId(0))])?;
        jit.publish(code)?;
        let jit_key = CodeCacheKey {
            module_hash: module.module_hash,
            target: 0,
            profile,
        };

        // Build the cross-runtime channel bridge: one router entry per language `chan`.
        let body_count = compiled.parsed.model.entities.len() as u128;
        let channel_ids: Vec<u128> = compiled
            .parsed
            .model
            .channels
            .iter()
            .enumerate()
            .map(|(i, _)| body_count + (i as u128) + 1)
            .collect();
        let mut entity_names: std::collections::BTreeMap<u128, String> = compiled
            .parsed
            .model
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| ((i as u128) + 1, e.name.clone()))
            .collect();
        for (i, c) in compiled.parsed.model.channels.iter().enumerate() {
            entity_names.insert(body_count + (i as u128) + 1, c.name.clone());
        }
        // RFC-0038: pool slots are named `<pool>#<k>`.
        for (name, base, count) in compiled.parsed.model.pool_ranges() {
            for k in 0..count {
                entity_names.insert(base + k as u128, format!("{name}#{k}"));
            }
        }
        let mut router = ChannelRouter::new(region);
        for &cid in &channel_ids {
            router.channel(ChannelAddr::new(region, ChannelId(cid as u64)), 1);
        }

        // `wave`'s `prev` field is an internal time-shift buffer, not a
        // physical quantity — keep it out of the 3D view.
        let hidden_fields: std::collections::BTreeSet<String> = compiled
            .parsed
            .systems
            .iter()
            .filter(|s| s.kind == "wave")
            .filter_map(|s| s.string_params.get("prev").cloned())
            .collect();
        Ok(Self {
            scene,
            program,
            module,
            clock: 0,
            sim_dt,
            hidden_fields,
            region,
            jit,
            jit_key,
            router,
            channel_ids,
            entity_names,
            peer_region: None,
            env: crate::eir::ExecEnv::default(),
            invariant_exprs: compiled
                .parsed
                .systems
                .iter()
                .filter(|s| s.kind == "invariant")
                .filter_map(|s| s.update.get("expr").cloned())
                .collect(),
        })
    }

    /// Configures the peer region; when set, `send` also routes the value to the
    /// peer's channel (which the peer can `ingest` and `recv`).
    pub fn set_peer(&mut self, peer: RegionId) {
        self.peer_region = Some(peer);
    }

    /// Events emitted by the language `emit(...)` during the most recent step,
    /// in deterministic order.
    pub fn emitted_events(&self) -> &[crate::eir::EmittedEvent] {
        &self.env.events
    }

    /// Values logged by the language `print(...)` during the most recent step,
    /// in execution order (a debugging side-channel).
    pub fn logs(&self) -> &[String] {
        &self.env.log
    }

    /// Drains and returns the values logged by `print(...)` since the last call.
    pub fn drain_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.env.log)
    }

    /// Pushes every channel's current value to the peer region's outbox (the
    /// cross-runtime / network transport).
    fn publish_channels(&mut self) {
        let Some(peer) = self.peer_region else {
            return;
        };
        for &cid in &self.channel_ids {
            let value = self
                .scene
                .get(EntityId(cid))
                .and_then(|e| e.state.as_ref())
                .map(|s| s.values[0])
                .unwrap_or(0.0);
            let _ = self.router.send(
                ChannelAddr::new(peer, ChannelId(cid as u64)),
                &value.to_bits().to_le_bytes(),
            );
        }
    }

    /// Serializes this runtime's outbound channel messages (the wire transport).
    pub fn transport_out(&self) -> Result<Vec<u8>> {
        self.router.encode_outbox()
    }

    /// Ingests a wire document from a peer, writing delivered messages into this
    /// runtime's channel entities so `recv` observes them.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<usize> {
        let delivered = self.router.ingest(bytes)?;
        for &cid in &self.channel_ids {
            let addr = ChannelAddr::new(self.region, ChannelId(cid as u64));
            if let Some(msg) = self.router.recv(addr)? {
                if msg.len() == 8 {
                    let value = f64::from_bits(u64::from_le_bytes(msg[..8].try_into().unwrap()));
                    if let Some(e) = self.scene.get_mut(EntityId(cid)) {
                        if let Some(st) = e.state.as_mut() {
                            st.values[0] = value;
                        }
                    }
                }
            }
        }
        Ok(delivered)
    }

    /// Exchanges channel messages with a peer runtime over the wire (both ways).
    pub fn exchange(&mut self, peer: &mut LangRuntime) -> Result<()> {
        let to_peer = self.router.encode_outbox()?;
        let to_self = peer.router.encode_outbox()?;
        self.ingest(&to_self)?;
        peer.ingest(&to_peer)?;
        Ok(())
    }

    /// A presentation frame for the 3D web viewer: bodies rendered as entities
    /// (position from transform or state), and declared channels reported as
    /// channel values.
    pub fn present_frame(
        &self,
        camera: Option<crate::present::CameraVisual>,
    ) -> crate::present::PresentationFrame {
        let mut frame = crate::present::snapshot_with(
            &self.entity_names,
            &self.channel_ids,
            &self.scene,
            camera,
        );
        if !self.hidden_fields.is_empty() {
            frame
                .fields
                .retain(|f| !self.hidden_fields.contains(&f.name));
        }
        frame
    }

    /// Resets the runtime to a previously captured scene (step 0, cleared
    /// execution context). Used by the live viewer's Restart.
    pub fn reset_to(&mut self, scene: Scene) {
        self.scene = scene;
        self.clock = 0;
        self.env = crate::eir::ExecEnv::default();
    }

    /// Advances the global simulation clock by one step.
    fn advance_clock(&mut self) {
        self.scene.sim_time += self.sim_dt;
        self.clock += 1;
        self.publish_channels();
    }

    /// Fails the step when any invariant verdict write is 1 — the invariant's
    /// expression evaluated to zero or NaN for some entity as the systems left
    /// the state. Verdict fields map back to their invariant expressions in
    /// source order for diagnostics.
    fn check_invariants(&self, writes: &[WorldWrite]) -> Result<()> {
        for w in writes {
            if w.component == crate::physics_eir::check_id() && f64::from_bits(w.value) >= 0.5 {
                let idx = (w.offset / crate::physics_eir::field::STATE_SLOT_BYTES) as usize;
                let desc = self
                    .invariant_exprs
                    .get(idx)
                    .map(String::as_str)
                    .unwrap_or("unknown");
                return Err(error_at(
                    Status::EirInvalid,
                    69,
                    0,
                    format!("invariant '{desc}' violated for entity {}", w.entity),
                ));
            }
        }
        Ok(())
    }

    /// Interpreter backend step: run the EIR, apply the ordered writes.
    pub fn step_interpreter(&mut self) -> Result<Vec<WorldWrite>> {
        let mut rt = SceneRuntime::new(&self.scene);
        self.env.time = self.scene.sim_time;
        self.env.step = self.clock;
        self.env.step_dt = self.sim_dt;
        self.env.events.clear();
        crate::eir::drain_due_events(&mut self.env);
        // The module was validated once at compile; executing skips the
        // dominance re-check every step (vital for unrolled field solvers).
        let writes = self
            .module
            .execute(&mut rt, &mut self.env, WorldId(0), WorldVersion(0))?;
        self.check_invariants(&writes)?;
        let overlays = rt.take_overlays();
        apply_writes(&mut self.scene, &writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        self.advance_clock();
        Ok(writes)
    }

    /// CPU JIT backend step over the same low-level IR.
    pub fn step_jit(&mut self) -> Result<Vec<WorldWrite>> {
        let mut rt = SceneRuntime::new(&self.scene);
        self.env.time = self.scene.sim_time;
        self.env.step = self.clock;
        self.env.step_dt = self.sim_dt;
        self.env.events.clear();
        crate::eir::drain_due_events(&mut self.env);
        let writes = self.jit.execute_with_env_validated(
            &self.jit_key,
            &mut rt,
            &mut self.env,
            WorldId(0),
            WorldVersion(0),
        )?;
        self.check_invariants(&writes)?;
        let overlays = rt.take_overlays();
        apply_writes(&mut self.scene, &writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        self.advance_clock();
        Ok(writes)
    }

    /// Cross-backend step: run the interpreter and the JIT on identical scene
    /// clones against an identical seeded execution context and require
    /// byte-identical writes, then apply one authoritative result. This is the
    /// "run cross" guarantee — both backends share EIR semantics and must agree,
    /// including for `time`/`random`/`emit`.
    pub fn step_cross(&mut self) -> Result<Vec<WorldWrite>> {
        let a = self.scene.clone();
        let b = self.scene.clone();
        let base = {
            let mut e = self.env.clone();
            e.time = self.scene.sim_time;
            e.step = self.clock;
            e.step_dt = self.sim_dt;
            e.events.clear();
            crate::eir::drain_due_events(&mut e);
            e
        };

        let mut env_a = base.clone();
        let mut rt_a = SceneRuntime::new(&a);
        let int_writes = self
            .module
            .execute(&mut rt_a, &mut env_a, WorldId(0), WorldVersion(0))?;

        let mut env_b = base.clone();
        let mut rt_b = SceneRuntime::new(&b);
        let jit_writes = self.jit.execute_with_env_validated(
            &self.jit_key,
            &mut rt_b,
            &mut env_b,
            WorldId(0),
            WorldVersion(0),
        )?;

        if int_writes != jit_writes {
            return Err(error(Status::EirInvalid, 50));
        }
        // Both backends share the emit/schedule contract: their event streams
        // and dynamic event queues must agree too.
        if env_a.events != env_b.events || env_a.queue != env_b.queue {
            return Err(error(Status::EirInvalid, 50));
        }
        // RFC-0037: bulk field sweeps live in the dense overlays rather than the
        // write list, so require byte-identical overlays too.
        if rt_a.field_overlays() != rt_b.field_overlays() {
            return Err(error(Status::EirInvalid, 50));
        }
        let overlays = rt_a.take_overlays();
        // An invariant violation fails the step before any write is applied.
        self.check_invariants(&int_writes)?;
        // Advance the live env to match the interpreter's consumed random state,
        // so random streams accumulate deterministically across steps.
        self.env = env_a;
        apply_writes(&mut self.scene, &int_writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        self.advance_clock();
        Ok(int_writes)
    }

    pub fn step_cross_n(&mut self, n: u64) -> Result<()> {
        for _ in 0..n {
            self.step_cross()?;
        }
        Ok(())
    }

    /// Batched cross-backend stepping: `k-1` interpreter-only steps followed by
    /// one cross-verified step, so the JIT runs once per `k` steps rather than
    /// every step. Steady-state cost approaches a single backend while still
    /// cross-checking every `k` steps. `step_cross`/`step_cross_n` stay strict
    /// (per-step verification) for tests and conformance.
    pub fn step_cross_batched(&mut self, k: u32) -> Result<()> {
        let k = k.max(1);
        for _ in 1..k {
            self.step_interpreter()?;
        }
        self.step_cross()?;
        Ok(())
    }
}

/// RFC-0038/RFC-0038: apply one `entity_field` grammar pair to a declaration
/// (shared by `entity` and `pool` declarations).
fn apply_entity_field(field: pest::iterators::Pair<'_, Rule>, decl: &mut EntityDecl) -> Result<()> {
    match field.as_rule() {
        Rule::position_field => {
            decl.position = Some(parse_vec3(field.into_inner().next().unwrap()))
        }
        Rule::velocity_field => {
            decl.velocity = Some(parse_vec3(field.into_inner().next().unwrap()))
        }
        Rule::state_field => {
            let list = field.into_inner().next().unwrap();
            match list.as_rule() {
                Rule::vecN => {
                    decl.state = Some(list.into_inner().map(parse_value).collect());
                }
                Rule::named_state => {
                    let mut values = Vec::new();
                    let mut names = Vec::new();
                    let mut units: Vec<Option<crate::units::Dim>> = Vec::new();
                    // `ident = value <unit>?` -> named slot with an
                    // optional compile-time dimension annotation.
                    let push_unit =
                        |it: &mut pest::iterators::Pairs<'_, Rule>,
                         units: &mut Vec<Option<crate::units::Dim>>| {
                            let u = it.next().and_then(|p| {
                                p.as_str()
                                    .trim_start_matches('[')
                                    .trim_end_matches(']')
                                    .parse::<crate::units::Dim>()
                                    .ok()
                            });
                            units.push(u);
                        };
                    for item in list.into_inner() {
                        // `vec3 pos` -> N consecutive slots named
                        // `pos`, `pos.0`, … `pos.{N-1}` (zero-init).
                        let text = item.as_str().trim();
                        if let Some(rest) = text.strip_prefix("vec") {
                            let digits: String =
                                rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                            let n: usize =
                                digits.parse().map_err(|_| error(Status::Invalid, 52))?;
                            if n == 0 || names.len() + n > crate::components::State::MAX_STATE_SLOTS
                            {
                                return Err(error(Status::Invalid, 52));
                            }
                            let vname = text[3 + digits.len()..].trim().to_string();
                            names.push(Some(vname.clone()));
                            for k in 0..n {
                                names.push(Some(format!("{vname}.{k}")));
                                values.push(0.0);
                                // `vecN` has no per-component unit.
                                units.push(None);
                            }
                            continue;
                        }
                        let mut it = item.into_inner();
                        match it.next() {
                            // `ident = value` -> named slot.
                            Some(p) if p.as_rule() == Rule::ident => {
                                names.push(Some(p.as_str().to_string()));
                                values.push(parse_value(it.next().unwrap()));
                                push_unit(&mut it, &mut units);
                            }
                            // bare `value` -> positional slot.
                            Some(p) => {
                                names.push(None);
                                values.push(parse_value(p));
                                push_unit(&mut it, &mut units);
                            }
                            None => {}
                        }
                    }
                    decl.state = Some(values);
                    decl.state_names = Some(names);
                    if units.iter().any(|u| u.is_some()) {
                        decl.state_units = Some(units);
                    }
                }
                _ => {}
            }
        }
        Rule::mass_field => decl.mass = Some(parse_value(field.into_inner().next().unwrap())),
        Rule::dynamic_field => {
            decl.dynamic = Some(field.into_inner().next().unwrap().as_str() == "true")
        }
        Rule::nbody_field => {
            decl.nbody = Some(field.into_inner().next().unwrap().as_str() == "true")
        }
        Rule::parent_field => {
            decl.parent = Some(field.into_inner().next().unwrap().as_str().to_string())
        }
        Rule::restitution_field => {
            decl.restitution = Some(parse_value(field.into_inner().next().unwrap()))
        }
        Rule::friction_field => {
            decl.friction = Some(parse_value(field.into_inner().next().unwrap()))
        }
        Rule::box_field => {
            let dims = parse_vec3(field.into_inner().next().unwrap());
            decl.collider = Some(ColliderDecl::Box { dims });
        }
        Rule::sphere_field => {
            let radius = parse_value(field.into_inner().next().unwrap());
            decl.collider = Some(ColliderDecl::Sphere { radius });
        }
        Rule::hull_field => {
            let list = field.into_inner().next().unwrap();
            let points: Vec<Vec3> = list.into_inner().map(parse_vec3).collect();
            if points.len() < 4 {
                return Err(error(Status::Invalid, 51));
            }
            decl.collider = Some(ColliderDecl::ConvexHull { points });
        }
        Rule::camera_field => {
            decl.camera = Some(field.into_inner().next().unwrap().as_str() == "true")
        }
        Rule::color_field => {
            let hex = field.into_inner().next().unwrap().as_str();
            let v = u32::from_str_radix(&hex[2..], 16).map_err(|_| error(Status::Invalid, 64))?;
            decl.color = Some(v);
        }
        // Presentation-only render hints.
        Rule::shape_field => {
            let name = field.into_inner().next().unwrap().as_str();
            let r = decl.render.get_or_insert_with(Default::default);
            match name {
                "point" | "sphere" | "box" | "capsule" => {
                    let code = match name {
                        "sphere" => 1,
                        "box" => 2,
                        "capsule" => 3,
                        _ => 0,
                    };
                    r.shape = Some(code);
                }
                // Any other name refers to a user-defined shape
                // declared in the world's `shape <name> { … }`.
                other => r.shape_name = Some(other.to_string()),
            }
        }
        Rule::size_field => {
            let inner = field.into_inner().next().unwrap();
            let r = decl.render.get_or_insert_with(Default::default);
            if inner.as_rule() == Rule::vec3 {
                let d = parse_vec3(inner);
                r.size3 = Some((d.x, d.y, d.z));
            } else {
                r.size = Some(parse_value(inner));
            }
        }
        Rule::opacity_field => {
            let v = parse_value(field.into_inner().next().unwrap());
            decl.render.get_or_insert_with(Default::default).opacity = Some(v);
        }
        Rule::glow_field => {
            let v = parse_value(field.into_inner().next().unwrap());
            decl.render.get_or_insert_with(Default::default).glow = Some(v);
        }
        Rule::label_field => {
            let v = field.into_inner().next().unwrap().as_str() == "true";
            decl.render.get_or_insert_with(Default::default).label = Some(v);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::EntityId;

    /// Regression: a `#` comment inside an `update` block must not swallow the
    /// preceding scalar parameter's value (`dt = 0.0005\n # note`).
    #[test]
    fn comment_in_update_does_not_swallow_scalar_param() {
        let src = "world { gravity=(0,0,0) entity reactor { state=(0,0,0,na=2.0,water=100.0,naoh=0.0,temp=300.0,h2=0.0); color=0xffb347 } }\n systems { update { on = reactor; dt = 0.0005\n # kinetics\n let k = 3.0 * exp(-900.0 / temp)\n na = -k * na * water\n temp = 260.0 * k * na * water\n } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(10).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values
            .clone();
        assert!(st[3] < 2.0, "Na should be consumed: {st:?}");
        assert!(st[6] > 300.0, "temperature should rise: {st:?}");
    }

    /// Diagnostics: a missing required system param reports which system and
    /// key, with a source caret via `render_diagnostic`.
    #[test]
    fn missing_param_yields_diagnostic_with_location() {
        clear_diagnostics();
        let src = "world { gravity=(0,0,0) entity b { state=(0,0,0) } }\n systems { gravity { } }";
        let err = match LangRuntime::compile(src) {
            Ok(_) => panic!("expected compile failure"),
            Err(e) => e,
        };
        let text = diagnose(src, &err);
        assert!(err.detail == 48, "expected missing-param error, got {text}");
        assert!(
            text.contains("gravity"),
            "diagnostic should name the system: {text}"
        );
        assert!(
            text.contains("gravity_y"),
            "diagnostic should name the param: {text}"
        );
        // `diagnose` rendered a source location (line/caret) from byte_offset.
        assert!(
            text.contains("line") && text.contains('^'),
            "diagnostic should carry a rendered source caret: {text}"
        );
    }

    const SOURCE: &str = r#"
        world {
            gravity = (0, -9.81, 0)
            entity vehicle {
                position = (0, 8, 0)
                velocity = (4, 0, 0)
                mass = 4
                dynamic = true
                box = (1, 0.5, 0.7)
            }
            entity ground {
                position = (0, -5, 0)
                dynamic = false
                box = (50, 5, 50)
            }
            entity camera {
                position = (0, 16, 24)
                camera = true
            }
        }
        systems {
            gravity { gravity_y = -9.81; dt = 1 / 60 }
            integrate { dt = 1 / 60 }
            ground_contact { restitution = 0.6 }
        }
    "#;

    #[test]
    fn parse_builds_world_and_systems() {
        let parsed = parse(SOURCE).unwrap();
        assert_eq!(parsed.model.entities.len(), 3);
        assert_eq!(parsed.systems.len(), 3);
        assert_eq!(parsed.systems[0].kind, "gravity");
        assert!((parsed.systems[0].params["dt"] - 1.0 / 60.0).abs() < 1e-9);
        // Camera and ground declarations survive.
        assert_eq!(parsed.model.entities[2].name, "camera");
        assert_eq!(parsed.model.entities[2].camera, Some(true));
    }

    #[test]
    fn compiles_to_low_level_eir_that_validates() {
        let compiled = compile(SOURCE).unwrap();
        assert!(!compiled.eir.functions.is_empty());
        assert!(compiled.eir.validate(true).is_ok());
    }

    #[test]
    fn interpreter_and_jit_agree_cross_backend() {
        let mut rt = LangRuntime::compile(SOURCE).unwrap();
        // Dynamic bodies only (ground is static, camera is not a body).
        assert_eq!(rt.program.entities, vec![1]);
        rt.step_cross_n(120).unwrap();
        let y = rt.scene.position(EntityId(1)).unwrap().y;
        // Vehicle settled onto the ground, never below it.
        assert!(y < 8.0, "vehicle fell to {y}");
        assert!(y >= -5.0, "vehicle should not tunnel, got {y}");
    }

    #[test]
    fn cross_step_requires_backend_agreement_each_step() {
        let mut rt = LangRuntime::compile(SOURCE).unwrap();
        for _ in 0..50 {
            rt.step_cross().unwrap();
        }
        // The interpreter and JIT advanced the same authoritative world.
        assert!(rt.clock == 50);
    }

    #[test]
    fn rejects_malformed_source() {
        assert!(parse("world { gravity = (0, -9.81) }").is_err());
        assert!(parse("systems {").is_err());
    }

    #[test]
    fn compiled_program_passes_linear_dominance_gate() {
        let compiled = compile(SOURCE).unwrap();
        // The RFC-0021 dominance gate must hold for the compiled EIR.
        assert!(compiled.eir.verify_linear_dominance().is_ok());
    }

    #[test]
    fn force_system_drifts_body_in_x() {
        // A wind/force in +x accelerates the vehicle's horizontal velocity.
        let src = r#"
            world {
                gravity = (0, 0, 0)
                entity body { position = (0, 0, 0); velocity = (0, 0, 0); mass = 1; dynamic = true; box = (1, 1, 1) }
            }
            systems {
                force { ax = 2; ay = 0; az = 0; dt = 1 / 60 }
                integrate { dt = 1 / 60 }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(60).unwrap(); // 1 second
        let vx = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .unwrap()
            .linear
            .x;
        let px = rt.scene.position(EntityId(1)).unwrap().x;
        assert!((vx - 2.0).abs() < 1e-6, "vx = {vx}");
        assert!(px > 0.5, "body should drift in +x, px = {px}");
    }

    #[test]
    fn damping_reduces_horizontal_velocity() {
        // Damping scales every velocity axis toward zero.
        let src = r#"
            world {
                gravity = (0, 0, 0)
                entity body { position = (0, 0, 0); velocity = (10, 5, 0); mass = 1; dynamic = true; box = (1, 1, 1) }
            }
            systems {
                damping { factor = 0.5 }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(3).unwrap();
        let v = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.velocity)
            .unwrap()
            .linear;
        // After 3 halvings: 10 -> 1.25, 5 -> 0.625.
        assert!((v.x - 1.25).abs() < 1e-6, "vx = {}", v.x);
        assert!((v.y - 0.625).abs() < 1e-6, "vy = {}", v.y);
    }

    #[test]
    fn state_linear_system_models_radioactive_decay() {
        // N' = -λ N  with λ = 0.1 / step. After n steps, N = N0·(1−λ)^n.
        let src = r#"
            world {
                gravity = (0, 0, 0)
                entity isotope { state = (100, 0) }
            }
            systems {
                linear { slots = 2; dt = 1
                    row0 = (-0.1, 0, 0)
                    row1 = (0, 0, 0) }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(20).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let expected = 100.0 * (1.0f64 - 0.1).powi(20);
        assert!(
            (st.values[0] - expected).abs() < 1e-6,
            "N = {}",
            st.values[0]
        );
    }

    #[test]
    fn state_linear_system_models_harmonic_oscillator() {
        // Spring: x' = v, v' = -ω²x with ω² = 10. Energy stays bounded.
        let src = r#"
            world {
                gravity = (0, 0, 0)
                entity spring { state = (1, 0) }
            }
            systems {
                linear { slots = 2; dt = 0.001
                    row0 = (0, 1, 0)
                    row1 = (-10, 0, 0) }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1000).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let (x, v) = (st.values[0], st.values[1]);
        // E = ½v² + ½ω²x²; the exact solution conserves it, explicit Euler stays
        // within a small band.
        let energy = 0.5 * v * v + 0.5 * 10.0 * x * x;
        assert!((energy - 5.0).abs() < 0.1, "energy drifted: {energy}");
    }

    #[test]
    fn parses_convex_hull_collider() {
        let src = r#"
            world {
                gravity = (0, -9.81, 0)
                entity hull {
                    position = (0, 2, 0); dynamic = true
                    hull = [ (0,0,0), (2,0,0), (0,0,2), (2,0,2), (1,2,1) ]
                }
            }
            systems {}
        "#;
        let parsed = parse(src).unwrap();
        let e = &parsed.model.entities[0];
        match &e.collider {
            Some(crate::dsl::ColliderDecl::ConvexHull { points }) => {
                assert_eq!(points.len(), 5);
                assert_eq!(*points.first().unwrap(), Vec3::new(0.0, 0.0, 0.0));
            }
            _ => panic!("expected ConvexHull collider"),
        }
        // It lowers into a scene with a convex-hull collider.
        let scene = parsed.model.build_scene();
        let collider = scene.get(EntityId(1)).unwrap().collider.as_ref().unwrap();
        assert!(matches!(
            collider,
            crate::components::Collider::ConvexHull { .. }
        ));
        // Too few points is rejected.
        assert!(parse(
            "world { gravity = (0,0,0) entity h { hull = [ (0,0,0), (1,0,0), (0,1,0) ] } } systems {}"
        )
        .is_err());
    }

    #[test]
    fn update_system_models_logistic_growth() {
        // N' = r·N·(1−N), r=1, N0=0.5, dt=0.01. Analytic N(t)=1/(1+e^(−t)).
        let src = r#"
            world { gravity = (0,0,0) entity pop { state = (0.5, 0) } }
            systems {
                update { dt = 0.01
                    s0 = 1 * s0 * (1 - s0) }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(100).unwrap(); // t = 1.0
        let n = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        let analytic = 1.0 / (1.0 + (-1.0f64).exp());
        assert!(
            (n - analytic).abs() < 0.02,
            "logistic N={n}, analytic {analytic}"
        );
        // Keep stepping: it must converge to (and never exceed) capacity 1.
        for _ in 0..2000 {
            rt.step_cross().unwrap();
        }
        let n2 = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        assert!((n2 - 1.0).abs() < 1e-3, "capacity N={n2}");
    }

    #[test]
    fn update_system_models_coupled_nonlinear_system() {
        // Two coupled nonlinear slots: A' = a·A − b·A·B, B' = c·A·B (a simple
        // resource–consumer model). Both remain bounded and the product coupling
        // executes across backends identically.
        let src = r#"
            world { gravity = (0,0,0) entity sys { state = (1, 0.5) } }
            systems {
                update { dt = 0.001
                    s0 = 2 * s0 - 1 * s0 * s1
                    s1 = 1 * s0 * s1 - 1 * s1 }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(500).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // Values stay finite and positive.
        assert!(st.values[0].is_finite() && st.values[0] > 0.0);
        assert!(st.values[1].is_finite() && st.values[1] > 0.0);
        // Cross-backend agreement is enforced on every step by step_cross.
        assert!(rt.clock == 500);
    }

    #[test]
    fn update_system_supports_transcendental_functions() {
        // exp() lowers and evaluates to e; sin()/cos() work in expressions.
        let src = r#"
            world { gravity = (0,0,0) entity a { state = (0, 0, 0) } }
            systems { update { dt = 1
                s0 = exp(1) - 2.718281828459045   # exp(1) = e ≈ 0 drift
                s1 = sin(0)                        # 0
                s2 = cos(0) - 1                    # 0
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert!(st.values[0].abs() < 1e-6, "s0 = {}", st.values[0]);
        assert!(st.values[1].abs() < 1e-6, "s1 = {}", st.values[1]);
        assert!(st.values[2].abs() < 1e-6, "s2 = {}", st.values[2]);
    }

    #[test]
    fn nbody_system_models_circular_orbit() -> pwe_api::Result<()> {
        // Two-body gravity with a near-fixed massive sun: a small body orbits at
        // radius r with circular speed v = sqrt(G·M/r).
        let src = r#"
            world { gravity = (0,0,0)
                entity sun   { state = (0, 0, 0, 0, 0, 0, 1000000) }
                entity planet{ state = (4, 0, 0, 0, 500, 0, 1) }   # v = sqrt(1·1e6/4) = 500
            }
            systems { nbody { G = 1.0; dt = 0.0001 } }
        "#;
        let mut rt = LangRuntime::compile(src)?;
        for _ in 0..10000 {
            rt.step_cross()?;
        }
        let p = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let (px, py) = (p.values[0], p.values[1]);
        let r = (px * px + py * py).sqrt();
        // The planet holds a ~circular orbit at r ≈ 4 (doesn't collapse or escape).
        assert!((r - 4.0).abs() < 0.5, "orbit radius {r}");
        Ok(())
    }

    #[test]
    fn nbody_system_models_micro_particle_repulsion() -> pwe_api::Result<()> {
        // Two like-charged particles at rest repel (G < 0): the gap grows and
        // linear momentum is conserved (equal/opposite for equal masses).
        let src = r#"
            world { gravity = (0,0,0)
                entity a { state = (-2, 0, 0,  0, 0, 0, 1) }
                entity b { state = ( 2, 0, 0,  0, 0, 0, 1) }
            }
            systems { nbody { G = -1.0; dt = 0.001 } }
        "#;
        let mut rt = LangRuntime::compile(src)?;
        for _ in 0..2000 {
            rt.step_cross()?;
        }
        let a = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let b = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // Repulsion pushes a left and b right.
        assert!(a.values[0] < -2.0, "a.x = {}", a.values[0]);
        assert!(b.values[0] > 2.0, "b.x = {}", b.values[0]);
        // Linear momentum is conserved: m_a·v_a + m_b·v_b ≈ initial 0.
        let mom = a.values[3] + b.values[3];
        assert!(mom.abs() < 0.1, "momentum = {mom}");
        Ok(())
    }

    #[test]
    fn chan_system_sends_and_receives_across_entities() {
        // A producer entity publishes its slot-0 value to channel `wire`, then a
        // consumer receives it into its slot 1. Both are plain cross-entity EIR
        // reads/writes, so the interpreter and JIT agree byte-for-byte.
        let src = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity probe { state = (3, 0) }
            }
            systems {
                send { chan = wire; value = s0 }
                recv { chan = wire; slot = 1 }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let probe = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // The producer's value (3) travelled through the channel into slot 1.
        assert!(
            (probe.values[1] - 3.0).abs() < 1e-9,
            "slot1 = {}",
            probe.values[1]
        );
        // The channel entity (id 2) holds the latest published value.
        let wire = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert!(
            (wire.values[0] - 3.0).abs() < 1e-9,
            "wire = {}",
            wire.values[0]
        );
        // And the producer's own slot 0 is unchanged.
        assert!((probe.values[0] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn chan_system_round_trips_cross_backend() {
        // Two producers publishing to one channel: last sender wins.
        let src = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity a { state = (1, 0) }
                entity b { state = (7, 0) }
            }
            systems { send { chan = wire; value = s0 } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        // b (id 2) is processed after a (id 1), so the channel holds b's value.
        let wire = rt
            .scene
            .get(EntityId(3))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert!(
            (wire.values[0] - 7.0).abs() < 1e-9,
            "wire = {}",
            wire.values[0]
        );
    }

    #[test]
    fn chan_system_communicates_across_runtimes() -> pwe_api::Result<()> {
        // Runtime A publishes to `wire`; Runtime B receives it over the wire
        // transport (cross-runtime / cross-network world communication).
        let src_a = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity producer { state = (42, 0) }
            }
            systems { send { chan = wire; value = s0 } }
        "#;
        let src_b = r#"
            world { gravity = (0,0,0)
                chan wire { value = 0 }
                entity consumer { state = (0, 0) }
            }
            systems { recv { chan = wire; slot = 1 } }
        "#;
        let mut a = LangRuntime::compile_in_region(src_a, RegionId(1))?;
        let mut b = LangRuntime::compile_in_region(src_b, RegionId(2))?;
        a.set_peer(RegionId(2));
        b.set_peer(RegionId(1));

        // A publishes 42 to wire; transport to B; B receives it into slot 1.
        a.step_cross()?;
        a.exchange(&mut b)?;
        b.step_cross()?;

        let consumer = b
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert!(
            (consumer.values[1] - 42.0).abs() < 1e-9,
            "consumer slot1 = {}",
            consumer.values[1]
        );
        Ok(())
    }

    #[test]
    fn update_system_supports_cross_entity_coupling() {
        // A dynamic earth body is attracted to a fixed massive `sun` at the
        // origin via `@sun.s0` (inverse-square gravity, A = 1). The sun is
        // declared `dynamic = false` (not updated), so only earth's state moves.
        let src = r#"
            world { gravity = (0,0,0)
                entity sun { dynamic = false; state = (0, 0) }   # fixed at x = 0
                entity earth { state = (5, 0) }                  # x = 5, vx = 0
            }
            systems { update { dt = 0.001
                s0 = s1                                                      # x' = vx
                s1 = -1 * (s0 - @sun.s0) / ((s0 - @sun.s0)*(s0 - @sun.s0)*sqrt((s0 - @sun.s0)*(s0 - @sun.s0)))
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(3000).unwrap();
        let earth_x = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        let sun_x = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap()
            .values[0];
        // Earth falls toward the fixed sun at the origin.
        assert!((sun_x - 0.0).abs() < 1e-9, "sun fixed at 0, got {sun_x}");
        assert!(earth_x < 5.0, "earth fell toward sun, x = {earth_x}");
        assert!(earth_x.is_finite());
        assert!(
            earth_x > -5.0,
            "earth did not fly past the sun, x = {earth_x}"
        );
    }

    #[test]
    fn update_system_supports_time_t() {
        // Driven oscillator (Newton):  x' = v,  v' = -ω²x + F·sin(ω_d·t).
        // Off-resonance (ω²=1, ω_d=0.37) the response is bounded.
        let src = r#"
            world { gravity = (0,0,0) entity osc { state = (0, 0) } }
            systems { update { dt = 0.001
                s0 = s1
                s1 = -1 * s0 + 0.6 * sin(0.37 * t) }
            }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(4000).unwrap(); // 4 seconds
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // The clock advanced with the update dt.
        assert!(
            (rt.scene.sim_time - 4.0).abs() < 1e-6,
            "t = {}",
            rt.scene.sim_time
        );
        // The oscillator is driven but bounded (forced-response amplitude < F/(ω²−ω_d²)).
        assert!(st.values[0].abs() < 1.0, "x = {}", st.values[0]);
        assert!(st.values[1].abs() < 1.0, "v = {}", st.values[1]);
    }

    #[test]
    fn parses_entity_color() {
        let src = r#"
            world { gravity = (0,0,0)
                entity red { position = (0,1,0); color = 0xFF0000; box = (1,1,1) }
            }
            systems {}
        "#;
        let parsed = parse(src).unwrap();
        assert_eq!(parsed.model.entities[0].color, Some(0xFF0000));
        // It lands on the scene entity for the presentation layer.
        let scene = parsed.model.build_scene();
        let e = scene.get(EntityId(1)).unwrap();
        assert_eq!(e.color, Some(0xFF0000));
    }

    #[test]
    fn update_system_supports_unary_minus() {
        // -x lowers to x·(-1); a negative growth rate.
        let src = r#"
            world { gravity = (0,0,0) entity a { state = (10, 0) } }
            systems { update { dt = 1
                s0 = -0.1 * s0 } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(10).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let expected = 10.0 * (0.9f64).powi(10);
        assert!(
            (st.values[0] - expected).abs() < 1e-6,
            "s0 = {}",
            st.values[0]
        );
    }

    #[test]
    fn update_system_supports_comparisons_and_select() {
        // `update` integrates: state[i] += dt·expr. Use `if` as a saturating
        // integrator (grows to 10 then holds), and `min`/`max` as bounded terms.
        let src = r#"
            world { gravity = (0,0,0) entity c { state = (0, 0, 0) } }
            systems { update { dt = 1
                s0 = if(s0 < 10, 1, 0)      # s0 += 1 while < 10  -> saturates at 10
                s1 = min(0.5, 10 - s0)      # bounded increment
                s2 = max(-1, s0 - 10)       # 0 once s0 = 10
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(20).unwrap();
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // The `if` integrator saturates exactly at 10.
        assert!((st.values[0] - 10.0).abs() < 1e-9, "s0 = {}", st.values[0]);
        // `max` bounds s2 to -1/step; with simultaneous update it accumulates
        // -1 for the 10 steps before s0 crossed 10 -> -10, then stops at 0.
        assert!((st.values[2] + 10.0).abs() < 1e-9, "s2 = {}", st.values[2]);
        // `min` keeps s1 bounded and finite.
        assert!(st.values[1].is_finite());
    }

    #[test]
    fn update_system_models_pendulum_with_sin() {
        // Nonlinear pendulum: θ'' = −(g/L)·sin(θ) via slots [θ, ω], L=1.
        let src = r#"
            world { gravity = (0,0,0) entity pend { state = (0.5, 0) } }
            systems { update { dt = 0.001
                s0 = s1                       # θ' = ω
                s1 = -9.81 * sin(s0)          # ω' = −(g/L)·sin(θ)
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(2000).unwrap(); // 2 seconds
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let (theta, omega) = (st.values[0], st.values[1]);
        assert!(theta.is_finite() && theta.abs() < 0.6, "θ = {theta}");
        // Energy E = ½ω² + g(1−cosθ) is conserved.
        let e = 0.5 * omega * omega + 9.81 * (1.0 - theta.cos());
        let initial = 9.81 * (1.0 - 0.5f64.cos());
        assert!(
            (e - initial).abs() < 0.1,
            "pendulum energy {e} vs {initial}"
        );
    }

    #[test]
    fn random_and_emit_work_cross_backend_and_are_reproducible() {
        // A stochastic rule: each step, s0 += random(); and emit an event with
        // the current slot. Cross-backend must agree, and replaying from a fresh
        // runtime must reproduce the same trajectory (seeded RNG).
        let src = r#"
            world { gravity = (0,0,0) entity walker { state = (0, 0) } }
            systems { update { dt = 1
                s0 = s0 + random()          # stochastic walk
                s1 = emit(7, s0)            # emit an ordered event each step
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(5).unwrap(); // exercises interpreter == JIT on random/emit
        let walker = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        let final_state = walker.values[0];
        assert!(final_state.is_finite() && final_state > 0.0);

        // Reproducibility: a fresh runtime with the same source yields the same
        // trajectory (the RNG is deterministically seeded).
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.step_cross_n(5).unwrap();
        let walker2 = rt2
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert_eq!(walker2.values[0], final_state);
    }

    #[test]
    fn user_defined_functions_lower_to_calls_and_run_cross_backend() {
        // Pure `funcs` (clamp + grow) called from an `update` rule via `CALL`.
        // The update system integrates s0 += dt·rule, so clamp caps the per-step
        // delta — the point here is that the functions are actually invoked by
        // both backends and the trajectory is reproducible.
        let src = r#"
            world { gravity = (0,0,0) entity x { state = (0.5, 0) } }
            funcs {
                # param `a` is slot s0, `b` is slot s1.
                clamp(a, b) { if(s0 < 0, 0, if(s0 > s1, s1, s0)) }
                grow(v)   { s0 * (1 - s0) }
            }
            systems { update { dt = 0.01
                s0 = clamp(grow(s0) + s0, 0.9)
                s1 = s0 * 2
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(500).unwrap(); // interpreter == JIT on CALLs
        let read = |rt: &LangRuntime| -> (f64, f64) {
            let st = rt
                .scene
                .get(EntityId(1))
                .and_then(|e| e.state.as_ref())
                .unwrap();
            (st.values[0], st.values[1])
        };
        let (s0, _s1) = read(&rt);
        // The CALLs had an effect: the population moved off its initial value
        // and stayed finite/positive.
        assert!(s0.is_finite() && s0 > 0.0, "s0 = {s0}");
        // Reproducibility: a fresh runtime reproduces the same trajectory.
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.step_cross_n(500).unwrap();
        assert_eq!(read(&rt2).0, s0);
    }

    #[test]
    fn named_state_slots_and_rich_builtins_work_cross_backend() {
        // Named state slots (`state = (x = 0, y = 0)`) accessed by name in rules
        // and via `@self.x`; rich builtins (abs, floor, sign, hypot, atan2, …).
        let src = r#"
            world { gravity = (0,0,0) entity bob { state = (x = 0.5, y = 0.1) } }
            systems { update { dt = 0.01
                x = abs(sign(@self.x)) + floor(@self.x)   # = 1 + 0 = 1
                y = hypot(3, 4) + @self.x                  # = 5 + x
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(2).unwrap(); // interpreter == JIT
        let read = |rt: &LangRuntime| -> (f64, f64) {
            let st = rt
                .scene
                .get(EntityId(1))
                .and_then(|e| e.state.as_ref())
                .unwrap();
            (st.values[0], st.values[1])
        };
        let (x, y) = read(&rt);
        // x: starts 0.5, each step x += 0.01*(1 + 0) => 0.52. y integrates
        // dt*(hypot(3,4) + x): 0.1 + 0.01*((5+0.5) + (5+0.51)) = 0.2101.
        assert!((x - 0.52).abs() < 1e-9, "x = {x}");
        assert!((y - 0.2101).abs() < 1e-9, "y = {y}");
        // Reproducible.
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.step_cross_n(2).unwrap();
        assert_eq!(read(&rt2), (x, y));
    }

    #[test]
    fn cross_entity_property_access_reads_physics_fields() {
        // `@other.mass`, `@other.position.x`, `@other.velocity.y` read the
        // corresponding physics components of another entity.
        let src = r#"
            world { gravity = (0,0,0)
                entity heavy { position = (3, 4, 0); velocity = (1, 2, 0); mass = 9 }
                entity probe { state = (0, 0) }
            }
            systems { update { dt = 0.01
                s0 = @heavy.mass                       # 9
                s1 = @heavy.position.x + @heavy.velocity.y   # 3 + 2 = 5
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(3).unwrap();
        let st = rt
            .scene
            .get(EntityId(2))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        // s0 accumulates 9 per step (dt*9*3 = 0.27), s1 accumulates 5.
        let (s0, s1) = (st.values[0], st.values[1]);
        assert!((s0 - 3.0 * 0.01 * 9.0).abs() < 1e-9, "s0 = {s0}");
        assert!((s1 - 3.0 * 0.01 * 5.0).abs() < 1e-9, "s1 = {s1}");
    }

    #[test]
    fn let_locals_enable_elegant_multi_step_rules() {
        // `let` locals compute intermediates once, reused by several rules —
        // the same model without inlining. Cross-backend + reproducible.
        let src = r#"
            world { gravity = (0,0,0)
                entity target { state = (tx = 3, ty = 4, 0) }
                entity g { state = (x = 0, y = 0, 0) }
            }
            systems { update { on = g; dt = 0.02
                let dx = @target.tx - @self.x
                let dy = @target.ty - @self.y
                let dist = hypot(dx, dy)
                let gain = min(dist * 0.1, 1.0)
                x = dx * gain
                y = dy * gain
                s2 = dist
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(50).unwrap();
        let read = |rt: &LangRuntime| -> (f64, f64, f64) {
            let st = rt
                .scene
                .get(EntityId(2))
                .and_then(|e| e.state.as_ref())
                .unwrap();
            (st.values[0], st.values[1], st.values[2])
        };
        let (x, y, dist) = read(&rt);
        // The glider moves toward the target (both coordinates grow from 0), and
        // `dist` (s2) is finite/positive (the current gap).
        assert!(x.is_finite() && x > 0.0 && y > 0.0, "x={x} y={y}");
        assert!(dist.is_finite() && dist > 0.0, "dist={dist}");
        // Reproducible.
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.step_cross_n(50).unwrap();
        assert_eq!(read(&rt2), (x, y, dist));
    }

    #[test]
    fn print_logs_values_for_debugging() {
        // `print(x)` is a transparent debug channel: it logs x and yields x, so
        // it never changes world state (deterministic).
        let src = r#"
            world { gravity = (0,0,0) entity e { state = (1, 0) } }
            systems { update { dt = 0.01
                s0 = print(s0 + 1)
                s1 = s0 * 2
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(3).unwrap();
        // Each step logs the value of (s0+1): 3 logs after 3 steps.
        assert_eq!(rt.logs().len(), 3, "one print per step");
        // `print` is transparent: s0 grew by dt*(s0+1) each step (finite, > initial).
        let st = rt
            .scene
            .get(EntityId(1))
            .and_then(|e| e.state.as_ref())
            .unwrap();
        assert!(
            st.values[0].is_finite() && st.values[0] > 1.0,
            "s0 = {}",
            st.values[0]
        );
    }

    /// Logical operators (`and` / `or` / `not`) yield 1.0 / 0.0 and combine
    /// comparisons for compound branch conditions.
    #[test]
    fn logical_operators_drive_branching() {
        let src = r#"
            world { gravity = (0,0,0) entity a { state = (s0=0.5, 0,0,0,0,0,1.0,0) } }
            systems { update { on = a; dt = 1.0
                let warm = s0 > 0.0
                s1 = warm and (s0 < 1.0)
                s2 = (s0 > 2.0) or (s0 > 0.1)
                s3 = not (s0 > 0.0)
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!((st.values[1], st.values[2], st.values[3]), (1.0, 1.0, 0.0));
    }

    /// A `let` bound to a reserved token (`t`, `pi`, `e`, `sN`) is rejected:
    /// the grammar resolves those names before bare idents, so such a binding
    /// could never be read back.
    #[test]
    fn shadowed_let_name_is_rejected() {
        for name in ["t", "pi", "e", "s0"] {
            let src = format!(
                "world {{ gravity=(0,0,0) entity a {{ state=(0,0,0,0,0,0,1,0); color=0x112233 }} }} \
                 systems {{ update {{ on = a; dt = 1.0 let {name} = 1.0 s5 = {name} }} }}"
            );
            match LangRuntime::compile(&src) {
                Ok(_) => panic!("let {name} should be rejected"),
                Err(e) => assert_eq!(e.detail, 67, "let {name}"),
            }
        }
    }

    /// `repeat n { … }` unrolls: Newton iteration converges to sqrt(2) within
    /// one step.
    #[test]
    fn repeat_loop_iterates_newton_sqrt() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let g = s1
                repeat 8 { let g = (g + s0 / g) * 0.5 }
                s1 = g - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!(
            (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
            "s1 = {}",
            st.values[1]
        );
    }

    /// `for i in lo..hi` binds the index per iteration.
    #[test]
    fn for_loop_binds_index() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let acc = s1
                for i in 1..6 { let acc = acc + i }
                s1 = acc - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 15.0, "1+2+3+4+5");
    }

    /// `break if (cond)` stops the loop early; convergence is exact.
    #[test]
    fn break_exits_loop_early() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let g = s1
                repeat 100 {
                    let g = (g + s0 / g) * 0.5
                    break if (abs(g * g - s0) < 1e-12)
                }
                s1 = g - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!(
            (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
            "s1 = {}",
            st.values[1]
        );
    }

    /// `repeat n until (cond)` exits after an iteration when cond is true;
    /// `repeat n while (cond)` skips an iteration when cond is false.
    #[test]
    fn until_and_while_gates_exit_early() {
        let until_src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=100.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let v = s1
                repeat 100 until (v < 4.0) { let v = v * 0.5 }
                s1 = v - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(until_src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 3.125, "100/2^5 = 3.125");

        let while_src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=5.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let v = s1
                repeat 100 while (v < 32.0) { let v = v * 2.0 }
                s1 = v - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(while_src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 40.0, "5*2^3 stops at 40 >= 32");
    }

    /// `continue if (cond)` skips the rest of the iteration only.
    #[test]
    fn continue_skips_rest_of_iteration() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let a = s1
                repeat 3 {
                    let a = a + 1.0
                    continue if (a > 2.0)
                    let a = a * 2.0
                }
                s1 = a - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 4.0);
    }

    /// A `break` exits only the innermost loop; the outer loop continues.
    #[test]
    fn nested_break_exits_inner_loop_only() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=0.0, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                let a = s1
                repeat 2 {
                    repeat 5 {
                        let a = a + 1.0
                        break if (a > 2.0)
                    }
                    let a = a + 10.0
                }
                s1 = a - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 24.0);
    }

    /// `rk4` supports loops too (shared let lowering): compiles and steps.
    #[test]
    fn rk4_supports_repeat_loop() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            systems { rk4 { on = n; dt = 0.01
                let g = s1
                repeat 8 { let g = (g + s0 / g) * 0.5 }
                s1 = s0 - s1 * s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(10).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!(st.values[1].is_finite(), "s1 = {}", st.values[1]);
    }

    /// Loop validation: bad counts, descending ranges, and stray `break` are
    /// rejected with specific diagnostics.
    #[test]
    fn loop_validation_errors() {
        let base = "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } systems { update { on = a; dt = 1.0 ";
        let cases: &[(&str, u32)] = &[
            ("repeat 0 { let x = 1.0 } s0 = x } }", 65),
            ("repeat 1001 { let x = 1.0 } s0 = x } }", 65),
            ("break s0 = x } }", 60),
            ("continue s0 = x } }", 60),
            ("for i in 5..3 { let x = i } s0 = x } }", 68),
        ];
        for (body, want) in cases {
            let src = format!("{base}{body}");
            match LangRuntime::compile(&src) {
                Ok(_) => panic!("should reject: {body}"),
                Err(e) => assert_eq!(e.detail, *want, "{body}"),
            }
        }
    }

    /// Scientific notation literals (`1e-12`) parse as single numbers.
    #[test]
    fn scientific_notation_parses() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=1e-3, s1=0.0, 0,0,0,0,1.0,0) } }
            systems { update { on = n; dt = 1.0
                s1 = s0 * 1e2
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!((st.values[0] - 1e-3).abs() < 1e-12);
        assert!((st.values[1] - 0.1).abs() < 1e-9, "s1 = {}", st.values[1]);
    }

    /// A function body may be a sequence of `let` bindings ending with an
    /// explicit `return expr`; parameter names bind as locals.
    #[test]
    fn function_return_with_lets() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, s2=0.0, 0,0,1.0,0) } }
            funcs {
                norm(a, b) {
                    let d = a - b
                    return abs(d)
                }
            }
            systems { update { on = n; dt = 1.0  s2 = norm(2.0, 1.0) } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[2], 1.0, "|2-1|");
    }

    /// Function bodies support bounded loops (`repeat` / `for`) before the
    /// `return`, with break/continue.
    #[test]
    fn function_return_with_loop() {
        let src = r#"
            world { gravity = (0,0,0) entity n { state = (s0=2.0, s1=1.0, 0,0,0,0,1.0,0) } }
            funcs {
                newton(x) {
                    let g = s0
                    repeat 8 { let g = (g + x / g) * 0.5 }
                    return g
                }
                newtonb(x) {
                    let g = s0
                    repeat 100 {
                        let g = (g + x / g) * 0.5
                        break if (abs(g * g - x) < 1e-12)
                    }
                    return g
                }
            }
            systems { update { on = n; dt = 1.0
                s1 = newton(2.0) - s1
                s1 = newtonb(2.0) - s1
            } }
        "#;
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!(
            (st.values[1] - 2.0_f64.sqrt()).abs() < 1e-9,
            "s1 = {}",
            st.values[1]
        );
    }

    /// A statement-form function body without `return` is rejected (the bare
    /// expression form remains available).
    #[test]
    fn function_body_requires_return() {
        let src = "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } \
                   funcs { f(a) { let d = s0 * 2.0 } } \
                   systems { update { on = a; dt = 1.0 s0 = f(1.0) } }";
        match LangRuntime::compile(src) {
            Ok(_) => panic!("statement body without return should be rejected"),
            // The grammar requires `return`; the program-level parse fails.
            Err(e) => assert_eq!(e.detail, 60),
        }
        // Bare-expression bodies still work.
        let src2 =
            "world { gravity=(0,0,0) entity a { state=(0,0,0,0,0,0,1,0); color=0x112233 } } \
                    funcs { f(a) { s0 * 2.0 } } \
                    systems { update { on = a; dt = 1.0 s0 = f(1.0) } }";
        LangRuntime::compile(src2).unwrap();
    }

    /// An invariant over a truthy expression holds across steps.
    #[test]
    fn invariant_holds_when_expression_truthy() {
        let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = 0.0 * a; b = 0.0 * b } \
                   invariant { on = chem; expr = (a + b) == 1.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(5).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 1.0);
        assert_eq!(st.values[1], 0.0);
    }

    /// A violated invariant fails the step before any write is applied: the
    /// scene keeps its pre-step state.
    #[test]
    fn invariant_violation_fails_step_and_preserves_scene() {
        let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = 0.0 - 1.0 } \
                   invariant { on = chem; expr = (a + b) == 1.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        match rt.step_cross() {
            Ok(_) => panic!("violated invariant should fail the step"),
            Err(e) => {
                assert_eq!(e.status, pwe_api::Status::EirInvalid);
                assert_eq!(e.detail, 69);
            }
        }
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 1.0, "scene must be untouched");
    }

    /// A NaN state fails the invariant step too (the EIR's own comparison
    /// rejection surfaces first), with the scene untouched.
    #[test]
    fn invariant_catches_nan_state() {
        let src = "world { gravity=(0,0,0) entity chem { state=(a=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = sqrt(0.0 - 1.0) } \
                   invariant { on = chem; expr = a == a } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        // The post-update check sees the NaN the step itself produced.
        assert!(rt.step_cross().is_err(), "NaN state must fail the step");
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 0.0, "scene must be untouched");
    }

    /// `neighbor_count` / `nearest_dist` read the scene through EIR, shared by
    /// both backends (cross-checked via `step_cross`).
    #[test]
    fn spatial_neighbor_count_and_nearest_dist() {
        let src = "world { gravity=(0,0,0) \
                   entity a { state=(x=0.0,y=0.0,z=0.0) } \
                   entity b { state=(x=1.0,y=0.0,z=0.0) } \
                   entity c { state=(x=5.0,y=0.0,z=0.0) } } \
                   systems { update { on = a; dt = 0.01 \
                     let n = neighbor_count(2.0); let d = nearest_dist() \
                     s3 = n; s4 = d } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[3] / 0.01, 1.0, "one neighbor within 2.0");
        assert!(
            (st.values[4] / 0.01 - 1.0).abs() < 1e-9,
            "nearest at distance 1"
        );
    }

    /// Gradual dimensional analysis: a correctly unit-annotated model compiles;
    /// an inconsistent rule is rejected with detail 77. Unit-free models are
    /// unaffected (wildcards never error).
    #[test]
    fn units_check_consistent_and_reject_mismatch() {
        let ok = "world { gravity=(0,0,0) params { k = 4.0 [1/s^2] } \
                  entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) } } \
                  systems { update { on = e; dt = 0.1 [s] \
                    vx = 0.0 - k * x \n x = vx } }";
        LangRuntime::compile(ok).unwrap();

        let bad = "world { gravity=(0,0,0) params { k = 4.0 [1/s^2] } \
                   entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) } } \
                   systems { update { on = e; dt = 0.1 [s] \
                     x = 0.0 - k * x \n vx = vx } }";
        match LangRuntime::compile(bad) {
            Ok(_) => panic!("dimension mismatch must be rejected"),
            Err(e) => assert_eq!(e.detail, 77),
        }

        // Unannotated model: never errors.
        let free = "world { gravity=(0,0,0) entity e { state=(x=1.0) } } \
                    systems { update { on = e; dt = 0.1; x = 0.0 - x } }";
        LangRuntime::compile(free).unwrap();
    }

    /// Units also cover `when` gates (must be dimensionless) and the internal
    /// consistency of `invariant` expressions.
    #[test]
    fn units_cover_when_and_invariant() {
        let bad_when = "world { gravity=(0,0,0) entity e { state=(x=1.0 [m]) } } \
                        systems { update { on = e; dt = 0.1 when = x x = 0.0 - x } }";
        match LangRuntime::compile(bad_when) {
            Ok(_) => panic!("a dimensioned `when` gate must be rejected"),
            Err(e) => assert_eq!(e.detail, 77),
        }
        // `x` (m) + `t` (s) is dimensionally inconsistent inside an invariant.
        let bad_inv = "world { gravity=(0,0,0) entity e { state=(x=1.0 [m]) } } \
                       systems { invariant { on = e; expr = x + t } }";
        match LangRuntime::compile(bad_inv) {
            Ok(_) => panic!("m + s must be rejected"),
            Err(e) => assert_eq!(e.detail, 77),
        }
        // A dimensionless gate over a dimensioned model is fine.
        let ok = "world { gravity=(0,0,0) \
                  entity e { state=(x=1.0 [m], vx=0.5 [m/s], gate=0.0) } } \
                  systems { update { on = e; dt = 0.1 [s] when = gate > 0.0 x = vx } }";
        LangRuntime::compile(ok).unwrap();
    }

    /// Scheduled events: `at(T)` fires in exactly one step (the window that
    /// contains `T`); `periodic(P)` fires once per period.
    #[test]
    fn scheduled_events_fire_exactly_once() {
        let src = "world { gravity=(0,0,0) entity e { state=(x=0.0, fires=0.0) } } \
                   systems { update { on = e; dt = 0.5 \
                     x = 0.0 + 1.0 \
                     fires = if(at(1.0), 1.0, 0.0) + if(periodic(1.0), 1.0, 0.0) } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        // 6 steps at dt=0.5 -> t = 0, 0.5, 1.0, 1.5, 2.0, 2.5.
        rt.step_cross_n(6).unwrap();
        let fires = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[1];
        // at(1.0) fires once; periodic(1.0) fires at t=0,1,2 (three times).
        // fires += dt * (1 + 3) = 0.5 * 4 = 2.0.
        assert!((fires - 2.0).abs() < 1e-12, "fires={fires}");
    }

    /// Python-style modules: `import "m"` binds namespace `m` (`m::f`, `m::G`),
    /// `from "m" import f` binds bare, packages resolve via `__init__.pwe`, and
    /// a module's own systems resolve their bare parameter/function names within
    /// their own namespace.
    #[test]
    fn modules_packages_and_namespaces() {
        let dir = std::env::temp_dir().join(format!("pwe_modules_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shapes")).unwrap();
        std::fs::write(
            dir.join("shapes/__init__.pwe"),
            "world { entity body { state=(x=1.0, vx=0.0) } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("physics.pwe"),
            "world { params { G = 2.0 } }\nfuncs { thrust(m) { m * 0.5 } }\n\
             systems { update { on = body; dt = 0.1 vx = 0.0 - G * x } }\n",
        )
        .unwrap();
        // Qualified access (`physics.G`, `physics.thrust`) + package entity.
        std::fs::write(
            dir.join("main.pwe"),
            "world { gravity=(0,0,0)\n import \"shapes\"\n import \"physics\" }\n\
             systems { update { on = body; dt = 0.1 \
               vx = 0.0 - physics.G * x + 0.0 * physics.thrust(2.0) } }\n",
        )
        .unwrap();
        let rt = LangRuntime::compile_file(&dir.join("main.pwe")).unwrap();
        assert_eq!(rt.scene.entities.len(), 1, "package entity present");
        assert_eq!(rt.scene.params.get("physics.G").copied(), Some(2.0));
        // `from … import …` binds bare.
        std::fs::write(
            dir.join("from.pwe"),
            "from \"physics\" import thrust\n\
             world { gravity=(0,0,0)\n import \"shapes\" }\n\
             systems { update { on = body; dt = 0.1 vx = 0.0 - 0.0 * x + 0.0 * thrust(3.0) } }\n",
        )
        .unwrap();
        LangRuntime::compile_file(&dir.join("from.pwe")).unwrap();
        // Missing module.
        std::fs::write(
            dir.join("missing.pwe"),
            "world { gravity=(0,0,0) }\nimport \"nope\"\n",
        )
        .unwrap();
        match LangRuntime::compile_file(&dir.join("missing.pwe")) {
            Ok(_) => panic!("missing module must fail"),
            Err(e) => assert_eq!(e.detail, 76),
        }
        // Duplicate entity across modules.
        std::fs::write(dir.join("dup.pwe"), "world { entity body { state=(0) } }\n").unwrap();
        std::fs::write(
            dir.join("dup2.pwe"),
            "world { gravity=(0,0,0) }\nimport \"shapes\"\nimport \"dup\"\n",
        )
        .unwrap();
        match LangRuntime::compile_file(&dir.join("dup2.pwe")) {
            Ok(_) => panic!("duplicate entity must be rejected"),
            Err(e) => assert_eq!(e.detail, 76),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bare name that resolves to no local/slot/param reads 0.0 (the
    /// documented unresolved-reference convention) instead of failing the step
    /// — an unresolved world-level component must not trip the entity lookup.
    /// RFC-0038: `spawn` activates one free slot per step and copies the
    /// caller's state; inactive slots are hidden; `despawn` clears them.
    #[test]
    fn pool_spawn_despawn_roundtrip() {
        let src = "world { gravity=(0,0,0) \
                   entity emitter { state = (x = 5.0, y = 1.0) } \
                   pool p[3] { state = (x = 0.0, y = 0.0) } } \
                   systems { spawn { on = emitter; pool = p } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        // Boot: only the emitter (id 1) is active.
        assert_eq!(rt.scene.entities.values().filter(|e| e.active).count(), 1);
        rt.step_cross().unwrap();
        rt.step_cross().unwrap();
        let active: Vec<u128> = rt
            .scene
            .entities
            .iter()
            .filter(|(_, e)| e.active)
            .map(|(id, _)| id.0)
            .collect();
        assert!(
            active.contains(&2) && active.contains(&3),
            "active={active:?}"
        );
        assert!(!active.contains(&4), "third step not yet taken: {active:?}");
        // The slot received the caller's state.
        let s = rt.scene.get(EntityId(2)).unwrap().state.as_ref().unwrap();
        assert_eq!((s.values[0], s.values[1]), (5.0, 1.0));

        // `despawn` with a `when` that never holds leaves the slots active.
        let src2 = "world { gravity=(0,0,0) \
                    entity emitter { state = (x = 5.0) } \
                    pool p[2] { state = (x = 0.0) } } \
                    systems { spawn { on = emitter; pool = p } \
                              despawn { on = p; when = x > 100.0 } }";
        let mut rt2 = LangRuntime::compile(src2).unwrap();
        rt2.step_cross().unwrap();
        assert!(rt2.scene.get(EntityId(2)).unwrap().active);
        // A `when` that always holds clears them.
        let src3 = "world { gravity=(0,0,0) \
                    entity emitter { state = (x = 5.0) } \
                    pool p[2] { state = (x = 0.0) } } \
                    systems { spawn { on = emitter; pool = p } \
                              despawn { on = p; when = active() and x > 0.0 } }";
        let mut rt3 = LangRuntime::compile(src3).unwrap();
        rt3.step_cross().unwrap();
        assert!(!rt3.scene.get(EntityId(2)).unwrap().active);
    }

    /// RFC-0038: `spawn` batches (`count = n`) and phases (`every`/`phase`).
    #[test]
    fn pool_spawn_batch_and_phase() {
        let count_active =
            |rt: &LangRuntime| rt.scene.entities.values().filter(|e| e.active).count();
        // Batch: three slots per step.
        let batch = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                     pool p[6] { state = (x = 0.0) } } \
                     systems { spawn { on = e; pool = p; count = 3 } }";
        let mut rt = LangRuntime::compile(batch).unwrap();
        rt.step_cross().unwrap();
        assert_eq!(count_active(&rt), 1 + 3);
        rt.step_cross().unwrap();
        assert_eq!(count_active(&rt), 1 + 6);
        // Phased: emit only on even steps.
        let phased = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                      pool p[6] { state = (x = 0.0) } } \
                      systems { spawn { on = e; pool = p; every = 2 } }";
        let mut rt = LangRuntime::compile(phased).unwrap();
        rt.step_cross().unwrap(); // step 0 -> emit
        assert_eq!(count_active(&rt), 1 + 1);
        rt.step_cross().unwrap(); // step 1 -> skip
        assert_eq!(count_active(&rt), 1 + 1);
        rt.step_cross().unwrap(); // step 2 -> emit
        assert_eq!(count_active(&rt), 1 + 2);
        // Phase offset: `every = 2; phase = 1` emits on odd steps.
        let offset = "world { gravity=(0,0,0) entity e { state = (x = 1.0) } \
                      pool p[6] { state = (x = 0.0) } } \
                      systems { spawn { on = e; pool = p; every = 2; phase = 1 } }";
        let mut rt = LangRuntime::compile(offset).unwrap();
        rt.step_cross().unwrap(); // step 0 -> skip
        assert_eq!(count_active(&rt), 1);
        rt.step_cross().unwrap(); // step 1 -> emit
        assert_eq!(count_active(&rt), 1 + 1);
    }

    #[test]
    fn unresolved_bare_name_reads_zero() {
        let src = "world { gravity=(0,0,0) entity e { state=(x=0.0) } } \
                   systems { update { on = e; dt = 0.5 x = 3.0 + zzz } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!((st.values[0] - 1.5).abs() < 1e-12, "got {}", st.values[0]);
    }

    /// The `wave` solver integrates `u_tt = c²∇²u` with a leapfrog over two
    /// fields: a symmetric initial pulse stays symmetric and splits outwards
    /// from the centre, and remains finite.
    #[test]
    fn wave_solver_propagates_symmetrically() {
        let src = "world { gravity=(0,0,0) \
                   field u { width=41; height=1; dx=1.0 } \
                   field um { width=41; height=1; dx=1.0 } \
                   entity e { state=(x=0.0) } } \
                   systems { wave { field = u; prev = um; velocity = 1.0; dt = 0.5 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.scene.fields.get_mut("u").unwrap().set(20, 0, 1.0);
        rt.scene.fields.get_mut("um").unwrap().set(20, 0, 1.0);
        rt.step_cross_n(8).unwrap();
        let u = rt.scene.fields.get("u").unwrap();
        for k in 0..20usize {
            assert!(
                (u.value(20 - k, 0) - u.value(20 + k, 0)).abs() < 1e-9,
                "asymmetric at k={k}"
            );
        }
        assert!(u.value(20, 0).abs() < 1.0, "centre should give up energy");
        assert!(u.value(20, 0).is_finite());
        assert!(
            u.value(20 + 5, 0).abs() > 1e-6 || u.value(20 + 8, 0).abs() > 1e-6,
            "the pulse must have spread outwards"
        );
    }

    /// 3D field solvers: `diffuse` is exactly conservative in a 3D grid, and
    /// `poisson` yields a potential well around a positive source (depth > 1).
    #[test]
    fn field_solver_systems_3d() {
        let d3 = "world { gravity=(0,0,0) field heat { width=5; height=5; depth=5; dx=1.0 } \
                  entity e { state=(x=0.0) } } \
                  systems { diffuse { field = heat; rate = 0.1 } \
                    update { on = e; dt = 1.0 \
                      let _ = fset(heat, 2.0, 2.0, 2.0, fget(heat, 2.0, 2.0, 2.0) + 1.0) \
                      x = 0.0 + 1.0 } }";
        let mut rt = LangRuntime::compile(d3).unwrap();
        rt.step_cross_n(12).unwrap();
        let f = rt.scene.fields.get("heat").unwrap();
        assert_eq!((f.width, f.height, f.depth), (5, 5, 5));
        assert!(
            (f.total() - 12.0).abs() < 1e-9,
            "3D diffuse conserves: {}",
            f.total()
        );

        let poi = "world { gravity=(0,0,0) field phi { width=5; height=5; depth=5; dx=1.0 } \
                   field rho { width=5; height=5; depth=5; dx=1.0 } entity e { state=(x=0.0) } } \
                   systems { poisson { field = phi; source = rho; iters = 20 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(rho, 2.0, 2.0, 2.0, 1.0) x = 0.0 + 1.0 } }";
        let mut rt = LangRuntime::compile(poi).unwrap();
        rt.step_cross_n(2).unwrap();
        let f = rt.scene.fields.get("phi").unwrap();
        assert!(
            f.value3(2, 2, 2) < 0.0,
            "3D potential well: {}",
            f.value3(2, 2, 2)
        );
    }

    /// Per-entity render attributes (`shape`/`size`/`opacity`/`glow`/`label`)
    /// parse into the entity's render style (presentation only).
    #[test]
    fn entity_render_attributes_parse() {
        let src = "world { gravity=(0,0,0) entity a { state=(x=0.0) shape=sphere; size=1.5; \
                   color=0xFF6B4A; glow=1.2; opacity=0.6; label=false } }";
        let model = parse(src).unwrap().model;
        let e = &model.entities[0];
        let r = e.render.as_ref().expect("render style");
        assert_eq!(r.shape, Some(1));
        assert_eq!(r.size, Some(1.5));
        assert_eq!(r.glow, Some(1.2));
        assert_eq!(r.opacity, Some(0.6));
        assert_eq!(r.label, Some(false));
        assert_eq!(e.color, Some(0xFF6B4A));
    }

    /// User-defined custom shapes (`shape <name> { part … }`) parse into the
    /// model and resolve to the entity's render parts at build time.
    #[test]
    fn custom_shapes_resolve_from_parts() {
        let src = "world { gravity=(0,0,0) \
            shape gizmo { part capsule = (0.05, 0.3, 0.05); part sphere = 0.1 at (0, 0.2, 0.0); \
              part svg = \"M 0,0 L 1,0 L 1,1 Z\" depth 0.2 scale 0.5 at (0, 1.0, 0.0); \
              part hull = [(0,0,0), (1,0,0), (0,1,0), (0,0,1)]; \
              part poly = [(0,1,0), (1,0,1), (-1,0,1)] faces = [[0,1,2]] scale 2.0; } \
            entity e { state=(x=0.0) shape = gizmo } }";
        let model = parse(src).unwrap().model;
        let g = model.shapes.get("gizmo").unwrap();
        assert_eq!(g.len(), 5);
        assert_eq!(g[2].kind, 4, "svg part");
        assert_eq!(g[2].path.as_deref(), Some("M 0,0 L 1,0 L 1,1 Z"));
        assert!((g[2].a - 0.2).abs() < 1e-9, "svg depth");
        assert!((g[2].scale - 0.5).abs() < 1e-9, "svg scale");
        assert_eq!(g[3].kind, 5, "hull part");
        assert_eq!(g[3].points.len(), 4);
        assert_eq!(g[4].kind, 6, "poly part");
        assert_eq!(g[4].faces, vec![vec![0u32, 1, 2]]);
        assert!((g[4].scale - 2.0).abs() < 1e-9, "poly amplitude");
        let scene = model.build_scene();
        let ent = scene.entities.values().next().unwrap();
        let parts = ent
            .render
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .expect("resolved parts");
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1].kind, 1);
        assert_eq!(parts[1].offset, (0.0, 0.2, 0.0));
    }

    /// A named state slot that merely starts with `s` (e.g. `speed`) is not a
    /// numeric `sN` token and must still resolve to its slot.
    #[test]
    fn named_slots_starting_with_s_resolve() {
        let src = "world { gravity=(0,0,0) entity e { state=(speed=0.0, s0x=0.0) } } \
                   systems { update { on = e; dt = 1.0
                     speed = 4.0 + 0.0
                     s0x = 9.0 + 0.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!((st.values[0], st.values[1]), (4.0, 9.0));
    }

    /// Field solver system kinds: `diffuse` conserves the injected total
    /// exactly (Jacobi sweep), and `poisson` relaxes toward a negative
    /// potential around a positive source.
    #[test]
    fn field_solver_systems() {
        let src = "world { gravity=(0,0,0) field heat { width=8; height=8; dx=1.0 } \
                   entity e { state=(x=0.0) } } \
                   systems { diffuse { field = heat; rate = 0.2 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(heat, 4.0, 4.0, fget(heat, 4.0, 4.0) + 1.0) \
                       x = 0.0 + 1.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(10).unwrap();
        let total = rt.scene.fields.get("heat").unwrap().total();
        assert!(
            (total - 10.0).abs() < 1e-9,
            "diffuse must conserve: {total}"
        );

        let poi = "world { gravity=(0,0,0) field phi { width=6; height=6; dx=1.0 } \
                   field rho { width=6; height=6; dx=1.0 } entity e { state=(x=0.0) } } \
                   systems { poisson { field = phi; source = rho; iters = 40 } \
                     update { on = e; dt = 1.0 \
                       let _ = fset(rho, 3.0, 3.0, 1.0) x = 0.0 + 1.0 } }";
        let mut rt = LangRuntime::compile(poi).unwrap();
        rt.step_cross_n(3).unwrap();
        let f = rt.scene.fields.get("phi").unwrap();
        assert!(
            f.value(3, 3) < 0.0,
            "positive source yields a negative potential well: {}",
            f.value(3, 3)
        );
    }

    /// Circular imports resolve (a module is loaded once and reachable by every
    /// alias): `a` imports `b` which imports `a`, and both are called.
    #[test]
    fn circular_imports_and_multiple_aliases_resolve() {
        let dir = std::env::temp_dir().join(format!("pwe_cycle_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("x.pwe"),
            "world { params { G = 1.0 } }\nfuncs { fx(v) { v + G } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("a.pwe"),
            "import \"x\" as alpha\nworld { }\nimport \"b\"\n\
             systems { update { on = e; dt = 1.0 s0 = alpha.fx(0.0) + b.g(0.0) } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.pwe"),
            "import \"a\"\nimport \"x\"\nworld { }\n\
             funcs { g(v) { v + 100.0 } }\n\
             systems { update { on = e; dt = 1.0 s1 = x.fx(0.0) } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.pwe"),
            "import \"a\"\nimport \"b\"\nworld { gravity=(0,0,0) entity e { state=(s0=0.0, s1=0.0) } }\n",
        )
        .unwrap();
        let mut rt = LangRuntime::compile_file(&dir.join("main.pwe")).unwrap();
        // Both aliases of the same module are usable, and the cycle merges once.
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert!(
            st.values[0] > 0.0 && st.values[1] > 0.0,
            "both alias references resolved: {:?}",
            st.values
        );
        // `--param`-style override reaches every alias of one parameter.
        let canon = rt
            .scene
            .params
            .keys()
            .filter(|k| k.ends_with(".G") || *k == "G")
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            canon.len() >= 2,
            "module imported under two namespaces: {canon:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `params { … }` declares runtime-settable model parameters that rules
    /// read by name (overridable on the scene before running).
    #[test]
    fn model_params_read_and_override() {
        let src =
            "world { gravity=(0,0,0) params { G = 10.0 } entity e { state=(x=1.0,vx=0.0) } } \
                   systems { update { on = e; dt = 0.1; vx = 0.0 - G * x; x = vx } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        assert_eq!(rt.scene.params.get("G").copied(), Some(10.0));
        rt.step_cross_n(5).unwrap();
        let a = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[1];
        // Same model, overridden parameter -> different trajectory.
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.scene.params.insert("G".to_string(), 1.0);
        rt2.step_cross_n(5).unwrap();
        let b = rt2
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[1];
        assert_ne!(a, b, "overriding G must change the trajectory");
    }

    /// Neighborhood aggregates and directional sensing: `neighbor_mean`    /// Neighborhood aggregates and directional sensing: `neighbor_mean`
    /// averages a State slot over neighbours within a radius (0 when none),
    /// and `nearest_dx/dy/dz` give the offset to the closest neighbour.
    #[test]
    fn neighborhood_aggregates_and_offsets() {
        let src = "world { gravity=(0,0,0) \
                   entity a { state=(x=0.0, y=0.0, z=0.0, vx=10.0, vy=0.0, vz=0.0) } \
                   entity b { state=(x=2.0, y=0.0, z=0.0, vx=20.0, vy=0.0, vz=0.0) } \
                   entity c { state=(x=9.0, y=0.0, z=0.0, vx=90.0, vy=0.0, vz=0.0) } } \
                   systems { update { on = a; dt = 1.0 \
                     let n = neighbor_count(3.0) \
                     let mx = neighbor_mean(0.0, 3.0) \
                     let mv = neighbor_mean(3.0, 3.0) \
                     s6 = n; s7 = mx; s8 = mv; s9 = nearest_dx(); s10 = nearest_dy() } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let v = &rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values;
        assert_eq!(v[6], 1.0, "one neighbour within 3.0 (b, not c)");
        assert_eq!(v[7], 2.0, "mean x of neighbours = b.x");
        assert_eq!(v[8], 20.0, "mean vx of neighbours = b.vx");
        assert_eq!(v[9], 2.0, "nearest_dx = b.x - a.x");
        assert_eq!(v[10], 0.0, "nearest_dy");

        // Alone: aggregate and offset are 0.
        let lone = "world { gravity=(0,0,0) entity only { state=(0.0,0.0,0.0) } } \
                    systems { update { on = only; dt = 1.0 \
                      s3 = neighbor_mean(0.0, 5.0); s4 = nearest_dx() } }";
        let mut rt = LangRuntime::compile(lone).unwrap();
        rt.step_cross_n(1).unwrap();
        let v = &rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values;
        assert_eq!(v[3], 0.0);
        assert_eq!(v[4], 0.0);
    }

    /// A spatial query in a function body is rejected at compile time: there
    /// is no per-entity context to target.    /// A spatial query in a function body is rejected at compile time: there
    /// is no per-entity context to target.
    #[test]
    fn spatial_query_rejected_in_function_body() {
        let src = "world { gravity=(0,0,0) entity a { state=(x=0.0) } } \
                   funcs { f() { neighbor_count(2.0) } } \
                   systems { update { on = a; dt = 0.01 s3 = f() } }";
        match LangRuntime::compile(src) {
            Ok(_) => panic!("query in a function body should be rejected"),
            Err(e) => assert_eq!(e.detail, 70),
        }
    }

    /// Two invariants coexist: each owns its own verdict field in the hidden
    /// check component.
    #[test]
    fn multiple_invariants_coexist() {
        let src = "world { gravity=(0,0,0) entity chem { state=(a=1.0,b=0.0) } } \
                   systems { update { on = chem; dt = 0.01 a = 0.0 * a; b = 0.0 * b } \
                   invariant { on = chem; expr = (a + b) == 1.0 } \
                   invariant { on = chem; expr = a >= 0.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(3).unwrap();
    }

    /// `noise()` draws a seeded standard normal; a fresh runtime reproduces it.
    #[test]
    fn noise_is_seeded_and_reproducible() {
        let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0) } } \
                   systems { update { on = e; dt = 1.0 let n = noise() s1 = n } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let v = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[1];
        assert!(v.is_finite(), "noise must be finite, got {v}");
        let mut rt2 = LangRuntime::compile(src).unwrap();
        rt2.step_cross_n(1).unwrap();
        let v2 = rt2
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[1];
        assert_eq!(v, v2, "seeded draws must reproduce");
    }

    /// `vlen`/`vdot`/`vdist` compute over scalar components (pure arithmetic).
    #[test]
    fn vector_builtins_compute_lengths_and_dots() {
        let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     let l = vlen(3.0, 4.0, 0.0) \
                     let d = vdot(1.0, 0.0, 0.0, 0.0, 1.0, 0.0) \
                     let dist = vdist(0.0,0.0,0.0, 3.0,4.0,0.0) \
                     s1 = l; s2 = d; s3 = dist } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 5.0);
        assert_eq!(st.values[2], 0.0);
        assert_eq!(st.values[3], 5.0);
    }

    /// `when = expr` gates every rule's write: the state is untouched when the
    /// gate is 0 (mode semantics).
    #[test]
    fn when_gates_rule_writes_by_mode() {
        let src = "world { gravity=(0,0,0) entity m { state=(mode=1.0,x=0.0) } } \
                   systems { update { on = m; dt = 1.0 when = mode == 0.0 x = 0.0 + 1.0 } \
                   update { on = m; dt = 1.0 when = mode == 1.0 mode = 0.0 - 2.0 * mode } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 0.0, "gated rule must not apply");
        assert_eq!(st.values[0], -1.0, "ungated rule must apply");
    }

    /// `substeps = n` runs the integration n times with dt/n; finer Euler
    /// tracks the analytic solution better.
    #[test]
    fn substeps_improve_euler_accuracy() {
        let mk = |sub: usize| {
            format!(
                "world {{ gravity=(0,0,0) entity o {{ state=(x=1.0) }} }} \
                 systems {{ update {{ on = o; dt = 0.5; substeps = {sub}; x = 0.0 - x }} }}"
            )
        };
        let analytic = (-1.0f64).exp();
        let mut rt = LangRuntime::compile(&mk(1)).unwrap();
        rt.step_cross_n(2).unwrap();
        let a1 = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[0];
        let mut rt = LangRuntime::compile(&mk(4)).unwrap();
        rt.step_cross_n(2).unwrap();
        let a4 = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[0];
        assert!(
            (a4 - analytic).abs() < (a1 - analytic).abs(),
            "substeps=4 ({a4}) must track the analytic {analytic} better than substeps=1 ({a1})"
        );
    }

    /// `substeps = n` on `rk4` repeats the whole RK4 step with dt/n; RK4's
    /// accuracy makes substeps redundant for smooth laws, but they must run.
    #[test]
    fn rk4_substeps_run() {
        let mk = |sub: usize| {
            format!(
                "world {{ gravity=(0,0,0) entity o {{ state=(x=1.0) }} }} \
                 systems {{ rk4 {{ on = o; dt = 0.5; substeps = {sub}; x = 0.0 - x }} }}"
            )
        };
        let analytic = (-1.0f64).exp();
        let mut rt = LangRuntime::compile(&mk(2)).unwrap();
        rt.step_cross_n(2).unwrap();
        let a = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[0];
        assert!(
            (a - analytic).abs() < 1e-4,
            "rk4 with substeps must stay accurate: {a} vs {analytic}"
        );
    }

    /// Grid field access: `fset` writes a cell, `fget` reads it back in the
    /// same step (the write overlay), `flap` computes the zero-flux stencil
    /// laplacian; the cell write persists in the scene's field.
    #[test]
    fn field_access_reads_and_writes_cells() {
        let src = "world { gravity=(0,0,0) \
                   field heat { width = 4; height = 4; dx = 1.0 } \
                   entity e { state=(s0=0.0,s1=0.0,s2=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     fset(heat, 1.0, 2.0, 0.0 + 7.0) \
                     s0 = fget(heat, 1.0, 2.0) \
                     s1 = flap(heat, 1.0, 2.0) \
                     s2 = fget(heat, 3.0, 3.0) } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 7.0, "fget sees the same-step write");
        assert_eq!(st.values[1], -28.0, "laplacian of a lone 7 among zeros");
        assert_eq!(st.values[2], 0.0);
        let f = rt.scene.fields.get("heat").unwrap();
        assert_eq!(f.value(1, 2), 7.0, "the cell write persists");
        assert_eq!(f.total(), 7.0);
    }

    /// A field declaration without dimensions is rejected.
    #[test]
    fn field_declaration_requires_dimensions() {
        let src = "world { gravity=(0,0,0) field heat { dx = 1.0 } }";
        match LangRuntime::compile(src) {
            Ok(_) => panic!("a field without width/height should be rejected"),
            Err(e) => assert_eq!(e.detail, 75),
        }
    }

    /// `vec3 pos` in a state list reserves N consecutive slots named `pos`,
    /// `pos.0` … `pos.{N-1}`; `@self.state.pos.1` reads component 1.
    #[test]
    fn vecn_state_reserves_named_slots() {
        let src = "world { gravity=(0,0,0) entity e { state = (vec3 pos, mass = 1.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     s0 = 0.0 + 5.0 \
                     s5 = @self.state.pos.1 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 5.0, "pos.0 (slot 0) written");
        assert_eq!(st.values[3], 1.0, "mass follows the 3 vec slots");
        // `pos.1` (slot 1) reads the pre-step value (simultaneous rules).
        assert_eq!(st.values[5], 0.0);
    }

    /// The world `title = "..."` and an entity's `parent = <name>` parse into
    /// the model (`pwe present` uses them for the run-info panel).
    #[test]
    fn title_and_parent_parse() {
        let src = "world { title = \"solar system (8 planets + Moon)\" gravity=(0,0,0) \
                   entity moon { parent = earth; state=(0) } entity earth { state=(0) } }";
        let p = parse(src).unwrap();
        assert_eq!(
            p.model.title.as_deref(),
            Some("solar system (8 planets + Moon)")
        );
        assert_eq!(p.model.entities[0].parent.as_deref(), Some("earth"));
        assert_eq!(p.model.entities[1].parent, None);
    }

    /// `%` is a remainder operator (fmod semantics) — used by grid walks.
    #[test]
    fn modulo_operator_computes_remainder() {
        let src = "world { gravity=(0,0,0) entity e { state=(0,0) } } \
                   systems { update { on = e; dt = 1.0 s0 = 255.0 % 16.0; s1 = 7.0 % 3.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 15.0);
        assert!((st.values[1] - 1.0).abs() < 1e-12);
    }

    /// An out-of-range grid field access fails the step with a clear error
    /// instead of panicking in the runtime path.
    #[test]
    fn out_of_range_field_access_is_an_error() {
        let src = "world { gravity=(0,0,0) field g { width=4; height=4; dx=1.0 } \
                   entity e { state=(0) } } \
                   systems { update { on = e; dt = 1.0 s0 = fget(g, 2.0, 9.0) } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        match rt.step_cross() {
            Ok(_) => panic!("out-of-range field access should fail the step"),
            Err(e) => assert_eq!(e.detail, 7),
        }
    }

    /// `last_event(kind)` reads the most recent event of that kind emitted so
    /// far in the step; unknown kinds read 0. Events are cleared each step.
    #[test]
    fn last_event_reads_within_the_step_and_clears() {
        let src = "world { gravity=(0,0,0) entity e { state=(s0=0.0,s1=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     emit(7.0, 42.0) \
                     s0 = last_event(7.0) \
                     s1 = last_event(9.0) } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[0], 42.0, "kind 7 resolves to its payload");
        assert_eq!(st.values[1], 0.0, "unknown kind reads 0");
        assert_eq!(rt.emitted_events().len(), 1, "one event this step");
        assert_eq!(
            rt.emitted_events()[0].kind,
            7,
            "the kind is the value, not bits"
        );
        rt.step_cross_n(1).unwrap();
        assert_eq!(
            rt.emitted_events().len(),
            1,
            "events are cleared per step (not accumulated)"
        );
    }

    /// `every = n` runs the system only when step % n == 0.
    #[test]
    fn every_runs_only_on_matching_steps() {
        let src = "world { gravity=(0,0,0) entity e { state=(x=0.0) } } \
                   systems { update { on = e; dt = 1.0 every = 3 x = 0.0 + 1.0 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(4).unwrap();
        let x = rt
            .scene
            .get(EntityId(1))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .values[0];
        assert_eq!(x, 2.0, "steps 0 and 3 apply; 1 and 2 are skipped");
    }

    /// `watch` flags a strict sign change between consecutive steps; the flag
    /// and the watch memory are ordinary state slots.
    #[test]
    fn watch_flags_zero_crossing() {
        let src = "world { gravity=(0,0,0) entity e { state=(x=-1.0,m=-1.0,f=0.0) } } \
                   systems { update { on = e; dt = 0.1 x = 0.0 + 6.0 } \
                   watch { on = e; expr = x; mem = 1; into = 2 } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        let flag = |rt: &LangRuntime| {
            rt.scene
                .get(EntityId(1))
                .unwrap()
                .state
                .as_ref()
                .unwrap()
                .values[2]
        };
        rt.step_cross().unwrap();
        assert_eq!(flag(&rt), 0.0, "no crossing yet");
        rt.step_cross().unwrap();
        assert_eq!(flag(&rt), 1.0, "x crossed zero between the steps");
        rt.step_cross().unwrap();
        assert_eq!(flag(&rt), 0.0, "no further crossing");
    }

    /// `s[i]` dynamic indexing reads and writes the State slot at a runtime
    /// index, shared by both backends (cross-checked via `step_cross`).
    #[test]
    fn dynamic_slot_index_reads_and_writes() {
        let src = "world { gravity=(0,0,0) entity e { state=(s0=10.0,s1=20.0,s2=0.0,s3=0.0) } } \
                   systems { update { on = e; dt = 1.0 \
                     let i = 0.0 + 1.0 \
                     s[i] = 0.0 + 100.0 \
                     s2 = s[0.0 + 1.0] } }";
        let mut rt = LangRuntime::compile(src).unwrap();
        rt.step_cross_n(1).unwrap();
        let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
        assert_eq!(st.values[1], 120.0, "s[1] += 1*100");
        assert_eq!(st.values[2], 20.0, "the read sees the pre-step state");
    }

    /// A dynamic slot LHS is rejected in `rk4`: its working states are
    /// compile-time register chains, so a runtime-index LHS cannot feed them.
    /// The `update` system supports dynamic LHS.
    #[test]
    fn dynamic_lhs_rejected_in_rk4() {
        let src = "world { gravity=(0,0,0) entity e { state=(s0=1.0) } } \
                   systems { rk4 { on = e; dt = 0.1 s[0.0] = 0.0 + 1.0 } }";
        match LangRuntime::compile(src) {
            Ok(_) => panic!("dynamic LHS in rk4 should be rejected"),
            Err(e) => assert_eq!(e.detail, 73),
        }
    }
}
