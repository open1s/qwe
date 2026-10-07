//! Expression lowering to EIR: the `LowerCtx`/`LowerParts` contexts, the
//! `Expr -> Instruction` lowerer, `let`/loop block lowering, and the shared
//! register/EIR emission helpers used by every system.
use super::*;

pub(crate) struct LowerCtx<'a> {
    pub(crate) slot_regs: &'a [u32],
    pub(crate) ref_regs: &'a std::collections::BTreeMap<(u128, usize), u32>,
    pub(crate) prop_regs: &'a std::collections::BTreeMap<(u128, PropKind), u32>,
    pub(crate) entity_map: &'a std::collections::BTreeMap<String, u128>,
    /// Named state slot -> index (from `state = (x = 0, …)`).
    pub(crate) state_names: &'a std::collections::BTreeMap<String, usize>,
    /// Per-entity named state layouts, for resolving `@name.x` cross-entity.
    pub(crate) state_names_by_id:
        &'a std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    /// `let` local name -> register id.
    pub(crate) locals: &'a std::collections::BTreeMap<String, u32>,
    /// Names of integer-annotated `let` locals (RFC-0043): their register holds
    /// an exact `I64`; a read outside integer context widens via `I64ToF64`.
    /// `None` at direct-construction sites (no integer locals in scope).
    pub(crate) int_locals:
        Option<std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>>>,
    /// Whether the expression currently being lowered is in integer context
    /// (an integer-annotated `let` RHS): integer literals stay `I64` and
    /// no `I64 -> F64` widening is applied.
    pub(crate) int_ctx: bool,
    /// User-defined function name -> EIR function id (for `CALL`).
    pub(crate) func_ids: &'a std::collections::BTreeMap<String, u64>,
    /// Grid field name -> width (compile-time, from the model), for
    /// `fget`/`fset`/`flap` cell access.
    pub(crate) field_dims: &'a std::collections::BTreeMap<String, (u32, u32)>,
    /// The module namespace of the system being lowered; unqualified function
    /// and parameter references resolve within it first.
    pub(crate) namespace: &'a str,
    /// Every declared parameter name (qualified), for `namespace` fallback.
    pub(crate) params: &'a std::collections::BTreeSet<String>,
    /// The entity whose rule is being lowered; spatial queries
    /// (`neighbor_count`/`nearest_dist`) target it. 0 in function bodies,
    /// where queries are rejected at compile time.
    pub(crate) current_entity: u128,
}

/// RFC-0048 slice C2: a stable 64-bit key for a resource name. The resource's
/// capacity/busy live in `ExecEnv.resources` keyed by this id, so the id must
/// be identical across the interpreter and both compilation paths — a plain
/// FNV-1a of the name is deterministic and does not depend on the scene layout.
/// The high bit is set so a resource id can never collide with the low ids the
/// calendar uses for `seq`-only state.
pub(crate) fn resource_id(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    0x8000_0000_0000_0000 | (h & 0x7fff_ffff_ffff_ffff)
}

/// Resolves an RFC-0044 array name to `(base_slot, length)` from the entity's
/// named layout (`name.0 … name.{len-1}`). `None` when `name` is not an array.
pub(crate) fn resolve_array(ctx: &LowerCtx<'_>, name: &str) -> Option<(usize, usize)> {
    let base = *ctx.state_names.get(&format!("{name}.0"))?;
    let mut len = 1usize;
    while ctx.state_names.contains_key(&format!("{name}.{len}")) {
        len += 1;
    }
    Some((base, len))
}

/// RFC-0044 follow-up: the register of each element of a named array, or `None`
/// when `name` is not an array on the current entity's layout (a detail-109
/// diagnostic is recorded). Used by the `sum`/`mean`/`dot`/… reductions, which
/// unroll over the compile-time length.
pub(crate) fn array_element_regs(
    ctx: &LowerCtx<'_>,
    name: &str,
    out: &mut Vec<crate::eir::Instruction>,
) -> Option<Vec<u32>> {
    let (base, len) = resolve_array(ctx, name)?;
    let mut regs = Vec::with_capacity(len);
    for k in 0..len {
        // A runtime-indexed read of a *constant* index is just the static slot;
        // read it through a slot register so the reduction sees the same value
        // the equivalent `name[k]` would.
        let r = *ctx.slot_regs.get(base + k)?;
        regs.push(r);
    }
    let _ = out;
    Some(regs)
}

/// The compile-time length of a named array on the current layout, or 0.
pub(crate) fn len_of(ctx: &LowerCtx<'_>, name: &str) -> usize {
    resolve_array(ctx, name).map(|(_, len)| len).unwrap_or(0)
}

/// RFC-0044: absolute-slot register for a runtime write index (`dyn_rules` /
/// `dyn_assigns`). A named array index (`name[j]`) is bound-checked against the
/// array's declared length against the *target entity's* layout; a raw `s[j]`
/// has no declared length and stays unchecked.
pub(crate) fn lower_dyn_index(
    idx: &super::ast::DynIndex,
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> Option<u32> {
    use crate::eir::{Immediate, Opcode, ValueType};
    match idx {
        super::ast::DynIndex::Slot(e) => Some(lower_expr_f64(e, ctx, next_id, out)),
        super::ast::DynIndex::Array(name, j) => {
            let (base, len) = resolve_array(ctx, name)?;
            let ri = lower_expr_f64(j, ctx, next_id, out);
            let base_reg = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                Opcode::Const,
                base_reg,
                Some(ValueType::F64),
                vec![],
                Some(Immediate::F64(base as f64)),
                None,
            ));
            let len_reg = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                Opcode::Const,
                len_reg,
                Some(ValueType::F64),
                vec![],
                Some(Immediate::F64(len as f64)),
                None,
            ));
            let checked = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                Opcode::BoundsCheck,
                checked,
                Some(ValueType::F64),
                vec![ri, base_reg, len_reg],
                None,
                None,
            ));
            Some(checked)
        }
    }
}

/// A non-negative integer constant index, if the expression is one.
pub(crate) fn const_index(e: &Expr) -> Option<i64> {
    match e {
        Expr::Const(c) if c.is_finite() && c.fract() == 0.0 => Some(*c as i64),
        Expr::Int(v) => Some(*v),
        _ => None,
    }
}

