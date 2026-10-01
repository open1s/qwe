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
pub(crate) use diagnostics::{error, error_at, push_diag};
use pwe_api::{Access, EntityId, Error, Hash256, RegionId, Result, Status, WorldId, WorldVersion};

mod ast;
pub use ast::*;

mod parser;
pub use parser::*;

mod lower;
pub(crate) use lower::*;

mod compile;
pub use compile::{
    build_systems, compile, compile_file, compile_program, load_error_source, load_program,
    load_program_sources, load_program_sources_with_root, merge_sources, CompiledProgram,
    ProgramSources, LANG_VERSION, SUPPORTED_LANG_VERSIONS,
};

mod systems;
pub use systems::*;

mod runtime;
pub use runtime::*;

mod format;
pub use format::format_source;
mod migrate;
pub use migrate::{migrate_v02_to_v03, Migration};

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
    // Merge with any slots already declared by `array N name` on this entity,
    // so `state = (…)` and `array` compose in either order.
    if let Some(existing) = decl.state.take() {
        let mut merged = existing;
        merged.append(&mut values);
        values = merged;
    }
    if values.len() > crate::components::State::MAX_STATE_SLOTS {
        return Err(error(Status::Invalid, 80));
    }
    if let Some(existing) = decl.state_names.take() {
        let mut merged = existing;
        merged.append(&mut names);
        names = merged;
    }
    if let Some(existing) = decl.state_units.take() {
        let mut merged = existing;
        // Pad the existing unit list to the pre-append length if needed.
        while merged.len() < values.len().saturating_sub(units.len()) {
            merged.push(None);
        }
        merged.append(&mut units);
        units = merged;
    }
    decl.state = Some(values);
    decl.state_names = Some(names);
    if units.iter().any(|u| u.is_some()) {
        decl.state_units = Some(units);
    }
    Ok(())
}

