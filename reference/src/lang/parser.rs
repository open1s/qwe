//! PEST grammar, AST types, and parsing into a `ParsedProgram`.
use super::*;

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
pub(crate) fn parse_value(pair: Pair<'_, Rule>) -> f64 {
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
pub(crate) fn parse_vec3(pair: Pair<'_, Rule>) -> Vec3 {
    let mut v = pair.into_inner().map(parse_value);
    Vec3::new(
        v.next().unwrap_or(0.0),
        v.next().unwrap_or(0.0),
        v.next().unwrap_or(0.0),
    )
}

/// The scalar (numeric) parameter names a system kind recognises. An
/// `name = <number>` whose name is not listed here is a **rule**, not a
/// parameter — so `update { x = 1.0 }` is a rule (and `update { dt = 0.01 }` is
/// the `dt` parameter). Keeps system config self-documenting and avoids the old
/// "bare number silently became a parameter" surprise.
pub(crate) fn numeric_param_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "gravity" => &["gravity_y", "dt"],
        "integrate" => &["dt"],
        "damping" => &["factor"],
        "ground_contact" => &["restitution"],
        "wall" => &["x", "z", "y_min", "restitution"],
        "force" => &["ax", "ay", "az", "dt"],
        "linear" => &["slots", "dt"],
        "nbody" => &["G", "dt"],
        "send" => &["value"],
        "recv" => &["slot"],
        "update" | "rk4" => &["dt", "every", "substeps"],
        "watch" => &["mem", "into"],
        "conserved" => &["tolerance"],
        "diffuse" => &["rate"],
        "poisson" => &["iters", "scale"],
        "wave" => &["velocity", "dt", "damping", "absorb", "absorb_width"],
        "spawn" => &["count", "every", "phase"],
        "soft" => &["stiffness", "damping", "iterations"],
        "joint" => &["length", "stiffness", "damping", "iterations"],
        _ => &[],
    }
}

/// Parses an `ident = rhs` parameter pair and stores it in the appropriate
/// bucket (scalar / vector / expression) of a `SystemDecl`.
/// The two value kinds the surface language distinguishes at `let` bindings:
/// comparisons/logical ops are boolean; everything else is numeric.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LangType {
    Float,
    Bool,
}

/// The known surface kind of an expression: comparisons/logical ops are `bool`,
/// everything else numeric, and a bare name takes the kind of the earlier `let`
/// that bound it (if any). Used by [`check_let_types`].
fn infer_kind(e: &Expr, env: &std::collections::HashMap<String, LangType>) -> Option<LangType> {
    match e {
        Expr::Cmp(..) | Expr::And(..) | Expr::Or(..) | Expr::Not(..) => Some(LangType::Bool),
        Expr::Const(_)
        | Expr::Add(..)
        | Expr::Sub(..)
        | Expr::Mul(..)
        | Expr::Div(..)
        | Expr::Rem(..)
        | Expr::Neg(..) => Some(LangType::Float),
        Expr::Name(n) => env.get(n).copied(),
        _ => None,
    }
}

/// The name of the first `bool`-typed value used in a *numeric* position
/// (arithmetic/comparison operand), if any. Boolean operators (`and`/`or`/`not`)
/// take condition operands, so they are not flagged.
fn bool_in_numeric(e: &Expr, env: &std::collections::HashMap<String, LangType>) -> Option<String> {
    match e {
        Expr::Name(n) if env.get(n) == Some(&LangType::Bool) => Some(n.clone()),
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Div(a, b)
        | Expr::Rem(a, b)
        | Expr::Cmp(_, a, b) => bool_in_numeric(a, env).or_else(|| bool_in_numeric(b, env)),
        Expr::Neg(a) | Expr::SlotDyn(a) => bool_in_numeric(a, env),
        Expr::Call(_, args) => args.iter().find_map(|a| bool_in_numeric(a, env)),
        _ => None,
    }
}

/// Validates `let` annotations using the kinds of earlier bindings in scope, so
/// aliasing a bool binding is accepted and a bool used as a number is rejected
/// (detail 89). Runs per statement list, in order.
pub(crate) fn check_let_types(stmts: &[UpdateStmt], offset: usize) -> Result<()> {
    let mut env: std::collections::HashMap<String, LangType> = Default::default();
    check_let_types_env(stmts, offset, &mut env)
}

