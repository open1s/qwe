//! Build/compile layer: `build_systems` (system decls -> `EirSystem`s), the
//! `import`/module loader, `CompiledProgram`, `compile`/`compile_file`, and the
//! gradual unit (dimension) checker.
use super::*;

/// Builds the concrete `EirSystem` list from parsed system declarations.
#[allow(clippy::too_many_arguments)]
pub fn build_systems(
    systems: &[SystemDecl],
    entity_ids: &std::collections::BTreeMap<String, u128>,
    nbody_bodies: &[u128],
    tag_ids: &std::collections::BTreeMap<String, Vec<u128>>,
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
    let mut conserved_count: usize = 0;
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
                // Velocity-Verlet (kick-drift-kick) as two systems: stage 1
                // half-kicks and drifts; stage 2 finishes from the committed
                // drifted positions (see NbodySystem).
                let (g, dt) = (
                    param(&s.params, "G", s.byte_offset, &s.kind)?,
                    param(&s.params, "dt", s.byte_offset, &s.kind)?,
                );
                out.push(Box::new(NbodySystem {
                    bodies: nbody_bodies.to_vec(),
                    g,
                    dt,
                    stage: 1,
                }));
                out.push(Box::new(NbodySystem {
                    bodies: nbody_bodies.to_vec(),
                    g,
                    dt,
                    stage: 2,
                }));
            }
            "drift" => {
                let tag = s.string_params.get("tag").cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "drift requires `tag = <name>`".to_string(),
                    )
                })?;
                let bodies = tag_ids.get(&tag).cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        format!("drift references unknown tag '{tag}'"),
                    )
                })?;
                let dt = param(&s.params, "dt", s.byte_offset, &s.kind)?;
                let damp = s.params.get("damp").copied().unwrap_or(0.0);
                out.push(Box::new(DriftSystem { bodies, dt, damp }));
            }
            "pair" => {
                let tag = s.string_params.get("tag").cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "pair requires `tag = <name>`".to_string(),
                    )
                })?;
                let a = tag_ids.get(&tag).cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        format!("pair references unknown tag '{tag}'"),
                    )
                })?;
                let b = match s.string_params.get("other") {
                    Some(t) => tag_ids.get(t).cloned().ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!("pair references unknown tag '{t}'"),
                        )
                    })?,
                    None => a.clone(),
                };
                if a.len() + b.len() < 2 {
                    return Err(error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        "pair needs at least two entities".to_string(),
                    ));
                }
                // The group the coordination is measured over: a `cohort` tag,
                // else the union of `tag` and `other`.
                let cohort = match s.string_params.get("cohort") {
                    Some(t) => tag_ids.get(t).cloned().ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!("pair references unknown cohort '{t}'"),
                        )
                    })?,
                    None => {
                        let mut u = a.clone();
                        for x in &b {
                            if !u.contains(x) {
                                u.push(*x);
                            }
                        }
                        u
                    }
                };
                let dt = param(&s.params, "dt", s.byte_offset, &s.kind)?;
                let law_text = s
                    .assigns
                    .get("law")
                    .or_else(|| s.update.get("law"))
                    .ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            "pair requires `law = <force expr in r>`".to_string(),
                        )
                    })?;
                let law = parse_expr_str(law_text)?;
                out.push(Box::new(PairSystem {
                    a: a.clone(),
                    b,
                    dt,
                    law,
                    entity_map: entity_ids.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    coord: s.params.get("coord").copied(),
                    cohort,
                }));
            }
            "send" | "recv" => {
                let op = if s.kind == "send" {
                    ChanOp::Send
                } else {
                    ChanOp::Recv
                };
                let chan_name = s.string_params.get("chan").cloned().ok_or_else(|| {
                    error_at(
                        Status::Invalid,
                        48,
                        s.byte_offset,
                        format!("`{}` requires a `chan = <channel>` parameter", s.kind),
                    )
                })?;
                let channel_entity = entity_ids
                    .get(&chan_name)
                    .copied()
                    .ok_or(error(Status::Invalid, 62))?;
                // `on` is required: broadcast-to-all is never what mailbox
                // semantics want (last-writer-wins would be undefined).
                let only = {
                    let name = s.string_params.get("on").ok_or_else(|| {
                        error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!("`{}` requires `on = <entity>`", s.kind),
                        )
                    })?;
                    let id = *entity_ids.get(name).ok_or(error(Status::Invalid, 62))?;
                    Some(std::iter::once(id).collect())
                };
                let mut chan = ChanSystem {
                    op,
                    channel_entity,
                    value: None,
                    slot: 0,
                    only,
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
                warn_state_param_collisions(s, state_names_by_id, entity_ids);
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
                        // Enforce the slot bound for `sN` assignments too (the
                        // `+=`/`inte` path already checks it, detail 52).
                        if let Some(digits) = key.strip_prefix('s') {
                            if !digits.is_empty()
                                && !key.contains('[')
                                && digits.bytes().all(|b| b.is_ascii_digit())
                                && digits
                                    .parse::<usize>()
                                    .map(|i| i >= crate::components::State::MAX_STATE_SLOTS)
                                    .unwrap_or(false)
                            {
                                return Err(error_at(
                                    Status::Invalid,
                                    52,
                                    s.byte_offset,
                                    format!("state slot index in `{key}` is out of range"),
                                ));
                            }
                        }
                        assigns.push((key.clone(), parse_expr_str(text)?));
                    }
                }
                if rules.is_empty()
                    && dyn_rules.is_empty()
                    && assigns.is_empty()
                    && dyn_assigns.is_empty()
                    && s.update_stmts.is_empty()
                {
                    return Err(error_at(
                        Status::Invalid,
                        55,
                        s.byte_offset,
                        "update system has no rules or statements; add `slot = <expr>` or a \
side-effecting `let`/call"
                            .to_string(),
                    ));
                }
                // `let name = expr` local bindings, in order.
                let lets = to_let_stmts(&s.update_stmts, s.byte_offset)?;
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
                warn_state_param_collisions(s, state_names_by_id, entity_ids);
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
                // `rk4` integrates derivatives (`inte slot = rate`); a plain
                // assignment (`slot = expr`) has no rk4 meaning and was silently
                // dropped — reject it explicitly (detail 93).
                if !s.assigns.is_empty() {
                    let key = s.assigns.keys().next().cloned().unwrap_or_default();
                    return Err(error_at(
                        Status::Invalid,
                        93,
                        s.byte_offset,
                        format!(
                            "`{key} = …` is a plain assignment, which is `update`-only; in \
`rk4` write `inte {key} = <rate>` to integrate a derivative"
                        ),
                    ));
                }
                if rules.is_empty() {
                    return Err(error_at(
                        Status::Invalid,
                        55,
                        s.byte_offset,
                        "rk4 system has no rules; add `inte slot = <rate>`".to_string(),
                    ));
                }
                let lets = to_let_stmts(&s.update_stmts, s.byte_offset)?;
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
                let lets = to_let_stmts(&s.update_stmts, s.byte_offset)?;
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
                    component: crate::physics_eir::check_id(),
                    conserved: false,
                    entity_map: entity_ids.clone(),
                    only,
                    func_ids: func_ids.clone(),
                    field_dims: field_dims.clone(),
                    namespace: s.namespace.clone(),
                    param_names: param_names.clone(),
                    state_names_by_id: state_names_by_id.clone(),
                }));
            }
            "conserved" => {
                // A conserved quantity: the runtime tracks its drift over the run
                // and fails (detail 87) past the `tolerance` (default 1e-4).
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
                            "conserved is missing required parameter 'expr'".to_string(),
                        )
                    })?;
                let expr = parse_expr_str(&expr_text)?;
                let lets = to_let_stmts(&s.update_stmts, s.byte_offset)?;
                let only = match s.string_params.get("on") {
                    Some(name) => {
                        Some(resolve_on(entity_ids, name).ok_or(error(Status::Invalid, 62))?)
                    }
                    None => None,
                };
                let check_offset =
                    (conserved_count as u32) * crate::physics_eir::field::STATE_SLOT_BYTES;
                conserved_count += 1;
                out.push(Box::new(InvariantSystem {
                    expr,
                    lets,
                    check_offset,
                    component: crate::physics_eir::conserved_id(),
                    conserved: true,
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
                for key in ["mem", "into"] {
                    if !s.params.contains_key(key)
                        && (s.string_params.contains_key(key) || s.assigns.contains_key(key))
                    {
                        return Err(error_at(
                            Status::Invalid,
                            48,
                            s.byte_offset,
                            format!(
                                "system 'watch' parameter '{key}' expects a slot index (number), got identifier `{}`",
                                s.string_params
                                    .get(key)
                                    .or_else(|| s.assigns.get(key))
                                    .map(String::as_str)
                                    .unwrap_or("")
                            ),
                        ));
                    }
                }
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
pub(crate) fn resolve_on(
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
pub(crate) fn normalize3(v: &[f64]) -> (f64, f64, f64) {
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
                        // A shape-ref's own display options override the inlined
                        // sub-part's ones (orbit radius scales like offsets).
                        orbit: p
                            .orbit
                            .or(s.orbit)
                            .map(|(radius, speed, phase)| (radius * p.scale, speed, phase)),
                        color: p.color.or(s.color),
                        opacity: p.opacity.or(s.opacity),
                        orbit_axis: p.orbit_axis.or(s.orbit_axis),
                        spin: if p.spin != 0.0 { p.spin } else { s.spin },
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

pub(crate) fn dynamic_entity_ids(model: &WorldModel) -> Vec<u128> {
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
pub(crate) struct ImportDirective {
    path: String,
    alias: Option<String>,
    /// `Some(names)` for `from … import …` (bind these names unqualified).
    names: Option<Vec<String>>,
}

/// One loaded module: its namespace, path, import-stripped source, parsed
/// program, and its own import directives (with resolved child paths).
pub(crate) struct ModuleInfo {
    /// The namespace used by this module's own rules (its first alias).
    ns: String,
    /// Every namespace this module has been imported under.
    aliases: Vec<String>,
    path: std::path::PathBuf,
    source: String,
    parsed: ParsedProgram,
    imports: Vec<(ImportDirective, std::path::PathBuf)>,
    /// A declared `module <name>` (RFC-0045), if any — the module's stable id.
    declared_name: Option<String>,
    /// A declared `module <name> <version>` version, if any (recorded).
    declared_version: Option<String>,
    /// A declared `export …` surface (RFC-0045); `None` = everything public.
    exports: Option<Vec<String>>,
}

pub(crate) fn ident_tokens(text: &str) -> Option<Vec<String>> {
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

/// Parses a `module <dotted.name>` line directive (RFC-0045): a module's stable
/// identity, independent of its file path. Returns the declared name.
pub(crate) fn parse_module_line(line: &str) -> Option<(String, Option<String>)> {
    let t = line.trim();
    let rest = t.strip_prefix("module")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let mut it = rest.split_whitespace();
    let name = it.next()?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    let version = it.next().map(str::to_string);
    Some((name.to_string(), version))
}

/// Parses an `export a, b` (or `export { a, b }`) line directive (RFC-0045):
/// the module's public surface. Returns the exported names.
pub(crate) fn parse_export_line(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    let rest = t.strip_prefix("export")?;
    if !(rest.starts_with(|c: char| c.is_whitespace()) || rest.starts_with('{')) {
        return None;
    }
    let list = rest.trim().trim_start_matches('{').trim_end_matches('}');
    let names: Vec<String> = list
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

/// The result of removing directive lines from a module source.
pub(crate) struct StripResult {
    pub source: String,
    pub declared_name: Option<String>,
    pub declared_version: Option<String>,
    pub exports: Option<Vec<String>>,
    pub imports: Vec<(ImportDirective, String)>,
}

/// Updates the string-literal state across one line, **ignoring quotes inside a
/// comment** (`#` / `//`): a `"` in a comment must not open a string (#68).
fn scan_string_state(line: &str, mut in_string: bool) -> bool {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_string {
            if c == b'"' {
                in_string = false;
            }
        } else if c == b'#' || (c == b'/' && b.get(i + 1) == Some(&b'/')) {
            break; // comment: the rest of the line cannot open/close a string
        } else if c == b'"' {
            in_string = true;
        }
        i += 1;
    }
    in_string
}

/// Removes `module` / `export` / `import` directive lines from `src` and
/// collects them. **String-aware**: a directive-looking line inside a
/// multi-line string literal is preserved verbatim (RFC-0045 / #66).
pub(crate) fn strip_directives(src: &str) -> StripResult {
    let mut source = String::new();
    let mut declared_name = None;
    let mut declared_version = None;
    let mut exports: Vec<String> = Vec::new();
    let mut exports_declared = false;
    let mut imports = Vec::new();
    let mut in_string = false;
    for line in src.lines() {
        if in_string {
            source.push_str(line);
            source.push('\n');
            in_string = scan_string_state(line, in_string);
            continue;
        }
        if let Some((n, v)) = parse_module_line(line) {
            declared_name = Some(n);
            declared_version = v;
            source.push('\n');
            continue;
        }
        if let Some(ns) = parse_export_line(line) {
            exports_declared = true;
            exports.extend(ns);
            source.push('\n');
            continue;
        }
        if let Some((d, tail)) = parse_import_line(line) {
            imports.push((d, tail.clone()));
            source.push_str(&tail);
            source.push('\n');
            continue;
        }
        source.push_str(line);
        source.push('\n');
        in_string = scan_string_state(line, in_string);
    }
    StripResult {
        source,
        declared_name,
        declared_version,
        exports: exports_declared.then_some(exports),
        imports,
    }
}

/// Parses a Python-style import directive at the start of a line, returning it
/// and the remainder of the line (preserved so `world { import "x" }` keeps
/// its brace).
pub(crate) fn parse_import_line(line: &str) -> Option<(ImportDirective, String)> {
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
pub(crate) fn resolve_module_path(dir: &std::path::Path, spec: &str) -> std::path::PathBuf {
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
pub(crate) fn module_stem(spec: &str) -> String {
    let last = spec.rsplit('/').next().unwrap_or(spec);
    if last == "__init__" {
        spec.rsplit('/').nth(1).unwrap_or(spec).to_string()
    } else {
        last.trim_end_matches(".pwe").to_string()
    }
}

/// Recursively loads a module and its imports (deduplicated by canonical path;
/// a module keeps the namespace of the first import that reached it).
pub(crate) fn collect_module(
    path: &std::path::Path,
    ns: &str,
    seen: &mut std::collections::BTreeMap<std::path::PathBuf, usize>,
    out: &mut Vec<ModuleInfo>,
    root_override: Option<&str>,
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
    // The root may be supplied in memory (an editor buffer): parse it as the
    // root and resolve its imports from disk, ignoring the disk root entirely so
    // a broken on-disk file cannot mask a valid buffer (RFC-0045 / #62).
    let raw = match root_override {
        Some(src) => src.to_string(),
        None => std::fs::read_to_string(path).map_err(|e| {
            error_at(
                Status::Invalid,
                76,
                0,
                format!("cannot read {}: {e}", path.display()),
            )
        })?,
    };
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let strip = strip_directives(&raw);
    let declared_name = strip.declared_name;
    let declared_version = strip.declared_version;
    let exports = strip.exports;
    let imports: Vec<(ImportDirective, std::path::PathBuf)> = strip
        .imports
        .iter()
        .map(|(d, _)| (d.clone(), resolve_module_path(&dir, &d.path)))
        .collect();
    let parsed = parse(&strip.source)?;
    let children: Vec<(ImportDirective, std::path::PathBuf)> = imports.clone();
    let mut aliases = vec![ns.to_string()];
    if let Some(dn) = &declared_name {
        if !aliases.contains(dn) {
            aliases.push(dn.clone());
        }
    }
    out.push(ModuleInfo {
        ns: ns.to_string(),
        aliases,
        path: path.to_path_buf(),
        source: strip.source,
        parsed,
        imports,
        declared_name,
        declared_version,
        exports,
    });
    for (d, child) in children {
        let default_ns = d.alias.clone().unwrap_or_else(|| module_stem(&d.path));
        collect_module(&child, &default_ns, seen, out, None)?;
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
        declared_name: strip_directives(&src.root).declared_name,
        declared_version: strip_directives(&src.root).declared_version,
        exports: None,
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
            declared_name: strip_directives(source).declared_name,
            declared_version: strip_directives(source).declared_version,
            exports: None,
            source: source.clone(),
            parsed,
            imports: Vec::new(),
        });
    }
    // RFC-0045: enforce duplicate declared module names here too (the
    // no-filesystem rebuild path).
    check_duplicate_module_names(&modules)?;
    // Inline shape references so every consumer (including the artifact `run`
    // path) sees concrete parts.
    let mut parsed = merge_modules(modules, src.aliases.clone())?;
    expand_shapes(&mut parsed.model.shapes)?;
    Ok(parsed)
}

/// Loads a program file and its module imports, returning both the merged
/// program and the sources needed to rebuild it without the filesystem.
pub fn load_program_sources(path: &std::path::Path) -> Result<(ParsedProgram, ProgramSources)> {
    load_program_sources_inner(path, None)
}

/// Like [`load_program_sources`], but uses `root` as the **root module's
/// source** (an editor buffer) while still resolving imports from the filesystem
/// relative to `path`. Used by the LSP so diagnostics track unsaved edits.
pub fn load_program_sources_with_root(
    path: &std::path::Path,
    root: &str,
) -> Result<(ParsedProgram, ProgramSources)> {
    load_program_sources_inner(path, Some(root))
}

fn load_program_sources_inner(
    path: &std::path::Path,
    root_override: Option<&str>,
) -> Result<(ParsedProgram, ProgramSources)> {
    let mut modules = Vec::new();
    let mut seen: std::collections::BTreeMap<std::path::PathBuf, usize> = Default::default();
    collect_module(path, "", &mut seen, &mut modules, root_override)?;
    check_duplicate_module_names(&modules)?;
    // RFC-0045: deterministic merge order (root first; the rest by namespace
    // then path), independent of filesystem discovery order.
    if modules.len() > 1 {
        let root = modules.remove(0);
        modules.sort_by(|a, b| {
            a.ns.cmp(&b.ns)
                .then_with(|| a.declared_version.cmp(&b.declared_version))
                .then_with(|| a.path.cmp(&b.path))
        });
        modules.insert(0, root);
    }
    // Module exports by namespace, for `from … import …` privacy (RFC-0045).
    let mut exports_by_ns: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    for m in &modules {
        if let Some(ex) = &m.exports {
            let set: std::collections::BTreeSet<String> = ex.iter().cloned().collect();
            for a in &m.aliases {
                if !a.is_empty() {
                    exports_by_ns.insert(a.clone(), set.clone());
                }
            }
        }
    }
    // Resolve `from … import …` alias requests against the child's namespace,
    // enforcing its `export` surface (detail 102).
    let mut aliases: Vec<(String, String)> = Vec::new();
    for m in &modules {
        for (d, child) in &m.imports {
            if let Some(names) = &d.names {
                // Resolve the child's namespace by canonical path against the
                // CURRENT `modules` order (the `seen` index was built before the
                // deterministic reorder and is stale — #70).
                let canon = child.canonicalize().unwrap_or_else(|_| child.clone());
                let child_ns = modules
                    .iter()
                    .find(|m| m.path.canonicalize().unwrap_or_else(|_| m.path.clone()) == canon)
                    .map(|m| m.ns.clone())
                    .unwrap_or_else(|| module_stem(&d.path));
                for n in names {
                    if let Some(ex) = exports_by_ns.get(&child_ns) {
                        if !ex.contains(n) {
                            return Err(error_at(
                                Status::Invalid,
                                102,
                                0,
                                format!("`{n}` is not exported by module `{child_ns}`"),
                            ));
                        }
                    }
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
    // RFC-0045: enforce the `export` surface on cross-module references.
    let mut export_map: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    for m in &modules {
        if let Some(ex) = &m.exports {
            let set: std::collections::BTreeSet<String> = ex.iter().cloned().collect();
            for a in &m.aliases {
                if !a.is_empty() {
                    export_map.insert(a.clone(), set.clone());
                }
            }
        }
    }
    let parsed = merge_modules(modules, aliases)?;
    check_module_privacy(&parsed, &export_map)?;
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
/// RFC-0045: a duplicate declared module name is a hard error (101).
fn check_duplicate_module_names(modules: &[ModuleInfo]) -> Result<()> {
    let mut names: std::collections::BTreeMap<String, String> = Default::default();
    for m in modules {
        if let Some(n) = &m.declared_name {
            if let Some(prev) = names.get(n) {
                return Err(error_at(
                    Status::Invalid,
                    101,
                    0,
                    format!(
                        "duplicate module name `{n}` (in {prev} and {})",
                        m.path.display()
                    ),
                ));
            }
            names.insert(n.clone(), m.path.display().to_string());
        }
    }
    Ok(())
}

/// RFC-0045: rejects a cross-module reference to a name a module did not
/// `export` (detail 102). Modules without an `export` declaration are fully
/// public (backward compatible).
fn check_module_privacy(
    parsed: &ParsedProgram,
    exports: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> Result<()> {
    if exports.is_empty() {
        return Ok(());
    }
    fn calls(e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::Call(n, args) => {
                out.push(n.to_string());
                for a in args {
                    calls(a, out);
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
                calls(a, out);
                calls(b, out);
            }
            Expr::Neg(a) | Expr::Not(a) | Expr::SlotDyn(a) => calls(a, out),
            _ => {}
        }
    }
    for sys in &parsed.systems {
        let mut names = Vec::new();
        for text in sys.update.values().chain(sys.assigns.values()) {
            if let Ok(e) = parse_expr_str(text) {
                calls(&e, &mut names);
            }
        }
        for st in &sys.update_stmts {
            if let crate::lang::UpdateStmt::Let(_, t, _) = st {
                if let Ok(e) = parse_expr_str(t) {
                    calls(&e, &mut names);
                }
            }
        }
        if let Some(w) = sys.string_params.get("when") {
            if let Ok(e) = parse_expr_str(w) {
                calls(&e, &mut names);
            }
        }
        for n in names {
            if let Some((ns, item)) = n.rsplit_once('.') {
                if let Some(ex) = exports.get(ns) {
                    if !ex.contains(item) && ns != sys.namespace {
                        return Err(error_at(
                            Status::Invalid,
                            102,
                            0,
                            format!("`{item}` is not exported by module `{ns}`"),
                        ));
                    }
                }
            }
        }
    }
    // Function bodies (`funcs`) may also make qualified calls (#67).
    for f in &parsed.funcs {
        let mut names = Vec::new();
        calls(&f.body, &mut names);
        for st in &f.stmts {
            if let crate::lang::UpdateStmt::Let(_, t, _) = st {
                if let Ok(e) = parse_expr_str(t) {
                    calls(&e, &mut names);
                }
            }
        }
        for n in names {
            if let Some((ns, item)) = n.rsplit_once('.') {
                if let Some(ex) = exports.get(ns) {
                    if !ex.contains(item) && ns != f.namespace {
                        return Err(error_at(
                            Status::Invalid,
                            102,
                            0,
                            format!("`{item}` is not exported by module `{ns}`"),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn merge_modules(
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
        if model.lang_version.is_none() {
            model.lang_version = m.parsed.model.lang_version.clone();
        }
        if !model.units_strict {
            model.units_strict = m.parsed.model.units_strict;
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
        // Render bonds (`bond a b`) merge by name-pair, deduplicated so a module
        // imported under several namespaces does not repeat its bonds.
        for b in &m.parsed.model.bonds {
            if !model.bonds.contains(b) {
                model.bonds.push(b.clone());
            }
        }
        for n in &m.parsed.model.bond_nets {
            if !model.bond_nets.contains(n) {
                model.bond_nets.push(n.clone());
            }
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
pub(crate) struct DimEnv<'a> {
    /// Byte offset of the system being checked (for diagnostics).
    pub(crate) offset: usize,
    /// User-function signatures: name -> (per-param unit, return unit).
    pub(crate) funcs: &'a std::collections::BTreeMap<
        String,
        (Vec<crate::units::MaybeDim>, crate::units::MaybeDim),
    >,
    slot_dims: &'a [crate::units::MaybeDim],
    name_to_slot: &'a std::collections::BTreeMap<String, usize>,
    params: &'a std::collections::BTreeMap<String, crate::units::Dim>,
    locals: std::collections::BTreeMap<String, crate::units::MaybeDim>,
}

impl DimEnv<'_> {
    fn of_expr(&self, expr: &Expr) -> Result<crate::units::MaybeDim> {
        use crate::units::{div, mul, unify, Dim, MaybeDim};
        let err = || {
            error_at(
                Status::Invalid,
                77,
                self.offset,
                "dimension mismatch within an expression".to_string(),
            )
        };
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
                    // A user function: unify each argument with its declared
                    // parameter unit; the call's dimension is the return unit.
                    _ => match self.funcs.get(*name) {
                        Some((params, ret)) => {
                            for (i, u) in params.iter().enumerate() {
                                unify(arg(i)?, *u).map_err(|_| err())?;
                            }
                            *ret
                        }
                        None => None,
                    },
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
pub(crate) fn check_dimensions(parsed: &ParsedProgram) -> Result<()> {
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
    // `units = "strict"`: every state slot and param must be annotated, and the
    // dimensional checks are always active.
    if parsed.model.units_strict {
        any_units = true;
        for e in &parsed.model.entities {
            let n = e.state.as_ref().map(|v| v.len()).unwrap_or(0);
            for i in 0..n {
                let annotated = e
                    .state_units
                    .as_ref()
                    .and_then(|u| u.get(i))
                    .copied()
                    .flatten()
                    .is_some();
                if !annotated {
                    return Err(error_at(
                        Status::Invalid,
                        90,
                        0,
                        format!(
                            "entity `{}` slot {i} needs a unit annotation (units = \"strict\")",
                            e.name
                        ),
                    ));
                }
            }
        }
        for k in parsed.model.params.keys() {
            if !parsed.model.param_units.contains_key(k) {
                return Err(error_at(
                    Status::Invalid,
                    90,
                    0,
                    format!("parameter `{k}` needs a unit annotation (units = \"strict\")"),
                ));
            }
        }
    }
    if !any_units {
        return Ok(());
    }
    // User-function signatures (bare and namespace-qualified names).
    let mut func_sigs: std::collections::BTreeMap<
        String,
        (Vec<crate::units::MaybeDim>, crate::units::MaybeDim),
    > = Default::default();
    for f in &parsed.funcs {
        let sig = (f.param_units.clone(), f.ret_unit);
        func_sigs.insert(f.name.clone(), sig.clone());
        if !f.namespace.is_empty() {
            func_sigs.insert(format!("{}.{}", f.namespace, f.name), sig);
        }
    }
    for sys in &parsed.systems {
        // Check every declared expression for internal consistency: rule bodies
        // and `let`s (update/rk4), the `when` gate, `invariant`/`watch` exprs.
        {
            let env = DimEnv {
                offset: sys.byte_offset,
                funcs: &func_sigs,
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
        let lets = to_let_stmts(&sys.update_stmts, sys.byte_offset)?;
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
                offset: sys.byte_offset,
                funcs: &func_sigs,
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
                offset: sys.byte_offset,
                funcs: &func_sigs,
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
    // Strip directive lines (`module`/`export`/`import`) so the in-memory path
    // (LSP, `LangRuntime::compile`) parses module-declaring sources too.
    compile_program(parse(&strip_directives(source).source)?)
}

/// The language semantics this build implements (frozen at v0.3). A model may
/// pin it with `world { lang_version = "0.3" }`; absence means "current".
pub const LANG_VERSION: &str = "0.3";

/// `lang_version` values this build accepts.
pub const SUPPORTED_LANG_VERSIONS: &[&str] = &["0.3"];

/// Rejects an explicit field solver whose settings violate the numerical
/// stability limit (detail 86): explicit diffusion (`T += rate·∇²T`) needs
/// `rate ≤ dx²/(2·dim)`, and the leapfrog wave needs `c·dt/dx ≤ 1/√dim`.
fn check_solver_stability(
    parsed: &ParsedProgram,
    field_info: &std::collections::BTreeMap<String, (u32, u32, u32, f64)>,
) -> Result<()> {
    let dim_of = |f: &str| -> Option<f64> {
        let (w, h, d, _) = field_info.get(f).copied()?;
        Some(((w > 1) as u32 + (h > 1) as u32 + (d > 1) as u32).max(1) as f64)
    };
    for s in &parsed.systems {
        match s.kind.as_str() {
            "diffuse" => {
                let Some(field) = s.string_params.get("field") else {
                    continue;
                };
                let (Some((_, _, _, dx)), Some(dim)) =
                    (field_info.get(field).copied(), dim_of(field))
                else {
                    continue;
                };
                let rate = s.params.get("rate").copied().unwrap_or(0.0);
                let limit = dx * dx / (2.0 * dim);
                if rate > limit * (1.0 + 1e-9) {
                    return Err(error_at(
                        Status::Invalid,
                        86,
                        s.byte_offset,
                        format!(
                            "diffuse `rate = {rate}` exceeds the explicit stability limit {limit:.4} (= dx²/(2·dim)) for a {dim:.0}-D field; lower `rate` or reduce `dx`"
                        ),
                    ));
                }
            }
            "wave" => {
                let Some(field) = s.string_params.get("field") else {
                    continue;
                };
                let (Some((_, _, _, dx)), Some(dim)) =
                    (field_info.get(field).copied(), dim_of(field))
                else {
                    continue;
                };
                let cfl = s.params.get("velocity").copied().unwrap_or(1.0)
                    * s.params.get("dt").copied().unwrap_or(0.0)
                    / dx;
                let limit = 1.0 / dim.sqrt();
                if cfl > limit * (1.0 + 1e-9) {
                    return Err(error_at(
                        Status::Invalid,
                        86,
                        s.byte_offset,
                        format!(
                            "wave CFL c·dt/dx = {cfl:.4} exceeds {limit:.4} (1/√dim) for a {dim:.0}-D field; lower `velocity`/`dt` or reduce `dx`"
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Warns (detail 94) when a state slot name collides with one of the system
/// kind's numeric parameter names — the assignment would silently configure the
/// parameter instead of writing the slot (issue #18).
/// #6: warns (detail 96) when a rule writes state slots 7/8/9 of an entity that
/// does not set `orient = true`, or when `orient = true` but the entity has
/// fewer than 9 slots (the euler convention needs slots 7/8/9).
fn warn_orient_slot_writes(
    parsed: &ParsedProgram,
    entity_ids: &std::collections::BTreeMap<String, u128>,
    state_names_by_id: &std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
) {
    for sys in &parsed.systems {
        let Some(on_name) = sys.string_params.get("on") else {
            continue;
        };
        let Some(&id) = entity_ids.get(on_name) else {
            continue;
        };
        let Some(e) = parsed.model.entities.get((id - 1) as usize) else {
            continue;
        };
        let orient = e.render.as_ref().map(|r| r.orient).unwrap_or(false);
        let slots = e.state.as_ref().map(|v| v.len()).unwrap_or(0);
        let names = state_names_by_id.get(&id);
        for key in sys.assigns.keys().chain(sys.update.keys()) {
            let idx = crate::lang::parser::numeric_slot(key)
                .or_else(|| names.and_then(|m| m.get(key.as_str()).copied()));
            if let Some(idx) = idx {
                if (7..=9).contains(&idx) && !orient {
                    push_diag(
                        96,
                        sys.byte_offset,
                        format!(
                            "rule writes state slot {idx} of `{on_name}` but `orient` is not true; \
slots 7/8/9 are euler angles only with `orient = true` (otherwise slot 7 is a Z-spin)"
                        ),
                    );
                }
            }
        }
        if orient && slots < 9 {
            push_diag(
                96,
                sys.byte_offset,
                format!(
                    "`{on_name}` sets `orient = true` but has only {slots} state slots; euler \
orientation needs slots 7/8/9"
                ),
            );
        }
    }
}

fn warn_state_param_collisions(
    s: &SystemDecl,
    state_names_by_id: &std::collections::BTreeMap<u128, std::collections::BTreeMap<String, usize>>,
    entity_ids: &std::collections::BTreeMap<String, u128>,
) {
    let keys = numeric_param_keys(&s.kind);
    if keys.is_empty() {
        return;
    }
    let mut names: std::collections::BTreeSet<&str> = Default::default();
    for id in entity_ids.values() {
        if let Some(m) = state_names_by_id.get(id) {
            for n in m.keys() {
                names.insert(n.as_str());
            }
        }
    }
    for k in keys {
        if names.contains(k) {
            push_diag(
                94,
                0,
                format!(
                    "state slot `{k}` collides with the `{}` parameter name; `{k} = …` in `{}` \
sets the parameter, not the slot",
                    s.kind, s.kind
                ),
            );
        }
    }
}

/// Rejects a user-`funcs` call whose argument count does not match the
/// definition (detail 59). Builtins are checked at lowering; user functions
/// previously were not checked at all (extra args silently ignored).
fn check_call_arities(parsed: &ParsedProgram) -> Result<()> {
    let mut arity: std::collections::BTreeMap<String, usize> = Default::default();
    for f in &parsed.funcs {
        arity.insert(f.name.clone(), f.params.len());
        if !f.namespace.is_empty() {
            arity.insert(format!("{}.{}", f.namespace, f.name), f.params.len());
        }
    }
    fn walk(e: &Expr, arity: &std::collections::BTreeMap<String, usize>) -> Result<()> {
        match e {
            Expr::Call(name, args) => {
                if let Some(&n) = arity.get(*name) {
                    if args.len() != n {
                        return Err(error_at(
                            Status::Invalid,
                            59,
                            0,
                            format!("`{name}` expects {n} argument(s), got {}", args.len()),
                        ));
                    }
                } else if !crate::lang::parser::is_builtin_call(name) {
                    return Err(error_at(
                        Status::Invalid,
                        59,
                        0,
                        format!(
                            "`{name}` is not a builtin function or a user-defined `funcs` function"
                        ),
                    ));
                }
                for a in args {
                    walk(a, arity)?;
                }
                Ok(())
            }
            Expr::SlotDyn(a) | Expr::Neg(a) | Expr::Not(a) => walk(a, arity),
            Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Mul(a, b)
            | Expr::Div(a, b)
            | Expr::Rem(a, b)
            | Expr::Cmp(_, a, b)
            | Expr::And(a, b)
            | Expr::Or(a, b) => {
                walk(a, arity)?;
                walk(b, arity)
            }
            _ => Ok(()),
        }
    }
    fn walk_lets(
        stmts: &[LetStmt],
        arity: &std::collections::BTreeMap<String, usize>,
    ) -> Result<()> {
        for s in stmts {
            match s {
                LetStmt::Let(_, e) => walk(e, arity)?,
                LetStmt::If(c, t, e) => {
                    walk(c, arity)?;
                    walk(t, arity)?;
                    if let Some(e) = e {
                        walk(e, arity)?;
                    }
                }
                LetStmt::Repeat(_, b) | LetStmt::For(_, _, _, b) => walk_lets(b, arity)?,
                LetStmt::Break(c) | LetStmt::Continue(c) => {
                    if let Some(e) = c {
                        walk(e, arity)?;
                    }
                }
            }
        }
        Ok(())
    }
    for sys in &parsed.systems {
        let lets = to_let_stmts(&sys.update_stmts, sys.byte_offset)?;
        walk_lets(&lets, &arity)?;
        for text in sys.update.values().chain(sys.assigns.values()) {
            walk(&parse_expr_str(text)?, &arity)?;
        }
        if let Some(w) = sys.string_params.get("when") {
            if let Ok(e) = parse_expr_str(w) {
                walk(&e, &arity)?;
            }
        }
    }
    for f in &parsed.funcs {
        walk(&f.body, &arity)?;
        let lets = to_let_stmts(&f.stmts, 0)?;
        walk_lets(&lets, &arity)?;
    }
    Ok(())
}

/// Rejects a present-but-unsupported `lang_version` (detail 83).
fn check_lang_version(parsed: &ParsedProgram) -> Result<()> {
    if let Some(v) = &parsed.model.lang_version {
        if !SUPPORTED_LANG_VERSIONS.contains(&v.as_str()) {
            return Err(error_at(
                Status::Invalid,
                83,
                0,
                format!(
                    "unsupported lang_version '{v}'; this build supports {}",
                    SUPPORTED_LANG_VERSIONS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

/// Compiles an already-parsed (and merged) program to EIR.
pub fn compile_program(mut parsed: ParsedProgram) -> Result<CompiledProgram> {
    check_lang_version(&parsed)?;
    check_call_arities(&parsed)?;
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
    // `nbody` reads (px,py,pz,vx,vy,vz,m) from state slots 0..6. A body with too
    // few slots would silently misbehave (detail 91); a `mass` field is ignored
    // (advisory, detail 92). Only relevant when the model has an `nbody` system.
    if parsed.systems.iter().any(|s| s.kind == "nbody") {
        for &id in &nbody_entities {
            if let Some(e) = parsed.model.entities.get((id - 1) as usize) {
                let slots = e.state.as_ref().map(|v| v.len()).unwrap_or(0);
                if slots < 7 {
                    return Err(error_at(
                        Status::Invalid,
                        91,
                        0,
                        format!(
                            "nbody body `{}` needs at least 7 state slots \
(px,py,pz,vx,vy,vz,m); it has {slots}",
                            e.name
                        ),
                    ));
                }
                if e.mass.is_some() {
                    push_diag(
                        92,
                        0,
                        format!(
                            "nbody body `{}` reads mass from state[6]; its `mass` field is ignored",
                            e.name
                        ),
                    );
                }
                // RFC orientation convention: for state bodies, slots 7/8/9 are
                // euler angles only when `orient = true`; otherwise slot 7 is a
                // Z-spin, so a body that exposes those slots is silently
                // reinterpreted (advisory, detail 95).
                let orient = e.render.as_ref().map(|r| r.orient).unwrap_or(false);
                if !orient && slots >= 8 {
                    push_diag(
                        95,
                        0,
                        format!(
                            "nbody body `{}` has state slot 7 (and possibly 8/9) but `orient` is not true; \
they are read as a Z-spin, not euler angles",
                            e.name
                        ),
                    );
                }
            }
        }
    }

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
    check_solver_stability(&parsed, &field_info)?;
    // #6: warn when a rule writes state slots 7/8/9 of an entity that does not
    // set `orient = true` (the slots are read as a Z-spin, not euler angles).
    warn_orient_slot_writes(&parsed, &entity_ids, &state_names_by_id);
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
    let mut tag_ids: std::collections::BTreeMap<String, Vec<u128>> =
        std::collections::BTreeMap::new();
    for (i, e) in parsed.model.entities.iter().enumerate() {
        for t in &e.tags {
            tag_ids.entry(t.clone()).or_default().push((i as u128) + 1);
        }
    }
    let systems = build_systems(
        &parsed.systems,
        &entity_ids,
        &nbody_entities,
        &tag_ids,
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
        let lets = to_let_stmts(&f.stmts, 0)?;
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
    module.validate(false).map_err(|e| {
        // EIR-level validation error: surface the reason instead of the generic
        // "unspecified compile error" (its byte_offset is an instruction index,
        // not a source position).
        let msg = format!("internal EIR validation failed (EIR detail {})", e.detail);
        push_diag(60, 0, msg.clone());
        error_at(Status::Invalid, 60, 0, msg)
    })?;
    let eir = module;
    Ok(CompiledProgram {
        parsed,
        program,
        eir,
    })
}
