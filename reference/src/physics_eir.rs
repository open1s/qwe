//! EIR-backed physics: lower a physics system to an `EirModule` and execute it
//! through the EIR interpreter against a `Scene`.
//!
//! This is the "do not bypass EIR" path: high-level component behavior is
//! expressed as a typed Domain/EIR program, not as ad-hoc host code. The
//! interpreter produces ordered `WorldWrite`s that are applied back to the
//! scene (RFC-0023 transaction effect).

use crate::components::{Transform, Velocity};
use crate::eir::{ComponentRef, EirModule, EirRuntime, Function, Instruction, Opcode, ValueType};
use crate::math::Vec3;
use crate::scene::Scene;
use crate::schema::ComponentIdentity;
use pwe_api::{ComponentTypeId, Hash256, Result};

/// RFC-0019 canonical `ComponentTypeId` for a component whose semantic identity
/// is `namespace.stable_name` at the given major version. This is the same value
/// a WIR `Schema` derives from its `ComponentIdentity`, so the WIR data plane and
/// the EIR runtime address components with one shared ID space.
///
/// Names are fixed, lowercase, and versioned here, so `type_id()` cannot fail.
fn canonical_component_id(namespace: &str, stable_name: &str, major: u32) -> ComponentTypeId {
    let identity = ComponentIdentity {
        namespace: namespace.into(),
        stable_name: stable_name.into(),
        major_version: major,
    };
    identity.type_id().expect("fixed valid component identity")
}

// The fixed component ids are pure functions of constant strings, but deriving
// one runs SHA-256. They are read on every field/view access, so memoize them:
// a single `OnceLock` per id turns a per-read hash into one atomic load.
fn fixed_id(
    slot: &'static std::sync::OnceLock<ComponentTypeId>,
    namespace: &str,
    name: &str,
) -> ComponentTypeId {
    *slot.get_or_init(|| canonical_component_id(namespace, name, 1))
}

/// Canonical id of the `Transform` component (`pwe.physics.transform` v1).
pub fn transform_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.physics", "transform")
}
/// Canonical id of the `Velocity` component (`pwe.physics.velocity` v1).
pub fn velocity_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.physics", "velocity")
}
/// Canonical id of the `RigidBody` component (`pwe.physics.rigid_body` v1).
pub fn rigid_body_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.physics", "rigid_body")
}
/// Canonical id of the `State` component (`pwe.physics.state` v1).
pub fn state_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.physics", "state")
}
/// Canonical id of the global simulation clock (`pwe.time.clock` v1).
pub fn sim_time_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.time", "clock")
}

/// Canonical `ComponentTypeId` for the hidden per-entity invariant verdict
/// component: each `invariant` system writes its 0/1 check result here so the
/// host can fail the step when an invariant is violated.
pub fn check_id() -> ComponentTypeId {
    static ID: std::sync::OnceLock<ComponentTypeId> = std::sync::OnceLock::new();
    fixed_id(&ID, "pwe.lang", "check")
}

/// Canonical `ComponentTypeId` for a named model parameter
/// (`pwe.lang.param.<name>`); rules read parameters through `read_field`.
///
/// Parameters are a language-owned addressing scheme, not WIR schema
/// components, so the id is derived directly (domain-separated SHA-256 of the
/// raw name) rather than through `ComponentIdentity` — parameter names may be
/// uppercase or otherwise outside the schema's lowercase name grammar.
pub fn param_component_id(name: &str) -> ComponentTypeId {
    let mut input = b"pwe.lang.param/v1\0".to_vec();
    input.extend_from_slice(name.as_bytes());
    let h = crate::sha256::digest(&input);
    ComponentTypeId(h.0[..16].try_into().unwrap())
}

/// Canonical `ComponentTypeId` for a named grid field (`pwe.lang.field.<name>`);
/// the EIR runtime addresses field cells with this component id, and the
/// field's width rides in the ComponentRef's offset.
pub fn field_component_id(name: &str) -> ComponentTypeId {
    canonical_component_id("pwe.lang", &format!("field.{name}"), 1)
}

/// Field offsets (bytes) within the canonical component encodings.
pub mod field {
    pub const POS_X: u32 = 0; // Transform.position.x
    pub const POS_Y: u32 = 8; // Transform.position.y
    pub const POS_Z: u32 = 16; // Transform.position.z
    pub const VEL_X: u32 = 0; // Velocity.linear.x
    pub const VEL_Y: u32 = 8; // Velocity.linear.y
    pub const VEL_Z: u32 = 16; // Velocity.linear.z
    /// RigidBody.mass (f64).
    pub const MASS: u32 = 0;
    /// RigidBody.is_dynamic (u8).
    pub const IS_DYNAMIC: u32 = 24;
    /// Byte offset of the simulation-clock field (a single f64 seconds value).
    pub const SIM_TIME: u32 = 0;
    /// Byte offset of state slot `i` in the generic `State` component.
    pub fn state_slot(i: usize) -> u32 {
        (i as u32) * crate::components::State::SLOT_BYTES as u32
    }
    pub const STATE_SLOT_BYTES: u32 = 8;
}

/// A runtime adapter that reads component fields from a `Scene`, applying
/// in-interpretation writes via an overlay so later reads observe them.
pub struct SceneRuntime<'a> {
    pub scene: &'a Scene,
    pending: std::collections::BTreeMap<(u128, ComponentTypeId, u32), u64>,
    /// A **dense** per-field overlay of cell values written so far this step,
    /// materialized lazily from the `Field` on the first grid write. Grid
    /// read-after-write therefore costs O(1), not a tree lookup per cell
    /// (RFC-0037). Materialized only when a grid cell is written.
    field_overlay: std::collections::BTreeMap<ComponentTypeId, Vec<f64>>,
    /// Canonical field component id -> the scene's grid field, for
    /// `ReadFieldCell`/`FieldLaplacian` cell access.
    field_ids: std::collections::BTreeMap<ComponentTypeId, &'a crate::field::Field>,
    /// Canonical parameter component id -> the parameter value.
    param_ids: std::collections::BTreeMap<ComponentTypeId, f64>,
}

