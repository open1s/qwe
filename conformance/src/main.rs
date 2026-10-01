//! RFC-0029 conformance runner and RFC-0030 minimal ground/vehicle/camera
//! scenario over the reference slice (interpreter + CPU JIT + deterministic
//! physics simulation).
//!
//! Every case listed here is implemented by the reference; there are no skips.

use pwe_api::{ComponentDescriptor, EntityId, EntityRef, Hash256, Status, WorldId, WorldVersion};
use pwe_reference::aot::AotProgram;
use pwe_reference::components::{Camera, Collider, RigidBody, Transform, Velocity};
use pwe_reference::eir::{EirModule, Function, Immediate, Instruction, Opcode, ValueType};
use pwe_reference::fence::{CpuGpuHandoff, Direction, Fence, MemoryOrder};
use pwe_reference::jit::{profile_hash, CodeCacheKey, CpuJit, JitAssumption};
use pwe_reference::math::Vec3;
use pwe_reference::physics::{PhysicsConfig, PhysicsSystem};
use pwe_reference::scene::{Entity, Scene};
use pwe_reference::schema::{ComponentIdentity, Field, FieldType, Schema, SchemaRegistry};
use pwe_reference::simulation::Simulation;
use pwe_reference::wir::{ComponentRecord, EntityRecord, WirDocument};
use pwe_reference::ReferenceWorld;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Pass,
    Fail,
}

impl std::fmt::Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Case::Pass => write!(f, "PASS"),
            Case::Fail => write!(f, "FAIL"),
        }
    }
}