/// RFC-0044: `array N name [{ v0, v1, … }]` — appends N uninitialized (or
/// partially initialized) state slots named `name.0 … name.{N-1}` and records
/// the length in `decl.arrays`, so `name[i]` is bounds-checked by name.
fn parse_array_field(field: pest::iterators::Pair<'_, Rule>, decl: &mut EntityDecl) -> Result<()> {
    // Parse from the field text: `array` writes digits/identifiers as anonymous
    // tokens whose pairs are version-sensitive, and the initializer is a
    // comma-separated `value` list inside braces.
    let text = field.as_str();
    let body = text.trim().strip_prefix("array").unwrap_or(text).trim();
    let (head, init) = match body.split_once('{') {
        Some((h, rest)) => {
            let rest = rest.trim_end().strip_suffix('}').unwrap_or(rest).trim();
            (h.trim(), Some(rest))
        }
        None => (body.trim_end_matches(';').trim(), None),
    };
    let (n_text, name) = head.split_once(char::is_whitespace).ok_or_else(|| {
        error_at(
            Status::Invalid,
            52,
            0,
            format!("malformed array declaration `{text}` (expected `array N name`)"),
        )
    })?;
    let n: usize = n_text
        .trim()
        .parse()
        .map_err(|_| error(Status::Invalid, 52))?;
    let name = name.trim().to_string();
    if n == 0 {
        return Err(error_at(
            Status::Invalid,
            52,
            0,
            format!("array `{name}` must have a length of at least 1"),
        ));
    }
    let mut values: Vec<f64> = vec![0.0; n];
    if let Some(init) = init {
        let mut given: Vec<f64> = Vec::with_capacity(n);
        for v in init.split(',') {
            let t = v.trim();
            // A malformed value must fail loudly, never default to 0.0.
            let num = parse_scalar_number(t).ok_or_else(|| {
                error_at(
                    Status::Invalid,
                    57,
                    0,
                    format!("invalid value `{t}` in the initializer of array `{name}`"),
                )
            })?;
            given.push(num);
        }
        if given.len() > n {
            return Err(error_at(
                Status::Invalid,
                52,
                0,
                format!(
                    "array `{name}` has {n} element(s) but its initializer gives {}",
                    given.len()
                ),
            ));
        }
        for (i, v) in given.into_iter().enumerate() {
            values[i] = v;
        }
    }
    let state = decl.state.get_or_insert_with(Vec::new);
    let names = decl.state_names.get_or_insert_with(Vec::new);
    let base = state.len();
    if base + n > crate::components::State::MAX_STATE_SLOTS {
        return Err(error_at(
            Status::Invalid,
            80,
            0,
            format!(
                "array `{name}` needs slots {base}..{} but an entity has at most {} state slots",
                base + n,
                crate::components::State::MAX_STATE_SLOTS
            ),
        ));
    }
    for (k, v) in values.into_iter().enumerate() {
        names.push(Some(format!("{name}.{k}")));
        state.push(v);
        if let Some(units) = decl.state_units.as_mut() {
            units.push(None);
        }
    }
    decl.arrays.insert(name, n);
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

/// RFC-0044: an entity's declared state slot names must be unique. This
/// catches `array` colliding with `state`/`vecN` names in either declaration
/// order, and two `array` fields with the same name (detail 107).
fn check_unique_state_names(decl: &EntityDecl) -> Result<()> {
    let Some(names) = &decl.state_names else {
        return Ok(());
    };
    let mut seen: std::collections::BTreeSet<&str> = Default::default();
    for n in names.iter().flatten() {
        if !seen.insert(n.as_str()) {
            return Err(error_at(
                Status::Invalid,
                107,
                0,
                format!(
                    "duplicate state slot name `{n}` (each name may appear once; check \
`array` / `state` / `vecN` collisions)"
                ),
            ));
        }
    }
    Ok(())
}

/// Apply one `entity_field` to its declaration.
///
/// `seen` accumulates the field rules already applied to *this* entity, so a
/// repeated single-valued field reports detail 107 instead of silently keeping
/// the last write (`tag` is exempt: it accumulates by design, and `array` is
/// exempt: an entity may declare several arrays, with name uniqueness checked
/// by `check_unique_state_names`). Every rule is single-valued here except
/// `tag_field` / `array_field`; `box`/`sphere`/`hull` additionally share the
/// one `collider` slot.
fn apply_entity_field(
    field: pest::iterators::Pair<'_, Rule>,
    decl: &mut EntityDecl,
    structs: &std::collections::BTreeMap<String, crate::dsl::StructDef>,
    seen: &mut Vec<Rule>,
) -> Result<()> {
    let rule = field.as_rule();
    let start = field.as_span().start();
    // `position=(0,0,0)` → `position`, for the diagnostic text.
    let name = field
        .as_str()
        .split(|c: char| c == '=' || c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_string();
    if rule != Rule::tag_field && rule != Rule::array_field {
        if seen.contains(&rule) {
            return Err(error_at(
                Status::Invalid,
                107,
                start,
                format!("duplicate `{name}` entity field (each field may appear at most once)"),
            ));
        }
        let collider_field =
            rule == Rule::box_field || rule == Rule::sphere_field || rule == Rule::hull_field;
        if collider_field && decl.collider.is_some() {
            return Err(error_at(
                Status::Invalid,
                107,
                start,
                format!(
                    "duplicate collider `{name}` (an entity may declare only one \
                     of box / sphere / hull)"
                ),
            ));
        }
        seen.push(rule);
    }
    match rule {
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
        Rule::tag_field => {
            for p in field.into_inner() {
                decl.tags.push(p.as_str().to_string());
            }
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
        Rule::array_field => parse_array_field(field, decl)?,
        Rule::color_field => {
            let hex = next_pair(&mut field.into_inner())?.as_str();
            // `0xRRGGBB` exactly (6 hex digits); only 24 bits are rendered
            // (`present::EntityVisual` masks to `0xFF_FFFF`).
            if hex.len() != 8 {
                return Err(error_at(
                    Status::Invalid,
                    64,
                    0,
                    format!("color literal `{hex}` must be 0xRRGGBB (exactly 6 hex digits)"),
                ));
            }
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
            // Documented `opacity` is `[0,1]` (docs §L9) and the viewer feeds
            // it straight into blending, so out-of-range is detail 105 — the
            // same code the per-part path reports.
            if !(0.0..=1.0).contains(&v) {
                return Err(error_at(
                    Status::Invalid,
                    105,
                    start,
                    format!("entity opacity {v} is outside 0..1"),
                ));
            }
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

#[cfg(test)]
mod tests;