impl<'a> SceneRuntime<'a> {
    pub fn new(scene: &'a Scene) -> Self {
        let mut field_ids = std::collections::BTreeMap::new();
        for (name, f) in &scene.fields {
            field_ids.insert(field_component_id(name), f);
        }
        let mut param_ids = std::collections::BTreeMap::new();
        for (name, v) in &scene.params {
            param_ids.insert(param_component_id(name), *v);
        }
        Self {
            scene,
            pending: std::collections::BTreeMap::new(),
            field_overlay: std::collections::BTreeMap::new(),
            field_ids,
            param_ids,
        }
    }
}

impl EirRuntime for SceneRuntime<'_> {
    fn read_field(&self, target: ComponentRef) -> Result<u64> {
        let key = (target.entity, target.component, target.offset);
        if let Some(&v) = self.pending.get(&key) {
            return Ok(v);
        }
        // The global simulation clock (`t`) does not belong to any entity.
        if target.component == sim_time_id() {
            return Ok(self.scene.sim_time.to_bits());
        }
        // A model parameter: not tied to any entity. Checked first so a
        // parameter read never falls through to the entity lookup.
        if let Some(v) = self.param_ids.get(&target.component) {
            return Ok(v.to_bits());
        }
        // A grid field cell: the linear index rides in the offset. Checked
        // before the entity lookup — fields do not belong to any entity.
        if let Some(f) = self.field_ids.get(&target.component) {
            // Grid-cell access: the linear `[k][j][i]` index rides in the
            // offset (the interpreter folds `(i, j + k·height)` into it).
            let idx = (target.offset / field::STATE_SLOT_BYTES) as usize;
            if idx >= f.cells().len() {
                return Err(pwe_api::Error {
                    status: pwe_api::Status::Invalid,
                    detail: 7,
                    byte_offset: 0,
                });
            }
            Ok(self
                .field_overlay
                .get(&target.component)
                .map(|c| c[idx])
                .unwrap_or_else(|| f.value_linear(idx))
                .to_bits())
        } else if target.entity == 0 {
            // A world-level component that is not a registered parameter, clock,
            // or field: an unresolved reference (a bare name bound to a
            // not-registered parameter component, or a bare `dt`/typo). These
            // read 0.0 — the documented unresolved-reference convention. Entity
            // ids are 1-based, so entity 0 is never a real body.
            Ok(0.0f64.to_bits())
        } else {
            let e = self
                .scene
                .get(pwe_api::EntityId(target.entity))
                .ok_or(pwe_api::Error {
                    status: pwe_api::Status::HandleStale,
                    detail: 1,
                    byte_offset: 0,
                })?;
            if target.component == transform_id() {
                let t = e.transform.ok_or(pwe_api::Error {
                    status: pwe_api::Status::HandleStale,
                    detail: 2,
                    byte_offset: 0,
                })?;
                Ok(match target.offset {
                    field::POS_X => t.position.x.to_bits(),
                    field::POS_Y => t.position.y.to_bits(),
                    field::POS_Z => t.position.z.to_bits(),
                    _ => 0,
                })
            } else if target.component == velocity_id() {
                let v = e.velocity.ok_or(pwe_api::Error {
                    status: pwe_api::Status::HandleStale,
                    detail: 3,
                    byte_offset: 0,
                })?;
                Ok(match target.offset {
                    field::VEL_X => v.linear.x.to_bits(),
                    field::VEL_Y => v.linear.y.to_bits(),
                    field::VEL_Z => v.linear.z.to_bits(),
                    _ => 0,
                })
            } else if target.component == rigid_body_id() {
                let rb = e.rigid_body.ok_or(pwe_api::Error {
                    status: pwe_api::Status::HandleStale,
                    detail: 4,
                    byte_offset: 0,
                })?;
                Ok(match target.offset {
                    field::MASS => rb.mass.to_bits(),
                    field::IS_DYNAMIC => rb.is_dynamic as u64,
                    _ => 0,
                })
            } else if target.component == state_id() {
                // A body without an explicit state reads as all-zeros (default).
                let Some(st) = e.state.as_ref() else {
                    return Ok(0);
                };
                let slot = (target.offset / field::STATE_SLOT_BYTES) as usize;
                Ok(st.values.get(slot).copied().map(f64::to_bits).unwrap_or(0))
            } else {
                Ok(0)
            }
        }
    }
    fn write_field(&mut self, target: ComponentRef, value: u64) {
        // Grid cells go to the dense per-field overlay (RFC-0037); the
        // interpreter still records the `WorldWrite`, so `apply_writes` and the
        // cross-backend write comparison are unchanged.
        if let Some(f) = self.field_ids.get(&target.component) {
            let idx = (target.offset / field::STATE_SLOT_BYTES) as usize;
            let cells = self
                .field_overlay
                .entry(target.component)
                .or_insert_with(|| f.cells().to_vec());
            if let Some(c) = cells.get_mut(idx) {
                *c = f64::from_bits(value);
            }
            return;
        }
        let key = (target.entity, target.component, target.offset);
        self.pending.insert(key, value);
    }
    fn query_neighbor_count(&self, entity: u128, radius: f64) -> Result<u64> {
        let origin = self.position_of(entity)?;
        let mut n = 0u64;
        // Deterministic scan: BTreeMap iteration is sorted by entity id.
        for (&id, e) in &self.scene.entities {
            if id.0 == entity || e.camera.is_some() {
                continue;
            }
            if origin.distance(self.effective_position(e, id.0)) <= radius {
                n += 1;
            }
        }
        Ok(n)
    }
    fn query_nearest_dist(&self, entity: u128) -> Result<u64> {
        let origin = self.position_of(entity)?;
        let mut best = f64::MAX;
        for (&id, e) in &self.scene.entities {
            if id.0 == entity || e.camera.is_some() {
                continue;
            }
            let d = origin.distance(self.effective_position(e, id.0));
            if d < best {
                best = d;
            }
        }
        Ok(best.to_bits())
    }
    fn query_neighbor_mean(&self, entity: u128, slot: u32, radius: f64) -> Result<f64> {
        let origin = self.position_of(entity)?;
        let mut sum = 0.0;
        let mut n = 0u64;
        for (&id, e) in &self.scene.entities {
            if id.0 == entity || e.camera.is_some() {
                continue;
            }
            if origin.distance(self.effective_position(e, id.0)) > radius {
                continue;
            }
            // Read the neighbor's State slot through `read_field` so it honors
            // any in-interpretation write (the same overlay as every read).
            let raw = self.read_field(crate::eir::ComponentRef {
                entity: id.0,
                component: state_id(),
                offset: slot.wrapping_mul(field::STATE_SLOT_BYTES),
            })?;
            sum += f64::from_bits(raw);
            n += 1;
        }
        Ok(if n == 0 { 0.0 } else { sum / n as f64 })
    }
    fn query_nearest_offset(&self, entity: u128) -> Result<(f64, f64, f64)> {
        let origin = self.position_of(entity)?;
        let mut best = f64::MAX;
        let mut off = (0.0, 0.0, 0.0);
        for (&id, e) in &self.scene.entities {
            if id.0 == entity || e.camera.is_some() {
                continue;
            }
            let p = self.effective_position(e, id.0);
            let d = origin.distance(p);
            if d < best {
                best = d;
                off = (p.x - origin.x, p.y - origin.y, p.z - origin.z);
            }
        }
        Ok(off)
    }
    fn field_laplacian(
        &self,
        component: ComponentTypeId,
        i: f64,
        j: f64,
        _width: f64,
    ) -> Result<f64> {
        let f = self.field_ids.get(&component).ok_or(pwe_api::Error {
            status: pwe_api::Status::HandleStale,
            detail: 6,
            byte_offset: 0,
        })?;
        let (w, h, d) = (f.width, f.height, f.depth);
        // The caller encodes `(j, k)` as `j + k·height` (the same packing the
        // linear index uses), so recover `j` and `k` from the field's geometry.
        let j_lin = j as usize;
        let ii = i as usize;
        let kk = j_lin.checked_div(h).unwrap_or(0);
        let jj = j_lin.checked_rem(h).unwrap_or(0);
        if ii >= w || jj >= h || kk >= d {
            return Err(pwe_api::Error {
                status: pwe_api::Status::Invalid,
                detail: 7,
                byte_offset: 0,
            });
        }
        // The Field's zero-flux stencil (off-edge neighbors = center), scaled
        // by 1/dx²; each cell prefers any in-interpretation write (the same
        // overlay `read_field` sees).
        let overlay = self.field_overlay.get(&component);
        let cell = |ci: usize, cj: usize, ck: usize| -> f64 {
            let idx = (ck * h + cj) * w + ci;
            match overlay {
                Some(c) => c[idx],
                None => f.value_linear(idx),
            }
        };
        let center = cell(ii, jj, kk);
        let left = if ii > 0 { cell(ii - 1, jj, kk) } else { center };
        let right = if ii + 1 < w {
            cell(ii + 1, jj, kk)
        } else {
            center
        };
        let up = if jj > 0 { cell(ii, jj - 1, kk) } else { center };
        let down = if jj + 1 < h {
            cell(ii, jj + 1, kk)
        } else {
            center
        };
        let back = if kk > 0 { cell(ii, jj, kk - 1) } else { center };
        let front = if kk + 1 < d {
            cell(ii, jj, kk + 1)
        } else {
            center
        };
        // 6-point stencil throughout; for a 2D slice the two off-edge z-terms
        // equal the centre, collapsing to the 5-point `−4·centre` form.
        Ok((left + right + up + down + back + front - 6.0 * center) / (f.dx * f.dx))
    }
}