fn check_let_types_env(
    stmts: &[UpdateStmt],
    offset: usize,
    env: &mut std::collections::HashMap<String, LangType>,
) -> Result<()> {
    for s in stmts {
        match s {
            UpdateStmt::Let(name, text, ann) => {
                if name == "_" {
                    continue;
                }
                let expr = parse_expr_str(text)?;
                let inferred = infer_kind(&expr, env);
                match ann {
                    Some(true) => {
                        if inferred == Some(LangType::Float) {
                            return Err(error_at(
                                Status::Invalid,
                                89,
                                offset,
                                format!("`let {name}: bool` but the expression is a number"),
                            ));
                        }
                    }
                    Some(false) => {
                        if inferred == Some(LangType::Bool) {
                            return Err(error_at(
                                Status::Invalid,
                                89,
                                offset,
                                format!(
                                    "`let {name}` is annotated numeric but the expression is bool"
                                ),
                            ));
                        }
                        if let Some(bn) = bool_in_numeric(&expr, env) {
                            return Err(error_at(
                                Status::Invalid,
                                89,
                                offset,
                                format!("`{bn}` is bool, used as a number in `let {name}`"),
                            ));
                        }
                    }
                    None => {}
                }
                if let Some(k) = inferred {
                    env.insert(name.clone(), k);
                }
            }
            UpdateStmt::Repeat(_, body) => check_let_types_env(body, offset, env)?,
            UpdateStmt::For(idx, _, _, body) => {
                env.insert(idx.clone(), LangType::Float);
                check_let_types_env(body, offset, env)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The names of built-in call forms (validated in [`build_call`]); anything else
/// in a call position must be a user-defined `funcs` function.
pub(crate) fn is_builtin_call(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "exp"
            | "ln"
            | "sqrt"
            | "pow"
            | "min"
            | "max"
            | "if"
            | "random"
            | "print"
            | "emit"
            | "active"
            | "neighbor_count"
            | "nearest_dist"
            | "schedule"
            | "at"
            | "periodic"
            | "neighbor_mean"
            | "nearest_dx"
            | "nearest_dy"
            | "nearest_dz"
            | "noise"
            | "vlen"
            | "vdot"
            | "vdist"
            | "last_event"
            | "fget"
            | "flap"
            | "fset"
            | "abs"
            | "floor"
            | "ceil"
            | "round"
            | "sign"
            | "log10"
            | "log2"
            | "sinh"
            | "cosh"
            | "tanh"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "hypot"
            | "inte"
            | "deriv"
    )
}

/// Evaluates a purely-integer constant expression with **integer** semantics
/// (truncating `/` and `%`), used for `<int>`-annotated `let`s so that e.g.
/// `let n: i64 = 7 / 2` yields 3, not 3.5. Non-constant expressions return None.
fn fold_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Const(c) if c.is_finite() && c.fract() == 0.0 && c.abs() < 9.0e15 => Some(*c as i64),
        Expr::Add(a, b) => Some(fold_int(a)?.checked_add(fold_int(b)?)?),
        Expr::Sub(a, b) => Some(fold_int(a)?.checked_sub(fold_int(b)?)?),
        Expr::Mul(a, b) => Some(fold_int(a)?.checked_mul(fold_int(b)?)?),
        Expr::Div(a, b) => {
            let d = fold_int(b)?;
            if d == 0 {
                None
            } else {
                Some(fold_int(a)? / d)
            }
        }
        Expr::Rem(a, b) => {
            let d = fold_int(b)?;
            if d == 0 {
                None
            } else {
                Some(fold_int(a)? % d)
            }
        }
        Expr::Neg(a) => Some(-fold_int(a)?),
        _ => None,
    }
}

/// Whether an expression tree contains a numeric literal with a fractional
/// part (used to reject `let i: i64 = 1.5`, detail 89).
fn contains_fractional_const(e: &Expr) -> bool {
    match e {
        Expr::Const(c) => c.is_finite() && c.fract() != 0.0,
        Expr::SlotDyn(a) | Expr::Neg(a) | Expr::Not(a) => contains_fractional_const(a),
        Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Mul(a, b)
        | Expr::Div(a, b)
        | Expr::Rem(a, b)
        | Expr::Cmp(_, a, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => contains_fractional_const(a) || contains_fractional_const(b),
        Expr::Call(_, args) => args.iter().any(contains_fractional_const),
        _ => false,
    }
}

/// Parses a `let` binding: `let name[: type] = expr`. Returns
/// `(name, expr_text, annotation)` where the annotation is `Some(true)` for
/// `: bool`, `Some(false)` for a numeric type (`f64`/`i64`/`i32`/`u64`/`u32`),
/// and `None` when unannotated. The annotation is checked later by
/// [`check_let_types`] (which tracks earlier bindings). Integer-annotated
/// constant expressions are folded with integer semantics.
pub(crate) fn parse_let_parts(
    first: Pair<'_, Rule>,
    offset: usize,
) -> Result<(String, String, Option<bool>)> {
    let mut li = first.into_inner();
    let name = next_pair(&mut li)?.as_str().to_string();
    check_let_name(&name, offset)?;
    let mut nxt = next_pair(&mut li)?;
    if nxt.as_rule() == Rule::type_name {
        let ty = nxt.as_str().to_string();
        let is_bool = match ty.as_str() {
            "bool" => true,
            "f64" | "i64" | "i32" | "u64" | "u32" => false,
            other => {
                return Err(error_at(
                    Status::Invalid,
                    89,
                    offset,
                    format!("unknown type `{other}` (expected f64/i64/i32/u64/u32/bool)"),
                ))
            }
        };
        nxt = next_pair(&mut li)?;
        let expr = nxt.as_str().trim().to_string();
        // Integer-annotated bindings take integer semantics for constant exprs.
        if matches!(ty.as_str(), "i64" | "i32" | "u64" | "u32") {
            let parsed = parse_expr_str(&expr)?;
            if let Some(v) = fold_int(&parsed) {
                return Ok((name, v.to_string(), Some(false)));
            }
            if contains_fractional_const(&parsed) {
                return Err(error_at(
                    Status::Invalid,
                    89,
                    offset,
                    format!("`let {name}: {ty}` but the expression is not an integer constant"),
                ));
            }
        }
        return Ok((name, expr, Some(is_bool)));
    }
    Ok((name, nxt.as_str().trim().to_string(), None))
}

pub(crate) fn store_param(param: Pair<'_, Rule>, decl: &mut SystemDecl) -> Result<()> {
    let mut inner = param.into_inner();
    let first = next_pair(&mut inner)?;
    // `let name = expr` is a local binding; otherwise `ident = rhs`.
    if first.as_rule() == Rule::let_stmt {
        // The literal `let` is transparent; children are [ident(name),
        // (type_name)?, expr].
        let (name, expr, ann) = parse_let_parts(first, decl.byte_offset)?;
        decl.update_stmts.push(UpdateStmt::Let(name, expr, ann));
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
            .push(UpdateStmt::Let("_".to_string(), text, None));
        return Ok(());
    }
    if first.as_rule() == Rule::ode_stmt || first.as_rule() == Rule::add_stmt {
        let is_ode = first.as_rule() == Rule::ode_stmt;
        let mut it = first.into_inner();
        if is_ode {
            let _ = it.next();
        }
        let lhs = next_pair(&mut it)?;
        let key = lhs.as_str().trim().to_string();
        let expr = next_pair(&mut it)?;
        if expr.as_rule() != Rule::expr {
            return Err(error(Status::Invalid, 55));
        }
        decl.update.insert(key, expr.as_str().trim().to_string());
        return Ok(());
    }
    if first.as_rule() == Rule::dot_lhs {
        let key = first.as_str().trim().to_string();
        let rhs = next_pair(&mut inner)?;
        if rhs.as_rule() != Rule::expr {
            return Err(error(Status::Invalid, 55));
        }
        decl.assigns.insert(key, rhs.as_str().trim().to_string());
        return Ok(());
    }
    if first.as_rule() == Rule::slot_lhs {
        let key = first.as_str().trim().to_string();
        let rhs = next_pair(&mut inner)?;
        if rhs.as_rule() != Rule::expr {
            return Err(error(Status::Invalid, 55));
        }
        decl.assigns.insert(key, rhs.as_str().trim().to_string());
        return Ok(());
    }
    let key = first.as_str().trim().to_string();
    let rhs = next_pair(&mut inner)?;
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
                "chan"
                    | "on"
                    | "when"
                    | "field"
                    | "source"
                    | "prev"
                    | "pool"
                    | "other"
                    | "type"
                    | "body"
            ) {
                decl.string_params.insert(key, text);
            } else if let Some(v) = parse_scalar_number(&text) {
                if numeric_param_keys(&decl.kind).contains(&key.as_str()) {
                    decl.params.insert(key, v);
                } else {
                    decl.assigns.insert(key, text);
                }
            } else {
                decl.assigns.insert(key, text);
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
            let d = parse_unit_expr(unit)?;
            decl.param_units.insert(first.as_str().to_string(), d);
        }
    }
    Ok(())
}

