//! Deterministic property-style tests (RFC-0029) with no external dependency.
//!
//! A seeded xorshift PRNG drives many bounded inputs through the canonical
//! codecs and runtime, checking that round-trips are byte-identical and that
//! transactional invariants hold. These are not exhaustive proofs but sweep a
//! large input space deterministically (same seed => same sequence).

use pwe_api::{
    Access, CapabilityClaims, ComponentTypeId, EntityId, EntityRef, Hash256, WorldId, WorldVersion,
};
use pwe_reference::eir::{
    ComponentRef, EirModule, Function, Immediate, Instruction, Opcode, ValueType,
};
use pwe_reference::schema::{ComponentIdentity, Field, FieldType, Schema};
use pwe_reference::wir::{ComponentRecord, EntityRecord, WirDocument, FLAG_INITIAL_STATE};
use pwe_reference::ReferenceWorld;

// ---- deterministic PRNG (xorshift64*) ----

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn range(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
    fn field_type(&mut self) -> FieldType {
        const TYPES: [FieldType; 8] = [
            FieldType::Bool,
            FieldType::I32,
            FieldType::U32,
            FieldType::U64,
            FieldType::F32,
            FieldType::F64,
            FieldType::EntityRef,
            FieldType::String,
        ];
        TYPES[self.range(TYPES.len() as u64) as usize]
    }
    fn name(&mut self) -> String {
        let len = 1 + self.range(6) as usize;
        let mut s = String::with_capacity(len);
        s.push('a');
        for _ in 1..len {
            s.push((b'a' + self.range(26) as u8) as char);
        }
        s
    }
}

fn rng(seed: u64) -> Rng {
    Rng::new(seed)
}

// ---- WIR round-trip ----

fn random_wir(rng: &mut Rng) -> WirDocument {
    let mut schemas = Vec::new();
    let mut registry_ids = Vec::new();
    let schema_count = 1 + rng.range(3) as usize;
    for _ in 0..schema_count {
        let field_count = 1 + rng.range(4) as usize;
        let mut fields = Vec::with_capacity(field_count);
        for i in 0..field_count {
            fields.push(Field {
                id: (i as u32) + 1,
                name: rng.name(),
                ty: rng.field_type(),
                flags: 0,
            });
        }
        let schema = Schema {
            identity: ComponentIdentity {
                namespace: format!("pwe.p{}", rng.range(100)),
                stable_name: rng.name(),
                major_version: 1,
            },
            fields,
        };
        registry_ids.push(schema.identity.type_id().unwrap());
        schemas.push(schema);
    }

    let mut entities = Vec::new();
    let entity_count = 1 + rng.range(4) as usize;
    for _ in 0..entity_count {
        entities.push(EntityRecord {
            id: EntityId((rng.range(1000) + 1) as u128),
            generation: 1 + rng.range(5) as u32,
            flags: 0,
        });
    }
    entities.sort_by_key(|e| (e.id, e.generation));

    let mut components = Vec::new();
    let component_count = rng.range(5) as usize;
    for _ in 0..component_count {
        if entities.is_empty() || registry_ids.is_empty() {
            break;
        }
        let e = &entities[rng.range(entities.len() as u64) as usize];
        components.push(ComponentRecord {
            type_id: registry_ids[rng.range(registry_ids.len() as u64) as usize],
            entity: e.id,
            generation: e.generation,
            value: vec![0u8; rng.range(24) as usize],
        });
    }

    WirDocument {
        flags: FLAG_INITIAL_STATE,
        world_id: WorldId((rng.range(1_000_000) + 1) as u128),
        world_version: WorldVersion(rng.range(1000)),
        sim_time_ns: rng.range(1_000_000) as i64,
        schemas,
        entities,
        components,
        resources: random_records(rng, 30),
        spatial: random_records(rng, 30),
        systems: random_records(rng, 30),
        events: random_records(rng, 30),
        timelines: random_records(rng, 30),
    }
}