/// Lowers an `Expr` into EIR instructions, returning the result register id.
/// Lowers the `inte(E)` / `deriv(E)` operators.
///
/// `inte(E)` is the integration increment `dt · E`. `deriv(E)` is the backward
/// difference `(E − E_prev)/dt`, where `E_prev` is the value `E` took at the
/// previous (sub)step, remembered per call site in the runtime's history
/// (`HistRead`/`HistWrite`). On the first step (no history) `deriv` is 0.
pub(crate) fn lower_inte_deriv(
    name: &str,
    args: &[Expr],
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    use crate::eir::{Immediate, Opcode, ValueType};
    let e = lower_expr(&args[0], ctx, next_id, out);
    let dt_reg = match ctx.locals.get("dt").copied() {
        Some(r) => r,
        None => const_reg(1.0, next_id, out),
    };
    if name == "inte" {
        return binary(Opcode::Mul, e, dt_reg, next_id, out);
    }
    // deriv(E) = (E - prev)/dt, with the site's previous value remembered.
    // `has` gates the difference so the first (sub)step yields 0.
    let site = out.iter().filter(|i| i.opcode == Opcode::HistRead).count() as u64;
    let raw = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::HistRead,
        raw,
        Some(ValueType::F64),
        vec![],
        Some(Immediate::U64(site)),
        None,
    ));
    let has = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::HistHas,
        has,
        Some(ValueType::F64),
        vec![],
        Some(Immediate::U64(site)),
        None,
    ));
    let diff = binary(Opcode::Sub, e, raw, next_id, out);
    let gated = binary(Opcode::Mul, diff, has, next_id, out);
    let res = binary(Opcode::Div, gated, dt_reg, next_id, out);
    // Remember this (sub)step's E for the next one.
    out.push(crate::physics_eir::instr(
        Opcode::HistWrite,
        0,
        None,
        vec![e],
        Some(Immediate::U64(site)),
        None,
    ));
    res
}

/// RFC-0048 zero-crossing operators: `cross(e)`, `rise(e)`, `fall(e)`, and
/// `last_cross(e)`.
///
/// The site's previous value and the last-cross timestamp live in the runtime's
/// per-call-site history (`ExecEnv.hist`/`cross_time`), exactly like
/// `deriv(E)`, so no user state slot is consumed and each system × entity is
/// automatically its own site (each function gets a fresh `out`). The site id
/// is the count of `HistRead`s emitted so far: `deriv` uses the same counter,
/// and every operator here emits exactly one `HistRead`, so ids are unique
/// within a function and never collide with `deriv`.
///
/// The edge operators take `[prev, cur, has]` and fire only when history exists
/// (`has`), so the first (sub)step can never report a crossing — the same rule
/// `deriv` uses. `last_cross(e)` detects the same strict sign change and
/// returns the simulation time of the most recent one (0.0 before any).
pub(crate) fn lower_zero_crossing(
    name: &str,
    args: &[Expr],
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    use crate::eir::{Immediate, Opcode, ValueType};
    // Lower the argument FIRST, then take the site id: a nested history operator
    // inside `e` (e.g. `cross(deriv(x))`) must claim its own site first, or
    // the two would share a key (issue #95). `deriv` orders it the same way.
    let e = lower_expr_f64(&args[0], ctx, next_id, out);
    let site = out.iter().filter(|i| i.opcode == Opcode::HistRead).count() as u64;
    // prev = history[site] (0.0 before the first step), has = history exists.
    let prev = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::HistRead,
        prev,
        Some(ValueType::F64),
        vec![],
        Some(Immediate::U64(site)),
        None,
    ));
    let has = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::HistHas,
        has,
        Some(ValueType::F64),
        vec![],
        Some(Immediate::U64(site)),
        None,
    ));
    // Remember this (sub)step's value for the next one.
    out.push(crate::physics_eir::instr(
        Opcode::HistWrite,
        0,
        None,
        vec![e],
        Some(Immediate::U64(site)),
        None,
    ));
    // `last_cross` reports *when* the (strict) crossing happened; it detects the
    // edge (which timestamps the site) and then reads the timestamp back.
    let op = match name {
        "rise" => Opcode::RiseEdge,
        "fall" => Opcode::FallEdge,
        _ => Opcode::CrossDown,
    };
    let edge = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        op,
        edge,
        Some(ValueType::F64),
        vec![prev, e, has],
        Some(Immediate::U64(site)),
        None,
    ));
    if name != "last_cross" {
        return edge;
    }
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::LastCross,
        r,
        Some(ValueType::F64),
        vec![],
        Some(Immediate::U64(site)),
        None,
    ));
    r
}

