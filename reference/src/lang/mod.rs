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

mod diagnostics;
pub use diagnostics::{
    clear_diagnostics, detail_name, diagnose, render_diagnostic, take_diagnostics, Diagnostic,
};
pub(crate) use diagnostics::{error, error_at};
use pwe_api::{Access, EntityId, Error, Hash256, RegionId, Result, Status, WorldId, WorldVersion};

mod parser;
pub use parser::*;

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
    /// Dynamic ODE rules `s[idx] += dt·expr`.
    pub dyn_rules: Vec<(Expr, Expr)>,
    /// Assignment rules (`slot = expr`).
    pub assigns: Vec<(String, Expr)>,
    /// Dynamic assignments `s[idx] = expr`.
    pub dyn_assigns: Vec<(Expr, Expr)>,
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
        for (_, expr) in &self.rules {
            expr_slot_span(expr, &sn, &mut let_max, &mut let_any);
        }
        for (lhs, expr) in &self.assigns {
            if let Some(idx) = numeric_slot(lhs).or_else(|| sn.get(lhs).copied()) {
                slots = slots.max(idx + 1);
            }
            expr_slot_span(expr, &sn, &mut let_max, &mut let_any);
        }
        for (idx_expr, expr) in &self.dyn_assigns {
            expr_slot_span(idx_expr, &sn, &mut let_max, &mut let_any);
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
        for (_, expr) in &self.assigns {
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

        // Read each distinct referenced (entity, slot) once, from the committed
        // (start-of-system) snapshot — one frozen view shared by every entity of
        // the system (simultaneous semantics; preserves Newton's third law).
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadCommitted,
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
        // Read each distinct cross-entity property once (committed snapshot).
        let mut prop_regs: std::collections::BTreeMap<(u128, PropKind), u32> = Default::default();
        for (pid, kind) in prop_ids {
            let (cid, off) = prop_component(kind);
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadCommitted,
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
            locals.insert("dt".to_string(), dt_reg);
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
            for (lhs, expr) in &self.assigns {
                let idx = match numeric_slot(lhs).or_else(|| sn.get(lhs).copied()) {
                    Some(i) => i,
                    None => continue,
                };
                if idx >= slots {
                    continue;
                }
                let val = lower_expr(expr, &ctx, &mut next_id, out);
                let val = match gate_reg {
                    Some(g) => {
                        let v = next_id;
                        next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Select,
                            v,
                            Some(crate::eir::ValueType::F64),
                            vec![g, val, slot_regs[idx]],
                            None,
                            None,
                        ));
                        v
                    }
                    None => val,
                };
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteView,
                    0,
                    None,
                    vec![val],
                    None,
                    Some(crate::physics_eir::cr(
                        entity,
                        crate::physics_eir::state_id(),
                        crate::physics_eir::field::state_slot(idx),
                    )),
                ));
            }
            for (idx_expr, expr) in &self.dyn_assigns {
                let ri = lower_expr(idx_expr, &ctx, &mut next_id, out);
                let val = lower_expr(expr, &ctx, &mut next_id, out);
                let val = match gate_reg {
                    Some(g) => {
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
                        let v = next_id;
                        next_id += 1;
                        out.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Select,
                            v,
                            Some(crate::eir::ValueType::F64),
                            vec![g, val, cur],
                            None,
                            None,
                        ));
                        v
                    }
                    None => val,
                };
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::WriteSlotDyn,
                    0,
                    None,
                    vec![ri, val],
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

        // Cross-entity references and properties are sampled once per step from
        // the committed (start-of-system) snapshot.
        let mut ref_regs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        for (rid, rslot) in ref_ids {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(crate::physics_eir::instr(
                crate::eir::Opcode::ReadCommitted,
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
                crate::eir::Opcode::ReadCommitted,
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
                let dt_reg = next_id;
                next_id += 1;
                out.push(crate::physics_eir::instr(
                    crate::eir::Opcode::Const,
                    dt_reg,
                    Some(crate::eir::ValueType::F64),
                    vec![],
                    Some(crate::eir::Immediate::F64(self.dt)),
                    None,
                ));
                locals.insert("dt".to_string(), dt_reg);
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
                let k1 = stage_k[0][*i].expect("rk4 stage register");
                let k2 = stage_k[1][*i].expect("rk4 stage register");
                let k3 = stage_k[2][*i].expect("rk4 stage register");
                let k4 = stage_k[3][*i].expect("rk4 stage register");
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
/// Lowers the `inte(E)` / `deriv(E)` operators.
///
/// `inte(E)` is the integration increment `dt · E`. `deriv(E)` is the backward
/// difference `(E − E_prev)/dt`, where `E_prev` is the value `E` took at the
/// previous (sub)step, remembered per call site in the runtime's history
/// (`HistRead`/`HistWrite`). On the first step (no history) `deriv` is 0.
fn lower_inte_deriv(
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
/// RFC-0039: the positional joint families.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JointKind {
    /// `distance` / `spring`: keep `|a - b| = rest` (spring adds velocity damping).
    Distance,
    /// `weld` / `hinge` / `ball`: coincide the anchor points.
    Anchor,
    /// `prismatic` / `slider`: keep `b` on the line through `a` along `axis`.
    Prismatic,
}

/// RFC-0039: a pairwise position-relaxation joint. Lowers once, for entity `a`.
pub struct JointSystem {
    pub a: u128,
    pub b: u128,
    pub kind: JointKind,
    pub rest: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub axis: (f64, f64, f64),
    pub anchor_a: (f64, f64, f64),
    pub anchor_b: (f64, f64, f64),
    pub limit: Option<(f64, f64)>,
    pub iterations: u32,
}

fn jt_read(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    e: u128,
    comp: pwe_api::ComponentTypeId,
    off: u32,
) -> u32 {
    let r = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::ReadView,
        r,
        Some(crate::eir::ValueType::F64),
        vec![],
        None,
        Some(crate::physics_eir::cr(e, comp, off)),
    ));
    r
}

fn jt_write(
    out: &mut Vec<crate::eir::Instruction>,
    e: u128,
    comp: pwe_api::ComponentTypeId,
    off: u32,
    v: u32,
) {
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::WriteView,
        0,
        None,
        vec![v],
        None,
        Some(crate::physics_eir::cr(e, comp, off)),
    ));
}