/// Generates an optional set of RFC-0020 section-4–8 opaque records, sometimes
/// empty (a section is only emitted when non-empty).
fn random_records(rng: &mut Rng, p: u64) -> Vec<Vec<u8>> {
    if rng.range(100) >= p {
        return Vec::new();
    }
    let n = rng.range(3) as usize;
    (0..n)
        .map(|_| {
            (0..rng.range(8) as usize)
                .map(|_| rng.next() as u8)
                .collect()
        })
        .collect()
}

#[test]
fn wir_round_trip_is_byte_identical_across_many_inputs() {
    let mut rng = rng(0x9E3779B97F4A7C15);
    for _ in 0..300 {
        let doc = random_wir(&mut rng);
        let bytes = match doc.encode() {
            Ok(b) => b,
            // Some generated inputs are intentionally rejected (e.g. duplicate
            // entities after sort); that is the codec's job, not a round-trip.
            Err(_) => continue,
        };
        let decoded = WirDocument::decode(&bytes).expect("valid WIR must decode");
        let reencoded = decoded.encode().expect("re-encode");
        assert_eq!(reencoded, bytes, "WIR round-trip must be byte-identical");
    }
}

// ---- EIR round-trip ----

fn random_instruction(rng: &mut Rng, next: &mut u32) -> Instruction {
    let opcode = match rng.range(6) {
        0 => Opcode::Const,
        1 => Opcode::Add,
        2 => Opcode::ReadView,
        3 => Opcode::WriteView,
        4 => Opcode::Store,
        _ => Opcode::Return,
    };
    match opcode {
        Opcode::Const => {
            let id = *next;
            *next += 1;
            Instruction {
                opcode,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: Some(Immediate::F64(rng.next() as f64)),
                target: None,
            }
        }
        Opcode::Add => {
            let id = *next;
            *next += 1;
            let a = rng.range(id as u64).max(1);
            let b = rng.range(id as u64).max(1);
            Instruction {
                opcode,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![a as u32, b as u32],
                constant: None,
                target: None,
            }
        }
        Opcode::ReadView => {
            let id = *next;
            *next += 1;
            Instruction {
                opcode,
                result_id: id,
                result_type: Some(ValueType::F64),
                operands: vec![],
                constant: None,
                target: Some(ComponentRef {
                    entity: rng.range(1000) as u128,
                    component: ComponentTypeId([0; 16]),
                    offset: rng.range(64) as u32,
                }),
            }
        }
        Opcode::WriteView => {
            let v = rng.range((*next).max(1) as u64).max(1) as u32;
            Instruction {
                opcode,
                result_id: 0,
                result_type: None,
                operands: vec![v],
                constant: None,
                target: Some(ComponentRef {
                    entity: rng.range(1000) as u128,
                    component: ComponentTypeId([0; 16]),
                    offset: rng.range(64) as u32,
                }),
            }
        }
        Opcode::Store => Instruction {
            opcode,
            result_id: 0,
            result_type: None,
            operands: vec![
                rng.range((*next).max(2) as u64).max(1) as u32,
                rng.range((*next).max(2) as u64).max(1) as u32,
            ],
            constant: None,
            target: None,
        },
        Opcode::Return
        | Opcode::Nop
        | Opcode::Sub
        | Opcode::Mul
        | Opcode::Div
        | Opcode::Rem
        | Opcode::Eq
        | Opcode::Ne
        | Opcode::Lt
        | Opcode::Le
        | Opcode::Gt
        | Opcode::Ge
        | Opcode::Select
        | Opcode::Load
        | Opcode::Trap
        | Opcode::Br
        | Opcode::CondBr
        | Opcode::Unreachable
        | Opcode::Call
        | Opcode::Atomic
        | Opcode::EmitEvent
        | Opcode::Time
        | Opcode::Random
        | Opcode::Io
        | Opcode::Abs
        | Opcode::Floor
        | Opcode::Ceil
        | Opcode::Round
        | Opcode::Sign
        | Opcode::Log10
        | Opcode::Log2
        | Opcode::Sinh
        | Opcode::Cosh
        | Opcode::Tanh
        | Opcode::Asin
        | Opcode::Acos
        | Opcode::Atan
        | Opcode::Atan2
        | Opcode::Hypot
        | Opcode::Print
        | Opcode::NeighborCount
        | Opcode::NearestDist
        | Opcode::Step
        | Opcode::ReadSlotDyn
        | Opcode::WriteSlotDyn
        | Opcode::ReadFieldCell
        | Opcode::WriteFieldCell
        | Opcode::FieldLaplacian
        | Opcode::ReadEvent
        | Opcode::Sin
        | Opcode::Cos
        | Opcode::Exp
        | Opcode::Ln
        | Opcode::Sqrt
        | Opcode::Pow => Instruction {
            opcode,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        },
    }
}