pub(crate) fn lower_expr(
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
        // RFC-0043: an exact integer literal lowers to an `I64` register with
        // integer semantics.
        Expr::Int(v) => {
            let r = *next_id;
            *next_id += 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::Const,
                r,
                Some(crate::eir::ValueType::I64),
                vec![],
                Some(crate::eir::Immediate::I64(*v)),
                None,
            ));
            // RFC-0043: the literal stays exact; a mixed-kind operation or an
            // f64-required boundary widens it (`as_f64`), so `2 * pi` still
            // evaluates in f64 and `7 / 2` stays integer.
            r
        }
        Expr::Slot(i) => ctx.slot_regs.get(*i).copied().unwrap_or(0),
        Expr::SlotDyn(idx) => {
            // `s[i]`: read the State slot at a runtime index via the EIR.
            let ri = lower_expr_f64(idx, ctx, next_id, out);
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
        Expr::Index(name, idx) => {
            // RFC-0044 typed array `name[i]`: resolve the base and length from
            // the entity's named layout, then a constant index becomes a static
            // slot read and a runtime index becomes `ReadSlotDyn`.
            match resolve_array(ctx, name) {
                Some((base, len)) => {
                    if let Some(k) = const_index(idx) {
                        let k = k as usize;
                        if k >= len {
                            push_diag(
                                52,
                                0,
                                format!(
                                    "array `{name}[{k}]` is out of range (length {len}) — reads 0.0"
                                ),
                            );
                            return 0;
                        }
                        ctx.slot_regs.get(base + k).copied().unwrap_or(0)
                    } else {
                        let ri = lower_expr_f64(idx, ctx, next_id, out);
                        let base_reg = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Const,
                            base_reg,
                            Some(crate::eir::ValueType::F64),
                            vec![],
                            Some(crate::eir::Immediate::F64(base as f64)),
                            None,
                        ));
                        let len_reg = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Const,
                            len_reg,
                            Some(crate::eir::ValueType::F64),
                            vec![],
                            Some(crate::eir::Immediate::F64(len as f64)),
                            None,
                        ));
                        // RFC-0044: a runtime index is bound-checked before the
                        // slot is touched; out-of-range traps (detail 18)
                        // instead of reading an arbitrary slot.
                        let checked = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::BoundsCheck,
                            checked,
                            Some(crate::eir::ValueType::F64),
                            vec![ri, base_reg, len_reg],
                            None,
                            None,
                        ));
                        let dyn_idx = checked;
                        let out_reg = *next_id;
                        *next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::ReadSlotDyn,
                            out_reg,
                            Some(crate::eir::ValueType::F64),
                            vec![dyn_idx],
                            None,
                            Some(crate::physics_eir::cr(
                                ctx.current_entity,
                                crate::physics_eir::state_id(),
                                0,
                            )),
                        ));
                        out_reg
                    }
                }
                None => {
                    push_diag(
                        109,
                        0,
                        format!("unknown array `{name}` in `{name}[…]` — reads 0.0"),
                    );
                    0
                }
            }
        }
        Expr::Neg(x) => {
            // Negation of an exact integer stays integer (RFC-0043).
            let rx = lower_expr(x, ctx, next_id, out);
            let empty: ExprIntLocals =
                std::rc::Rc::new(std::cell::RefCell::new(std::collections::BTreeSet::new()));
            let int_locs = ctx.int_locals.clone().unwrap_or(empty);
            if expr_kind(x, &int_locs) == Kind::Int {
                return binary_typed(
                    crate::eir::Opcode::Sub,
                    const_i64_reg(0, next_id, out),
                    rx,
                    crate::eir::ValueType::I64,
                    next_id,
                    out,
                );
            }
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
            binary_typed(
                crate::eir::Opcode::Mul,
                rx,
                neg_one,
                crate::eir::ValueType::F64,
                next_id,
                out,
            )
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
            if let Some(&reg) = ctx.locals.get(name.as_str()) {
                // RFC-0043: an integer-annotated local used where a number is
                // required (outside integer context) widens to f64.
                return reg;
            }
            // Own named state slot — including dotted struct fields (`pos.x`).
            // A dotted name not in this entity's layout (e.g. `a.x`) falls
            // through to parameters / cross-entity handling below.
            if let Some(slot) = ctx.state_names.get(name.as_str()).copied() {
                return ctx.slot_regs.get(slot).copied().unwrap_or(0);
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
            // A dotted name that is not a parameter is a cross-entity reference
            // (`@name.slot`) whose leading segment must be a declared entity.
            if let Some((ent, rest)) = name.split_once('.') {
                let slot_name = rest.strip_prefix("state.").unwrap_or(rest);
                let Some(&id) = ctx.entity_map.get(ent) else {
                    push_diag(
                        85,
                        0,
                        format!("unknown entity `{ent}` in `{name}` — reads 0.0"),
                    );
                    return 0;
                };
                let slot = ctx
                    .state_names_by_id
                    .get(&id)
                    .and_then(|m| m.get(slot_name))
                    .copied()
                    .unwrap_or(0);
                return ctx.ref_regs.get(&(id, slot)).copied().unwrap_or(0);
            }
            // An undeclared bare name: read 0.0, but record a diagnostic (a typo
            // must not silently do nothing).
            push_diag(
                85,
                0,
                format!("unknown identifier `{name}` — reads 0.0 (typo?)"),
            );
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
        // RFC-0043 kind-driven binary arithmetic: `Int op Int` keeps exact
        // integer semantics (`/`/`%` truncate toward zero); any other pair
        // widens both operands to f64, exactly as before RFC-0043.
        Expr::Add(a, b) => arith_rule(Expr::Add(a.clone(), b.clone()), ctx, next_id, out),
        Expr::Sub(a, b) => arith_rule(Expr::Sub(a.clone(), b.clone()), ctx, next_id, out),
        Expr::Mul(a, b) => arith_rule(Expr::Mul(a.clone(), b.clone()), ctx, next_id, out),
        Expr::Div(a, b) => arith_rule(Expr::Div(a.clone(), b.clone()), ctx, next_id, out),
        Expr::Rem(a, b) => arith_rule(Expr::Rem(a.clone(), b.clone()), ctx, next_id, out),
        Expr::Cmp(op, a, b) => {
            // Compute the boolean comparison, then Select(cond, 1.0, 0.0).
            // Operands are f64 (RFC-0043: an integer operand widens first).
            let ra = lower_expr_f64(a, ctx, next_id, out);
            let rb = lower_expr_f64(b, ctx, next_id, out);
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
            let ra = lower_expr_f64(a, ctx, next_id, out);
            let rb = lower_expr_f64(b, ctx, next_id, out);
            let na = truthy(ra, next_id, out);
            let nb = truthy(rb, next_id, out);
            binary(crate::eir::Opcode::Mul, na, nb, next_id, out)
        }
        Expr::Or(a, b) => {
            // bool(a) OR bool(b) via de Morgan: 1 - (1-na)*(1-nb).
            let ra = lower_expr_f64(a, ctx, next_id, out);
            let rb = lower_expr_f64(b, ctx, next_id, out);
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
            let ra = lower_expr_f64(a, ctx, next_id, out);
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
            if *name == "inte" || *name == "deriv" {
                return lower_inte_deriv(name, args, ctx, next_id, out);
            }
            // RFC-0048: runtime-owned zero-crossing detection.
            if *name == "cross" || *name == "rise" || *name == "fall" || *name == "last_cross" {
                return lower_zero_crossing(name, args, ctx, next_id, out);
            }
            // RFC-0043 explicit casts. `__i64_to_f64` widens an exact integer
            // (a no-op on an already-f64 value); the integer casts truncate
            // toward zero via `F64ToI64`.
            if *name == "__i64_to_f64" {
                // `f64(x)` is the widening opcode for an exact-integer operand
                // and the identity for one that is already an f64 value.
                let int_locs = ctx.int_locals.clone().unwrap_or_else(|| {
                    std::rc::Rc::new(std::cell::RefCell::new(Default::default()))
                });
                let is_int = expr_kind(&args[0], &int_locs) == Kind::Int;
                let a = lower_expr(&args[0], ctx, next_id, out);
                return if is_int {
                    i64_to_f64(a, next_id, out)
                } else {
                    a
                };
            }
            if *name == "__f64_to_i64" {
                let a = lower_expr(&args[0], ctx, next_id, out);
                let i = f64_to_i64(a, next_id, out);
                // Outside integer context the cast is only a truncation step;
                // the value flows on as a number.
                return if ctx.int_ctx {
                    i
                } else {
                    i64_to_f64(i, next_id, out)
                };
            }
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
                    // User functions take f64 parameters (RFC-0043: integer
                    // arguments are widened at the call boundary).
                    operands.push(lower_expr_f64(a, ctx, next_id, out));
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
                    // The condition is truthiness (any type); the selected
                    // values are f64 (RFC-0043: integer branches widen).
                    let cond = lower_expr(&args[0], ctx, next_id, out);
                    let a = lower_expr_f64(&args[1], ctx, next_id, out);
                    let b = lower_expr_f64(&args[2], ctx, next_id, out);
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
                    let a = lower_expr_f64(&args[0], ctx, next_id, out);
                    let b = lower_expr_f64(&args[1], ctx, next_id, out);
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
                    // The event queue carries f64 fields (RFC-0043: widen).
                    let kind = lower_expr_f64(&args[0], ctx, next_id, out);
                    let payload = lower_expr_f64(&args[1], ctx, next_id, out);
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
                // RFC-0048 slice B: the event calendar as a first-class value.
                // `event_count`/`next_event_*` read the pending queue with no
                // side effect; `pop_event` removes the earliest entry. All are
                // deterministic reads of the execution context's calendar.
                "event_count"
                | "next_event_time"
                | "next_event_kind"
                | "next_event_payload"
                | "next_event_priority"
                | "pop_event" => {
                    let op = match *name {
                        "event_count" => crate::eir::Opcode::EventCount,
                        "next_event_time" => crate::eir::Opcode::NextEventTime,
                        "next_event_kind" => crate::eir::Opcode::NextEventKind,
                        "next_event_payload" => crate::eir::Opcode::NextEventPayload,
                        "next_event_priority" => crate::eir::Opcode::NextEventPriority,
                        _ => crate::eir::Opcode::PopEvent,
                    };
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        op,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        None,
                    ));
                    out_reg
                }
                "events_seen" => {
                    // Count of events already delivered (drained) this step whose
                    // kind matches the argument.
                    let kind = lower_expr_f64(&args[0], ctx, next_id, out);
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::EventSeenCount,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![kind],
                        None,
                        None,
                    ));
                    out_reg
                }
                "at" | "periodic" | "schedule" | "schedule_at" => {
                    // Scheduled events: `at`/`periodic` probe the step's time
                    // window; `schedule(gate, delay, kind, payload)` pushes an
                    // event into the queue when `gate` is nonzero;
                    // `schedule_at(gate, delay, kind, payload, priority)` also
                    // sets the entry's queue priority.
                    let op = match *name {
                        "at" => crate::eir::Opcode::FiredAt,
                        "periodic" => crate::eir::Opcode::FiredEvery,
                        "schedule_at" => crate::eir::Opcode::ScheduleEventAt,
                        _ => crate::eir::Opcode::ScheduleEvent,
                    };
                    let operands: Vec<u32> = args
                        .iter()
                        .map(|a| lower_expr_f64(a, ctx, next_id, out))
                        .collect();
                    // `schedule`/`schedule_at` yield nothing (void opcodes) —
                    // emit with no result so the validator accepts them; their
                    // value is discarded.
                    if matches!(
                        op,
                        crate::eir::Opcode::ScheduleEvent | crate::eir::Opcode::ScheduleEventAt
                    ) {
                        out.push(crate::physics_eir::instr(op, 0, None, operands, None, None));
                        return 0;
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
                        .map(|a| lower_expr_f64(a, ctx, next_id, out))
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
                        .map(|a| lower_expr_f64(a, ctx, next_id, out))
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
                // RFC-0048 slice C2: capacity-gated resources.
                // `seize(r, cap)` / `release(r)` / `resource_busy(r)` /
                // `resource_capacity(r)`. The first argument is a literal
                // resource name; its id keys `ExecEnv.resources` (execution-
                // context state, parallel to the event calendar). Busy/capacity
                // are not stored in the world, so a `step_cross` comparison of
                // the maps is what guarantees interpreter ≡ JIT.
                "seize" | "release" | "resource_busy" | "resource_capacity" => {
                    let Expr::Name(rname) = &args[0] else {
                        // The parse requires a literal resource name.
                        return 0;
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let rid = resource_id(rname);
                    let (op, operand) = match *name {
                        "release" => (crate::eir::Opcode::ReleaseResource, None),
                        "resource_busy" => (crate::eir::Opcode::ResourceBusy, None),
                        "resource_capacity" => (crate::eir::Opcode::ResourceCapacity, None),
                        _ => {
                            let cap = lower_expr_f64(&args[1], ctx, next_id, out);
                            (crate::eir::Opcode::SeizeResource, Some(cap))
                        }
                    };
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        op,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        operand.into_iter().collect(),
                        Some(crate::eir::Immediate::U64(rid)),
                        None,
                    ));
                    out_reg
                }
                // RFC-0049: deterministic time scaling. `time_scale()` reads the
                // scale governing the current step (the step-start snapshot) and
                // `step_dt()` the effective step length — both are world
                // pseudo-component reads (`pwe.time.scale` / `pwe.time.step_dt`),
                // so they stay eligible for the JIT/AOT and threaded dispatchers,
                // exactly like `t`. `set_time_scale(x)` requests a new scale for
                // the **next** step (finite, non-negative; clamped, invalid
                // requests ignored) and yields the **applied** scale. Only the
                // control write is an opcode; `step_cross` compares the resulting
                // execution-context state.
                "time_scale" | "step_dt" => {
                    let component = if *name == "time_scale" {
                        crate::physics_eir::time_scale_id()
                    } else {
                        crate::physics_eir::step_dt_id()
                    };
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::ReadView,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![],
                        None,
                        Some(crate::physics_eir::cr(0, component, 0)),
                    ));
                    out_reg
                }
                "set_time_scale" => {
                    let x = lower_expr_f64(&args[0], ctx, next_id, out);
                    let out_reg = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        crate::eir::Opcode::SetTimeScale,
                        out_reg,
                        Some(crate::eir::ValueType::F64),
                        vec![x],
                        None,
                        None,
                    ));
                    out_reg
                }
                // RFC-0044 follow-up: array reductions over a named array. The
                // length is compile-time, so these unroll to the existing
                // arithmetic/comparison opcodes (no new opcode, no runtime
                // support needed, and they stay eligible for every backend).
                "sum" | "mean" | "norm" | "asum" | "prod" | "min_of" | "max_of" => {
                    let Expr::Name(aname) = &args[0] else {
                        return 0;
                    };
                    let Some(elems) = array_element_regs(ctx, aname, out) else {
                        push_diag(
                            109,
                            0,
                            format!("array `{aname}` is not in this entity's state"),
                        );
                        return 0;
                    };
                    match *name {
                        "prod" => {
                            let mut acc = const_reg(1.0, next_id, out);
                            for e in elems {
                                acc = binary(crate::eir::Opcode::Mul, acc, e, next_id, out);
                            }
                            acc
                        }
                        "min_of" | "max_of" => {
                            let cmp = if *name == "min_of" {
                                crate::eir::Opcode::Lt
                            } else {
                                crate::eir::Opcode::Gt
                            };
                            let mut acc = elems[0];
                            for e in elems.into_iter().skip(1) {
                                let b = *next_id;
                                *next_id += 1;
                                out.push(crate::physics_eir::instr(
                                    cmp,
                                    b,
                                    Some(crate::eir::ValueType::Bool),
                                    vec![e, acc],
                                    None,
                                    None,
                                ));
                                let sel = *next_id;
                                *next_id += 1;
                                out.push(crate::physics_eir::instr(
                                    crate::eir::Opcode::Select,
                                    sel,
                                    Some(crate::eir::ValueType::F64),
                                    vec![b, e, acc],
                                    None,
                                    None,
                                ));
                                acc = sel;
                            }
                            acc
                        }
                        "norm" => {
                            let mut acc = const_reg(0.0, next_id, out);
                            for e in elems {
                                let sq = binary(crate::eir::Opcode::Mul, e, e, next_id, out);
                                acc = binary(crate::eir::Opcode::Add, acc, sq, next_id, out);
                            }
                            unary(crate::eir::Opcode::Sqrt, acc, next_id, out)
                        }
                        "asum" => {
                            let mut acc = const_reg(0.0, next_id, out);
                            for e in elems {
                                let a = unary(crate::eir::Opcode::Abs, e, next_id, out);
                                acc = binary(crate::eir::Opcode::Add, acc, a, next_id, out);
                            }
                            acc
                        }
                        _ => {
                            // `sum` and `mean` share the sum; `mean` divides by
                            // the (compile-time) length.
                            let mut acc = const_reg(0.0, next_id, out);
                            for e in elems {
                                acc = binary(crate::eir::Opcode::Add, acc, e, next_id, out);
                            }
                            if *name == "mean" {
                                let len = const_reg(len_of(ctx, aname) as f64, next_id, out);
                                acc = binary(crate::eir::Opcode::Div, acc, len, next_id, out);
                            }
                            acc
                        }
                    }
                }
                "dot" => {
                    let (Expr::Name(a), Expr::Name(b)) = (&args[0], &args[1]) else {
                        return 0;
                    };
                    let (Some(ea), Some(eb)) = (
                        array_element_regs(ctx, a, out),
                        array_element_regs(ctx, b, out),
                    ) else {
                        push_diag(109, 0, "`dot` needs two declared arrays");
                        return 0;
                    };
                    if ea.len() != eb.len() {
                        push_diag(
                            52,
                            0,
                            format!(
                                "`dot({a}, {b})` needs equal lengths ({} vs {})",
                                ea.len(),
                                eb.len()
                            ),
                        );
                        return 0;
                    }
                    let mut acc = const_reg(0.0, next_id, out);
                    for (x, y) in ea.into_iter().zip(eb) {
                        let p = binary(crate::eir::Opcode::Mul, x, y, next_id, out);
                        acc = binary(crate::eir::Opcode::Add, acc, p, next_id, out);
                    }
                    acc
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
                    let ri = lower_expr_f64(&args[1], ctx, next_id, out);
                    let rj = lower_expr_f64(&args[2], ctx, next_id, out);
                    let rj = if three_d {
                        let rk = lower_expr_f64(&args[3], ctx, next_id, out);
                        let h = const_reg(height as f64, next_id, out);
                        let kt = binary(crate::eir::Opcode::Mul, rk, h, next_id, out);
                        binary(crate::eir::Opcode::Add, rj, kt, next_id, out)
                    } else {
                        rj
                    };
                    if is_fset {
                        let rv = lower_expr_f64(&args[idx_args], ctx, next_id, out);
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
                        // Elementary/binary builtins (`sin`, `hypot`, …) are f64
                        // functions (RFC-0043: widen integer arguments).
                        operands.push(lower_expr_f64(a, ctx, next_id, out));
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
/// Maps a cross-entity property to its (component id, byte offset).
pub(crate) fn prop_component(kind: PropKind) -> (pwe_api::ComponentTypeId, u32) {
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
pub(crate) fn collect_refs(
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
        // A typed-array read is an own-slot read by name; only its index can
        // carry cross-entity references.
        Expr::Index(_, idx) => {
            collect_refs(idx, out, props, named_refs, entity_map, state_names_by_id)
        }
        Expr::Const(_) | Expr::Slot(_) | Expr::Time => {}
        Expr::Int(_) => {}
    }
}

/// An `I64` constant register (RFC-0043 exact-integer lowering).
pub(crate) fn const_i64_reg(
    value: i64,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Const,
        r,
        Some(crate::eir::ValueType::I64),
        vec![],
        Some(crate::eir::Immediate::I64(value)),
        None,
    ));
    r
}