impl SceneRuntime<'_> {
    /// Position of a scene entity by id: its `Transform`, or `state[0..2]` for
    /// state-only bodies (the same convention the viewer uses).
    fn position_of(&self, entity: u128) -> Result<Vec3> {
        let e = self
            .scene
            .get(pwe_api::EntityId(entity))
            .ok_or(pwe_api::Error {
                status: pwe_api::Status::HandleStale,
                detail: 1,
                byte_offset: 0,
            })?;
        Ok(self.effective_position(e, entity))
    }
    /// A body's position, preferring any position written earlier in this
    /// interpretation (the write overlay), then `Transform`, then `state[0..2]`.
    fn effective_position(&self, e: &crate::scene::Entity, entity: u128) -> Vec3 {
        let base = match e.transform.map(|t| t.position) {
            Some(p) => p,
            None => Vec3::new(
                e.state
                    .as_ref()
                    .and_then(|st| st.values.first().copied())
                    .unwrap_or(0.0),
                e.state
                    .as_ref()
                    .and_then(|st| st.values.get(1).copied())
                    .unwrap_or(0.0),
                e.state
                    .as_ref()
                    .and_then(|st| st.values.get(2).copied())
                    .unwrap_or(0.0),
            ),
        };
        let px = self
            .pending
            .get(&(entity, transform_id(), field::POS_X))
            .map(|v| f64::from_bits(*v));
        let py = self
            .pending
            .get(&(entity, transform_id(), field::POS_Y))
            .map(|v| f64::from_bits(*v));
        let pz = self
            .pending
            .get(&(entity, transform_id(), field::POS_Z))
            .map(|v| f64::from_bits(*v));
        Vec3::new(
            px.unwrap_or(base.x),
            py.unwrap_or(base.y),
            pz.unwrap_or(base.z),
        )
    }
}

/// Lowered physics: a function per dynamic entity that (a) integrates gravity
/// into velocity.y, then (b) integrates velocity.y into position.y.
pub struct LoweredPhysics {
    pub module: EirModule,
    pub entities: Vec<u128>,
    pub dt: f64,
}