/// Parses a bracketed unit annotation `[m/s^2]`, erroring (detail 84) when the
/// text is not a valid unit — a typo must not silently disable dimension checks.
pub(crate) fn parse_unit_expr(pair: Pair<'_, Rule>) -> Result<crate::units::Dim> {
    let text = pair.as_str().trim_start_matches('[').trim_end_matches(']');
    text.parse::<crate::units::Dim>().map_err(|_| {
        error_at(
            Status::Invalid,
            84,
            pair.as_span().start(),
            format!("malformed unit annotation `{}`", pair.as_str()),
        )
    })
}

/// Maximum iteration count of one loop.
pub(crate) const MAX_REPEAT_COUNT: usize = 1_000;
/// Maximum number of statements one loop may unroll into.
/// The numeric slot index of an `sN` token, or `None` for any other name
/// (including a named slot like `smooth` that merely starts with `s`).
pub(crate) fn numeric_slot(key: &str) -> Option<usize> {
    key.strip_prefix('s').and_then(|d| {
        if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) {
            d.parse().ok()
        } else {
            None
        }
    })
}

pub(crate) const MAX_UNROLLED_STMTS: usize = 10_000;

/// Rejects `let` names that can never be read back: the grammar resolves
/// `t` (time), `pi`/`e` (constants), and `sN` (slot) before bare idents, so a
/// binding with such a name is silently unreachable.
/// Takes the next PEST pair, or reports a program parse failure (detail 60).
/// The grammar guarantees the child, so this never panics on well-formed input.
pub(crate) fn next_pair<'a>(
    pairs: &mut pest::iterators::Pairs<'a, Rule>,
) -> Result<pest::iterators::Pair<'a, Rule>> {
    pairs.next().ok_or_else(|| error(Status::Invalid, 60))
}