/// Emits a `Const` instruction and returns its result register.
pub(crate) fn const_reg(
    value: f64,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
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
pub(crate) fn const_u64_reg(
    value: u64,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
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

/// The surface value kind of an expression (RFC-0043). Only `Int` and `Float`
/// are needed for the arithmetic rule; comparisons/logicals are handled
/// separately and every unknown operand coerces to f64.
fn expr_kind(e: &Expr, int_locals: &ExprIntLocals) -> Kind {
    match e {
        Expr::Int(_) => Kind::Int,
        Expr::Name(n) => {
            if int_locals.borrow().contains(n) {
                Kind::Int
            } else {
                Kind::Float
            }
        }
        Expr::Neg(a) => expr_kind(a, int_locals),
        // `Int op Int` is exact integer (RFC-0043); any other pair is f64.
        Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Rem(a, b) => {
            if expr_kind(a, int_locals) == Kind::Int && expr_kind(b, int_locals) == Kind::Int {
                Kind::Int
            } else {
                Kind::Float
            }
        }
        // A runtime slot read, parameter, time, call, cast result, comparison,
        // or anything else is an f64 value.
        _ => Kind::Float,
    }
}

/// RFC-0043 value kinds used by the arithmetic rule.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Int,
    Float,
}

/// Shared set of in-scope integer-annotated `let` names (RFC-0043).
pub(crate) type ExprIntLocals = std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>>;

/// Lowers `a op b` with RFC-0043 kind semantics: `Int op Int` stays exact
/// (`I64` operands, truncating `/`/`%`); any other pair coerces each integer
/// operand to f64 and computes in f64, preserving pre-RFC-0043 behavior.
fn arith_rule(
    e: Expr,
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let (op, a, b) = match e {
        Expr::Add(a, b) => (crate::eir::Opcode::Add, a, b),
        Expr::Sub(a, b) => (crate::eir::Opcode::Sub, a, b),
        Expr::Mul(a, b) => (crate::eir::Opcode::Mul, a, b),
        Expr::Div(a, b) => (crate::eir::Opcode::Div, a, b),
        Expr::Rem(a, b) => (crate::eir::Opcode::Rem, a, b),
        _ => unreachable!(),
    };
    let empty: ExprIntLocals =
        std::rc::Rc::new(std::cell::RefCell::new(std::collections::BTreeSet::new()));
    let int_locs = ctx.int_locals.clone().unwrap_or(empty);
    let a_int = expr_kind(&a, &int_locs) == Kind::Int;
    let b_int = expr_kind(&b, &int_locs) == Kind::Int;
    if a_int && b_int {
        let ra = lower_expr(&a, ctx, next_id, out);
        let rb = lower_expr(&b, ctx, next_id, out);
        return binary_typed(op, ra, rb, crate::eir::ValueType::I64, next_id, out);
    }
    let ra = lower_expr(&a, ctx, next_id, out);
    let ra = if a_int {
        i64_to_f64(ra, next_id, out)
    } else {
        ra
    };
    let rb = lower_expr(&b, ctx, next_id, out);
    let rb = if b_int {
        i64_to_f64(rb, next_id, out)
    } else {
        rb
    };
    binary_typed(op, ra, rb, crate::eir::ValueType::F64, next_id, out)
}

/// Whether an expression already yields an exact integer (RFC-0043), so an
/// integer-annotated `let` needs no `F64ToI64` coercion. Integer literals,
/// integer-typed locals, and arithmetic over them qualify.
fn expr_is_integer(
    e: &Expr,
    int_locals: &std::cell::RefCell<std::collections::BTreeSet<String>>,
) -> bool {
    match e {
        Expr::Int(_) => true,
        Expr::Name(n) => int_locals.borrow().contains(n),
        Expr::Neg(a) => expr_is_integer(a, int_locals),
        Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Rem(a, b) => {
            expr_is_integer(a, int_locals) && expr_is_integer(b, int_locals)
        }
        _ => false,
    }
}

/// Lowers an expression that must produce an f64 value (a state-slot write, a
/// comparison operand, an f64 parameter, …): an exact-integer expression is
/// widened with `I64ToF64` (RFC-0043's implicit coercion), everything else is
/// lowered unchanged.
pub(crate) fn lower_expr_f64(
    e: &Expr,
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let int_locs = ctx
        .int_locals
        .clone()
        .unwrap_or_else(|| std::rc::Rc::new(std::cell::RefCell::new(Default::default())));
    let is_int = expr_kind(e, &int_locs) == Kind::Int;
    let r = lower_expr(e, ctx, next_id, out);
    if is_int {
        i64_to_f64(r, next_id, out)
    } else {
        r
    }
}

/// RFC-0043 `I64ToF64`: widens an exact integer register to f64 (the implicit
/// coercion applied whenever an integer is used where a number is required).
pub(crate) fn i64_to_f64(a: u32, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::I64ToF64,
        r,
        Some(crate::eir::ValueType::F64),
        vec![a],
        None,
        None,
    ));
    r
}

