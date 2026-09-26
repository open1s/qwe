//! PWE modeling language — an embedded Rust DSL (layer #2), modeled after
//! Erlang↔BEAM: the DSL is the *language*, it compiles to low-level IR, and the
//! runtime loads and executes that IR.
//!
//! Erlang       → `erlc`  → BEAM bytecode (a `.beam` module) → BEAM VM
//! PWE DSL      → compiler → `EirModule` (a `PweModule` of EIR) → runtime
//!
//! `world!` declares a world model (entities + components) — the WIR-level
//! IR, like a module's data. `program!` declares systems that lower to EIR —
//! the module's functions. `compile_module` fuses them into a `PweModule`;
//! `PweRuntime::load` then executes it against a `Scene` (BEAM `code:load_binary`).
//!
//! The DSL never touches runtime state during declaration; it only *describes*.
//! All mutation happens later, when the runtime executes the compiled IR.

use crate::math::Vec3;
use crate::wir::WirDocument;
use pwe_api::{ComponentTypeId, EntityId, Hash256, Result};

/// A field of a `struct` type: a scalar default or a nested struct type.
#[derive(Clone, Debug, PartialEq)]
pub enum StructFieldType {
    Scalar(f64),
    Struct(String),
}

/// RFC-0042: user-defined composite types (`struct`) flattened to dotted state
/// slots. `structs[name] = [(field, type/default), …]`, in declaration order.
pub type StructDef = Vec<(String, StructFieldType)>;

/// RFC-0040: a mass-spring soft body — an `nx × ny` grid of dynamic particles
/// (all active), connected by structural/shear/bend spring constraints.
#[derive(Clone, Debug, PartialEq)]
pub struct SoftDecl {
    pub name: String,
    pub nx: u32,
    pub ny: u32,
    /// Grid depth (RFC-0040 3D extension); 1 = a 2D sheet.
    pub nz: u32,
    pub spacing: f64,
    pub origin: Vec3,
    pub mass: f64,
    /// Presentation shape/size for the particles.
    pub shape: Option<String>,
    pub size: Option<f64>,
}

/// RFC-0038: an entity pool — `count` contiguous slots sharing one declaration,
/// all inactive at boot.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolDecl {
    pub name: String,
    pub count: u32,
    /// Per-slot template (state/position/...); slots are named `<name>#<k>`.
    pub decl: EntityDecl,
}

/// One declared entity in a world model.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDecl {
    pub name: String,
    pub position: Option<Vec3>,
    pub velocity: Option<Vec3>,
    pub mass: Option<f64>,
    pub dynamic: Option<bool>,
    /// Static euler rotation in radians (`rotation = (rx, ry, rz)`), e.g. a
    /// tilted ground plane. Applies to transform-based entities.
    pub rotation: Option<Vec3>,
    pub restitution: Option<f64>,
    pub friction: Option<f64>,
    /// `0` = box (dims), `1` = sphere (radius in `radius`).
    pub collider: Option<ColliderDecl>,
    /// Presence of a camera component on this entity.
    pub camera: Option<bool>,
    /// Generic scalar state slots for user-defined dynamical systems.
    pub state: Option<Vec<f64>>,
    /// Optional names for the state slots (from `state = (x = 0, …)`), enabling
    /// named access in rules. `None` marks an unnamed (positional) slot. Aligned
    /// with `state` by index.
    pub state_names: Option<Vec<Option<String>>>,
    /// Presentation color as `0xRRGGBB` (visualization only).
    pub color: Option<u32>,
    /// `false` excludes this entity from the mutual `nbody` system (e.g. a moon
    /// whose motion is driven by a targeted `update` rule instead).
    pub nbody: Option<bool>,
    /// Optional parent body name (`parent = earth`), for satellites.
    pub parent: Option<String>,
    /// Optional per-slot dimensions (`state = (x = 0 m, vx = 0 m/s)`), aligned
    /// with `state` by index; `None` marks an unannotated slot. Compile-time
    /// only (gradual dimensional analysis).
    pub state_units: Option<Vec<Option<crate::units::Dim>>>,
    /// Presentation-only render overrides (`shape`/`size`/`opacity`/`glow`/`label`).
    pub render: Option<crate::components::RenderStyle>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColliderDecl {
    Box { dims: Vec3 },
    Sphere { radius: f64 },
    ConvexHull { points: Vec<Vec3> },
}