#[test]
fn eir_round_trip_is_byte_identical_across_many_inputs() {
    let mut rng = rng(0xC2B2AE3D27D4EB4F);
    for _ in 0..300 {
        let mut next = 1u32;
        let instr_count = 1 + rng.range(12) as usize;
        let mut instructions = Vec::with_capacity(instr_count);
        for _ in 0..instr_count {
            instructions.push(random_instruction(&mut rng, &mut next));
        }
        // Every function ends with a terminator.
        if !matches!(instructions.last().map(|i| i.opcode), Some(Opcode::Return)) {
            instructions.push(Instruction {
                opcode: Opcode::Return,
                result_id: 0,
                result_type: None,
                operands: vec![],
                constant: None,
                target: None,
            });
        }
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([rng.next() as u8; 32]),
            domain_ir_hash: Hash256([rng.next() as u8; 32]),
            target_kind: rng.range(4) as u16,
            functions: vec![Function {
                id: 1,
                effect_mask: rng.range(1024) as u32,
                argument_count: 0,
                instructions,
            }],
        };
        let bytes = module.encode().expect("EIR encode");
        let decoded = EirModule::decode(&bytes).expect("EIR decode");
        assert_eq!(decoded.encode().unwrap(), bytes, "EIR round-trip");
    }
}

// ---- transaction atomicity ----

fn capability(access: Access) -> CapabilityClaims {
    CapabilityClaims {
        issuer: Hash256([1; 32]),
        subject: Hash256([2; 32]),
        world: WorldId(1),
        region: pwe_api::RegionId(7),
        access,
        expires_at_version: WorldVersion(u64::MAX),
        nonce: [0; 16],
    }
}

#[test]
fn random_transactions_commit_or_abort_without_partial_state() {
    let mut rng = rng(0xD1B54A32D192ED03);
    for _ in 0..200 {
        let mut world = ReferenceWorld::new(WorldId(1));
        let mut version = 0u64;
        let steps = 1 + rng.range(20) as usize;
        for _ in 0..steps {
            let action = rng.range(4);
            let tx = world.begin(
                pwe_api::TransactionOptions {
                    base_version: WorldVersion(version),
                    ownership: pwe_api::Ownership {
                        region: pwe_api::RegionId(7),
                        epoch: pwe_api::OwnershipEpoch(1),
                    },
                    policy: pwe_api::ConflictPolicy::Reject,
                },
                capability(Access::CREATE.union(Access::WRITE)),
            );
            let mut tx = match tx {
                Ok(tx) => tx,
                Err(_) => continue,
            };
            let entity = EntityRef {
                id: EntityId((rng.range(50) + 1) as u128),
                generation: 1,
            };
            let mut committed = false;
            match action {
                0 => {
                    if tx.create(entity).is_ok() {
                        committed = tx.commit().is_ok();
                    }
                }
                1 => {
                    let _ = tx.create(entity);
                    tx.abort();
                }
                2 => {
                    let _ = tx.destroy(entity);
                    committed = tx.commit().is_ok();
                }
                _ => {
                    committed = tx.commit().is_ok();
                }
            }
            if committed {
                // A successful commit must advance the world exactly once.
                assert!(world.version().0 > version);
                version = world.version().0;
            } else {
                // A failed/aborted transaction must not advance the world.
                assert_eq!(world.version().0, version);
            }
        }
    }
}