/// `is_dynamic ? 1/mass : 0` (RFC-0039 mass weighting).
fn jt_inv_mass(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    e: u128,
    zero: u32,
    one: u32,
    eps: u32,
) -> u32 {
    use crate::physics_eir::{field, rigid_body_id};
    let m = jt_read(out, next, e, rigid_body_id(), field::MASS);
    let d = jt_read(out, next, e, rigid_body_id(), field::IS_DYNAMIC);
    let ms = nb_arith(out, next, crate::eir::Opcode::Add, m, eps);
    let inv = nb_arith(out, next, crate::eir::Opcode::Div, one, ms);
    let dneq = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Ne,
        dneq,
        Some(crate::eir::ValueType::Bool),
        vec![d, zero],
        None,
        None,
    ));
    let dflt = *next;
    *next += 1;
    out.push(crate::physics_eir::instr(
        crate::eir::Opcode::Select,
        dflt,
        Some(crate::eir::ValueType::F64),
        vec![dneq, one, zero],
        None,
        None,
    ));
    nb_arith(out, next, crate::eir::Opcode::Mul, inv, dflt)
}

impl EirSystem for JointSystem {
    fn name(&self) -> &'static str {
        "physics.joint"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<crate::eir::Instruction>) {
        use crate::eir::Opcode;
        use crate::physics_eir::{field, transform_id, velocity_id};
        let ret = |out: &mut Vec<crate::eir::Instruction>| {
            out.push(crate::physics_eir::instr(
                Opcode::Return,
                0,
                None,
                vec![],
                None,
                None,
            ));
        };
        if entity != self.a {
            ret(out);
            return;
        }
        let mut next = 1u32;
        let zero = nb_const(out, &mut next, 0.0);
        let eps = nb_const(out, &mut next, 1e-12);
        let one = nb_const(out, &mut next, 1.0);
        let inv_a = jt_inv_mass(out, &mut next, self.a, zero, one, eps);
        let inv_b = jt_inv_mass(out, &mut next, self.b, zero, one, eps);
        let total = nb_arith(out, &mut next, Opcode::Add, inv_a, inv_b);
        // A joint with no movable side is a no-op (guard, not silent NaN).
        let skip = next;
        next += 1;
        out.push(crate::physics_eir::instr(
            Opcode::Le,
            skip,
            Some(crate::eir::ValueType::Bool),
            vec![total, zero],
            None,
            None,
        ));
        let gi = out.len();
        out.push(crate::physics_eir::instr(
            Opcode::CondBr,
            0,
            None,
            vec![skip, 0, 0],
            None,
            None,
        ));

        let stiff = nb_const(out, &mut next, self.stiffness);
        let sd_total = nb_arith(out, &mut next, Opcode::Div, stiff, total);
        let wa = nb_arith(out, &mut next, Opcode::Mul, sd_total, inv_a);
        let wb = nb_arith(out, &mut next, Opcode::Mul, sd_total, inv_b);
        let axis = (
            nb_const(out, &mut next, self.axis.0),
            nb_const(out, &mut next, self.axis.1),
            nb_const(out, &mut next, self.axis.2),
        );
        let aan = (
            nb_const(out, &mut next, self.anchor_a.0),
            nb_const(out, &mut next, self.anchor_a.1),
            nb_const(out, &mut next, self.anchor_a.2),
        );
        let abn = (
            nb_const(out, &mut next, self.anchor_b.0),
            nb_const(out, &mut next, self.anchor_b.1),
            nb_const(out, &mut next, self.anchor_b.2),
        );
        let rest = nb_const(out, &mut next, self.rest);
        let damping = nb_const(out, &mut next, self.damping);
        let (lo, hi) = match self.limit {
            Some((l, h)) => (
                Some(nb_const(out, &mut next, l)),
                Some(nb_const(out, &mut next, h)),
            ),
            None => (None, None),
        };

        for _ in 0..self.iterations.max(1) {
            let ax = jt_read(out, &mut next, self.a, transform_id(), field::POS_X);
            let ay = jt_read(out, &mut next, self.a, transform_id(), field::POS_Y);
            let az = jt_read(out, &mut next, self.a, transform_id(), field::POS_Z);
            let bx = jt_read(out, &mut next, self.b, transform_id(), field::POS_X);
            let by = jt_read(out, &mut next, self.b, transform_id(), field::POS_Y);
            let bz = jt_read(out, &mut next, self.b, transform_id(), field::POS_Z);
            let (nax, nay, naz, nbx, nby, nbz) = match self.kind {
                JointKind::Distance => {
                    let dx = nb_arith(out, &mut next, Opcode::Sub, ax, bx);
                    let dy = nb_arith(out, &mut next, Opcode::Sub, ay, by);
                    let dz = nb_arith(out, &mut next, Opcode::Sub, az, bz);
                    let r2 = {
                        let x2 = nb_arith(out, &mut next, Opcode::Mul, dx, dx);
                        let y2 = nb_arith(out, &mut next, Opcode::Mul, dy, dy);
                        let z2 = nb_arith(out, &mut next, Opcode::Mul, dz, dz);
                        let s = nb_arith(out, &mut next, Opcode::Add, x2, y2);
                        nb_arith(out, &mut next, Opcode::Add, s, z2)
                    };
                    let r2e = nb_arith(out, &mut next, Opcode::Add, r2, eps);
                    let r = nb_un(Opcode::Sqrt, out, &mut next, r2e);
                    let err0 = nb_arith(out, &mut next, Opcode::Sub, r, rest);
                    let mut err = nb_arith(out, &mut next, Opcode::Mul, err0, stiff);
                    // spring damping along the axis: err += damping·((va-vb)·dir).
                    // Only emitted when damping is non-zero (a static side has no
                    // velocity component, so an unconditional read would fail).
                    if self.damping != 0.0 {
                        let vax = jt_read(out, &mut next, self.a, velocity_id(), field::VEL_X);
                        let vay = jt_read(out, &mut next, self.a, velocity_id(), field::VEL_Y);
                        let vaz = jt_read(out, &mut next, self.a, velocity_id(), field::VEL_Z);
                        let vbx = jt_read(out, &mut next, self.b, velocity_id(), field::VEL_X);
                        let vby = jt_read(out, &mut next, self.b, velocity_id(), field::VEL_Y);
                        let vbz = jt_read(out, &mut next, self.b, velocity_id(), field::VEL_Z);
                        let dvx = nb_arith(out, &mut next, Opcode::Sub, vax, vbx);
                        let dvy = nb_arith(out, &mut next, Opcode::Sub, vay, vby);
                        let dvz = nb_arith(out, &mut next, Opcode::Sub, vaz, vbz);
                        let t1 = nb_arith(out, &mut next, Opcode::Mul, dvx, dx);
                        let t2 = nb_arith(out, &mut next, Opcode::Mul, dvy, dy);
                        let t3 = nb_arith(out, &mut next, Opcode::Mul, dvz, dz);
                        let s1 = nb_arith(out, &mut next, Opcode::Add, t1, t2);
                        let s2 = nb_arith(out, &mut next, Opcode::Add, s1, t3);
                        let relv = nb_arith(out, &mut next, Opcode::Div, s2, r);
                        let dv = nb_arith(out, &mut next, Opcode::Mul, damping, relv);
                        err = nb_arith(out, &mut next, Opcode::Add, err, dv);
                    }
                    let den = nb_arith(out, &mut next, Opcode::Mul, r, total);
                    let scale = nb_arith(out, &mut next, Opcode::Div, err, den);
                    let ca = nb_arith(out, &mut next, Opcode::Mul, scale, inv_a);
                    let cb = nb_arith(out, &mut next, Opcode::Mul, scale, inv_b);
                    (
                        nb_sub_mul(out, &mut next, ax, dx, ca),
                        nb_sub_mul(out, &mut next, ay, dy, ca),
                        nb_sub_mul(out, &mut next, az, dz, ca),
                        nb_add_mul(out, &mut next, bx, dx, cb),
                        nb_add_mul(out, &mut next, by, dy, cb),
                        nb_add_mul(out, &mut next, bz, dz, cb),
                    )
                }
                JointKind::Anchor => {
                    let ex = {
                        let aa = nb_arith(out, &mut next, Opcode::Add, ax, aan.0);
                        let bb = nb_arith(out, &mut next, Opcode::Add, bx, abn.0);
                        nb_arith(out, &mut next, Opcode::Sub, aa, bb)
                    };
                    let ey = {
                        let aa = nb_arith(out, &mut next, Opcode::Add, ay, aan.1);
                        let bb = nb_arith(out, &mut next, Opcode::Add, by, abn.1);
                        nb_arith(out, &mut next, Opcode::Sub, aa, bb)
                    };
                    let ez = {
                        let aa = nb_arith(out, &mut next, Opcode::Add, az, aan.2);
                        let bb = nb_arith(out, &mut next, Opcode::Add, bz, abn.2);
                        nb_arith(out, &mut next, Opcode::Sub, aa, bb)
                    };
                    (
                        nb_sub_mul(out, &mut next, ax, ex, wa),
                        nb_sub_mul(out, &mut next, ay, ey, wa),
                        nb_sub_mul(out, &mut next, az, ez, wa),
                        nb_add_mul(out, &mut next, bx, ex, wb),
                        nb_add_mul(out, &mut next, by, ey, wb),
                        nb_add_mul(out, &mut next, bz, ez, wb),
                    )
                }
                JointKind::Prismatic => {
                    let dx = nb_arith(out, &mut next, Opcode::Sub, bx, ax);
                    let dy = nb_arith(out, &mut next, Opcode::Sub, by, ay);
                    let dz = nb_arith(out, &mut next, Opcode::Sub, bz, az);
                    let t1 = nb_arith(out, &mut next, Opcode::Mul, dx, axis.0);
                    let t2 = nb_arith(out, &mut next, Opcode::Mul, dy, axis.1);
                    let t3 = nb_arith(out, &mut next, Opcode::Mul, dz, axis.2);
                    let s1 = nb_arith(out, &mut next, Opcode::Add, t1, t2);
                    let along = nb_arith(out, &mut next, Opcode::Add, s1, t3);
                    let clamped = match (lo, hi) {
                        (Some(l), Some(h)) => {
                            let lt = next;
                            next += 1;
                            out.push(crate::physics_eir::instr(
                                Opcode::Lt,
                                lt,
                                Some(crate::eir::ValueType::Bool),
                                vec![along, h],
                                None,
                                None,
                            ));
                            let mn = next;
                            next += 1;
                            out.push(crate::physics_eir::instr(
                                Opcode::Select,
                                mn,
                                Some(crate::eir::ValueType::F64),
                                vec![lt, along, h],
                                None,
                                None,
                            ));
                            let gt = next;
                            next += 1;
                            out.push(crate::physics_eir::instr(
                                Opcode::Gt,
                                gt,
                                Some(crate::eir::ValueType::Bool),
                                vec![mn, l],
                                None,
                                None,
                            ));
                            let mx = next;
                            next += 1;
                            out.push(crate::physics_eir::instr(
                                Opcode::Select,
                                mx,
                                Some(crate::eir::ValueType::F64),
                                vec![gt, mn, l],
                                None,
                                None,
                            ));
                            mx
                        }
                        _ => along,
                    };
                    let px = nb_sub_mul(out, &mut next, dx, axis.0, clamped);
                    let py = nb_sub_mul(out, &mut next, dy, axis.1, clamped);
                    let pz = nb_sub_mul(out, &mut next, dz, axis.2, clamped);
                    (
                        nb_add_mul(out, &mut next, ax, px, wa),
                        nb_add_mul(out, &mut next, ay, py, wa),
                        nb_add_mul(out, &mut next, az, pz, wa),
                        nb_sub_mul(out, &mut next, bx, px, wb),
                        nb_sub_mul(out, &mut next, by, py, wb),
                        nb_sub_mul(out, &mut next, bz, pz, wb),
                    )
                }
            };
            jt_write(out, self.a, transform_id(), field::POS_X, nax);
            jt_write(out, self.a, transform_id(), field::POS_Y, nay);
            jt_write(out, self.a, transform_id(), field::POS_Z, naz);
            jt_write(out, self.b, transform_id(), field::POS_X, nbx);
            jt_write(out, self.b, transform_id(), field::POS_Y, nby);
            jt_write(out, self.b, transform_id(), field::POS_Z, nbz);
        }
        // Body ends in a terminator; the guard skips to a second Return.
        ret(out);
        let ret_idx = out.len();
        ret(out);
        if let Some(ins) = out.get_mut(gi) {
            ins.operands = vec![skip, ret_idx as u32, (gi + 1) as u32];
        }
    }
}