impl EntityDecl {
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            position: None,
            velocity: None,
            mass: None,
            dynamic: None,
            rotation: None,
            restitution: None,
            friction: None,
            collider: None,
            camera: None,
            state: None,
            state_names: None,
            color: None,
            nbody: None,
            parent: None,
            state_units: None,
            render: None,
        }
    }
}

/// A declared channel: a named mailbox holding a single latest value (slot 0 of
/// a channel entity's state). Senders write it; receivers read it, Go-style.
#[derive(Clone, Debug, PartialEq)]
pub struct ChanDecl {
    pub name: String,
    pub value: f64,
}

/// A declared deterministic scalar grid field: the PDE substrate the language's
/// rules read and write (`fget`/`fset`/`flap`). Storage, discrete operators,
/// and determinism hashing live in `field::Field`; this is the declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub dx: f64,
}

/// A WIR-level world model: gravity plus declared entities. This is the
/// portable, runtime-neutral representation the DSL compiles to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldModel {
    pub gravity: Vec3,
    /// Optional human-readable title (`title = "..."`), shown by `pwe present`.
    pub title: Option<String>,
    /// Runtime-settable model parameters (`params { G = 1.0 }`), overridable
    /// with `pwe run --param G=2`.
    pub params: std::collections::BTreeMap<String, f64>,
    /// Declared units for parameters (`G = 6.7e-11 m^3*kg^-1*s^-2`).
    pub param_units: std::collections::BTreeMap<String, crate::units::Dim>,
    /// Parameter alias -> canonical key, for modules imported under several
    /// namespaces (`--param` updates every alias of one parameter).
    pub param_alias: std::collections::BTreeMap<String, String>,
    pub entities: Vec<EntityDecl>,
    /// RFC-0038: entity pools (fixed blocks of initially-inactive slots).
    pub pools: Vec<PoolDecl>,
    /// RFC-0040: soft bodies (grids of dynamic mass-spring particles).
    pub softs: Vec<SoftDecl>,
    /// RFC-0042: user-defined `struct` record types.
    pub structs: std::collections::BTreeMap<String, StructDef>,
    pub channels: Vec<ChanDecl>,
    pub fields: Vec<FieldDecl>,
    /// User-defined custom shapes: name -> parts (multi-primitive, local offsets).
    pub shapes: std::collections::BTreeMap<String, Vec<crate::components::ShapePart>>,
}

impl WorldModel {
    pub fn new(gravity: Vec3) -> Self {
        Self {
            gravity,
            title: None,
            params: std::collections::BTreeMap::new(),
            param_units: std::collections::BTreeMap::new(),
            param_alias: std::collections::BTreeMap::new(),
            entities: Vec::new(),
            pools: Vec::new(),
            softs: Vec::new(),
            structs: std::collections::BTreeMap::new(),
            channels: Vec::new(),
            fields: Vec::new(),
            shapes: std::collections::BTreeMap::new(),
        }
    }