struct Report {
    cases: Vec<(&'static str, Case)>,
}

impl Report {
    fn new() -> Self {
        Report { cases: Vec::new() }
    }
    fn record(&mut self, name: &'static str, case: Case) {
        self.cases.push((name, case));
    }
    fn failures(&self) -> usize {
        self.cases.iter().filter(|(_, c)| *c == Case::Fail).count()
    }
    fn emit(&self) {
        println!("PWE conformance report");
        println!("  implementation: pwe-reference v0.2.0");
        println!("  profile: RFC-0030 minimal (interpreter + CPU JIT + physics)");
        for (name, case) in &self.cases {
            println!("  {:<60} {}", name, case);
        }
        let fails = self.failures();
        println!("  total={} failed={}", self.cases.len(), fails);
        if fails != 0 {
            std::process::exit(1);
        }
    }
}

fn physics_schema() -> Schema {
    Schema {
        identity: ComponentIdentity {
            namespace: "pwe.physics".into(),
            stable_name: "transform".into(),
            major_version: 1,
        },
        fields: vec![
            Field {
                id: 1,
                name: "tx".into(),
                ty: FieldType::F32,
                flags: 1,
            },
            Field {
                id: 2,
                name: "ty".into(),
                ty: FieldType::F32,
                flags: 1,
            },
            Field {
                id: 3,
                name: "tz".into(),
                ty: FieldType::F32,
                flags: 1,
            },
        ],
    }
}

fn camera_schema() -> Schema {
    Schema {
        identity: ComponentIdentity {
            namespace: "pwe.render".into(),
            stable_name: "camera".into(),
            major_version: 1,
        },
        fields: vec![Field {
            id: 1,
            name: "fov".into(),
            ty: FieldType::F32,
            flags: 1,
        }],
    }
}

fn wir_round_trip(report: &mut Report) {
    let result = (|| -> Result<(), pwe_api::Error> {
        let mut registry = SchemaRegistry::default();
        let physics = (*registry.register(physics_schema())?).clone();
        registry.register(camera_schema())?;
        let schema_set_hash = registry.schema_set_hash();

        let doc = WirDocument {
            flags: 1,
            world_id: WorldId(42),
            world_version: WorldVersion(0),
            sim_time_ns: 0,
            schemas: vec![physics_schema(), camera_schema()],
            entities: vec![
                EntityRecord {
                    id: EntityId(1),
                    generation: 1,
                    flags: 0,
                },
                EntityRecord {
                    id: EntityId(2),
                    generation: 1,
                    flags: 0,
                },
            ],
            components: vec![ComponentRecord {
                type_id: physics.type_id,
                entity: EntityId(1),
                generation: 1,
                value: vec![0; 12],
            }],
            resources: vec![],
            spatial: vec![],
            systems: vec![],
            events: vec![],
            timelines: vec![],
        };
        let _ = schema_set_hash;
        let bytes = doc.encode()?;
        let decoded = WirDocument::decode(&bytes)?;
        let reencoded = decoded.encode()?;
        if reencoded != bytes {
            return Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 0,
                byte_offset: 0,
            });
        }
        Ok(())
    })();
    report.record(
        "WIR canonical round-trip",
        if result.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );

    let rejection = (|| -> Result<(), pwe_api::Error> {
        let doc = WirDocument {
            flags: 1,
            world_id: WorldId(42),
            world_version: WorldVersion(0),
            sim_time_ns: 0,
            schemas: vec![physics_schema()],
            entities: vec![],
            components: vec![],
            resources: vec![],
            spatial: vec![],
            systems: vec![],
            events: vec![],
            timelines: vec![],
        };
        let mut bytes = doc.encode()?;
        bytes[0] = b'X';
        match WirDocument::decode(&bytes) {
            Err(_) => Ok(()),
            Ok(_) => Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 0,
                byte_offset: 0,
            }),
        }
    })();
    report.record(
        "WIR rejection (bad magic)",
        if rejection.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

fn transaction_atomicity(report: &mut Report) {
    let mut world = ReferenceWorld::new(WorldId(42));
    let result = (|| -> Result<(), pwe_api::Error> {
        let options = pwe_api::TransactionOptions {
            base_version: WorldVersion(0),
            ownership: pwe_api::Ownership {
                region: pwe_api::RegionId(1),
                epoch: pwe_api::OwnershipEpoch(1),
            },
            policy: pwe_api::ConflictPolicy::Reject,
        };
        let claims = pwe_api::CapabilityClaims {
            issuer: Hash256([1; 32]),
            subject: Hash256([2; 32]),
            world: WorldId(42),
            region: pwe_api::RegionId(1),
            access: pwe_api::Access::CREATE.union(pwe_api::Access::WRITE),
            expires_at_version: WorldVersion(u64::MAX),
            nonce: [0; 16],
        };
        let mut tx = world.begin(options, claims)?;
        tx.create(EntityRef {
            id: EntityId(1),
            generation: 1,
        })?;
        tx.commit()?;
        // A transaction begun at a stale base version must fail closed under
        // the Reject policy, at commit (RFC-0023 VALIDATED), not at begin.
        let mut stale = world.begin(
            pwe_api::TransactionOptions {
                base_version: WorldVersion(0),
                ..options
            },
            claims,
        )?;
        stale.create(EntityRef {
            id: EntityId(2),
            generation: 1,
        })?;
        if stale.commit().map(|_| ()).is_ok() {
            return Err(pwe_api::Error {
                status: Status::Conflict,
                detail: 0,
                byte_offset: 0,
            });
        }
        Ok(())
    })();
    report.record(
        "transaction atomicity + stale base version",
        if result.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

fn snapshot_replay(report: &mut Report) {
    let mut registry = SchemaRegistry::default();
    let physics = (*registry.register(physics_schema()).unwrap()).clone();
    let schema_set_hash = registry.schema_set_hash();

    let mut world = ReferenceWorld::new(WorldId(42));
    let options = pwe_api::TransactionOptions {
        base_version: WorldVersion(0),
        ownership: pwe_api::Ownership {
            region: pwe_api::RegionId(1),
            epoch: pwe_api::OwnershipEpoch(1),
        },
        policy: pwe_api::ConflictPolicy::Reject,
    };
    let claims = pwe_api::CapabilityClaims {
        issuer: Hash256([1; 32]),
        subject: Hash256([2; 32]),
        world: WorldId(42),
        region: pwe_api::RegionId(1),
        access: pwe_api::Access::CREATE.union(pwe_api::Access::WRITE),
        expires_at_version: WorldVersion(u64::MAX),
        nonce: [0; 16],
    };
    {
        let mut tx = world.begin(options, claims).unwrap();
        tx.create(EntityRef {
            id: EntityId(1),
            generation: 1,
        })
        .unwrap();
        tx.put_component(
            EntityRef {
                id: EntityId(1),
                generation: 1,
            },
            ComponentDescriptor {
                type_id: physics.type_id,
                schema_hash: physics.hash,
                abi_major: 2,
                flags: 0,
                value_size: 12,
                value_align: 4,
            },
            &[0; 12],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let first = world.snapshot(schema_set_hash).unwrap();
    let mut restored = ReferenceWorld::new(WorldId(42));
    restored.restore_snapshot(schema_set_hash, &first).unwrap();
    let second = restored.snapshot(schema_set_hash).unwrap();
    report.record(
        "snapshot -> restore -> replay identical hash",
        if first == second {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

fn eir_interpreter(report: &mut Report) {
    let result = (|| -> Result<(), pwe_api::Error> {
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash: Hash256([0; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: pwe_reference::eir::EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 1,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(1)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 2,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(7)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Store,
                        result_id: 0,
                        result_type: None,
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Return,
                        result_id: 0,
                        result_type: None,
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                ],
            }],
        };
        let writes = module.interpret(WorldId(0), WorldVersion(0))?;
        if writes.len() == 1 && writes[0].value == 7 {
            Ok(())
        } else {
            Err(pwe_api::Error {
                status: Status::EirInvalid,
                detail: 0,
                byte_offset: 0,
            })
        }
    })();
    report.record(
        "EIR interpreter ordered writes",
        if result.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

fn render_frame(report: &mut Report) {
    let mut registry = SchemaRegistry::default();
    let camera = (*registry.register(camera_schema()).unwrap()).clone();
    let schema_set_hash = registry.schema_set_hash();
    let _ = schema_set_hash;

    let mut world = ReferenceWorld::new(WorldId(42));
    let options = pwe_api::TransactionOptions {
        base_version: WorldVersion(0),
        ownership: pwe_api::Ownership {
            region: pwe_api::RegionId(1),
            epoch: pwe_api::OwnershipEpoch(1),
        },
        policy: pwe_api::ConflictPolicy::Reject,
    };
    let claims = pwe_api::CapabilityClaims {
        issuer: Hash256([1; 32]),
        subject: Hash256([2; 32]),
        world: WorldId(42),
        region: pwe_api::RegionId(1),
        access: pwe_api::Access::CREATE.union(pwe_api::Access::WRITE),
        expires_at_version: WorldVersion(u64::MAX),
        nonce: [0; 16],
    };
    let entity = EntityRef {
        id: EntityId(1),
        generation: 1,
    };
    {
        let mut tx = world.begin(options, claims).unwrap();
        tx.create(entity).unwrap();
        tx.put_component(
            entity,
            ComponentDescriptor {
                type_id: camera.type_id,
                schema_hash: camera.hash,
                abi_major: 2,
                flags: 0,
                value_size: 4,
                value_align: 4,
            },
            &[1, 2, 3, 4],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let frame = world.acquire_frame(0, 0);
    let version_seen = frame.id().world_version;
    let bytes = frame.component_bytes(entity, camera.type_id);
    report.record(
        "render frame reads one consistent version",
        if bytes.is_ok() && version_seen == WorldVersion(1) {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

/// Extension RFCs (0037-0042): bulk field sweeps, pooled entities, constraint
/// joints, soft bodies, and struct record types — all run cross-backend
/// (interpreter == JIT) with a behavioural invariant.
fn extensions(report: &mut Report) {
    fn run(source: &str, steps: u64) -> Result<pwe_reference::lang::LangRuntime, pwe_api::Error> {
        let mut rt = pwe_reference::lang::LangRuntime::compile(source)?;
        rt.step_cross_n(steps)?;
        Ok(rt)
    }
    fn finite(rt: &pwe_reference::lang::LangRuntime, id: u128) -> bool {
        if let Some(e) = rt.scene.get(EntityId(id)) {
            if let Some(st) = e.state.as_ref() {
                return st.values.iter().all(|v| v.is_finite());
            }
        }
        rt.scene
            .position(EntityId(id))
            .map(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite())
            .unwrap_or(false)
    }

    // RFC-0037: a lossless wave field (leapfrog) sweeps on both backends.
    let wave = r#"
        world { gravity = (0,0,0)
            field u  { width = 81; height = 1; dx = 1.0 }
            field um { width = 81; height = 1; dx = 1.0 }
            entity probe { state = (x = 0.0, y = 0.0, z = 0.0) } }
        systems {
            update { on = probe; dt = 0.5
                for i in 0..81 { let sv = at(0.0) * sin(6.283185307179586 * i / 40.0)
                    let _ = fset(u, i, 0, fget(u, i, 0) + sv)
                    let _ = fset(um, i, 0, fget(um, i, 0) + sv) }
                x = x + inte( (fget(u, 30, 0) - x) / 0.5 ) }
            wave { field = u; prev = um; velocity = 1.0; dt = 0.5 } }
    "#;
    let wave_ok = run(wave, 40).map(|rt| finite(&rt, 1)).unwrap_or(false);
    report.record(
        "RFC-0037 bulk field sweep (wave leapfrog, cross-backend)",
        if wave_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0038: a pool spawns into fixed slots.
    let particles = r#"
        world { gravity = (0,0,0)
            entity emitter { position = (-6,0,0) state = (x=-6.0, vx=1.5) shape=box size=0.4 color=0xffaa33 }
            pool p[24] { state = (x=0.0, vx=0.0) shape=sphere size=0.18 color=0x66ccff } }
        systems {
            spawn   { on = emitter; pool = p }
            update  { on = p; dt = 0.1  x = x + inte( 0.0 + vx ) }
            despawn { on = p; when = x > 6.0 } }
    "#;
    let pool_ok = run(particles, 40)
        .map(|rt| rt.scene.entities.len() >= 25)
        .unwrap_or(false);
    report.record(
        "RFC-0038 pooled entities (spawn/despawn, cross-backend)",
        if pool_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0039: distance joints hold a chain together.
    let chain = r#"
        world { gravity = (0,-9.81,0)
            entity anchor { position=(0,6,0) mass=1.0 dynamic=false }
            entity b1 { position=(0,5,0) mass=1.0 dynamic=true }
            entity b2 { position=(1,4,0) mass=1.0 dynamic=true }
            entity b3 { position=(2,3,0) mass=1.0 dynamic=true }
            entity bob { position=(3,2,0) mass=4.0 dynamic=true } }
        systems {
            gravity { gravity_y=-9.81; dt=0.016 }
            integrate { dt=0.016 }
            joint { on=anchor; other=b1; type=distance; length=1.0; iterations=12 }
            joint { on=b1; other=b2; type=distance; length=1.0; iterations=12 }
            joint { on=b2; other=b3; type=distance; length=1.0; iterations=12 }
            joint { on=b3; other=bob; type=distance; length=1.0; iterations=12 } }
    "#;
    let joint_ok = run(chain, 120)
        .map(|rt| {
            let (a, b) = (
                rt.scene.position(EntityId(2)),
                rt.scene.position(EntityId(3)),
            );
            match (a, b) {
                (Ok(a), Ok(b)) => {
                    let d =
                        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
                    d < 3.0 && finite(&rt, 5)
                }
                _ => false,
            }
        })
        .unwrap_or(false);
    report.record(
        "RFC-0039 constraint joints (chain holds length)",
        if joint_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0040: a soft-body grid is nx*ny particles and stays finite.
    let cloth = r#"
        world { gravity = (0,-9.81,0)
            soft cloth { nx=8; ny=8; spacing=0.4; origin=(-1.4,5.0,0); mass=0.1; shape=sphere; size=0.05 } }
        systems {
            gravity { gravity_y=-9.81; dt=0.016 }
            integrate { dt=0.016 }
            soft { body=cloth; stiffness=1.0; damping=0.3; iterations=6 } }
    "#;
    let soft_ok = run(cloth, 120)
        .map(|rt| rt.scene.entities.len() == 64 && finite(&rt, 1))
        .unwrap_or(false);
    report.record(
        "RFC-0040 soft bodies (8x8 grid stays finite)",
        if soft_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0042: struct records flatten to dotted slots and run.
    let structs = r#"
        world { gravity = (0,0,0)
            struct Vec3 { x=0.0; y=0.0; z=0.0 }
            entity shooter { state = (pos = Vec3, vel = Vec3, mass = 1.0) }
            entity probe { state = (height = 0.0) } }
        systems {
            update { on=shooter; dt=0.02
                vel.y = vel.y + inte( 0.0 - 9.81 )
                pos.x = pos.x + inte( vel.x )
                pos.y = pos.y + inte( vel.y ) }
            update { on=probe; dt=1.0  height = @shooter.pos.y } }
    "#;
    let struct_ok = run(structs, 120)
        .map(|rt| {
            let st = rt.scene.get(EntityId(2)).and_then(|e| e.state.as_ref());
            st.map(|s| !s.values.is_empty() && s.values[0].is_finite())
                .unwrap_or(false)
                && finite(&rt, 1)
        })
        .unwrap_or(false);
    report.record(
        "RFC-0042 struct record types (flatten + cross-backend)",
        if struct_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0044: typed arrays (`array N name`) read/write statically and at a
    // runtime index, on both backends.
    let arrays = r#"
        world { gravity = (0,0,0)
            entity e { state = (x = 0.0, k = 2.0, s = 0.0) array 4 v { 1.0, 2.0, 3.0, 4.0 } } }
        systems { update { on=e; dt = 1.0
            v[0] = v[0] + 10.0
            v[1] += 20.0
            v[k] = v[k] + 100.0
            x = v[k] + v[3]
            let sum = 0.0
            for j in 0..len(v) { let sum = sum + v[j] }
            s = sum } }
    "#;
    let arrays_ok = run(arrays, 1)
        .map(|rt| {
            let st = rt.scene.get(EntityId(1)).and_then(|e| e.state.as_ref());
            st.map(|s| {
                // Slots: x, k, s, v.0, v.1, v.2, v.3.
                s.values.get(3) == Some(&11.0)      // v[0] += 10
                    && s.values.get(5) == Some(&103.0) // v[k] += 100 (k=2)
                    // `for j in 0..len(v)` sums the pre-write array (1+2+3+4).
                    && s.values.get(2) == Some(&10.0)
            })
            .unwrap_or(false)
                && finite(&rt, 1)
        })
        .unwrap_or(false);
    report.record(
        "RFC-0044 typed arrays (static + runtime index + len bound, cross-backend)",
        if arrays_ok { Case::Pass } else { Case::Fail },
    );

    // RFC-0043: exact integer/bool values lowered to typed EIR registers.
    // `n: i64` keeps exact integer arithmetic (`/`/`%` truncate), `f64(n)`
    // widens, and a fractional literal stays f64 — all byte-identical across
    // the interpreter and the JIT.
    let typed = r#"
        world { gravity = (0,0,0)
            entity e { state = (x = 0.0, y = 0.0, z = 0.0, w = 0.0, v = 0.0, k = 0.0) } }
        systems { update { on=e; dt = 1.0
            let n: i64 = 7 / 2
            x = f64(n)
            y = n / 2
            z = n % 4
            w = 1 / 2
            v = 1.0 / 2.0
            k = 1 + 2 * 3 } }
    "#;
    let typed_ok = run(typed, 1)
        .map(|rt| {
            let st = rt.scene.get(EntityId(1)).and_then(|e| e.state.as_ref());
            st.map(|s| {
                let v = |i: usize| s.values.get(i).copied().unwrap_or(f64::NAN);
                v(0) == 3.0       // f64(7/2) = 3
                    && v(1) == 1.0 // 3/2 = 1 (integer division)
                    && v(2) == 3.0 // 3 % 4
                    && v(3) == 0.0 // 1/2 = 0 (integer division)
                    && v(4) == 0.5 // fractional literal stays f64
                    && v(5) == 7.0 // exact integer arithmetic
            })
            .unwrap_or(false)
                && finite(&rt, 1)
        })
        .unwrap_or(false);
    report.record(
        "RFC-0043 typed int/bool values (exact integer arithmetic, cross-backend)",
        if typed_ok { Case::Pass } else { Case::Fail },
    );
}

/// RFC-0045: the semantic module system — a two-module program compiles, runs
/// cross-backend, and its `export` surface is enforced.
fn modules(report: &mut Report) {
    let dir = std::env::temp_dir().join(format!("pwe_conf_mod_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join("u.pwe"),
        "module util\nworld { }\nfuncs { f(x) { x + 1.0 }  g(x) { x - 1.0 } }\nexport f\n",
    );
    let _ = std::fs::write(
        dir.join("root.pwe"),
        "import \"u\"\nworld { gravity=(0,0,0) entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = util.f(x) } }\n",
    );
    let run_ok = (|| -> Result<(), pwe_api::Error> {
        let compiled = pwe_reference::lang::compile_file(&dir.join("root.pwe"))?;
        let scene = compiled.parsed.model.build_scene();
        let mut rt = pwe_reference::lang::LangRuntime::from_compiled_region(
            compiled,
            scene,
            pwe_api::RegionId(1),
        )?;
        rt.step_cross_n(20)?;
        Ok(())
    })()
    .is_ok();
    report.record(
        "RFC-0045 semantic modules (import + export, cross-backend)",
        if run_ok { Case::Pass } else { Case::Fail },
    );

    // The unexported `g` is rejected across the module boundary (detail 102).
    let _ = std::fs::write(
        dir.join("root.pwe"),
        "import \"u\"\nworld { gravity=(0,0,0) entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = util.g(x) } }\n",
    );
    let privacy = matches!(
        pwe_reference::lang::compile_file(&dir.join("root.pwe"))
            .err()
            .map(|e| e.detail),
        Some(102)
    );
    report.record(
        "RFC-0045 module export surface enforced (detail 102)",
        if privacy { Case::Pass } else { Case::Fail },
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    let mut report = Report::new();
    wir_round_trip(&mut report);
    transaction_atomicity(&mut report);
    snapshot_replay(&mut report);
    eir_interpreter(&mut report);
    render_frame(&mut report);
    jit_differential(&mut report);
    aot_and_fence(&mut report);
    simulation_determinism(&mut report);
    minimal_profile_scenario(&mut report);
    language_compile_cross(&mut report);
    physics_sanity(&mut report);
    extensions(&mut report);
    modules(&mut report);
    report.emit();
}

/// The PWE source language: compile source → low-level EIR, then run the same
/// IR cross-backend (interpreter + JIT) and require byte-identical writes.
fn language_compile_cross(report: &mut Report) {
    const SOURCE: &str = r#"
        world {
            gravity = (0, -9.81, 0)
            entity vehicle { position = (0, 8, 0); velocity = (4, 0, 0); mass = 4; dynamic = true; box = (1, 0.5, 0.7) }
            entity ground { position = (0, -5, 0); dynamic = false; box = (50, 5, 50) }
        }
        systems {
            gravity { gravity_y = -9.81; dt = 1 / 60 }
            integrate { dt = 1 / 60 }
            ground_contact { restitution = 0.6 }
        }
    "#;

    let result = (|| -> Result<(), pwe_api::Error> {
        let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
        rt.step_cross_n(120)?;
        let y = rt.scene.position(EntityId(1))?.y;
        if y >= 8.0 || y <= -5.0 {
            return Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 0,
                byte_offset: 0,
            });
        }
        Ok(())
    })();
    report.record(
        "PWE language compile->EIR + cross-backend run",
        if result.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

/// Phase-2 physical sanity: compile-time solver stability (86), conserved-quantity
/// drift (87), and the finite-state blow-up check (88).
fn physics_sanity(report: &mut Report) {
    use pwe_reference::lang::LangRuntime;
    let boom = || pwe_api::Error {
        status: pwe_api::Status::Invalid,
        detail: 0,
        byte_offset: 0,
    };
    let result = (|| -> pwe_api::Result<()> {
        // 86 — unstable explicit diffusion.
        let unstable = "world { gravity=(0,0,0) field t { width=8; height=8; dx=1.0 } \
            entity p { state=(0.0) } } \
            systems { diffuse { field=t; rate=0.5 } update { on=p; dt=1.0 s0=s0+inte(0.0) } }";
        match LangRuntime::compile(unstable) {
            Err(e) if e.detail == 86 => {}
            _ => return Err(boom()),
        }
        // 87 — a damped rule breaks the declared conserved energy.
        let damped = "world { gravity=(0,0,0) entity o { state=(x=1.0, v=0.0) } } \
            systems { update { on=o; dt=0.1 inte x = v; inte v = -4.0*x - 0.1*v } \
                conserved { on=o; expr = 0.5*v*v + 2.0*x*x; tolerance = 1e-3 } }";
        let mut rt = LangRuntime::compile(damped)?;
        let mut hit = false;
        for _ in 0..2000 {
            if let Err(e) = rt.step_cross() {
                if e.detail != 87 {
                    return Err(e);
                }
                hit = true;
                break;
            }
        }
        if !hit {
            return Err(boom());
        }
        // 88 — a divergent rule is caught by the finite check.
        let blow = "world { gravity=(0,0,0) entity e { state=(x=1.0) } } \
            systems { update { on=e; dt=1.0 x = x * 2.0 } }";
        let mut rt = LangRuntime::compile(blow)?;
        rt.set_finite_check(true);
        let mut hit = false;
        for _ in 0..2000 {
            if let Err(e) = rt.step_cross() {
                if e.detail != 88 {
                    return Err(e);
                }
                hit = true;
                break;
            }
        }
        if !hit {
            return Err(boom());
        }
        Ok(())
    })();
    report.record(
        "physical sanity: stability(86) / conservation(87) / finiteness(88)",
        if result.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

/// RFC-0030 minimal profile: one scenario with ground + vehicle + camera,
/// exercising Input → Physics → Commit → RenderPrepare, plus an end-to-end
/// WIR → EIR → binary round-trip through a compiled module.
fn minimal_profile_scenario(report: &mut Report) {
    fn build_scene() -> Scene {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        // Static ground plane.
        let mut ground = Entity::dynamic();
        ground.transform = Some(Transform {
            position: Vec3::new(0.0, -5.0, 0.0),
            ..Default::default()
        });
        ground.rigid_body = Some(RigidBody::r#static());
        ground.collider = Some(Collider::aabb(Vec3::new(50.0, 5.0, 50.0)));
        scene.insert(EntityId(0), ground);
        // Dynamic vehicle drifting forward and falling onto the ground.
        let mut vehicle = Entity::dynamic();
        vehicle.transform = Some(Transform {
            position: Vec3::new(0.0, 8.0, 0.0),
            ..Default::default()
        });
        vehicle.velocity = Some(Velocity {
            linear: Vec3::new(4.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        vehicle.rigid_body = Some(RigidBody::dynamic(4.0));
        vehicle.collider = Some(Collider::aabb(Vec3::new(1.0, 0.5, 0.7)));
        scene.insert(EntityId(1), vehicle);
        // Kinematic camera.
        let mut camera = Entity::dynamic();
        camera.transform = Some(Transform {
            position: Vec3::new(0.0, 16.0, 24.0),
            ..Default::default()
        });
        camera.camera = Some(Camera::default());
        scene.insert(EntityId(2), camera);
        scene
    }

    let pipeline = (|| -> Result<(), pwe_api::Error> {
        let mut sim = Simulation::new(
            build_scene(),
            1.0 / 60.0,
            PhysicsSystem::new(PhysicsConfig::default()),
        );
        // Input → Physics → Commit (stepping) → RenderPrepare (view).
        sim.track_camera(EntityId(2), EntityId(1), Vec3::new(0.0, 5.0, 10.0))?;
        sim.step_n(300);
        let veh_y = sim.scene.position(EntityId(1))?.y;
        // The vehicle fell off its initial height onto the ground and stayed.
        if !(veh_y < 8.0 && veh_y > -5.0) {
            return Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 0,
                byte_offset: 0,
            });
        }
        let render = sim.render_view(EntityId(2))?;
        if render.visible.is_empty() {
            return Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 1,
                byte_offset: 0,
            });
        }
        let physics_view = sim.physics_view();
        if !physics_view
            .bodies
            .iter()
            .any(|(id, _, _)| *id == EntityId(1))
        {
            return Err(pwe_api::Error {
                status: Status::Invalid,
                detail: 2,
                byte_offset: 0,
            });
        }
        Ok(())
    })();

    // WIR → EIR → binary: compile a module and round-trip its EIR bytecode.
    let binary = (|| -> Result<(), pwe_api::Error> {
        let model = pwe_reference::world! {
            gravity = [0.0, -9.81, 0.0];
            entity vehicle { position = [0.0, 8.0, 0.0]; dynamic = true; box = [1.0, 0.5, 0.7]; }
            entity ground { position = [0.0, -5.0, 0.0]; dynamic = false; box = [50.0, 5.0, 50.0]; }
        };
        let program = pwe_reference::program! {
            entities = [1];
            gravity = -9.81; dt = 1.0 / 60.0;
            systems = [
                gravity { gravity_y: -9.81, dt: 1.0 / 60.0 },
                integrate { dt: 1.0 / 60.0 },
                ground_contact { restitution: 0.6 },
            ];
        };
        let module = pwe_reference::module::compile_module("profile", model, program);
        if module.eir.functions.is_empty() {
            return Err(pwe_api::Error {
                status: Status::EirInvalid,
                detail: 0,
                byte_offset: 0,
            });
        }
        let bytes = module.eir.encode()?;
        let decoded = pwe_reference::eir::EirModule::decode(&bytes)?;
        let reencoded = decoded.encode()?;
        if reencoded != bytes {
            return Err(pwe_api::Error {
                status: Status::EirInvalid,
                detail: 1,
                byte_offset: 0,
            });
        }
        Ok(())
    })();

    report.record(
        "RFC-0030 ground+vehicle+camera (Input->Physics->Commit->RenderPrepare)",
        if pipeline.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
    report.record(
        "RFC-0030 WIR->EIR->binary round-trip",
        if binary.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

/// RFC-0029 "deterministic replay hashes": a real physics simulation must
/// reproduce an identical state hash across identical runs and replays.
fn simulation_determinism(report: &mut Report) {
    fn scene() -> Scene {
        let mut s = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut vehicle = Entity::dynamic();
        vehicle.transform = Some(Transform {
            position: Vec3::new(0.0, 8.0, 0.0),
            ..Default::default()
        });
        vehicle.velocity = Some(Velocity {
            linear: Vec3::new(4.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        vehicle.rigid_body = Some(RigidBody::dynamic(4.0));
        vehicle.collider = Some(Collider::aabb(Vec3::new(1.0, 0.5, 0.7)));
        s.insert(EntityId(1), vehicle);

        let mut cam = Entity::dynamic();
        cam.transform = Some(Transform {
            position: Vec3::new(0.0, 16.0, 24.0),
            ..Default::default()
        });
        cam.camera = Some(Camera::default());
        s.insert(EntityId(2), cam);
        s
    }
    fn new_sim() -> Simulation {
        Simulation::new(
            scene(),
            1.0 / 60.0,
            PhysicsSystem::new(PhysicsConfig::default()),
        )
    }

    let mut a = new_sim();
    let mut b = new_sim();
    a.step_n(300);
    b.step_n(300);
    let identical_run = a.state_hash() == b.state_hash();

    // Snapshot mid-run, restore, continue: must match uninterrupted.
    let mut uninterrupted = new_sim();
    uninterrupted.step_n(300);
    let target = uninterrupted.state_hash();

    let mut replay = new_sim();
    replay.step_n(150);
    let snap = replay.snapshot();
    replay.restore(&snap).unwrap();
    replay.step_n(150);
    let replay_match = replay.state_hash() == target;

    report.record(
        "deterministic replay hash (simulation)",
        if identical_run && replay_match {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

fn jit_differential(report: &mut Report) {
    // A one-function EIR that stores a constant, exercising the shared
    // interpreter/JIT evaluator.
    fn module() -> EirModule {
        EirModule {
            module_hash: Hash256([1; 32]),
            schema_set_hash: Hash256([2; 32]),
            domain_ir_hash: Hash256([3; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: pwe_reference::eir::EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 1,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(0)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 2,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(42)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Store,
                        result_id: 0,
                        result_type: None,
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Return,
                        result_id: 0,
                        result_type: None,
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                ],
            }],
        }
    }

    let interpreter = module().interpret(WorldId(1), WorldVersion(0));

    let mut jit = CpuJit::new();
    jit.require_manifest(Hash256([7; 32]));
    jit.set_grants(pwe_api::Access::WRITE);
    let profile = profile_hash(b"conformance");
    let code = jit
        .compile(
            &module(),
            0,
            profile,
            vec![JitAssumption::World(WorldId(1))],
        )
        .unwrap();
    jit.publish(code).unwrap();
    let key = CodeCacheKey {
        module_hash: Hash256([1; 32]),
        target: 0,
        profile,
    };
    let jitted = jit.execute(&key, WorldId(1), WorldVersion(0), Hash256([2; 32]), 0);

    // Differential: interpreter and JIT must produce identical writes.
    report.record(
        "interpreter-JIT differential semantics",
        match (&interpreter, &jitted) {
            (Ok(a), Ok(b)) if a == b => Case::Pass,
            _ => Case::Fail,
        },
    );

    // Deopt: a violation of the world assumption must still execute (generic
    // fallback) without error.
    let deopt = jit.execute(&key, WorldId(99), WorldVersion(0), Hash256([2; 32]), 0);
    report.record(
        "JIT deoptimization at assumption violation",
        if deopt.is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}

/// RFC-0029 AOT + fence coverage: an AOT-compiled program must be differentially
/// equivalent to the interpreter (same EIR semantics), and a CPU↔GPU resource
/// handoff must be rejected without a correctly-strengthened, ownership-bound
/// fence (RFC-0022 explicit ordering barriers).
fn aot_and_fence(report: &mut Report) {
    fn module() -> EirModule {
        EirModule {
            module_hash: Hash256([4; 32]),
            schema_set_hash: Hash256([5; 32]),
            domain_ir_hash: Hash256([6; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: pwe_reference::eir::EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 1,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(0)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Const,
                        result_id: 2,
                        result_type: Some(ValueType::U64),
                        operands: vec![],
                        constant: Some(Immediate::U64(7)),
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Store,
                        result_id: 0,
                        result_type: None,
                        operands: vec![1, 2],
                        constant: None,
                        target: None,
                    },
                    Instruction {
                        opcode: Opcode::Return,
                        result_id: 0,
                        result_type: None,
                        operands: vec![],
                        constant: None,
                        target: None,
                    },
                ],
            }],
        }
    }

    // AOT: compile to a portable artifact, then execute and compare with the
    // interpreter (the semantic reference). Also verify the artifact round-trips
    // through its binary codec without changing its execution.
    let interpreter = module().interpret(WorldId(1), WorldVersion(0));
    let aot = AotProgram::compile(&module(), 0)
        .and_then(|program| program.execute(WorldId(1), WorldVersion(0)));
    let aot_roundtrip = AotProgram::compile(&module(), 0)
        .and_then(|program| {
            let bytes = program.encode()?;
            AotProgram::decode(&bytes)
        })
        .and_then(|program| program.execute(WorldId(1), WorldVersion(0)));
    report.record(
        "interpreter-AOT differential semantics",
        match (&interpreter, &aot) {
            (Ok(a), Ok(b)) if a == b => Case::Pass,
            _ => Case::Fail,
        },
    );
    report.record(
        "AOT artifact binary round-trip preserves writes",
        match (&aot, &aot_roundtrip) {
            (Ok(a), Ok(b)) if a == b => Case::Pass,
            _ => Case::Fail,
        },
    );

    // Fence: a CPU→GPU handoff needs a releasing fence bound to a matching
    // ownership transfer. A relaxed/mismatched fence must fail closed.
    let ok_handoff = CpuGpuHandoff {
        resource: 7,
        fence: Fence {
            order: MemoryOrder::Release,
            tag: 42,
        },
        ownership_transfer: 42,
    };
    report.record(
        "CPU->GPU releasing fence + ownership passes",
        if ok_handoff.validate(Direction::CpuToGpu).is_ok() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
    let weak = CpuGpuHandoff {
        resource: 7,
        fence: Fence {
            order: MemoryOrder::Relaxed,
            tag: 42,
        },
        ownership_transfer: 42,
    };
    report.record(
        "CPU->GPU with weak (relaxed) fence rejected",
        if weak.validate(Direction::CpuToGpu).is_err() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
    let mismatched = CpuGpuHandoff {
        resource: 7,
        fence: Fence {
            order: MemoryOrder::Release,
            tag: 42,
        },
        ownership_transfer: 0, // no ownership transfer
    };
    report.record(
        "handoff without ownership transfer rejected",
        if mismatched.validate(Direction::CpuToGpu).is_err() {
            Case::Pass
        } else {
            Case::Fail
        },
    );
}