/// RFC-0043 `F64ToI64`: truncates a numeric register toward zero (trap on
/// non-finite), the coercion applied at an integer-annotated `let`.
pub(crate) fn f64_to_i64(a: u32, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::F64ToI64,
        r,
        Some(crate::eir::ValueType::I64),
        vec![a],
        None,
        None,
    ));
    r
}

/// Emits a single-operand f64 instruction, returning its register.
pub(crate) fn unary(
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
pub(crate) fn random_reg(next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
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
/// RFC-0043: an integer-valued expression is widened first, so `Ne` sees a
/// single numeric kind.
pub(crate) fn truthy_f64(
    e: &Expr,
    ctx: &LowerCtx<'_>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let r = lower_expr_f64(e, ctx, next_id, out);
    truthy(r, next_id, out)
}

/// Normalizes an already-lowered register to a strict 1.0 / 0.0 boolean.
pub(crate) fn truthy(reg: u32, next_id: &mut u32, out: &mut Vec<crate::eir::Instruction>) -> u32 {
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
pub(crate) fn select_bool(
    cond: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
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
pub(crate) fn parse_substeps(
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
pub(crate) fn parse_every(
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
pub(crate) fn expr_slot_span(
    expr: &Expr,
    sn: &std::collections::BTreeMap<String, usize>,
    max: &mut usize,
    any: &mut bool,
) {
    match expr {
        Expr::Const(_) | Expr::Time => {}
        Expr::Int(_) => {}
        Expr::Slot(i) => {
            *max = (*max).max(*i);
            *any = true;
        }
        Expr::SlotDyn(idx) => expr_slot_span(idx, sn, max, any),
        Expr::Index(name, idx) => {
            // `name[j]` reads the whole array; reserve every element's register.
            if let Some(&base) = sn.get(&format!("{name}.0")) {
                let mut len = 1usize;
                while sn.contains_key(&format!("{name}.{len}")) {
                    len += 1;
                }
                *max = (*max).max(base + len - 1);
                *any = true;
            }
            expr_slot_span(idx, sn, max, any);
        }
        // Own named slot, including dotted struct fields (`pos.x`). Dotted
        // cross-entity names (`a.x`) are not in `sn` and so are ignored here.
        Expr::Name(n) => {
            if let Some(&s) = sn.get(n.as_str()) {
                *max = (*max).max(s);
                *any = true;
            }
        }
        Expr::Ref(..) | Expr::PropRef(..) => {}
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
        Expr::Call(name, args) => {
            // An array reduction (`sum(v)`, `dot(a, b)`, …) names the whole
            // array, so reserve every element's register — not just the base.
            if matches!(
                *name,
                "sum" | "mean" | "norm" | "asum" | "prod" | "min_of" | "max_of" | "dot"
            ) {
                for a in args {
                    if let Expr::Name(n) = a {
                        if let Some(&base) = sn.get(&format!("{n}.0")) {
                            let mut len = 1usize;
                            while sn.contains_key(&format!("{n}.{len}")) {
                                len += 1;
                            }
                            *max = (*max).max(base + len - 1);
                            *any = true;
                        }
                    }
                }
            }
            for a in args {
                expr_slot_span(a, sn, max, any);
            }
        }
    }
}

/// Walks a statement tree recording the highest own state slot its
/// expressions read (`sN` or a bare name), resolved against the entity's
/// layout. Mirrors `expr_slot_span` for `let` statement trees.
pub(crate) fn let_stmts_slot_span(
    stmts: &[LetStmt],
    sn: &std::collections::BTreeMap<String, usize>,
    max: &mut usize,
    any: &mut bool,
) {
    for s in stmts {
        match s {
            LetStmt::Let(_, e) => expr_slot_span(e, sn, max, any),
            LetStmt::LetInt(_, e) => expr_slot_span(e, sn, max, any),
            LetStmt::ArrAssign {
                name, idx, value, ..
            } => {
                expr_slot_span(
                    &Expr::Index(name.clone(), Box::new(idx.clone())),
                    sn,
                    max,
                    any,
                );
                expr_slot_span(value, sn, max, any);
            }
            LetStmt::If(c, t, e) => {
                expr_slot_span(c, sn, max, any);
                expr_slot_span(t, sn, max, any);
                if let Some(e) = e {
                    expr_slot_span(e, sn, max, any);
                }
            }
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

pub(crate) fn collect_let_refs(
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
            LetStmt::LetInt(_, e) => {
                collect_refs(e, out, props, named_refs, entity_map, state_names_by_id);
            }
            LetStmt::ArrAssign { idx, value, .. } => {
                collect_refs(idx, out, props, named_refs, entity_map, state_names_by_id);
                collect_refs(value, out, props, named_refs, entity_map, state_names_by_id);
            }
            LetStmt::If(c, t, e) => {
                collect_refs(c, out, props, named_refs, entity_map, state_names_by_id);
                collect_refs(t, out, props, named_refs, entity_map, state_names_by_id);
                if let Some(e) = e {
                    collect_refs(e, out, props, named_refs, entity_map, state_names_by_id);
                }
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
pub(crate) struct LowerParts<'a> {
    pub(crate) slot_regs: &'a [u32],
    pub(crate) ref_regs: &'a std::collections::BTreeMap<(u128, usize), u32>,
    pub(crate) prop_regs: &'a std::collections::BTreeMap<(u128, PropKind), u32>,
    pub(crate) entity_map: &'a std::collections::BTreeMap<String, u128>,
    pub(crate) state_names: &'a std::collections::BTreeMap<String, usize>,
    pub(crate) state_names_by_id:
        &'a std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    pub(crate) func_ids: &'a std::collections::BTreeMap<String, u64>,
    pub(crate) field_dims: &'a std::collections::BTreeMap<String, (u32, u32)>,
    pub(crate) namespace: &'a str,
    pub(crate) params: &'a std::collections::BTreeSet<String>,
    pub(crate) current_entity: u128,
    /// RFC-0043: names of integer-annotated `let` locals in scope (shared,
    /// interior-mutable so nested blocks push/pop without threading a map).
    pub(crate) int_locals: std::rc::Rc<std::cell::RefCell<std::collections::BTreeSet<String>>>,
}

impl<'a> LowerParts<'a> {
    pub(crate) fn ctx<'l>(
        &self,
        locals: &'l std::collections::BTreeMap<String, u32>,
    ) -> LowerCtx<'l>
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
            int_locals: Some(std::rc::Rc::clone(&self.int_locals)),
            int_ctx: false,
            func_ids: self.func_ids,
            field_dims: self.field_dims,
            namespace: self.namespace,
            params: self.params,
            current_entity: self.current_entity,
        }
    }
}

/// `a or b` over F64 0/1 gate registers: `a + b*(1-a)`.
pub(crate) fn or_gate(
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
pub(crate) fn has_control(stmts: &[LetStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LetStmt::Break(_) | LetStmt::Continue(_) | LetStmt::If(..) => true,
        LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => has_control(body),
        LetStmt::Let(..) | LetStmt::LetInt(..) | LetStmt::ArrAssign { .. } => false,
    })
}

/// Whether an expression contains a spatial query (`neighbor_count` /
/// `nearest_dist`). Queries need a per-entity lowering context, which
/// function bodies do not have.
pub(crate) fn expr_has_query(expr: &Expr) -> bool {
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
        Expr::Not(a) | Expr::Neg(a) | Expr::SlotDyn(a) | Expr::Index(_, a) => expr_has_query(a),
        _ => false,
    }
}

/// Whether a statement tree contains a spatial query anywhere.
pub(crate) fn stmts_have_query(stmts: &[LetStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LetStmt::Let(_, e) => expr_has_query(e),
        LetStmt::LetInt(_, e) => expr_has_query(e),
        LetStmt::ArrAssign { idx, value, .. } => expr_has_query(idx) || expr_has_query(value),
        LetStmt::If(c, t, e) => {
            expr_has_query(c)
                || expr_has_query(t)
                || e.as_ref().map(expr_has_query).unwrap_or(false)
        }
        LetStmt::Repeat(_, body) | LetStmt::For(_, _, _, body) => stmts_have_query(body),
        LetStmt::Break(g) | LetStmt::Continue(g) => g.as_ref().map(expr_has_query).unwrap_or(false),
    })
}

/// Binds a `for`-loop index to a constant value, predicated on the iteration's
/// run gate when the loop is gated (`new = prev + run*(lit - prev)`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn bind_loop_index(
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
/// A deferred array-element write discovered while unrolling a loop body.
///
/// Array writes are side effects, so they must be emitted after the enclosing
/// system's `when` gate is known (the gate may depend on `let` locals bound by
/// the same block). They are recorded during unrolling and emitted by the
/// caller via [`emit_array_writes`], which applies the iteration gate and the
/// system gate exactly like the top-level `dyn_assigns`/`dyn_rules` path.
pub(crate) struct ArrayWrite {
    /// Absolute, bound-checked State slot register (`lower_dyn_index`).
    pub(crate) slot: u32,
    /// `=`: the value expression. `+=`: the already-`dt`-scaled delta.
    pub(crate) value: u32,
    /// `true` for `+=` (add to the current value), `false` for `=`.
    pub(crate) add: bool,
    /// The loop iteration's run/skip gate (1.0 executes, 0.0 skipped).
    pub(crate) gate: Option<u32>,
}

/// The effective gate for a side-effecting loop-body statement:
/// `run * (1 - skip)`, or `None` when the block is ungated.
fn effective_gate(
    run: Option<u32>,
    skip: Option<u32>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> Option<u32> {
    use crate::eir::Opcode;
    match (run, skip) {
        (None, None) => None,
        (r, None) => r,
        (None, Some(s)) => {
            let one = const_reg(1.0, next_id, out);
            Some(binary(Opcode::Sub, one, s, next_id, out))
        }
        (Some(r), Some(s)) => {
            let one = const_reg(1.0, next_id, out);
            let ns = binary(Opcode::Sub, one, s, next_id, out);
            Some(binary(Opcode::Mul, r, ns, next_id, out))
        }
    }
}

/// Reads a State slot at a runtime index (sees earlier writes in the same
/// interpretation, like `dyn_rules`).
fn read_slot_dyn_reg(
    entity: u128,
    slot: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    use crate::eir::{Opcode, ValueType};
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        Opcode::ReadSlotDyn,
        r,
        Some(ValueType::F64),
        vec![slot],
        None,
        Some(crate::physics_eir::cr(
            entity,
            crate::physics_eir::state_id(),
            0,
        )),
    ));
    r
}

/// Emits the deferred array writes recorded while unrolling a loop body,
/// applying the iteration gate and the system `when` gate. A `+=` reads the
/// element's current value (seeing earlier array writes, like `dyn_rules`); a
/// gated-off write leaves the element untouched.
pub(crate) fn emit_array_writes(
    writes: &[ArrayWrite],
    when: Option<u32>,
    entity: u128,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) {
    use crate::eir::{Opcode, ValueType};
    for w in writes {
        // `gate = iteration * when` (each factor is 1.0/0.0); `None` = always.
        let gate = match (w.gate, when) {
            (None, None) => None,
            (g, None) => g,
            (None, Some(w)) => Some(w),
            (Some(g), Some(w)) => Some(binary(Opcode::Mul, g, w, next_id, out)),
        };
        let value = if w.add {
            let cur = read_slot_dyn_reg(entity, w.slot, next_id, out);
            let sum = binary(Opcode::Add, cur, w.value, next_id, out);
            match gate {
                Some(g) => {
                    let v = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        Opcode::Select,
                        v,
                        Some(ValueType::F64),
                        vec![g, sum, cur],
                        None,
                        None,
                    ));
                    v
                }
                None => sum,
            }
        } else {
            match gate {
                Some(g) => {
                    let cur = read_slot_dyn_reg(entity, w.slot, next_id, out);
                    let v = *next_id;
                    *next_id += 1;
                    out.push(crate::physics_eir::instr(
                        Opcode::Select,
                        v,
                        Some(ValueType::F64),
                        vec![g, w.value, cur],
                        None,
                        None,
                    ));
                    v
                }
                None => w.value,
            }
        };
        out.push(crate::physics_eir::instr(
            Opcode::WriteSlotDyn,
            0,
            None,
            vec![w.slot, value],
            None,
            Some(crate::physics_eir::cr(
                entity,
                crate::physics_eir::state_id(),
                0,
            )),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_let_block(
    stmts: &[LetStmt],
    run: Option<u32>,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
    locals: &mut std::collections::BTreeMap<String, u32>,
    parts: &LowerParts<'_>,
    writes: &mut Vec<ArrayWrite>,
) -> Option<u32> {
    let mut skip: Option<u32> = None; // block-local continue gate
    let mut broke: Option<u32> = None; // block-local break accumulator
    for stmt in stmts {
        match stmt {
            // RFC-0043: an integer-annotated `let` binds an exact `I64`.
            LetStmt::LetInt(name, expr) => {
                let ctx = parts.ctx(locals);
                let ctx = LowerCtx {
                    int_ctx: true,
                    ..ctx
                };
                let computed = lower_expr(expr, &ctx, next_id, out);
                // If the RHS is not already an exact integer (e.g. `let n: i64
                // = x/2` with a state read), truncate toward zero.
                let computed = if expr_is_integer(expr, &parts.int_locals) {
                    computed
                } else {
                    f64_to_i64(computed, next_id, out)
                };
                locals.insert(name.clone(), computed);
                parts.int_locals.borrow_mut().insert(name.clone());
                continue;
            }
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
            // RFC-0044 follow-up: `name[idx] = value` / `name[idx] += value`
            // inside a loop body. The index is bound-checked against this
            // entity's layout; the write is deferred so the system `when` gate
            // (which may read `let` locals) can be applied at emission.
            LetStmt::ArrAssign {
                name,
                idx,
                add,
                value,
            } => {
                let ctx = parts.ctx(locals);
                let target = super::ast::DynIndex::Array(name.clone(), Box::new(idx.clone()));
                match lower_dyn_index(&target, &ctx, next_id, out) {
                    None => push_diag(
                        109,
                        0,
                        format!("array `{name}` is not in this entity's state - write ignored"),
                    ),
                    Some(slot) => {
                        let rhs = lower_expr_f64(value, &ctx, next_id, out);
                        let value_reg = if *add {
                            // `+=` is the `inte` sugar: integrate at this
                            // substep's scaled dt (RFC-0049).
                            let dt_reg = match locals.get("dt").copied() {
                                Some(r) => r,
                                None => const_reg(1.0, next_id, out),
                            };
                            binary(crate::eir::Opcode::Mul, rhs, dt_reg, next_id, out)
                        } else {
                            rhs
                        };
                        let gate = effective_gate(run, skip, next_id, out);
                        writes.push(ArrayWrite {
                            slot,
                            value: value_reg,
                            add: *add,
                            gate,
                        });
                    }
                }
            }
            LetStmt::Break(cond) | LetStmt::Continue(cond) => {
                let ctx = parts.ctx(locals);
                let c = match cond {
                    Some(e) => truthy_f64(e, &ctx, next_id, out),
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
            LetStmt::If(..) => {
                // Control-flow `if` is only valid in function bodies (handled by
                // the function lowering); it cannot appear in a system's lets.
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
                        lower_let_block(body, inner_run, next_id, out, locals, parts, writes);
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
                        let b =
                            lower_let_block(body, Some(run_k), next_id, out, locals, parts, writes);
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
                        lower_let_block(body, inner_run, next_id, out, locals, parts, writes);
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
                        let b =
                            lower_let_block(body, Some(run_k), next_id, out, locals, parts, writes);
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

pub(crate) fn binary(
    op: crate::eir::Opcode,
    a: u32,
    b: u32,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    binary_typed(op, a, b, crate::eir::ValueType::F64, next_id, out)
}

/// Binary op with an explicit result/operand value type (RFC-0043 uses this to
/// emit `I64` arithmetic for `Int op Int` and `F64` otherwise).
pub(crate) fn binary_typed(
    op: crate::eir::Opcode,
    a: u32,
    b: u32,
    ty: crate::eir::ValueType,
    next_id: &mut u32,
    out: &mut Vec<crate::eir::Instruction>,
) -> u32 {
    let r = *next_id;
    *next_id += 1;
    out.push(crate::physics_eir::instr(
        op,
        r,
        Some(ty),
        vec![a, b],
        None,
        None,
    ));
    r
}