/// Builds the EIR program for gravity+position integration of `entities`.
pub fn lower_physics(entities: &[u128], gravity_y: f64, dt: f64) -> LoweredPhysics {
    let mut functions = Vec::with_capacity(entities.len());
    let mut lowered_entities = Vec::with_capacity(entities.len());
    for (index, entity) in entities.iter().enumerate() {
        let mut instrs: Vec<Instruction> = Vec::new();
        let mut next = 1u32;

        // r1 = gravity * dt
        instrs.push(Instruction {
            opcode: Opcode::Const,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: Some(crate::eir::Immediate::F64(gravity_y * dt)),
            target: None,
        });
        next += 1;

        // r2 = ReadView(velocity.y)
        instrs.push(Instruction {
            opcode: Opcode::ReadView,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: None,
            target: Some(ComponentRef {
                entity: *entity,
                component: velocity_id(),
                offset: field::VEL_Y,
            }),
        });
        next += 1;

        // r3 = r2 + r1  (velocity.y += gravity*dt)
        let vel = next;
        instrs.push(Instruction {
            opcode: Opcode::Add,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![vel - 1, vel - 2],
            constant: None,
            target: None,
        });
        next += 1;

        // WriteView(velocity.y, r3)
        instrs.push(Instruction {
            opcode: Opcode::WriteView,
            result_id: 0,
            result_type: None,
            operands: vec![vel],
            constant: None,
            target: Some(ComponentRef {
                entity: *entity,
                component: velocity_id(),
                offset: field::VEL_Y,
            }),
        });

        // r4 = ReadView(position.y)
        let pos = next;
        instrs.push(Instruction {
            opcode: Opcode::ReadView,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: None,
            target: Some(ComponentRef {
                entity: *entity,
                component: transform_id(),
                offset: field::POS_Y,
            }),
        });
        next += 1;

        // r5 = ReadView(velocity.y) again
        let vel2 = next;
        instrs.push(Instruction {
            opcode: Opcode::ReadView,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: None,
            target: Some(ComponentRef {
                entity: *entity,
                component: velocity_id(),
                offset: field::VEL_Y,
            }),
        });
        next += 1;

        // r6 = const dt
        let const_dt = next;
        instrs.push(Instruction {
            opcode: Opcode::Const,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![],
            constant: Some(crate::eir::Immediate::F64(dt)),
            target: None,
        });
        next += 1;

        // r7 = r5 * dt
        let mul = next;
        instrs.push(Instruction {
            opcode: Opcode::Mul,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![vel2, const_dt],
            constant: None,
            target: None,
        });
        next += 1;

        // r8 = r4 + r7  (position.y += velocity.y*dt)
        let new_pos = next;
        instrs.push(Instruction {
            opcode: Opcode::Add,
            result_id: next,
            result_type: Some(ValueType::F64),
            operands: vec![pos, mul],
            constant: None,
            target: None,
        });

        // WriteView(position.y, r7)
        instrs.push(Instruction {
            opcode: Opcode::WriteView,
            result_id: 0,
            result_type: None,
            operands: vec![new_pos],
            constant: None,
            target: Some(ComponentRef {
                entity: *entity,
                component: transform_id(),
                offset: field::POS_Y,
            }),
        });

        // Return
        instrs.push(Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        });

        functions.push(Function {
            id: index as u64 + 1,
            effect_mask: crate::eir::EIR_EFFECT_READ_WORLD | crate::eir::EIR_EFFECT_WRITE_WORLD,
            argument_count: 0,
            instructions: instrs,
        });
        lowered_entities.push(*entity);
    }

    let module = EirModule {
        module_hash: Hash256([0; 32]),
        schema_set_hash: Hash256([0; 32]),
        domain_ir_hash: Hash256([0; 32]),
        target_kind: 0,
        functions,
    };
    LoweredPhysics {
        module,
        entities: lowered_entities,
        dt,
    }
}

/// A high-level, reusable component system that lowers to EIR. Each system is
/// expressed as a sequence of typed EIR instructions over `Scene` components —
/// the host never mutates component state directly during Compute.
pub trait EirSystem {
    /// Human-readable system name (stable Domain IR id basis).
    fn name(&self) -> &'static str;
    /// Appends this system's lowering for one entity to `out`.
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>);
}