    /// Lowers to a runtime `Scene` (executable world state).
    pub fn build_scene(&self) -> crate::scene::Scene {
        let mut scene = crate::scene::Scene::new(self.gravity);
        scene.params = self.params.clone();
        for decl in &self.fields {
            scene.fields.insert(
                decl.name.clone(),
                crate::field::Field::new3(decl.width, decl.height, decl.depth, decl.dx),
            );
        }
        for (index, decl) in self.entities.iter().enumerate() {
            let e = self.build_entity(decl, true);
            scene.insert(EntityId((index as u128) + 1), e);
        }
        // Channel entities follow the bodies; each holds its latest value in
        // state slot 0 and is never updated (not a dynamic body).
        let body_count = self.entities.len() as u128;
        for (index, chan) in self.channels.iter().enumerate() {
            let mut e = crate::scene::Entity::dynamic();
            e.state = Some(crate::components::State::new(vec![chan.value]));
            scene.insert(EntityId(body_count + (index as u128) + 1), e);
        }
        // RFC-0038: pool slots follow the channels, all inactive.
        let mut next_slot = body_count + self.channels.len() as u128 + 1;
        for pool in &self.pools {
            for _ in 0..pool.count {
                let e = self.build_entity(&pool.decl, false);
                scene.insert(EntityId(next_slot), e);
                next_slot += 1;
            }
        }
        // RFC-0040: soft-body particles follow the pools, all active.
        for sd in &self.softs {
            for k in 0..sd.nz {
                for j in 0..sd.ny {
                    for i in 0..sd.nx {
                        let mut e = crate::scene::Entity::dynamic();
                        e.transform = Some(crate::components::Transform {
                            position: Vec3::new(
                                sd.origin.x + i as f64 * sd.spacing,
                                sd.origin.y + j as f64 * sd.spacing,
                                sd.origin.z + k as f64 * sd.spacing,
                            ),
                            rotation: Default::default(),
                        });
                        e.velocity = Some(crate::components::Velocity::default());
                        e.rigid_body = Some(crate::components::RigidBody::dynamic(sd.mass));
                        if let Some(shape) = &sd.shape {
                            let mut r = crate::components::RenderStyle::default();
                            let mut custom = false;
                            match shape.as_str() {
                                "sphere" => r.shape = Some(1),
                                "box" => r.shape = Some(2),
                                "capsule" => r.shape = Some(3),
                                other => {
                                    r.shape_name = Some(other.to_string());
                                    custom = true;
                                }
                            }
                            r.size = sd.size;
                            if custom {
                                // custom shape parts resolve elsewhere; keep the name
                            }
                            e.render = Some(r);
                        }
                        scene.insert(EntityId(next_slot), e);
                        next_slot += 1;
                    }
                }
            }
        }
        scene
    }

    /// RFC-0040: `(name, base id, nx, ny, nz, spacing)` per soft body.
    pub fn soft_ranges(&self) -> Vec<(String, u128, u32, u32, u32, f64)> {
        let pool_slots: u128 = self.pools.iter().map(|p| p.count as u128).sum();
        let mut next = self.entities.len() as u128 + self.channels.len() as u128 + pool_slots + 1;
        let mut out = Vec::new();
        for sd in &self.softs {
            out.push((sd.name.clone(), next, sd.nx, sd.ny, sd.nz, sd.spacing));
            next += sd.nx as u128 * sd.ny as u128 * sd.nz as u128;
        }
        out
    }

    /// RFC-0038: `(name, base id, count)` for each pool, in declaration order,
    /// matching `build_scene`'s slot assignment.
    pub fn pool_ranges(&self) -> Vec<(String, u128, u32)> {
        let mut out = Vec::new();
        let mut next = self.entities.len() as u128 + self.channels.len() as u128 + 1;
        for p in &self.pools {
            out.push((p.name.clone(), next, p.count));
            next += p.count as u128;
        }
        out
    }

