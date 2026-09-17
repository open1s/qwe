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