/// Gravity system: `velocity.y += gravity_y * dt`.
pub struct GravitySystem {
    pub gravity_y: f64,
    pub dt: f64,
}
impl EirSystem for GravitySystem {
    fn name(&self) -> &'static str {
        "physics.gravity"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
        let gdt = crate::eir::Immediate::F64(self.gravity_y * self.dt);
        // const gdt
        out.push(instr(
            Opcode::Const,
            next,
            Some(ValueType::F64),
            vec![],
            Some(gdt),
            None,
        ));
        let const_id = next;
        next += 1;
        // read vel.y
        out.push(instr(
            Opcode::ReadView,
            next,
            Some(ValueType::F64),
            vec![],
            None,
            Some(cr(entity, velocity_id(), field::VEL_Y)),
        ));
        let vel_id = next;
        next += 1;
        // vel + gdt
        out.push(instr(
            Opcode::Add,
            next,
            Some(ValueType::F64),
            vec![vel_id, const_id],
            None,
            None,
        ));
        let sum_id = next;
        // write vel.y
        out.push(instr(
            Opcode::WriteView,
            0,
            None,
            vec![sum_id],
            None,
            Some(cr(entity, velocity_id(), field::VEL_Y)),
        ));
        // return
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// A constant force (acceleration) applied to every dynamic body each step:
/// `velocity += force * dt` on each axis. Models wind, propulsion, or a drift.
pub struct ForceSystem {
    pub ax: f64,
    pub ay: f64,
    pub az: f64,
    pub dt: f64,
}
impl EirSystem for ForceSystem {
    fn name(&self) -> &'static str {
        "physics.force"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        // velocity.<axis> += force.<axis> * dt for each of x, y, z.
        for (offset, a) in [
            (field::VEL_X, self.ax),
            (field::VEL_Y, self.ay),
            (field::VEL_Z, self.az),
        ] {
            if a == 0.0 {
                continue;
            }
            let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
            out.push(instr(
                Opcode::Const,
                next,
                Some(ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(a * self.dt)),
                None,
            ));
            let delta_id = next;
            next += 1;
            out.push(instr(
                Opcode::ReadView,
                next,
                Some(ValueType::F64),
                vec![],
                None,
                Some(cr(entity, velocity_id(), offset)),
            ));
            let vel_id = next;
            next += 1;
            out.push(instr(
                Opcode::Add,
                next,
                Some(ValueType::F64),
                vec![vel_id, delta_id],
                None,
                None,
            ));
            let sum_id = next;
            out.push(instr(
                Opcode::WriteView,
                0,
                None,
                vec![sum_id],
                None,
                Some(cr(entity, velocity_id(), offset)),
            ));
        }
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// A user-defined linear dynamical system over generic state slots:
///
/// ```text
///   state'[i] = Σ_j  coeff[i][j] · state[j]  +  offset[i]
/// ```
///
/// `coeff[i]` has one entry per state slot plus a final constant offset. This
/// single system expresses a large class of physical phenomena — harmonic
/// oscillators (springs), radioactive decay, population growth, mixing, RLC
/// circuits, damped motion — entirely from source-declared matrices. Updates
/// are explicit Euler: all slots are read first, then written, so coupling is
/// simultaneous.
pub struct LinearSystem {
    pub rows: Vec<Vec<f64>>,
    pub slots: usize,
    pub dt: f64,
}
impl EirSystem for LinearSystem {
    fn name(&self) -> &'static str {
        "physics.linear"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        // Read every slot once into registers first (simultaneous update).
        let mut slot_regs: Vec<u32> = Vec::with_capacity(self.slots);
        for i in 0..self.slots {
            let next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            out.push(instr(
                Opcode::ReadView,
                next,
                Some(ValueType::F64),
                vec![],
                None,
                Some(cr(entity, state_id(), field::state_slot(i))),
            ));
            slot_regs.push(next);
        }
        for (i, row) in self.rows.iter().enumerate() {
            if i >= self.slots {
                break;
            }
            // delta = dt·offset + Σ_j dt·coeff[i][j]·state[j]
            let mut next = out.iter().map(|x| x.result_id).max().unwrap_or(0) + 1;
            let mut acc = next;
            out.push(instr(
                Opcode::Const,
                next,
                Some(ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(
                    row.get(self.slots).copied().unwrap_or(0.0) * self.dt,
                )),
                None,
            ));
            for (j, &state_reg) in slot_regs.iter().enumerate() {
                let coeff = row.get(j).copied().unwrap_or(0.0);
                if coeff == 0.0 {
                    continue;
                }
                next += 1;
                // dt·coeff
                out.push(instr(
                    Opcode::Const,
                    next,
                    Some(ValueType::F64),
                    vec![],
                    Some(crate::eir::Immediate::F64(coeff * self.dt)),
                    None,
                ));
                let c_id = next;
                next += 1;
                // dt·coeff·state[j]
                out.push(instr(
                    Opcode::Mul,
                    next,
                    Some(ValueType::F64),
                    vec![state_reg, c_id],
                    None,
                    None,
                ));
                let term_id = next;
                next += 1;
                // delta += term
                out.push(instr(
                    Opcode::Add,
                    next,
                    Some(ValueType::F64),
                    vec![acc, term_id],
                    None,
                    None,
                ));
                acc = next;
            }
            next += 1;
            // new = state[i] + delta  (explicit Euler)
            out.push(instr(
                Opcode::Add,
                next,
                Some(ValueType::F64),
                vec![slot_regs[i], acc],
                None,
                None,
            ));
            let new_val = next;
            // write state[i] = new
            out.push(instr(
                Opcode::WriteView,
                0,
                None,
                vec![new_val],
                None,
                Some(cr(entity, state_id(), field::state_slot(i))),
            ));
        }
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// Integrate system (semi-implicit Euler): `position += velocity * dt` on each
/// axis.
pub struct IntegrateSystem {
    pub dt: f64,
}
impl EirSystem for IntegrateSystem {
    fn name(&self) -> &'static str {
        "physics.integrate"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        for (pos_off, vel_off) in [
            (field::POS_X, field::VEL_X),
            (field::POS_Y, field::VEL_Y),
            (field::POS_Z, field::VEL_Z),
        ] {
            let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
            // read pos.<axis>
            out.push(instr(
                Opcode::ReadView,
                next,
                Some(ValueType::F64),
                vec![],
                None,
                Some(cr(entity, transform_id(), pos_off)),
            ));
            let pos_id = next;
            next += 1;
            // read vel.<axis>
            out.push(instr(
                Opcode::ReadView,
                next,
                Some(ValueType::F64),
                vec![],
                None,
                Some(cr(entity, velocity_id(), vel_off)),
            ));
            let vel_id = next;
            next += 1;
            // const dt
            out.push(instr(
                Opcode::Const,
                next,
                Some(ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(self.dt)),
                None,
            ));
            let dt_id = next;
            next += 1;
            // vel * dt
            out.push(instr(
                Opcode::Mul,
                next,
                Some(ValueType::F64),
                vec![vel_id, dt_id],
                None,
                None,
            ));
            let disp_id = next;
            next += 1;
            // pos + disp
            out.push(instr(
                Opcode::Add,
                next,
                Some(ValueType::F64),
                vec![pos_id, disp_id],
                None,
                None,
            ));
            let new_pos = next;
            // write pos.<axis>
            out.push(instr(
                Opcode::WriteView,
                0,
                None,
                vec![new_pos],
                None,
                Some(cr(entity, transform_id(), pos_off)),
            ));
        }
        // return
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// Damping system: `velocity *= factor` on each axis (linear drag).
pub struct DampingSystem {
    pub factor: f64,
}
impl EirSystem for DampingSystem {
    fn name(&self) -> &'static str {
        "physics.damping"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        for offset in [field::VEL_X, field::VEL_Y, field::VEL_Z] {
            let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
            out.push(instr(
                Opcode::ReadView,
                next,
                Some(ValueType::F64),
                vec![],
                None,
                Some(cr(entity, velocity_id(), offset)),
            ));
            let vel_id = next;
            next += 1;
            out.push(instr(
                Opcode::Const,
                next,
                Some(ValueType::F64),
                vec![],
                Some(crate::eir::Immediate::F64(self.factor)),
                None,
            ));
            let factor_id = next;
            next += 1;
            out.push(instr(
                Opcode::Mul,
                next,
                Some(ValueType::F64),
                vec![vel_id, factor_id],
                None,
                None,
            ));
            let scaled_id = next;
            out.push(instr(
                Opcode::WriteView,
                0,
                None,
                vec![scaled_id],
                None,
                Some(cr(entity, velocity_id(), offset)),
            ));
        }
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// Ground-contact system (RFC-0008 contact stage) lowered to EIR: if the body's
/// position drops below y=0, clamp it to 0 and reflect (bounce) velocity.y by
/// `restitution`. Expressed with `Lt` + `Select`, so the contact decision lives
/// in EIR, not host code.
pub struct GroundContactSystem {
    pub restitution: f64,
}
impl EirSystem for GroundContactSystem {
    fn name(&self) -> &'static str {
        "physics.ground_contact"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
        // cond = pos.y < 0
        out.push(instr(
            Opcode::ReadView,
            next,
            Some(ValueType::F64),
            vec![],
            None,
            Some(cr(entity, transform_id(), field::POS_Y)),
        ));
        let pos_id = next;
        next += 1;
        out.push(instr(
            Opcode::Const,
            next,
            Some(ValueType::F64),
            vec![],
            Some(crate::eir::Immediate::F64(0.0)),
            None,
        ));
        let zero_id = next;
        next += 1;
        out.push(instr(
            Opcode::Lt,
            next,
            Some(ValueType::Bool),
            vec![pos_id, zero_id],
            None,
            None,
        ));
        let below = next;
        next += 1;