    /// RFC-0040: the render bonds (structural + shear edges) of every soft body.
    pub fn soft_bonds(&self) -> Vec<(u128, u128)> {
        let mut out = Vec::new();
        for (_, base, nx, ny, nz, _) in self.soft_ranges() {
            let idx = |i: u32, j: u32, k: u32| base + ((k * ny + j) * nx + i) as u128;
            for k in 0..nz {
                for j in 0..ny {
                    for i in 0..nx {
                        if i + 1 < nx {
                            out.push((idx(i, j, k), idx(i + 1, j, k)));
                        }
                        if j + 1 < ny {
                            out.push((idx(i, j, k), idx(i, j + 1, k)));
                        }
                        if k + 1 < nz {
                            out.push((idx(i, j, k), idx(i, j, k + 1)));
                        }
                        if i + 1 < nx && j + 1 < ny {
                            out.push((idx(i, j, k), idx(i + 1, j + 1, k)));
                            out.push((idx(i + 1, j, k), idx(i, j + 1, k)));
                        }
                        if i + 1 < nx && k + 1 < nz {
                            out.push((idx(i, j, k), idx(i + 1, j, k + 1)));
                            out.push((idx(i + 1, j, k), idx(i, j, k + 1)));
                        }
                        if j + 1 < ny && k + 1 < nz {
                            out.push((idx(i, j, k), idx(i, j + 1, k + 1)));
                            out.push((idx(i, j + 1, k), idx(i, j, k + 1)));
                        }
                    }
                }
            }
        }
        out
    }

    /// Builds one runtime entity from a declaration; `active` is the RFC-0038
    /// lifecycle flag (true for ordinary entities, false for pool slots).
    fn build_entity(&self, decl: &EntityDecl, active: bool) -> crate::scene::Entity {
        use crate::components::{Collider, RigidBody, Transform, Velocity};
        let mut e = crate::scene::Entity::dynamic();
        e.active = active;
        if let Some(p) = decl.position {
            let t = e.transform.get_or_insert_with(Transform::default);
            t.position = p;
        }
        if let Some(r) = decl.rotation {
            let t = e.transform.get_or_insert_with(Transform::default);
            t.rotation = crate::math::Quat::from_euler(r.x, r.y, r.z);
        }
        if let Some(v) = decl.velocity {
            let vel = e.velocity.get_or_insert_with(Velocity::default);
            vel.linear = v;
        }
        if decl.mass.is_some()
            || decl.dynamic.is_some()
            || decl.restitution.is_some()
            || decl.friction.is_some()
        {
            let rb = e.rigid_body.get_or_insert_with(|| RigidBody::dynamic(1.0));
            if let Some(m) = decl.mass {
                rb.mass = m;
            }
            if let Some(d) = decl.dynamic {
                rb.is_dynamic = if d { 1 } else { 0 };
            }
            if let Some(r) = decl.restitution {
                rb.restitution = r;
            }
            if let Some(f) = decl.friction {
                rb.friction = f;
            }
            // A dynamic body always carries a velocity component.
            if rb.is_dynamic == 1 && e.velocity.is_none() {
                e.velocity = Some(Velocity::default());
            }
        }
        if let Some(c) = &decl.collider {
            e.collider = Some(match c {
                ColliderDecl::Box { dims } => Collider::aabb(*dims),
                ColliderDecl::Sphere { radius } => Collider::sphere(*radius),
                ColliderDecl::ConvexHull { points } => Collider::convex_hull(points.clone()),
            });
        }
        if decl.camera == Some(true) {
            e.camera = Some(crate::components::Camera::default());
        }
        if let Some(values) = &decl.state {
            e.state = Some(crate::components::State::new(values.clone()));
        }
        if let Some(mut r) = decl.render.clone() {
            if let Some(name) = &r.shape_name {
                r.parts = self.shapes.get(name).cloned();
            }
            e.render = Some(r);
        }
        if let Some(c) = decl.color {
            e.color = Some(c);
        }
        e
    }

