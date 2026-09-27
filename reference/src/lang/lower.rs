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
            if let Some(&reg) = ctx.locals.get(name.as_str()) {
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
            if *name == "inte" || *name == "deriv" {
                return lower_inte_deriv(name, args, ctx, next_id, out);
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
                    // `schedule` yields nothing (a void opcode) — emit with no
                    // result so the validator accepts it; its value is discarded.
                    if op == crate::eir::Opcode::ScheduleEvent {
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
        Expr::Const(_) | Expr::Slot(_) | Expr::Time => {}
    }
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
        Expr::Slot(i) => {
            *max = (*max).max(*i);
            *any = true;
        }
        Expr::SlotDyn(idx) => expr_slot_span(idx, sn, max, any),
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
pub(crate) fn let_stmts_slot_span(
    stmts: &[LetStmt],
    sn: &std::collections::BTreeMap<String, usize>,
    max: &mut usize,
    any: &mut bool,
) {
    for s in stmts {
        match s {
            LetStmt::Let(_, e) => expr_slot_span(e, sn, max, any),
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
        LetStmt::Let(..) => false,
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
        Expr::Not(a) | Expr::Neg(a) | Expr::SlotDyn(a) => expr_has_query(a),
        _ => false,
    }
}

/// Whether a statement tree contains a spatial query anywhere.
pub(crate) fn stmts_have_query(stmts: &[LetStmt]) -> bool {
    stmts.iter().any(|s| match s {
        LetStmt::Let(_, e) => expr_has_query(e),
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_let_block(
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

pub(crate) fn binary(
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