        // new_vel = cond ? (-restitution * vel.y) : vel.y
        out.push(instr(
            Opcode::ReadView,
            next,
            Some(ValueType::F64),
            vec![],
            None,
            Some(cr(entity, velocity_id(), field::VEL_Y)),
        ));
        let vel_id = next;
        next += 1;
        out.push(instr(
            Opcode::Const,
            next,
            Some(ValueType::F64),
            vec![],
            Some(crate::eir::Immediate::F64(-self.restitution)),
            None,
        ));
        let neg_e = next;
        next += 1;
        out.push(instr(
            Opcode::Mul,
            next,
            Some(ValueType::F64),
            vec![neg_e, vel_id],
            None,
            None,
        ));
        let bounced = next;
        next += 1;
        out.push(instr(
            Opcode::Select,
            next,
            Some(ValueType::F64),
            vec![below, bounced, vel_id],
            None,
            None,
        ));
        let new_vel = next;
        next += 1;
        out.push(instr(
            Opcode::WriteView,
            0,
            None,
            vec![new_vel],
            None,
            Some(cr(entity, velocity_id(), field::VEL_Y)),
        ));

        // new_pos = cond ? 0 : pos.y
        out.push(instr(
            Opcode::Select,
            next,
            Some(ValueType::F64),
            vec![below, zero_id, pos_id],
            None,
            None,
        ));
        let new_pos = next;
        out.push(instr(
            Opcode::WriteView,
            0,
            None,
            vec![new_pos],
            None,
            Some(cr(entity, transform_id(), field::POS_Y)),
        ));
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// A bounded walled domain (RFC-0008 contact stage): keeps every dynamic body
/// inside a box `|x| ≤ x`, `|z| ≤ z` (and optionally `y ≥ y_min`). On hitting a
/// wall the offending velocity component is reflected by `restitution` and the
/// position is clamped to the wall — i.e. the body's response is adjusted by the
/// impact (impulse ∝ approach velocity, scaled by restitution). Lowered to EIR
/// with comparisons + `Select`, so the contact decision lives in EIR.
pub struct WallSystem {
    pub x: f64,
    pub z: f64,
    pub y_min: f64,
    pub restitution: f64,
}
impl EirSystem for WallSystem {
    fn name(&self) -> &'static str {
        "physics.wall"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        let mut next = out.iter().map(|i| i.result_id).max().unwrap_or(0) + 1;
        for (pos_off, vel_off, limit) in [
            (field::POS_X, field::VEL_X, self.x),
            (field::POS_Z, field::VEL_Z, self.z),
        ] {
            let px = read_wall(out, &mut next, entity, transform_id(), pos_off);
            let vx = read_wall(out, &mut next, entity, velocity_id(), vel_off);
            let lo = const_wall(out, &mut next, -limit);
            let hi = const_wall(out, &mut next, limit);
            let hit_r = cmp_wall(out, &mut next, Opcode::Gt, px, hi);
            let hit_l = cmp_wall(out, &mut next, Opcode::Lt, px, lo);
            let one = const_wall(out, &mut next, 1.0);
            let zero = const_wall(out, &mut next, 0.0);
            let hr = sel_wall(out, &mut next, hit_r, one, zero);
            let hl = sel_wall(out, &mut next, hit_l, one, zero);
            let any = add_wall(out, &mut next, hr, hl);
            let half = const_wall(out, &mut next, 0.5);
            let hit = cmp_wall(out, &mut next, Opcode::Gt, any, half);
            let neg = const_wall(out, &mut next, -self.restitution);
            let reflected = mul_wall(out, &mut next, vx, neg);
            let new_v = sel_wall(out, &mut next, hit, reflected, vx);
            let inner = sel_wall(out, &mut next, hit_l, lo, px);
            let new_p = sel_wall(out, &mut next, hit_r, hi, inner);
            write_wall(out, entity, transform_id(), pos_off, new_p);
            write_wall(out, entity, velocity_id(), vel_off, new_v);
        }
        if self.y_min.is_finite() {
            let py = read_wall(out, &mut next, entity, transform_id(), field::POS_Y);
            let vy = read_wall(out, &mut next, entity, velocity_id(), field::VEL_Y);
            let lo = const_wall(out, &mut next, self.y_min);
            let hit = cmp_wall(out, &mut next, Opcode::Lt, py, lo);
            let neg = const_wall(out, &mut next, -self.restitution);
            let reflected = mul_wall(out, &mut next, vy, neg);
            let new_v = sel_wall(out, &mut next, hit, reflected, vy);
            let new_p = sel_wall(out, &mut next, hit, lo, py);
            write_wall(out, entity, transform_id(), field::POS_Y, new_p);
            write_wall(out, entity, velocity_id(), field::VEL_Y, new_v);
        }
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

fn read_wall(
    out: &mut Vec<Instruction>,
    next: &mut u32,
    e: u128,
    comp: ComponentTypeId,
    off: u32,
) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(
        Opcode::ReadView,
        r,
        Some(ValueType::F64),
        vec![],
        None,
        Some(cr(e, comp, off)),
    ));
    r
}
fn write_wall(out: &mut Vec<Instruction>, e: u128, comp: ComponentTypeId, off: u32, v: u32) {
    out.push(instr(
        Opcode::WriteView,
        0,
        None,
        vec![v],
        None,
        Some(cr(e, comp, off)),
    ));
}
fn const_wall(out: &mut Vec<Instruction>, next: &mut u32, v: f64) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(
        Opcode::Const,
        r,
        Some(ValueType::F64),
        vec![],
        Some(crate::eir::Immediate::F64(v)),
        None,
    ));
    r
}
fn cmp_wall(out: &mut Vec<Instruction>, next: &mut u32, op: Opcode, a: u32, b: u32) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(op, r, Some(ValueType::Bool), vec![a, b], None, None));
    r
}
fn sel_wall(out: &mut Vec<Instruction>, next: &mut u32, cond: u32, a: u32, b: u32) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(
        Opcode::Select,
        r,
        Some(ValueType::F64),
        vec![cond, a, b],
        None,
        None,
    ));
    r
}
fn add_wall(out: &mut Vec<Instruction>, next: &mut u32, a: u32, b: u32) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(
        Opcode::Add,
        r,
        Some(ValueType::F64),
        vec![a, b],
        None,
        None,
    ));
    r
}
fn mul_wall(out: &mut Vec<Instruction>, next: &mut u32, a: u32, b: u32) -> u32 {
    let r = *next;
    *next += 1;
    out.push(instr(
        Opcode::Mul,
        r,
        Some(ValueType::F64),
        vec![a, b],
        None,
        None,
    ));
    r
}