pub(crate) fn check_let_name(name: &str, offset: usize) -> Result<()> {
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
pub(crate) fn parse_int_bound(pair: Pair<'_, Rule>, offset: usize, what: &str) -> Result<f64> {
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
pub(crate) fn unrolled_size(stmts: &[UpdateStmt]) -> usize {
    stmts
        .iter()
        .map(|s| match s {
            UpdateStmt::Let(..) | UpdateStmt::Break(_) | UpdateStmt::Continue(_) => 1,
            UpdateStmt::If(..) => 1,
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
pub(crate) fn build_loop_stmt(pair: Pair<'_, Rule>, offset: usize) -> Result<UpdateStmt> {
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
                    let kw = next_pair(&mut ci)?.as_str().to_string();
                    let cond = next_pair(&mut ci)?.as_str().trim().to_string();
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
pub(crate) fn to_let_stmts(stmts: &[UpdateStmt], offset: usize) -> Result<Vec<LetStmt>> {
    check_let_types(stmts, offset)?;
    stmts
        .iter()
        .map(|s| match s {
            UpdateStmt::Let(name, text, _) => Ok(LetStmt::Let(name.clone(), parse_expr_str(text)?)),
            UpdateStmt::Repeat(n, body) => Ok(LetStmt::Repeat(*n, to_let_stmts(body, offset)?)),
            UpdateStmt::For(name, lo, hi, body) => Ok(LetStmt::For(
                name.clone(),
                *lo,
                *hi,
                to_let_stmts(body, offset)?,
            )),
            UpdateStmt::Break(cond) => Ok(LetStmt::Break(match cond {
                Some(text) => Some(parse_expr_str(text)?),
                None => None,
            })),
            UpdateStmt::If(c, t, e) => Ok(LetStmt::If(
                parse_expr_str(c)?,
                parse_expr_str(t)?,
                e.as_ref().map(|x| parse_expr_str(x)).transpose()?,
            )),
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
pub(crate) fn build_loop_body<'a>(
    items: impl Iterator<Item = Pair<'a, Rule>>,
    offset: usize,
    iterations: usize,
) -> Result<Vec<UpdateStmt>> {
    let mut body: Vec<UpdateStmt> = Vec::new();
    for item in items {
        let inner = item.into_inner().next().ok_or(error(Status::Invalid, 56))?;
        match inner.as_rule() {
            Rule::let_stmt => {
                let (name, expr, ann) = parse_let_parts(inner, offset)?;
                body.push(UpdateStmt::Let(name, expr, ann));
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
pub(crate) fn strip_trailing_comment(text: &str) -> &str {
    let cut = text
        .find('#')
        .or_else(|| text.find("//"))
        .unwrap_or(text.len());
    text[..cut].trim()
}

/// A plain numeric scalar: a single number or an `a / b` fraction. Any other
/// text (operators, state-slot identifiers) is treated as an expression.
pub(crate) fn parse_scalar_number(text: &str) -> Option<f64> {
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

pub(crate) fn build_expr(pair: Pair<'_, Rule>) -> Result<Expr> {
    // `expr` wraps a single `logical_or` chain.
    let inner = pair.into_inner().next().ok_or(error(Status::Invalid, 56))?;
    build_logical_or(inner)
}

/// `a or b or c` — left-associative logical disjunction.
pub(crate) fn build_logical_or(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_logical_and(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(_op) = it.next() {
        let rhs = build_logical_and(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = Expr::Or(Box::new(acc), Box::new(rhs));
    }
    Ok(acc)
}

/// `a and b and c` — left-associative logical conjunction.
pub(crate) fn build_logical_and(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let mut acc = build_comparison(it.next().ok_or(error(Status::Invalid, 56))?)?;
    while let Some(_op) = it.next() {
        let rhs = build_comparison(it.next().ok_or(error(Status::Invalid, 56))?)?;
        acc = Expr::And(Box::new(acc), Box::new(rhs));
    }
    Ok(acc)
}

/// `a <op> b` comparisons over additive operands, left-associative.
pub(crate) fn build_comparison(pair: Pair<'_, Rule>) -> Result<Expr> {
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

pub(crate) fn build_additive(pair: Pair<'_, Rule>) -> Result<Expr> {
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

pub(crate) fn build_term(pair: Pair<'_, Rule>) -> Result<Expr> {
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

pub(crate) fn build_factor(pair: Pair<'_, Rule>) -> Result<Expr> {
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

pub(crate) fn build_call(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let name = next_pair(&mut it)?.as_str().to_string();
    let mut args = Vec::new();
    for a in it {
        args.push(build_expr(a)?);
    }
    // The `inte`/`deriv` operators are special-cased in lowering; keep their
    // names so `lower_inte_deriv` can distinguish them.
    if (name == "inte" || name == "deriv") && args.len() != 1 {
        return Err(error_at(
            Status::Invalid,
            59,
            0,
            format!("`{name}` called with the wrong number of arguments"),
        ));
    }
    // Explicit numeric casts, desugared to existing ops (no new opcodes):
    // `i64(x)` truncates toward zero; `f64(x)` is the identity; `bool(x)` is
    // `x != 0`. They make the value-kind boundaries explicit in source.
    if let Some(ty) = matches!(
        name.as_str(),
        "i64" | "i32" | "u64" | "u32" | "f64" | "bool"
    )
    .then_some(name.clone())
    {
        if args.len() != 1 {
            return Err(error_at(
                Status::Invalid,
                59,
                0,
                format!("`{name}` called with the wrong number of arguments"),
            ));
        }
        let x = args.into_iter().next().ok_or(error(Status::Invalid, 59))?;
        return Ok(match ty.as_str() {
            "f64" => x,
            "bool" => Expr::Cmp("!=", Box::new(x), Box::new(Expr::Const(0.0))),
            // trunc(x) = sign(x) * floor(abs(x))
            _ => Expr::Mul(
                Box::new(Expr::Call("sign", vec![x.clone()])),
                Box::new(Expr::Call("floor", vec![Expr::Call("abs", vec![x])])),
            ),
        });
    }
    let static_name = match name.as_str() {
        "sin" | "cos" | "exp" | "ln" | "sqrt" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            match name.as_str() {
                "sin" => "sin",
                "cos" => "cos",
                "exp" => "exp",
                "ln" => "ln",
                _ => "sqrt",
            }
        }
        "pow" => {
            if args.len() != 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "pow"
        }
        "min" | "max" => {
            if args.len() != 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            if name == "min" {
                "min"
            } else {
                "max"
            }
        }
        "if" => {
            if args.len() != 3 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "if"
        }
        "random" => {
            if !args.is_empty() {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "random"
        }
        "print" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "print"
        }
        "emit" => {
            if args.len() != 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "emit"
        }
        // RFC-0038: the current entity's activation flag.
        "active" => {
            if !args.is_empty() {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "active"
        }
        // Spatial queries: `neighbor_count(radius)` / `nearest_dist()`.
        "neighbor_count" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "neighbor_count"
        }
        "nearest_dist" => {
            if !args.is_empty() {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "nearest_dist"
        }
        // Scheduled events (discrete-event scheduling on the step grid).
        "schedule" => {
            if args.len() != 4 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "schedule"
        }
        "at" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "at"
        }
        "periodic" => {
            if args.is_empty() || args.len() > 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "periodic"
        }
        // Neighborhood aggregates / directional sensing.
        "neighbor_mean" => {
            if args.len() != 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "neighbor_mean"
        }
        "nearest_dx" | "nearest_dy" | "nearest_dz" => {
            if !args.is_empty() {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Statistical noise: `noise()` (standard normal, Box–Muller).
        "noise" => {
            if !args.is_empty() {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "noise"
        }
        // Vector helpers over scalar components.
        "vlen" => {
            if args.len() != 3 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "vlen"
        }
        "vdot" | "vdist" => {
            if args.len() != 6 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Event consumption: `last_event(kind)` reads the most recent event
        // of that kind emitted so far in this step.
        "last_event" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "last_event"
        }
        // Grid field access: `fget(f, i, j)` / `flap(f, i, j)` / `fset(f, i, j, v)`;
        // the first argument must be a literal field name.
        "fget" | "flap" => {
            // 2D `fget(f, i, j)` or 3D `fget(f, i, j, k)`.
            if !(args.len() == 3 || args.len() == 4) || !matches!(args.first(), Some(Expr::Name(_)))
            {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        "fset" => {
            // 2D `fset(f, i, j, v)` or 3D `fset(f, i, j, k, v)`.
            if !(args.len() == 4 || args.len() == 5) || !matches!(args.first(), Some(Expr::Name(_)))
            {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            "fset"
        }
        // Extended unary math builtins (1 arg).
        "abs" | "floor" | "ceil" | "round" | "sign" | "log10" | "log2" | "sinh" | "cosh"
        | "tanh" | "asin" | "acos" | "atan" => {
            if args.len() != 1 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Extended binary math builtins (2 args).
        "atan2" | "hypot" => {
            if args.len() != 2 {
                return Err(error_at(
                    Status::Invalid,
                    59,
                    0,
                    format!("`{name}` called with the wrong number of arguments"),
                ));
            }
            Box::leak(name.clone().into_boxed_str())
        }
        // Any other name is a user-defined function (resolved at lowering); its
        // existence and arity are validated there.
        _ => Box::leak(name.clone().into_boxed_str()),
    };
    Ok(Expr::Call(static_name, args))
}

pub(crate) fn build_ref(pair: Pair<'_, Rule>) -> Result<Expr> {
    let mut it = pair.into_inner();
    let name = next_pair(&mut it)?.as_str().to_string();
    let path = next_pair(&mut it)?;
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
                            // `@name.<dotted-named-state-slot>` — a struct field
                            // such as `@e.pos.x` (entity `name`, slot `pos.x`).
                            _ => {
                                let path_txt = segs.join(".");
                                let expr_name = if name == "self" {
                                    path_txt
                                } else {
                                    format!("{name}.{path_txt}")
                                };
                                return Ok(Expr::Name(expr_name));
                            }
                        },
                    }
                }
            };
            Ok(Expr::PropRef(name, kind))
        }
        _ => Err(error(Status::Invalid, 58)),
    }
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
                // Pass 1: collect `struct` types (entities may reference them in
                // any order).
                for item in section.clone().into_inner() {
                    if item.as_rule() != Rule::struct_stmt {
                        continue;
                    }
                    let mut it = item.into_inner();
                    let name = next_pair(&mut it)?.as_str().to_string();
                    let mut fields: crate::dsl::StructDef = Vec::new();
                    for f in it {
                        let mut fi = f.into_inner();
                        let fname = next_pair(&mut fi)?.as_str().to_string();
                        let rhs = next_pair(&mut fi)?;
                        let ft = if rhs.as_rule() == Rule::value {
                            crate::dsl::StructFieldType::Scalar(parse_value(rhs))
                        } else {
                            crate::dsl::StructFieldType::Struct(rhs.as_str().to_string())
                        };
                        fields.push((fname, ft));
                    }
                    model.structs.insert(name, fields);
                }
                let structs = model.structs.clone();
                for item in section.into_inner() {
                    match item.as_rule() {
                        Rule::gravity_stmt => {
                            let vec3 = next_pair(&mut item.into_inner())?;
                            model.gravity = parse_vec3(vec3);
                        }
                        Rule::title_stmt => {
                            let inner = next_pair(&mut item.into_inner())?;
                            let text = inner.as_str().trim();
                            // Strip the surrounding quotes.
                            model.title = Some(
                                text.trim_start_matches('"')
                                    .trim_end_matches('"')
                                    .to_string(),
                            );
                        }
                        Rule::lang_version_stmt => {
                            let inner = next_pair(&mut item.into_inner())?;
                            let text = inner.as_str().trim();
                            model.lang_version = Some(
                                text.trim_start_matches('"')
                                    .trim_end_matches('"')
                                    .to_string(),
                            );
                        }
                        Rule::units_stmt => {
                            let inner = next_pair(&mut item.into_inner())?;
                            model.units_strict =
                                inner.as_str().trim().trim_matches('"') == "strict";
                        }
                        Rule::chan_stmt => {
                            let mut inner = item.into_inner();
                            let name = next_pair(&mut inner)?.as_str().to_string();
                            let value = inner.next().map(parse_value).unwrap_or(0.0);
                            model.channels.push(crate::dsl::ChanDecl { name, value });
                        }
                        Rule::bond_stmt => {
                            let mut inner = item.into_inner();
                            let a = next_pair(&mut inner)?.as_str().to_string();
                            let b = next_pair(&mut inner)?.as_str().to_string();
                            let mut order = 1u8;
                            let mut polarity = 0.0f64;
                            let mut cloud = false;
                            for opt in inner {
                                let rule = opt.as_rule();
                                let v = next_pair(&mut opt.into_inner())?;
                                match rule {
                                    Rule::bond_order => {
                                        order = parse_value(v).round().clamp(1.0, 3.0) as u8;
                                    }
                                    Rule::bond_polarity => {
                                        polarity = parse_value(v).clamp(0.0, 1.0);
                                    }
                                    Rule::bond_cloud => cloud = v.as_str() == "true",
                                    _ => {}
                                }
                            }
                            model.bonds.push(crate::dsl::BondDecl {
                                a,
                                b,
                                order,
                                polarity,
                                cloud,
                            });
                        }
                        Rule::shape_stmt => {
                            // `shape <name> { part <kind> = <params> [at (x,y,z)]; }`
                            let mut it = item.into_inner();
                            let name = next_pair(&mut it)?.as_str().to_string();
                            let mut parts: Vec<crate::components::ShapePart> = Vec::new();
                            for part in it {
                                let part_rule = part.as_rule();
                                let mut pi = part.into_inner();
                                if part_rule == Rule::shape_ref {
                                    let refname = next_pair(&mut pi)?.as_str().to_string();
                                    let mut offset = (0.0, 0.0, 0.0);
                                    let mut scale = 1.0f64;
                                    for opt in pi {
                                        let rule = opt.as_rule();
                                        let inner = next_pair(&mut opt.into_inner())?;
                                        match rule {
                                            Rule::at_opt => {
                                                let o = parse_vec3(inner);
                                                offset = (o.x, o.y, o.z);
                                            }
                                            Rule::scale_opt => scale = parse_value(inner),
                                            _ => {}
                                        }
                                    }
                                    parts.push(crate::components::ShapePart {
                                        kind: 7,
                                        a: 0.0,
                                        b: 0.0,
                                        c: 0.0,
                                        offset,
                                        path: None,
                                        points: Vec::new(),
                                        faces: Vec::new(),
                                        scale,
                                        name: Some(refname),
                                        ..Default::default()
                                    });
                                    continue;
                                }
                                let kind = match next_pair(&mut pi)?.as_str() {
                                    "sphere" => 1u8,
                                    "box" => 2,
                                    "capsule" => 3,
                                    "svg" => 4,
                                    "hull" => 5,
                                    "poly" => 6,
                                    "ring" => 8,
                                    _ => 0,
                                };
                                let val = next_pair(&mut pi)?;
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
                                } else if val.as_rule() == Rule::pair {
                                    let mut it = val.into_inner();
                                    let x = it.next().map(parse_value).unwrap_or(0.0);
                                    let y = it.next().map(parse_value).unwrap_or(0.0);
                                    (x, y, 0.0)
                                } else {
                                    let x = parse_value(val);
                                    (x, x, x)
                                };
                                let mut faces: Vec<Vec<u32>> = Vec::new();
                                let mut scale = 1.0f64;
                                let mut offset = (0.0, 0.0, 0.0);
                                let mut color: Option<u32> = None;
                                let mut orbit: Option<(f64, f64, f64)> = None;
                                let mut orbit_axis: Option<(f64, f64, f64)> = None;
                                let mut spin = 0.0f64;
                                for opt in pi {
                                    let rule = opt.as_rule();
                                    if rule == Rule::part_color_opt {
                                        // `color = 0xRRGGBB` (an atomic rule).
                                        let text = opt.as_str();
                                        if let Some(pos) = text.find("0x") {
                                            color = u32::from_str_radix(text[pos + 2..].trim(), 16)
                                                .ok();
                                        }
                                        continue;
                                    }
                                    let inner = next_pair(&mut opt.into_inner())?;
                                    match rule {
                                        Rule::at_opt => {
                                            let o = parse_vec3(inner);
                                            offset = (o.x, o.y, o.z);
                                        }
                                        Rule::depth_opt => a = parse_value(inner),
                                        Rule::scale_opt => scale = parse_value(inner),
                                        Rule::orbit_opt => {
                                            let v = parse_vec3(inner);
                                            orbit = Some((v.x, v.y, v.z));
                                        }
                                        Rule::axis_opt => {
                                            let v = parse_vec3(inner);
                                            orbit_axis = Some((v.x, v.y, v.z));
                                        }
                                        Rule::spin_opt => spin = parse_value(inner),
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
                                        name: None,
                                        color,
                                        orbit,
                                        orbit_axis,
                                        spin,
                                    });
                                }
                            }
                            model.shapes.insert(name, parts);
                        }
                        Rule::field_stmt => {
                            let mut inner = item.into_inner();
                            let name = next_pair(&mut inner)?.as_str().to_string();
                            // Component identities are lowercase; reject any other
                            // field name here with a clear diagnostic.
                            if !name
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                            {
                                return Err(error(Status::Invalid, 103));
                            }
                            let mut params: std::collections::BTreeMap<String, f64> =
                                Default::default();
                            for p in inner {
                                let mut pi = p.into_inner();
                                let key = next_pair(&mut pi)?.as_str().to_string();
                                let rhs = next_pair(&mut pi)?;
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
                                        // Keep `pending_key` for the optional
                                        // trailing `unit_expr` (do not consume it).
                                        if let (Some(k), Some(v)) = (
                                            pending_key.clone(),
                                            parse_scalar_number(pair.as_str().trim()),
                                        ) {
                                            model.params.insert(k, v);
                                        }
                                    }
                                    Rule::unit_expr => {
                                        if let Some(k) = pending_key.clone() {
                                            model.param_units.insert(k, parse_unit_expr(pair)?);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Rule::entity_stmt => {
                            let mut inner = item.into_inner();
                            let name = next_pair(&mut inner)?.as_str().to_string();
                            let mut decl = EntityDecl::named(&name);
                            for field in inner {
                                apply_entity_field(field, &mut decl, &structs)?;
                            }
                            model.entities.push(decl);
                        }
                        Rule::soft_stmt => {
                            let mut inner = item.into_inner();
                            let name = next_pair(&mut inner)?.as_str().to_string();
                            let mut sd = crate::dsl::SoftDecl {
                                name,
                                nx: 8,
                                ny: 8,
                                nz: 1,
                                spacing: 1.0,
                                origin: Vec3::ZERO,
                                mass: 1.0,
                                shape: None,
                                size: None,
                            };
                            for p in inner {
                                let mut it = p.into_inner();
                                let Some(key) = it.next().map(|k| k.as_str().to_string()) else {
                                    continue;
                                };
                                let Some(rhs) = it.next() else { continue };
                                match rhs.as_rule() {
                                    Rule::vecN => {
                                        let v: Vec<f64> =
                                            rhs.into_inner().map(parse_value).collect();
                                        sd.origin = Vec3::new(
                                            v.first().copied().unwrap_or(0.0),
                                            v.get(1).copied().unwrap_or(0.0),
                                            v.get(2).copied().unwrap_or(0.0),
                                        );
                                    }
                                    Rule::ident => {
                                        if key == "shape" {
                                            sd.shape = Some(rhs.as_str().to_string());
                                        }
                                    }
                                    Rule::expr => {
                                        let text = rhs.as_str().trim();
                                        if let Some(v) = parse_scalar_number(text) {
                                            match key.as_str() {
                                                "nx" => sd.nx = v as u32,
                                                "ny" => sd.ny = v as u32,
                                                "nz" => sd.nz = v as u32,
                                                "spacing" => sd.spacing = v,
                                                "mass" => sd.mass = v,
                                                "size" => sd.size = Some(v),
                                                _ => {}
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            model.softs.push(sd);
                        }
                        Rule::pool_stmt => {
                            let mut inner = item.into_inner();
                            let name = next_pair(&mut inner)?.as_str().to_string();
                            let count = inner
                                .next()
                                .map(|n| n.as_str().parse::<u32>().unwrap_or(0))
                                .unwrap_or(0);
                            let mut decl = EntityDecl::named(&name);
                            for field in inner {
                                apply_entity_field(field, &mut decl, &structs)?;
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
                        _ => next_pair(&mut inner)?.as_str().to_string(),
                    };
                    let mut decl = SystemDecl {
                        kind,
                        params: std::collections::BTreeMap::new(),
                        vec_params: std::collections::BTreeMap::new(),
                        update: std::collections::BTreeMap::new(),
                        assigns: std::collections::BTreeMap::new(),
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
                    let name = next_pair(&mut inner)?.as_str().to_string();
                    let mut params = Vec::new();
                    let mut param_units = Vec::new();
                    let mut ret_unit = None;
                    let mut body_child: Option<Pair<'_, Rule>> = None;
                    for child in inner {
                        match child.as_rule() {
                            Rule::param_decl_list => {
                                for pd in child.into_inner() {
                                    let mut pdi = pd.into_inner();
                                    params.push(next_pair(&mut pdi)?.as_str().to_string());
                                    let u = match pdi.next() {
                                        Some(ue) if ue.as_rule() == Rule::unit_expr => {
                                            Some(parse_unit_expr(ue)?)
                                        }
                                        _ => None,
                                    };
                                    param_units.push(u);
                                }
                            }
                            Rule::unit_expr => ret_unit = Some(parse_unit_expr(child)?),
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
                                        let (lname, text, ann) =
                                            parse_let_parts(children[i].clone(), 0)?;
                                        stmts.push(UpdateStmt::Let(lname, text, ann));
                                        i += 1;
                                    }
                                    Rule::repeat_stmt | Rule::for_stmt => {
                                        stmts.push(build_loop_stmt(children[i].clone(), 0)?);
                                        i += 1;
                                    }
                                    Rule::if_stmt => {
                                        let mut it = children[i].clone().into_inner();
                                        let _if_kw = it.next(); // `if` keyword token
                                        let cond = next_pair(&mut it)?.as_str().trim().to_string();
                                        let _rk = it.next(); // return_kw (transparent)
                                        let then_e =
                                            next_pair(&mut it)?.as_str().trim().to_string();
                                        let mut else_e = None;
                                        if it.next().is_some() {
                                            // else_kw, return_kw, expr
                                            let _rk2 = it.next();
                                            else_e =
                                                it.next().map(|e| e.as_str().trim().to_string());
                                        }
                                        stmts.push(UpdateStmt::If(cond, then_e, else_e));
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
                            // A trailing `func_return` supplies the body; a terminal
                            // control-flow `if` supplies it via its branch returns, so
                            // the fall-through body is a dummy (a no-`else` terminal `if`
                            // returns 0.0 on its false path).
                            let body = match body {
                                Some(e) => e,
                                // A terminal control-flow `if` supplies the result
                                // via its branch returns; the fall-through is a
                                // dummy (a no-`else` terminal `if` returns 0.0).
                                None => match stmts.last() {
                                    Some(UpdateStmt::If(..)) => Expr::Const(0.0),
                                    _ => return Err(error(Status::Invalid, 55)),
                                },
                            };
                            (stmts, body)
                        }
                        _ => return Err(error(Status::Invalid, 55)),
                    };
                    funcs.push(FuncDecl {
                        name,
                        namespace: String::new(),
                        params,
                        param_units,
                        ret_unit,
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