/// RFC-0040: a distance-spring pass between two soft particles (no guard; the
/// particles are always dynamic).
#[allow(clippy::too_many_arguments)]
fn soft_pair(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    a: u128,
    b: u128,
    rest: f64,
    stiff: u32,
    damping: f64,
    zero: u32,
    one: u32,
    eps: u32,
) {
    use crate::eir::Opcode;
    use crate::physics_eir::{field, transform_id, velocity_id};
    let inv_a = jt_inv_mass(out, next, a, zero, one, eps);
    let inv_b = jt_inv_mass(out, next, b, zero, one, eps);
    let total = nb_arith(out, next, Opcode::Add, inv_a, inv_b);
    let ax = jt_read(out, next, a, transform_id(), field::POS_X);
    let ay = jt_read(out, next, a, transform_id(), field::POS_Y);
    let az = jt_read(out, next, a, transform_id(), field::POS_Z);
    let bx = jt_read(out, next, b, transform_id(), field::POS_X);
    let by = jt_read(out, next, b, transform_id(), field::POS_Y);
    let bz = jt_read(out, next, b, transform_id(), field::POS_Z);
    let dx = nb_arith(out, next, Opcode::Sub, ax, bx);
    let dy = nb_arith(out, next, Opcode::Sub, ay, by);
    let dz = nb_arith(out, next, Opcode::Sub, az, bz);
    let r2 = {
        let x2 = nb_arith(out, next, Opcode::Mul, dx, dx);
        let y2 = nb_arith(out, next, Opcode::Mul, dy, dy);
        let z2 = nb_arith(out, next, Opcode::Mul, dz, dz);
        let sxy = nb_arith(out, next, Opcode::Add, x2, y2);
        nb_arith(out, next, Opcode::Add, sxy, z2)
    };
    let r2e = nb_arith(out, next, Opcode::Add, r2, eps);
    let r = nb_un(Opcode::Sqrt, out, next, r2e);
    let rest_c = nb_const(out, next, rest);
    let err0 = nb_arith(out, next, Opcode::Sub, r, rest_c);
    let mut err = nb_arith(out, next, Opcode::Mul, err0, stiff);
    if damping != 0.0 {
        let damp = nb_const(out, next, damping);
        let vax = jt_read(out, next, a, velocity_id(), field::VEL_X);
        let vay = jt_read(out, next, a, velocity_id(), field::VEL_Y);
        let vaz = jt_read(out, next, a, velocity_id(), field::VEL_Z);
        let vbx = jt_read(out, next, b, velocity_id(), field::VEL_X);
        let vby = jt_read(out, next, b, velocity_id(), field::VEL_Y);
        let vbz = jt_read(out, next, b, velocity_id(), field::VEL_Z);
        let dvx = nb_arith(out, next, Opcode::Sub, vax, vbx);
        let dvy = nb_arith(out, next, Opcode::Sub, vay, vby);
        let dvz = nb_arith(out, next, Opcode::Sub, vaz, vbz);
        let t1 = nb_arith(out, next, Opcode::Mul, dvx, dx);
        let t2 = nb_arith(out, next, Opcode::Mul, dvy, dy);
        let t3 = nb_arith(out, next, Opcode::Mul, dvz, dz);
        let s1 = nb_arith(out, next, Opcode::Add, t1, t2);
        let s2 = nb_arith(out, next, Opcode::Add, s1, t3);
        let relv = nb_arith(out, next, Opcode::Div, s2, r);
        let dv = nb_arith(out, next, Opcode::Mul, damp, relv);
        err = nb_arith(out, next, Opcode::Add, err, dv);
    }
    let den = nb_arith(out, next, Opcode::Mul, r, total);
    let scale = nb_arith(out, next, Opcode::Div, err, den);
    let ca = nb_arith(out, next, Opcode::Mul, scale, inv_a);
    let cb = nb_arith(out, next, Opcode::Mul, scale, inv_b);
    let nax = nb_sub_mul(out, next, ax, dx, ca);
    let nay = nb_sub_mul(out, next, ay, dy, ca);
    let naz = nb_sub_mul(out, next, az, dz, ca);
    let nbx = nb_add_mul(out, next, bx, dx, cb);
    let nby = nb_add_mul(out, next, by, dy, cb);
    let nbz = nb_add_mul(out, next, bz, dz, cb);
    jt_write(out, a, transform_id(), field::POS_X, nax);
    jt_write(out, a, transform_id(), field::POS_Y, nay);
    jt_write(out, a, transform_id(), field::POS_Z, naz);
    jt_write(out, b, transform_id(), field::POS_X, nbx);
    jt_write(out, b, transform_id(), field::POS_Y, nby);
    jt_write(out, b, transform_id(), field::POS_Z, nbz);
}