pub(crate) fn instr(
    opcode: Opcode,
    result_id: u32,
    result_type: Option<ValueType>,
    operands: Vec<u32>,
    constant: Option<crate::eir::Immediate>,
    target: Option<ComponentRef>,
) -> Instruction {
    Instruction {
        opcode,
        result_id,
        result_type,
        operands,
        constant,
        target,
    }
}

pub(crate) fn cr(entity: u128, component: ComponentTypeId, offset: u32) -> ComponentRef {
    ComponentRef {
        entity,
        component,
        offset,
    }
}

/// A composed set of reusable EIR systems run in declaration order each step.
pub struct PhysicsProgram {
    pub systems: Vec<Box<dyn EirSystem>>,
    pub entities: Vec<u128>,
    pub module: EirModule,
}

impl PhysicsProgram {
    /// Lower every system × every entity into one EIR module (deterministic
    /// ordering: system order, then entity id order).
    pub fn build(systems: Vec<Box<dyn EirSystem>>, entities: Vec<u128>) -> Self {
        let mut functions = Vec::new();
        let mut fid = 1u64;
        for (s_idx, sys) in systems.iter().enumerate() {
            let mut ents = entities.clone();
            ents.sort_unstable();
            for entity in ents {
                let mut instrs = Vec::new();
                sys.lower_entity(entity, &mut instrs);
                // Skip systems that intentionally target no instruction at this
                // entity (e.g. a sensor only emits for its own entity).
                if !instrs.iter().any(|i| i.opcode == Opcode::Return) {
                    continue;
                }
                functions.push(Function {
                    id: fid,
                    effect_mask: crate::eir::EIR_EFFECT_READ_WORLD
                        | crate::eir::EIR_EFFECT_WRITE_WORLD,
                    argument_count: 0,
                    instructions: instrs,
                });
                fid += 1;
                let _ = s_idx;
            }
        }
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions,
        };
        PhysicsProgram {
            systems,
            entities,
            module,
        }
    }
}

/// Applies ordered writes back to a scene, decoding f64 fields.
pub fn apply_writes(scene: &mut Scene, writes: &[crate::eir::WorldWrite]) -> Result<()> {
    // Canonical field component id -> field name, for grid cell writes.
    let field_names: std::collections::BTreeMap<ComponentTypeId, String> = scene
        .fields
        .keys()
        .map(|n| (field_component_id(n), n.clone()))
        .collect();
    for w in writes {
        if let Some(name) = field_names.get(&w.component) {
            // A grid field cell write: the field is not an entity; the
            // linear index rides in the offset.
            if let Some(f) = scene.fields.get_mut(name) {
                let idx = (w.offset / field::STATE_SLOT_BYTES) as usize;
                if idx >= f.cells().len() {
                    return Err(pwe_api::Error {
                        status: pwe_api::Status::Invalid,
                        detail: 7,
                        byte_offset: 0,
                    });
                }
                f.set_linear(idx, f64::from_bits(w.value));
            }
            continue;
        }
        let entity = pwe_api::EntityId(w.entity);
        let e = scene.get_mut(entity).ok_or(pwe_api::Error {
            status: pwe_api::Status::HandleStale,
            detail: 5,
            byte_offset: 0,
        })?;
        let value = f64::from_bits(w.value);
        if w.component == velocity_id() {
            let v = e.velocity.get_or_insert_with(Velocity::default);
            match w.offset {
                field::VEL_X => v.linear.x = value,
                field::VEL_Y => v.linear.y = value,
                field::VEL_Z => v.linear.z = value,
                _ => {}
            }
        } else if w.component == transform_id() {
            let t = e.transform.get_or_insert_with(Transform::default);
            match w.offset {
                field::POS_X => t.position.x = value,
                field::POS_Y => t.position.y = value,
                field::POS_Z => t.position.z = value,
                _ => {}
            }
        } else if w.component == state_id() {
            let st = e.state.get_or_insert_with(|| {
                crate::components::State::new(vec![0.0; crate::components::State::MAX_STATE_SLOTS])
            });
            let slot = (w.offset / field::STATE_SLOT_BYTES) as usize;
            if let Some(v) = st.values.get_mut(slot) {
                *v = value;
            }
        }
    }
    Ok(())
}

/// A high-level simulation driven entirely by EIR: the physics system is
/// lowered once to an `EirModule`, and every step is executed through the
/// interpreter (RFC-0004: interpreter is the semantic oracle). The host only
/// supplies a `Scene` and applies the interpreter's ordered writes.
pub struct EirSimulation {
    pub scene: Scene,
    pub dt: f64,
    pub clock: u64,
    lowered: LoweredPhysics,
}

impl EirSimulation {
    pub fn new(scene: Scene, dt: f64) -> Self {
        let entities: Vec<u128> = scene
            .iter()
            .filter(|(_, e)| e.rigid_body.map(|rb| rb.is_dynamic != 0).unwrap_or(false))
            .map(|(id, _)| id.0)
            .collect();
        let lowered = lower_physics(&entities, scene.gravity.y, dt);
        Self {
            scene,
            dt,
            clock: 0,
            lowered,
        }
    }

    /// Advances one step purely through EIR.
    pub fn step(&mut self) -> Result<()> {
        let mut rt = SceneRuntime::new(&self.scene);
        let writes = self.lowered.module.interpret_with(
            &mut rt,
            pwe_api::WorldId(0),
            pwe_api::WorldVersion(0),
        )?;
        apply_writes(&mut self.scene, &writes)?;
        self.clock += 1;
        Ok(())
    }