    /// Lowers this `WorldModel` into a canonical WIR document (RFC-0020) — the
    /// portable, runtime-neutral serialization of the model. Component records
    /// carry the same RFC-0019 canonical type IDs the EIR runtime reads, so the
    /// emitted WIR and the EIR execution path share one component ID space.
    pub fn lower_to_wir(&self) -> Result<WirDocument> {
        use crate::components::{RigidBody, Transform, Velocity};
        use crate::schema::{ComponentIdentity, Field, FieldType, Schema};
        use crate::wir::{ComponentRecord, EntityRecord, WirDocument, FLAG_INITIAL_STATE};
        use pwe_api::{WorldId, WorldVersion};

        fn physics_schema(stable_name: &str, fields: Vec<Field>) -> Schema {
            Schema {
                identity: ComponentIdentity {
                    namespace: "pwe.physics".into(),
                    stable_name: stable_name.into(),
                    major_version: 1,
                },
                fields,
            }
        }

        let mut schemas: Vec<Schema> = Vec::new();
        let mut type_id: std::collections::BTreeMap<&'static str, ComponentTypeId> =
            std::collections::BTreeMap::new();

        // Register the physics component schemas used by this model.
        let mut register = |name: &'static str, schema: Schema| {
            let id = schema.identity.type_id()?;
            schemas.push(schema);
            type_id.insert(name, id);
            Ok::<(), pwe_api::Error>(())
        };
        register(
            "transform",
            physics_schema(
                "transform",
                vec![
                    Field {
                        id: 1,
                        name: "tx".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                    Field {
                        id: 2,
                        name: "ty".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                    Field {
                        id: 3,
                        name: "tz".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                ],
            ),
        )?;
        register(
            "velocity",
            physics_schema(
                "velocity",
                vec![
                    Field {
                        id: 1,
                        name: "vx".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                    Field {
                        id: 2,
                        name: "vy".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                    Field {
                        id: 3,
                        name: "vz".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                ],
            ),
        )?;
        register(
            "rigid_body",
            physics_schema(
                "rigid_body",
                vec![
                    Field {
                        id: 1,
                        name: "mass".into(),
                        ty: FieldType::F64,
                        flags: 1,
                    },
                    Field {
                        id: 2,
                        name: "is_dynamic".into(),
                        ty: FieldType::Bool,
                        flags: 1,
                    },
                ],
            ),
        )?;

        let mut entities = Vec::with_capacity(self.entities.len());
        let mut components: Vec<ComponentRecord> = Vec::new();
        for (index, decl) in self.entities.iter().enumerate() {
            let entity = EntityId((index as u128) + 1);
            let generation = 1u32;
            entities.push(EntityRecord {
                id: entity,
                generation,
                flags: 0,
            });

            // Transform component (always present; scene builds one per entity).
            if let Some(p) = decl.position {
                components.push(ComponentRecord {
                    type_id: type_id["transform"],
                    entity,
                    generation,
                    value: Transform {
                        position: p,
                        rotation: crate::math::Quat::IDENTITY,
                    }
                    .encode(),
                });
            } else {
                components.push(ComponentRecord {
                    type_id: type_id["transform"],
                    entity,
                    generation,
                    value: Transform::default().encode(),
                });
            }

            // Velocity component (dynamic bodies always carry one).
            let mut velocity = Velocity::default();
            if let Some(v) = decl.velocity {
                velocity.linear = v;
            }
            components.push(ComponentRecord {
                type_id: type_id["velocity"],
                entity,
                generation,
                value: velocity.encode(),
            });

            // Rigid body (declared dynamics only).
            if decl.mass.is_some()
                || decl.dynamic.is_some()
                || decl.restitution.is_some()
                || decl.friction.is_some()
            {
                let mut rb = RigidBody::dynamic(decl.mass.unwrap_or(1.0));
                if let Some(d) = decl.dynamic {
                    rb.is_dynamic = if d { 1 } else { 0 };
                }
                if let Some(r) = decl.restitution {
                    rb.restitution = r;
                }
                if let Some(f) = decl.friction {
                    rb.friction = f;
                }
                components.push(ComponentRecord {
                    type_id: type_id["rigid_body"],
                    entity,
                    generation,
                    value: rb.encode(),
                });
            }
        }

        Ok(WirDocument {
            flags: FLAG_INITIAL_STATE,
            world_id: WorldId(1),
            world_version: WorldVersion(0),
            sim_time_ns: 0,
            schemas,
            entities,
            components,
            resources: Vec::new(),
            spatial: Vec::new(),
            systems: Vec::new(),
            events: Vec::new(),
            timelines: Vec::new(),
        })
    }

    /// A deterministic content hash identifying this model (its "schema-set").
    pub fn model_hash(&self) -> Hash256 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.gravity.x.to_bits().to_le_bytes());
        bytes.extend_from_slice(&self.gravity.y.to_bits().to_le_bytes());
        bytes.extend_from_slice(&self.gravity.z.to_bits().to_le_bytes());
        for d in &self.entities {
            bytes.extend_from_slice(d.name.as_bytes());
            if let Some(p) = d.position {
                for c in [p.x, p.y, p.z] {
                    bytes.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
            if let Some(v) = d.velocity {
                for c in [v.x, v.y, v.z] {
                    bytes.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
            if let Some(m) = d.mass {
                bytes.extend_from_slice(&m.to_bits().to_le_bytes());
            }
            if let Some(r) = d.restitution {
                bytes.extend_from_slice(&r.to_bits().to_le_bytes());
            }
            if let Some(f) = d.friction {
                bytes.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            if let Some(_c) = &d.collider {}
        }
        crate::sha256::digest(&bytes)
    }
}

/// Builds a `WorldModel` from a declarative world definition. Supported fields
/// per entity: `position`, `velocity`, `mass`, `dynamic`, `restitution`,
/// `friction`, `box`, `sphere`.
#[macro_export]
macro_rules! world {
    // Entry with explicit gravity.
    (gravity = [$gx:expr, $gy:expr, $gz:expr]; $($rest:tt)*) => {
        $crate::world!(@ent [$gx, $gy, $gz] ; $($rest)*)
    };

    // --- internal entity accumulation (must precede the no-gravity catch-all) ---
    (@ent $g:tt ; entity $name:tt { $($fields:tt)* } $($rest:tt)*) => {{
        let mut __m = $crate::world!(@ent $g ; $($rest)*);
        let mut __e = $crate::dsl::EntityDecl::named(stringify!($name));
        $crate::world!(@fields __e ; $($fields)*);
        __m.entities.insert(0, __e);
        __m
    }};
    (@ent $g:tt ;) => {{
        $crate::dsl::WorldModel::new($crate::math::Vec3::new($g[0], $g[1], $g[2]))
    }};

    (@fields $e:ident ; ) => {};
    (@fields $e:ident ; position = [$x:expr, $y:expr, $z:expr]; $($rest:tt)*) => {
        { $e.position = Some($crate::math::Vec3::new($x, $y, $z)); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; velocity = [$x:expr, $y:expr, $z:expr]; $($rest:tt)*) => {
        { $e.velocity = Some($crate::math::Vec3::new($x, $y, $z)); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; mass = $m:expr; $($rest:tt)*) => {
        { $e.mass = Some($m); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; dynamic = $d:expr; $($rest:tt)*) => {
        { $e.dynamic = Some($d); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; restitution = $r:expr; $($rest:tt)*) => {
        { $e.restitution = Some($r); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; friction = $f:expr; $($rest:tt)*) => {
        { $e.friction = Some($f); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; box = [$hx:expr, $hy:expr, $hz:expr]; $($rest:tt)*) => {
        { $e.collider = Some($crate::dsl::ColliderDecl::Box { dims: $crate::math::Vec3::new($hx, $hy, $hz) }); }
        $crate::world!(@fields $e ; $($rest)*)
    };
    (@fields $e:ident ; sphere = $r:expr; $($rest:tt)*) => {
        { $e.collider = Some($crate::dsl::ColliderDecl::Sphere { radius: $r }); }
        $crate::world!(@fields $e ; $($rest)*)
    };

    // Entry without gravity (must be LAST so internal @ent/@fields win).
    ($($rest:tt)*) => {
        $crate::world!(@ent [0.0, 0.0, 0.0] ; $($rest)*)
    };
}

/// Builds a composed `PhysicsProgram` (domain systems that lower to EIR).
///
/// Systems are declared by keyword and mapped to concrete reusable `EirSystem`
/// types:
/// `gravity { .. }`, `integrate { .. }`, `damping { .. }`,
/// `ground_contact { .. }`.
#[macro_export]
macro_rules! program {
    (entities = [$($e:expr),*]; $($rest:tt)*) => {
        $crate::program!(@build vec![$($e as u128),*] ; $($rest)*)
    };
    (@build $es:expr ; gravity = $g:expr; dt = $dt:expr; systems = [$($k:ident { $($f:tt)* }),* $(,)?];) => {
        {
            let __systems: Vec<Box<dyn $crate::physics_eir::EirSystem>> =
                vec![$($crate::program!(@mk $k { $($f)* })),*];
            let __entities: Vec<u128> = $es;
            let _ = $g;
            let _ = $dt;
            $crate::physics_eir::PhysicsProgram::build(__systems, __entities)
        }
    };
    // --- system keyword mapping (field blocks matched as tt, safe for `/`) ---
    (@mk gravity { $($f:tt)* }) => {
        Box::new($crate::physics_eir::GravitySystem { $($f)* })
    };
    (@mk integrate { $($f:tt)* }) => {
        Box::new($crate::physics_eir::IntegrateSystem { $($f)* })
    };
    (@mk damping { $($f:tt)* }) => {
        Box::new($crate::physics_eir::DampingSystem { $($f)* })
    };
    (@mk ground_contact { $($f:tt)* }) => {
        Box::new($crate::physics_eir::GroundContactSystem { $($f)* })
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics_eir::{rigid_body_id, transform_id, velocity_id};

    fn model() -> WorldModel {
        world!(
            gravity = [0.0, -9.81, 0.0];
            entity vehicle {
                position = [0.0, 3.0, 0.0];
                velocity = [5.0, 0.0, 0.0];
                mass = 1000.0;
            }
            entity ground {
                box = [5.0, 0.5, 5.0];
            }
        )
    }

    #[test]
    fn lower_to_wir_emits_valid_round_trippable_document() {
        let doc = model().lower_to_wir().unwrap();
        // Encode then decode: canonical WIR must round-trip byte-identically.
        let bytes = doc.encode().unwrap();
        let decoded = crate::wir::WirDocument::decode(&bytes).unwrap();
        let reencoded = decoded.encode().unwrap();
        assert_eq!(reencoded, bytes);
        assert_eq!(decoded.entities.len(), 2);
        assert_eq!(decoded.schemas.len(), 3);
    }

    #[test]
    fn lower_to_wir_shares_component_ids_with_eir_runtime() {
        let doc = model().lower_to_wir().unwrap();
        // The WIR component type IDs must match the EIR runtime's canonical IDs
        // so a lowered WIR is addressable by the same EIR execution path.
        let mut registry = crate::schema::SchemaRegistry::default();
        for schema in &doc.schemas {
            registry.register(schema.clone()).unwrap();
        }
        let transform_schema = registry
            .get(transform_id())
            .expect("transform schema present");
        assert_eq!(transform_schema.type_id, transform_id());
        assert!(registry.get(velocity_id()).is_some());
        assert!(registry.get(rigid_body_id()).is_some());
    }

    #[test]
    fn lower_to_wir_encodes_entity_and_component_records() {
        let doc = model().lower_to_wir().unwrap();
        // Two entities: vehicle (id 1) and ground (id 2).
        assert_eq!(
            doc.entities.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![EntityId(1), EntityId(2)]
        );
        // vehicle has transform + velocity + rigid_body; ground has transform + velocity.
        assert_eq!(doc.components.len(), 5);
        // Ground is static (no dynamics), vehicle is dynamic.
        assert!(doc.components.iter().all(|c| c.generation == 1));
    }
}