/// RFC-0040: a mass-spring soft body. Lowers one function per particle, applying
/// (per iteration) every constraint whose lower endpoint is that particle.
pub struct SoftSystem {
    pub base: u128,
    pub count: u32,
    /// `(lo index, hi index, rest length)` — the lower endpoint owns the pass.
    pub constraints: Vec<(u32, u32, f64)>,
    pub stiffness: f64,
    pub damping: f64,
    pub iterations: u32,
}
impl EirSystem for SoftSystem {
    fn name(&self) -> &'static str {
        "physics.soft"
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
        if entity < self.base || entity >= self.base + self.count as u128 {
            ret(out);
            return;
        }
        let p = (entity - self.base) as u32;
        let mut next = 1u32;
        let zero = nb_const(out, &mut next, 0.0);
        let one = nb_const(out, &mut next, 1.0);
        let eps = nb_const(out, &mut next, 1e-12);
        let stiff = nb_const(out, &mut next, self.stiffness);
        for _ in 0..self.iterations.max(1) {
            for &(lo, hi, rest) in &self.constraints {
                if lo != p {
                    continue;
                }
                soft_pair(
                    out,
                    &mut next,
                    self.base + lo as u128,
                    self.base + hi as u128,
                    rest,
                    stiff,
                    self.damping,
                    zero,
                    one,
                    eps,
                );
            }
        }
        ret(out);
    }
}

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
                let expr = self.value.as_ref().expect("ChanOp::Send carries a value");
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