    pub fn step_n(&mut self, n: u64) -> Result<()> {
        for _ in 0..n {
            self.step()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::RigidBody;
    use crate::math::Vec3;
    use crate::scene::Entity;
    use pwe_api::EntityId;

    #[test]
    fn composed_program_gravity_integrate_damping_is_deterministic_and_valid() {
        // Compose gravity + integrate + damping and run against a scene.
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        body(&mut scene, 1, 10.0, 0.0, true);
        body(&mut scene, 2, 20.0, 0.0, true);

        let entities: Vec<u128> = vec![1, 2];
        let program = PhysicsProgram::build(
            vec![
                Box::new(GravitySystem {
                    gravity_y: -9.81,
                    dt: 1.0 / 60.0,
                }),
                Box::new(IntegrateSystem { dt: 1.0 / 60.0 }),
                Box::new(DampingSystem { factor: 0.995 }),
            ],
            entities,
        );

        // Module validates (SSA/type/terminator).
        program.module.validate(true).unwrap();

        // Two identical runs must yield identical positions.
        let run = |scene: &mut Scene| -> f64 {
            let mut rt = SceneRuntime::new(scene);
            let writes = program
                .module
                .interpret_with(&mut rt, pwe_api::WorldId(0), pwe_api::WorldVersion(0))
                .unwrap();
            apply_writes(scene, &writes).unwrap();
            scene.position(EntityId(1)).unwrap().y
        };
        let mut s1 = scene.clone();
        let mut s2 = scene.clone();
        let a = run(&mut s1);
        let b = run(&mut s2);
        assert_eq!(a, b);
        // Gravity acts: body 1 fell.
        assert!(a < 10.0, "body fell to {a}");
    }

    #[test]
    fn composed_program_with_ground_contact_clamps_and_bounces() {
        // Gravity + integrate + ground contact: a body dropped from height with
        // restitution must land, not tunnel below y=0.
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        body(&mut scene, 1, 5.0, 0.0, true);

        let program = PhysicsProgram::build(
            vec![
                Box::new(GravitySystem {
                    gravity_y: -9.81,
                    dt: 1.0 / 60.0,
                }),
                Box::new(IntegrateSystem { dt: 1.0 / 60.0 }),
                Box::new(GroundContactSystem { restitution: 0.6 }),
            ],
            vec![1],
        );
        program.module.validate(true).unwrap();

        let mut min_y = f64::INFINITY;
        let mut bounced_above = false;
        let mut below = false;
        for _ in 0..240 {
            let mut rt = SceneRuntime::new(&scene);
            let writes = program
                .module
                .interpret_with(&mut rt, pwe_api::WorldId(0), pwe_api::WorldVersion(0))
                .unwrap();
            apply_writes(&mut scene, &writes).unwrap();
            let y = scene.position(EntityId(1)).unwrap().y;
            min_y = min_y.min(y);
            // Once we've contacted the ground (pos dropped near 0) and later
            // rise above it, we observed a bounce.
            if y <= 1e-6 {
                below = true;
            }
            if below && y > 0.01 {
                bounced_above = true;
            }
        }
        // Must never go below the ground plane.
        assert!(min_y >= -1e-9, "body tunneled to {min_y}");
        // The body must have contacted the ground and then bounced above it.
        assert!(bounced_above, "expected a bounce, min_y={min_y}");
    }

    fn body(scene: &mut Scene, id: u128, y: f64, vy: f64, dynamic: bool) {
        let mut e = Entity::dynamic();
        e.transform = Some(Transform {
            position: Vec3::new(0.0, y, 0.0),
            ..Default::default()
        });
        e.velocity = Some(Velocity {
            linear: Vec3::new(0.0, vy, 0.0),
            angular: Vec3::ZERO,
        });
        e.rigid_body = Some(RigidBody {
            mass: 1.0,
            restitution: 0.0,
            friction: 0.0,
            is_dynamic: dynamic as u8,
        });
        scene.insert(EntityId(id), e);
    }

    #[test]
    fn eir_simulation_matches_analytic_free_fall() {
        // Semi-implicit Euler: v_{n+1} = v_n + g*dt; y_{n+1} = y_n + v_{n+1}*dt.
        // Closed form after n steps: y = y0 + v0*t + 0.5*g*t*t + 0.5*g*dt*t.
        let g = -9.81;
        let dt = 1.0 / 60.0;
        let mut scene = Scene::new(Vec3::new(0.0, g, 0.0));
        body(&mut scene, 1, 10.0, 2.0, true);
        let mut eir = EirSimulation::new(scene, dt);

        let n = 120u64;
        eir.step_n(n).unwrap();
        let t = n as f64 * dt;
        let expected = 10.0 + 2.0 * t + 0.5 * g * t * t + 0.5 * g * dt * t;
        let actual = eir.scene.position(EntityId(1)).unwrap().y;
        assert!(
            (actual - expected).abs() < 1e-6,
            "eir={actual} expected={expected}"
        );
    }

    #[test]
    fn lower_and_run_gravity_integration() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        body(&mut scene, 1, 10.0, 0.0, true);
        let dt = 1.0 / 60.0;

        let lowered = lower_physics(&[1u128], scene.gravity.y, dt);
        let mut rt = SceneRuntime::new(&scene);
        let writes = lowered
            .module
            .interpret_with(&mut rt, pwe_api::WorldId(0), pwe_api::WorldVersion(0))
            .unwrap();
        assert!(!writes.is_empty());

        // Apply writes: velocity.y should become -9.81*dt; position.y should drop by that.
        let vy0 = scene.get(EntityId(1)).unwrap().velocity.unwrap().linear.y;
        let y0 = scene
            .get(EntityId(1))
            .unwrap()
            .transform
            .unwrap()
            .position
            .y;
        apply_writes(&mut scene, &writes).unwrap();
        let vy1 = scene.get(EntityId(1)).unwrap().velocity.unwrap().linear.y;
        let y1 = scene
            .get(EntityId(1))
            .unwrap()
            .transform
            .unwrap()
            .position
            .y;
        assert!((vy1 - (vy0 + scene.gravity.y * dt)).abs() < 1e-9);
        assert!((y1 - (y0 + vy1 * dt)).abs() < 1e-9);
    }
}
