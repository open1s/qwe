//! EIR system implementations lowered from the language: `update`, `rk4`,
//! `invariant`, `watch`, `joint`, `soft`, pool spawn/despawn, field solvers,
//! `send`/`recv` channels, and `nbody`.
#![allow(clippy::too_many_arguments)]
use super::*;

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
            match idx {
                Some(idx) => {
                    slots = slots.max(idx + 1);
                    resolved.push((idx, expr));
                }
                None => crate::lang::push_diag(
                    100,
                    0,
                    format!(
                        "assignment to unknown slot `{lhs}` — ignored (not in this entity's state; typo?)"
                    ),
                ),
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
                    None => {
                        crate::lang::push_diag(
                            100,
                            0,
                            format!(
                                "assignment to unknown slot `{lhs}` — ignored (not in this entity's state; typo?)"
                            ),
                        );
                        continue;
                    }
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
            match idx {
                Some(idx) => {
                    slots = slots.max(idx + 1);
                    resolved.push((idx, expr));
                }
                None => crate::lang::push_diag(
                    100,
                    0,
                    format!(
                        "assignment to unknown slot `{lhs}` — ignored (not in this entity's state; typo?)"
                    ),
                ),
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
pub struct InvariantSystem {
    /// The invariant expression: holds when truthy (non-zero, non-NaN).
    pub expr: Expr,
    /// `let name = expr` local bindings, computed sequentially before the check.
    pub lets: Vec<LetStmt>,
    /// Byte offset of this system's field within its hidden component. Each
    /// `invariant`/`conserved` system owns one offset so several coexist.
    pub check_offset: u32,
    /// The hidden component to write: `check_id()` (invariant verdict) or
    /// `conserved_id()` (conserved quantity value).
    pub component: pwe_api::ComponentTypeId,
    /// True for a `conserved` system (writes the quantity), false for an
    /// `invariant` (writes a 0/1 violation verdict).
    pub conserved: bool,
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
        if self.conserved {
            "lang.conserved"
        } else {
            "lang.invariant"
        }
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
        // `conserved` records the quantity itself; `invariant` records a 0/1
        // verdict (1 = violated, when the expression is exactly zero / NaN).
        let value = if self.conserved {
            expr_reg
        } else {
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
            viol
        };
        out.push(crate::physics_eir::instr(
            crate::eir::Opcode::WriteView,
            0,
            None,
            vec![value],
            None,
            Some(crate::physics_eir::cr(
                entity,
                self.component,
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
    /// Optional `on = <entity>`: restrict the send/recv to that entity (default:
    /// every dynamic entity, the historical behavior).
    pub only: Option<std::collections::BTreeSet<u128>>,
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
    /// Velocity-Verlet stage: 1 = half-kick + drift (writes v, p); 2 = second
    /// half-kick from the drifted (now committed) positions (writes v only).
    pub stage: u8,
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

        // Self state: px,py,pz,vx,vy,vz,m = slots 0..6 (committed/start-of-step).
        let px = nb_read(out, &mut next_id, entity, 0);
        let py = nb_read(out, &mut next_id, entity, 1);
        let pz = nb_read(out, &mut next_id, entity, 2);
        let vx = nb_read(out, &mut next_id, entity, 3);
        let vy = nb_read(out, &mut next_id, entity, 4);
        let vz = nb_read(out, &mut next_id, entity, 5);

        // a = acceleration at the current (committed) positions.
        let (ax, ay, az) = nb_accel(
            out,
            &mut next_id,
            entity,
            &self.bodies,
            self.g,
            eps,
            px,
            py,
            pz,
        );
        // Half kick: v_half = v + a·dt/2.
        let half = nb_const(out, &mut next_id, self.dt * 0.5);
        let hax = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, ax, half);
        let hay = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, ay, half);
        let haz = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, az, half);
        let vhx = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vx, hax);
        let vhy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vy, hay);
        let vhz = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, vz, haz);

        if self.stage == 1 {
            // Drift: p_new = p + v_half·dt; publish v_half and p_new.
            let dtc = nb_const(out, &mut next_id, self.dt);
            let dvx = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, vhx, dtc);
            let dvy = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, vhy, dtc);
            let dvz = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, vhz, dtc);
            let npx = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, px, dvx);
            let npy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, py, dvy);
            let npz = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, pz, dvz);
            nb_write(out, entity, 3, vhx);
            nb_write(out, entity, 4, vhy);
            nb_write(out, entity, 5, vhz);
            nb_write(out, entity, 0, npx);
            nb_write(out, entity, 1, npy);
            nb_write(out, entity, 2, npz);
        } else {
            // Second half kick at the drifted positions: the freshly-read v is
            // v_half, so v_final = v + a'·dt/2 = vhx (computed above).
            nb_write(out, entity, 3, vhx);
            nb_write(out, entity, 4, vhy);
            nb_write(out, entity, 5, vhz);
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

/// Accumulates the inverse-square acceleration on `entity` from every other
/// body, using the (committed) positions passed for `entity` and read for the
/// others. Shared by both velocity-Verlet stages.
fn nb_accel(
    out: &mut Vec<crate::eir::Instruction>,
    next_id: &mut u32,
    entity: u128,
    bodies: &[u128],
    g: f64,
    eps: f64,
    px: u32,
    py: u32,
    pz: u32,
) -> (u32, u32, u32) {
    let mut ax = nb_const(out, next_id, 0.0);
    let mut ay = nb_const(out, next_id, 0.0);
    let mut az = nb_const(out, next_id, 0.0);
    for &j in bodies {
        if j == entity {
            continue;
        }
        let jx = nb_read(out, next_id, j, 0);
        let jy = nb_read(out, next_id, j, 1);
        let jz = nb_read(out, next_id, j, 2);
        let jm = nb_read(out, next_id, j, 6);
        let dx = nb_arith(out, next_id, crate::eir::Opcode::Sub, jx, px);
        let dy = nb_arith(out, next_id, crate::eir::Opcode::Sub, jy, py);
        let dz = nb_arith(out, next_id, crate::eir::Opcode::Sub, jz, pz);
        let dx2 = nb_arith(out, next_id, crate::eir::Opcode::Mul, dx, dx);
        let dy2 = nb_arith(out, next_id, crate::eir::Opcode::Mul, dy, dy);
        let dz2 = nb_arith(out, next_id, crate::eir::Opcode::Mul, dz, dz);
        let sxy = nb_arith(out, next_id, crate::eir::Opcode::Add, dx2, dy2);
        let r2 = nb_arith(out, next_id, crate::eir::Opcode::Add, sxy, dz2);
        let epsc = nb_const(out, next_id, eps);
        let r2eps = nb_arith(out, next_id, crate::eir::Opcode::Add, r2, epsc);
        let r = nb_un(crate::eir::Opcode::Sqrt, out, next_id, r2eps);
        let r3 = nb_arith(out, next_id, crate::eir::Opcode::Mul, r2eps, r);
        let gc = nb_const(out, next_id, g);
        let gm = nb_arith(out, next_id, crate::eir::Opcode::Mul, gc, jm);
        let scale = nb_arith(out, next_id, crate::eir::Opcode::Div, gm, r3);
        let tx = nb_arith(out, next_id, crate::eir::Opcode::Mul, scale, dx);
        let ty = nb_arith(out, next_id, crate::eir::Opcode::Mul, scale, dy);
        let tz = nb_arith(out, next_id, crate::eir::Opcode::Mul, scale, dz);
        ax = nb_arith(out, next_id, crate::eir::Opcode::Add, ax, tx);
        ay = nb_arith(out, next_id, crate::eir::Opcode::Add, ay, ty);
        az = nb_arith(out, next_id, crate::eir::Opcode::Add, az, tz);
    }
    (ax, ay, az)
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

/// `pair { tag = <t>; dt = h; law = <expr in r> }` — a general pairwise force
/// between every pair of `<t>`-tagged entities (state slots `px,py,pz,vx,vy,vz,m`
/// = 0..6). `law` is the **force magnitude on `i` from `j`** as a function of the
/// pair distance `r` (positive = repulsive); e.g. `law = micro.lj_force(1,1,r)`.
/// Integrated with semi-implicit (symplectic) Euler:
/// `a = (Σ_j law(r_ij)·(p_i−p_j)/r_ij) / m`; `v += a·dt`; `p += v·dt`.
pub struct PairSystem {
    pub bodies: Vec<u128>,
    pub dt: f64,
    pub law: Expr,
    pub entity_map: std::collections::BTreeMap<String, u128>,
    pub state_names_by_id:
        std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    pub func_ids: std::collections::BTreeMap<String, u64>,
    pub field_dims: std::collections::BTreeMap<String, (u32, u32)>,
    pub namespace: String,
    pub param_names: std::collections::BTreeSet<String>,
}

impl EirSystem for PairSystem {
    fn name(&self) -> &'static str {
        "physics.pair"
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
        if !self.bodies.contains(&entity) {
            ret(out);
            return;
        }
        let eps = 1e-12;
        let mut next_id = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
        let px = nb_read(out, &mut next_id, entity, 0);
        let py = nb_read(out, &mut next_id, entity, 1);
        let pz = nb_read(out, &mut next_id, entity, 2);
        let vx = nb_read(out, &mut next_id, entity, 3);
        let vy = nb_read(out, &mut next_id, entity, 4);
        let vz = nb_read(out, &mut next_id, entity, 5);
        let m = nb_read(out, &mut next_id, entity, 6);
        let mut fx = nb_const(out, &mut next_id, 0.0);
        let mut fy = nb_const(out, &mut next_id, 0.0);
        let mut fz = nb_const(out, &mut next_id, 0.0);
        let sn = self
            .state_names_by_id
            .get(&entity)
            .cloned()
            .unwrap_or_default();
        let empty_refs: std::collections::BTreeMap<(u128, usize), u32> = Default::default();
        let empty_props: std::collections::BTreeMap<(u128, super::ast::PropKind), u32> =
            Default::default();
        for &j in &self.bodies {
            if j == entity {
                continue;
            }
            let jx = nb_read(out, &mut next_id, j, 0);
            let jy = nb_read(out, &mut next_id, j, 1);
            let jz = nb_read(out, &mut next_id, j, 2);
            let dx = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, px, jx);
            let dy = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, py, jy);
            let dz = nb_arith(out, &mut next_id, crate::eir::Opcode::Sub, pz, jz);
            let dx2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dx, dx);
            let dy2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dy, dy);
            let dz2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, dz, dz);
            let sxy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, dx2, dy2);
            let r2 = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, sxy, dz2);
            let epsc = nb_const(out, &mut next_id, eps);
            let r2e = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, r2, epsc);
            let r = nb_un(crate::eir::Opcode::Sqrt, out, &mut next_id, r2e);
            // Lower the user law with `r` bound to the pair distance.
            let mut locals: std::collections::BTreeMap<String, u32> = Default::default();
            locals.insert("r".to_string(), r);
            let ctx = super::lower::LowerCtx {
                slot_regs: &[],
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
            let f = super::lower::lower_expr(&self.law, &ctx, &mut next_id, out);
            // Force on i from j: f · (p_i − p_j)/r.
            let ux = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, dx, r);
            let uy = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, dy, r);
            let uz = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, dz, r);
            let t = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, f, ux);
            fx = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, fx, t);
            let t = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, f, uy);
            fy = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, fy, t);
            let t = nb_arith(out, &mut next_id, crate::eir::Opcode::Mul, f, uz);
            fz = nb_arith(out, &mut next_id, crate::eir::Opcode::Add, fz, t);
        }
        // a = f / m; then semi-implicit Euler: v += a·dt; p += v·dt.
        let dtc = nb_const(out, &mut next_id, self.dt);
        let ax = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, fx, m);
        let ay = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, fy, m);
        let az = nb_arith(out, &mut next_id, crate::eir::Opcode::Div, fz, m);
        let nvx = nb_add_mul(out, &mut next_id, vx, ax, dtc);
        let nvy = nb_add_mul(out, &mut next_id, vy, ay, dtc);
        let nvz = nb_add_mul(out, &mut next_id, vz, az, dtc);
        let npx = nb_add_mul(out, &mut next_id, px, nvx, dtc);
        let npy = nb_add_mul(out, &mut next_id, py, nvy, dtc);
        let npz = nb_add_mul(out, &mut next_id, pz, nvz, dtc);
        nb_write(out, entity, 3, nvx);
        nb_write(out, entity, 4, nvy);
        nb_write(out, entity, 5, nvz);
        nb_write(out, entity, 0, npx);
        nb_write(out, entity, 1, npy);
        nb_write(out, entity, 2, npz);
        ret(out);
    }
}