fn nb_sub_mul(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    a: u32,
    b: u32,
    c: u32,
) -> u32 {
    let p = nb_arith(out, next, crate::eir::Opcode::Mul, b, c);
    nb_arith(out, next, crate::eir::Opcode::Sub, a, p)
}
fn nb_add_mul(
    out: &mut Vec<crate::eir::Instruction>,
    next: &mut u32,
    a: u32,
    b: u32,
    c: u32,
) -> u32 {
    let p = nb_arith(out, next, crate::eir::Opcode::Mul, b, c);
    nb_arith(out, next, crate::eir::Opcode::Add, a, p)
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
        crate::eir::Opcode::ReadCommitted,
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
fn let_stmts_slot_span(
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
        LetStmt::Break(_) | LetStmt::Continue(_) | LetStmt::If(..) => true,
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
    softs: &[(String, u128, u32, u32, u32, f64)],
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
                        let value = if let Some(t) =
                            s.assigns.get("value").or_else(|| s.update.get("value"))
                        {
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
                let mut assigns: Vec<(String, Expr)> = Vec::new();
                let mut dyn_assigns: Vec<(Expr, Expr)> = Vec::new();
                for (key, text) in &s.assigns {
                    if key.starts_with('s') && key.contains('[') {
                        let inner = key
                            .trim_start_matches('s')
                            .trim_start_matches('[')
                            .trim_end_matches(']');
                        dyn_assigns.push((parse_expr_str(inner)?, parse_expr_str(text)?));
                    } else {
                        assigns.push((key.clone(), parse_expr_str(text)?));
                    }
                }
                if rules.is_empty()
                    && dyn_rules.is_empty()
                    && assigns.is_empty()
                    && dyn_assigns.is_empty()
                {
                    return Err(error_at(
                        Status::Invalid,
                        55,
                        s.byte_offset,
                        "update system has no rules; add `slot = <expr>`".to_string(),
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
                    assigns,
                    dyn_assigns,
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
                let expr_text = s
                    .assigns
                    .get("expr")
                    .or_else(|| s.update.get("expr"))
                    .cloned()
                    .ok_or_else(|| {
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
                let expr_text = s
                    .assigns
                    .get("expr")
                    .or_else(|| s.update.get("expr"))
                    .cloned()
                    .ok_or_else(|| {
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
            // RFC-0040: mass-spring soft bodies over an nx×ny particle grid.
            "soft" => {
                let name = s.string_params.get("body").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "soft requires `body = <name>`".to_string(),
                    )
                })?;
                let (_, base, nx, ny, nz, spacing) = softs
                    .iter()
                    .find(|(n, ..)| n == name)
                    .cloned()
                    .ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            78,
                            s.byte_offset,
                            format!("soft references unknown body '{name}'"),
                        )
                    })?;
                let idx = |i: u32, j: u32, k: u32| (k * ny + j) * nx + i;
                let mut constraints: Vec<(u32, u32, f64)> = Vec::new();
                let diag = spacing * std::f64::consts::SQRT_2;
                for k in 0..nz {
                    for j in 0..ny {
                        for i in 0..nx {
                            // structural (3 axes)
                            if i + 1 < nx {
                                constraints.push((idx(i, j, k), idx(i + 1, j, k), spacing));
                            }
                            if j + 1 < ny {
                                constraints.push((idx(i, j, k), idx(i, j + 1, k), spacing));
                            }
                            if k + 1 < nz {
                                constraints.push((idx(i, j, k), idx(i, j, k + 1), spacing));
                            }
                            // shear (both diagonals of each coordinate plane)
                            if i + 1 < nx && j + 1 < ny {
                                constraints.push((idx(i, j, k), idx(i + 1, j + 1, k), diag));
                                constraints.push((idx(i + 1, j, k), idx(i, j + 1, k), diag));
                            }
                            if i + 1 < nx && k + 1 < nz {
                                constraints.push((idx(i, j, k), idx(i + 1, j, k + 1), diag));
                                constraints.push((idx(i + 1, j, k), idx(i, j, k + 1), diag));
                            }
                            if j + 1 < ny && k + 1 < nz {
                                constraints.push((idx(i, j, k), idx(i, j + 1, k + 1), diag));
                                constraints.push((idx(i, j + 1, k), idx(i, j, k + 1), diag));
                            }
                            // bend (two cells along each axis)
                            if i + 2 < nx {
                                constraints.push((idx(i, j, k), idx(i + 2, j, k), 2.0 * spacing));
                            }
                            if j + 2 < ny {
                                constraints.push((idx(i, j, k), idx(i, j + 2, k), 2.0 * spacing));
                            }
                            if k + 2 < nz {
                                constraints.push((idx(i, j, k), idx(i, j, k + 2), 2.0 * spacing));
                            }
                        }
                    }
                }
                let stiffness = s.params.get("stiffness").copied().unwrap_or(1.0);
                let damping = s.params.get("damping").copied().unwrap_or(0.0);
                let iterations = match s.params.get("iterations").copied() {
                    Some(v) if (1.0..=64.0).contains(&v) && v.fract() == 0.0 => v as u32,
                    Some(_) => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "soft `iterations` must be an integer in 1..=64".to_string(),
                        ))
                    }
                    None => 4,
                };
                out.push(Box::new(SoftSystem {
                    base,
                    count: nx * ny,
                    constraints,
                    stiffness,
                    damping,
                    iterations,
                }));
            }
            // RFC-0039: pairwise constraint joints (position relaxation).
            "joint" => {
                let a_name = s.string_params.get("on").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "joint requires `on = <entity>`".to_string(),
                    )
                })?;
                let b_name = s.string_params.get("other").ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "joint requires `other = <entity>`".to_string(),
                    )
                })?;
                let a0 = entity_ids
                    .get(a_name)
                    .copied()
                    .ok_or(error(Status::Invalid, 62))?;
                let b0 = entity_ids
                    .get(b_name)
                    .copied()
                    .ok_or(error(Status::Invalid, 62))?;
                // The joint lowers once, on whichever side runs systems (a static
                // anchor is excluded from `dynamic`); the math is symmetric.
                let (a, b) = if dynamic.contains(&a0) {
                    (a0, b0)
                } else if dynamic.contains(&b0) {
                    (b0, a0)
                } else {
                    (a0, b0)
                };
                let ty = s
                    .string_params
                    .get("type")
                    .map(|t| t.as_str())
                    .unwrap_or("distance");
                let kind = match ty {
                    "distance" | "spring" => JointKind::Distance,
                    "weld" | "fixed" | "hinge" | "revolute" | "ball" | "spherical" => {
                        JointKind::Anchor
                    }
                    "prismatic" | "slider" => JointKind::Prismatic,
                    "cone" | "twist" | "universal" | "gear" | "rack" | "pulley" => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!(
                                "joint type '{ty}' enforces a rotational/ratio constraint; \
the engine has no rotational state (RFC-0039)"
                            ),
                        ))
                    }
                    other => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!("unknown joint type '{other}'"),
                        ))
                    }
                };
                let rest = s.params.get("length").copied().unwrap_or(0.0);
                let stiffness = s.params.get("stiffness").copied().unwrap_or(1.0);
                let damping = match (ty, s.params.get("damping").copied()) {
                    ("spring", None) => 0.5,
                    (_, Some(d)) => d,
                    _ => 0.0,
                };
                let axis_raw = s
                    .vec_params
                    .get("axis")
                    .cloned()
                    .unwrap_or_else(|| vec![0.0, 0.0, 1.0]);
                let axis = normalize3(&axis_raw);
                let anchor_raw = s
                    .vec_params
                    .get("anchor")
                    .cloned()
                    .unwrap_or_else(|| vec![0.0, 0.0, 0.0]);
                let limit = s.vec_params.get("limit").cloned().map(|v| {
                    (
                        v.first().copied().unwrap_or(0.0),
                        v.get(1).copied().unwrap_or(0.0),
                    )
                });
                let iterations = match s.params.get("iterations").copied() {
                    Some(v) if (1.0..=64.0).contains(&v) && v.fract() == 0.0 => v as u32,
                    Some(_) => {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "joint `iterations` must be an integer in 1..=64".to_string(),
                        ))
                    }
                    None => 4,
                };
                out.push(Box::new(JointSystem {
                    a,
                    b,
                    kind,
                    rest,
                    stiffness,
                    damping,
                    axis,
                    anchor_a: (
                        anchor_raw.first().copied().unwrap_or(0.0),
                        anchor_raw.get(1).copied().unwrap_or(0.0),
                        anchor_raw.get(2).copied().unwrap_or(0.0),
                    ),
                    anchor_b: (0.0, 0.0, 0.0),
                    limit,
                    iterations,
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

/// Normalize a 3-vector (RFC-0039 axes); a zero vector falls back to +z.
fn normalize3(v: &[f64]) -> (f64, f64, f64) {
    let (x, y, z) = (
        v.first().copied().unwrap_or(0.0),
        v.get(1).copied().unwrap_or(0.0),
        v.get(2).copied().unwrap_or(0.0),
    );
    let len = (x * x + y * y + z * z).sqrt();
    if len < 1e-12 {
        (0.0, 0.0, 1.0)
    } else {
        (x / len, y / len, z / len)
    }
}

/// RFC: inline shape references (`shape A { part B at (x,y,z); … }`) into
/// concrete parts, applying the reference's `at` offset and `scale`. Errors on
/// an unknown shape name or a reference cycle (detail 79).
pub fn expand_shapes(
    shapes: &mut std::collections::BTreeMap<String, Vec<crate::components::ShapePart>>,
) -> Result<()> {
    use crate::components::ShapePart;
    fn resolve(
        name: &str,
        shapes: &std::collections::BTreeMap<String, Vec<ShapePart>>,
        stack: &mut Vec<String>,
        done: &mut std::collections::BTreeMap<String, Vec<ShapePart>>,
    ) -> Result<Vec<ShapePart>> {
        if let Some(v) = done.get(name) {
            return Ok(v.clone());
        }
        if stack.iter().any(|n| n == name) {
            return Err(error_at(
                Status::Invalid,
                79,
                0,
                format!("shape reference cycle at '{name}'"),
            ));
        }
        let parts = shapes
            .get(name)
            .cloned()
            .ok_or_else(|| error_at(Status::Invalid, 79, 0, format!("unknown shape '{name}'")))?;
        stack.push(name.to_string());
        let mut out = Vec::new();
        for p in parts {
            if p.kind == 7 {
                let sub = resolve(p.name.as_deref().unwrap_or(""), shapes, stack, done)?;
                for s in sub {
                    out.push(ShapePart {
                        offset: (
                            p.offset.0 + s.offset.0 * p.scale,
                            p.offset.1 + s.offset.1 * p.scale,
                            p.offset.2 + s.offset.2 * p.scale,
                        ),
                        scale: s.scale * p.scale,
                        ..s
                    });
                }
            } else {
                out.push(p);
            }
        }
        stack.pop();
        done.insert(name.to_string(), out.clone());
        Ok(out)
    }
    let names: Vec<String> = shapes.keys().cloned().collect();
    let mut done: std::collections::BTreeMap<String, Vec<ShapePart>> =
        std::collections::BTreeMap::new();
    for n in &names {
        let r = resolve(n, shapes, &mut Vec::new(), &mut done)?;
        shapes.insert(n.clone(), r);
    }
    Ok(())
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
    // Inline shape references so every consumer (including the artifact `run`
    // path) sees concrete parts.
    let mut parsed = merge_modules(modules, src.aliases.clone())?;
    expand_shapes(&mut parsed.model.shapes)?;
    Ok(parsed)
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
        for sd in &m.parsed.model.softs {
            model.softs.push(sd.clone());
        }
        for (name, def) in &m.parsed.model.structs {
            model.structs.insert(name.clone(), def.clone());
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
                LetStmt::If(c, t, e) => {
                    self.of_expr(c)?;
                    self.of_expr(t)?;
                    if let Some(e) = e {
                        self.of_expr(e)?;
                    }
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
                if let Some(text) = sys.assigns.get("expr").or_else(|| sys.update.get("expr")) {
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
            env.locals.insert("dt".to_string(), Some(dt_dim));
            env.of_lets(&lets)?;
            let rhs = env.of_expr(&expr)?;
            // `inte slot = expr` integrates as `slot += dt · expr`.
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
        // Assignment rules (`slot = expr`) are checked without the implicit
        // `dt` scaling that derivative rules carry.
        for (key, text) in &sys.assigns {
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
            let lhs = slot_dims.get(idx).copied().flatten();
            let expr = match parse_expr_str(text) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let mut env = DimEnv {
                slot_dims: &slot_dims,
                name_to_slot: &name_to_slot,
                params: &parsed.model.param_units,
                locals: Default::default(),
            };
            env.locals.insert("dt".to_string(), Some(dt_dim));
            env.of_lets(&lets)?;
            let rhs = env.of_expr(&expr)?;
            if unify(lhs, rhs).is_err() {
                let lhs_name = match lhs {
                    Some(d) => d.name(),
                    None => "1".to_string(),
                };
                let rhs_name = match rhs {
                    Some(d) => d.name(),
                    None => "1".to_string(),
                };
                return Err(error_at(
                    Status::Invalid,
                    77,
                    sys.byte_offset,
                    format!(
                        "rule `{text}` is not dimensionally consistent: `{lhs_name}` expected, \
right-hand side has `{rhs_name}`"
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
pub fn compile_program(mut parsed: ParsedProgram) -> Result<CompiledProgram> {
    check_dimensions(&parsed)?;
    // Inline `part <other-shape>` references so render paths see concrete parts.
    expand_shapes(&mut parsed.model.shapes)?;
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
    // RFC-0040: soft-body particles are dynamic entities too.
    let softs = parsed.model.soft_ranges();
    for (name, base, nx, ny, nz, _) in &softs {
        for k in 0..(nx * ny * nz) {
            entity_ids.insert(format!("{name}#{k}"), base + k as u128);
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
            // `vecN pos` names slots `pos.0 …`; alias the bare `pos` to `pos.0`.
            let entries: Vec<(String, usize)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
            for (k, v) in entries {
                if let Some(prefix) = k.strip_suffix(".0") {
                    map.entry(prefix.to_string()).or_insert(v);
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
        &softs,
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
        // Lower the body statements in order. `let`/loops go through the shared
        // block lowerer; a control-flow `if … { return … }` emits a real branch
        // (CondBr to two Return-terminated blocks), so calls nest on the call
        // stack and recursion terminates.
        for stmt in &lets {
            match stmt {
                LetStmt::If(cond, then_e, else_e) => {
                    let ctx = parts.ctx(&locals);
                    let c = truthy(
                        lower_expr(cond, &ctx, &mut next_id, &mut instrs),
                        &mut next_id,
                        &mut instrs,
                    );
                    let cb = instrs.len();
                    instrs.push(crate::physics_eir::instr(
                        crate::eir::Opcode::CondBr,
                        0,
                        None,
                        vec![c, 0, 0],
                        None,
                        None,
                    ));
                    let then_start = instrs.len() as u32;
                    let ctx_t = parts.ctx(&locals);
                    let rt = lower_expr(then_e, &ctx_t, &mut next_id, &mut instrs);
                    instrs.push(crate::physics_eir::instr(
                        crate::eir::Opcode::Return,
                        0,
                        None,
                        vec![rt],
                        None,
                        None,
                    ));
                    let else_target = if let Some(e) = else_e {
                        let else_start = instrs.len() as u32;
                        let ctx_e = parts.ctx(&locals);
                        let re = lower_expr(e, &ctx_e, &mut next_id, &mut instrs);
                        instrs.push(crate::physics_eir::instr(
                            crate::eir::Opcode::Return,
                            0,
                            None,
                            vec![re],
                            None,
                            None,
                        ));
                        else_start
                    } else {
                        instrs.len() as u32
                    };
                    instrs[cb].operands = vec![c, then_start, else_target];
                }
                other => {
                    lower_let_block(
                        std::slice::from_ref(other),
                        None,
                        &mut next_id,
                        &mut instrs,
                        &mut locals,
                        &parts,
                    );
                }
            }
        }
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

/// RFC-0038/RFC-0038: apply one `entity_field` grammar pair to a declaration
/// (shared by `entity` and `pool` declarations).
/// RFC-0042: flatten `state = (…)` (including nested records and `struct`
/// references) into flat scalar slots with dotted names (`pos.x`, `vel.y`, …).
fn parse_state_field(
    field: pest::iterators::Pair<'_, Rule>,
    decl: &mut EntityDecl,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
) -> Result<()> {
    let list = next_pair(&mut field.into_inner())?;
    let mut values: Vec<f64> = Vec::new();
    let mut names: Vec<Option<String>> = Vec::new();
    let mut units: Vec<Option<crate::units::Dim>> = Vec::new();
    match list.as_rule() {
        Rule::vecN => {
            values = list.into_inner().map(parse_value).collect();
            names = vec![None; values.len()];
            units = vec![None; values.len()];
        }
        Rule::type_ref => expand_struct(
            list.as_str(),
            "",
            structs,
            &mut values,
            &mut names,
            &mut units,
            &mut Vec::new(),
        )?,
        Rule::named_state => expand_named_state(
            list.into_inner(),
            "",
            structs,
            &mut values,
            &mut names,
            &mut units,
        )?,
        _ => {}
    }
    if values.len() > crate::components::State::MAX_STATE_SLOTS {
        return Err(error(Status::Invalid, 80));
    }
    decl.state = Some(values);
    decl.state_names = Some(names);
    if units.iter().any(|u| u.is_some()) {
        decl.state_units = Some(units);
    }
    Ok(())
}

fn state_unit(pair: Option<pest::iterators::Pair<'_, Rule>>) -> Option<crate::units::Dim> {
    pair.and_then(|p| {
        p.as_str()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<crate::units::Dim>()
            .ok()
    })
}

#[allow(clippy::too_many_arguments)]
fn expand_struct(
    name: &str,
    prefix: &str,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
    values: &mut Vec<f64>,
    names: &mut Vec<Option<String>>,
    units: &mut Vec<Option<crate::units::Dim>>,
    stack: &mut Vec<String>,
) -> Result<()> {
    use crate::dsl::StructFieldType;
    if stack.iter().any(|n| n == name) {
        return Err(error_at(
            Status::Invalid,
            80,
            0,
            format!("struct cycle at '{name}'"),
        ));
    }
    let def = structs.get(name).ok_or_else(|| {
        error_at(
            Status::Invalid,
            80,
            0,
            format!("unknown struct type '{name}'"),
        )
    })?;
    stack.push(name.to_string());
    for (fname, ft) in def {
        if values.len() >= crate::components::State::MAX_STATE_SLOTS {
            return Err(error(Status::Invalid, 80));
        }
        let full = format!("{prefix}{fname}");
        match ft {
            StructFieldType::Scalar(v) => {
                names.push(Some(full));
                values.push(*v);
                units.push(None);
            }
            StructFieldType::Struct(t) => {
                expand_struct(t, &format!("{full}."), structs, values, names, units, stack)?
            }
        }
    }
    stack.pop();
    Ok(())
}

fn expand_named_state(
    items: pest::iterators::Pairs<'_, Rule>,
    prefix: &str,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
    values: &mut Vec<f64>,
    names: &mut Vec<Option<String>>,
    units: &mut Vec<Option<crate::units::Dim>>,
) -> Result<()> {
    for item in items {
        expand_state_item(item, prefix, structs, values, names, units)?;
    }
    Ok(())
}

fn expand_state_item(
    item: pest::iterators::Pair<'_, Rule>,
    prefix: &str,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
    values: &mut Vec<f64>,
    names: &mut Vec<Option<String>>,
    units: &mut Vec<Option<crate::units::Dim>>,
) -> Result<()> {
    let text = item.as_str().trim();
    if let Some(rest) = text.strip_prefix("vec") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: usize = digits.parse().map_err(|_| error(Status::Invalid, 52))?;
        let vname = text[3 + digits.len()..].trim().to_string();
        let full = format!("{prefix}{vname}");
        // `vecN pos` reserves N aligned slots `pos.0 … pos.{N-1}`; the bare name
        // `pos` is an alias for component 0 (added to the name map).
        for k in 0..n {
            names.push(Some(format!("{full}.{k}")));
            values.push(0.0);
            units.push(None);
        }
        return Ok(());
    }
    let mut it = item.into_inner();
    match it.next() {
        Some(p) if p.as_rule() == Rule::ident => {
            let full = format!("{prefix}{}", p.as_str());
            let rhs = next_pair(&mut it)?;
            match rhs.as_rule() {
                Rule::named_state => expand_named_state(
                    rhs.into_inner(),
                    &format!("{full}."),
                    structs,
                    values,
                    names,
                    units,
                )?,
                Rule::type_ref => expand_struct(
                    rhs.as_str(),
                    &format!("{full}."),
                    structs,
                    values,
                    names,
                    units,
                    &mut Vec::new(),
                )?,
                Rule::value => {
                    names.push(Some(full));
                    values.push(parse_value(rhs));
                    units.push(state_unit(it.next()));
                }
                _ => {}
            }
        }
        Some(p) => {
            names.push(None);
            values.push(parse_value(p));
            units.push(state_unit(it.next()));
        }
        None => {}
    }
    Ok(())
}

fn apply_entity_field(
    field: pest::iterators::Pair<'_, Rule>,
    decl: &mut EntityDecl,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
) -> Result<()> {
    match field.as_rule() {
        Rule::position_field => {
            decl.position = Some(parse_vec3(next_pair(&mut field.into_inner())?))
        }
        Rule::rotation_field => {
            decl.rotation = Some(parse_vec3(next_pair(&mut field.into_inner())?))
        }
        Rule::velocity_field => {
            decl.velocity = Some(parse_vec3(next_pair(&mut field.into_inner())?))
        }
        Rule::state_field => parse_state_field(field, decl, structs)?,

        Rule::mass_field => decl.mass = Some(parse_value(next_pair(&mut field.into_inner())?)),
        Rule::dynamic_field => {
            decl.dynamic = Some(next_pair(&mut field.into_inner())?.as_str() == "true")
        }
        Rule::nbody_field => {
            decl.nbody = Some(next_pair(&mut field.into_inner())?.as_str() == "true")
        }
        Rule::parent_field => {
            decl.parent = Some(next_pair(&mut field.into_inner())?.as_str().to_string())
        }
        Rule::restitution_field => {
            decl.restitution = Some(parse_value(next_pair(&mut field.into_inner())?))
        }
        Rule::friction_field => {
            decl.friction = Some(parse_value(next_pair(&mut field.into_inner())?))
        }
        Rule::box_field => {
            let dims = parse_vec3(next_pair(&mut field.into_inner())?);
            decl.collider = Some(ColliderDecl::Box { dims });
        }
        Rule::sphere_field => {
            let radius = parse_value(next_pair(&mut field.into_inner())?);
            decl.collider = Some(ColliderDecl::Sphere { radius });
        }
        Rule::hull_field => {
            let list = next_pair(&mut field.into_inner())?;
            let points: Vec<Vec3> = list.into_inner().map(parse_vec3).collect();
            if points.len() < 4 {
                return Err(error(Status::Invalid, 51));
            }
            decl.collider = Some(ColliderDecl::ConvexHull { points });
        }
        Rule::camera_field => {
            decl.camera = Some(next_pair(&mut field.into_inner())?.as_str() == "true")
        }
        Rule::color_field => {
            let hex = next_pair(&mut field.into_inner())?.as_str();
            let v = u32::from_str_radix(&hex[2..], 16).map_err(|_| error(Status::Invalid, 64))?;
            decl.color = Some(v);
        }
        // Presentation-only render hints.
        Rule::shape_field => {
            let name = next_pair(&mut field.into_inner())?.as_str();
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
            let inner = next_pair(&mut field.into_inner())?;
            let r = decl.render.get_or_insert_with(Default::default);
            if inner.as_rule() == Rule::vec3 {
                let d = parse_vec3(inner);
                r.size3 = Some((d.x, d.y, d.z));
            } else {
                r.size = Some(parse_value(inner));
            }
        }
        Rule::opacity_field => {
            let v = parse_value(next_pair(&mut field.into_inner())?);
            decl.render.get_or_insert_with(Default::default).opacity = Some(v);
        }
        Rule::glow_field => {
            let v = parse_value(next_pair(&mut field.into_inner())?);
            decl.render.get_or_insert_with(Default::default).glow = Some(v);
        }
        Rule::orient_field => {
            let v = next_pair(&mut field.into_inner())?.as_str() == "true";
            decl.render.get_or_insert_with(Default::default).orient = v;
        }
        Rule::vector_field => {
            let v = next_pair(&mut field.into_inner())?.as_str() == "true";
            decl.render.get_or_insert_with(Default::default).no_velocity = !v;
        }
        Rule::label_field => {
            let v = next_pair(&mut field.into_inner())?.as_str() == "true";
            decl.render.get_or_insert_with(Default::default).label = Some(v);
        }
        _ => {}
    }
    Ok(())
}

mod runtime;
pub use runtime::*;

#[cfg(test)]
mod tests;
