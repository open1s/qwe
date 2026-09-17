//! RFC-0010 / RFC-0027 AOT compile path.
//!
//! "Interpreter, JIT and AOT MUST share EIR semantics" (AGENTS.md §4). This is
//! the ahead-of-time compile path: an [`EirModule`] is validated once at compile
//! time and folded into an immutable [`AotProgram`] whose execution produces
//! byte-identical [`WorldWrite`]s to the interpreter (the semantic oracle).
//!
//! Consistent with the reference JIT being a "compiled cache" that locks the
//! contract without emitting native code, the AOT path is a *validated, frozen
//! program object*: compile-time validation happens exactly once, and every
//! execution reuses the immutable compiled form. The artifact carries a content
//! identity (RFC-0035) so it can be fingerprinted and cached on disk.

use crate::eir::{EirModule, EirRuntime, WorldWrite};
use crate::sha256::digest;
use pwe_api::{Error, Hash256, Result, Status, WorldId, WorldVersion};

/// AOT artifact envelope.
pub const AOT_MAGIC: [u8; 8] = *b"PWEAOT2\0";
pub const AOT_MAJOR: u16 = 1;
pub const AOT_MINOR: u16 = 0;
/// Byte offset of the artifact-hash field inside the encoded artifact
/// (`magic(8) + major(2) + minor(2) + target(2)`).
pub const AOT_HASH_OFFSET: usize = 8 + 2 + 2 + 2;

/// An immutable, validated AOT-compiled program.
///
/// Compilation validates the EIR once; the resulting object is frozen. Every
/// `execute` reuses it and yields interpreter-identical writes.
#[derive(Clone, Debug)]
pub struct AotProgram {
    /// The compiled module, frozen after validation.
    eir: EirModule,
    /// The backend target this program was compiled for.
    ///
    /// TODO(backends): the GPU, NPU, and SIMD execution backends are not
    /// implemented — this reference ships the interpreter (semantic reference),
    /// the interpreter-backed JIT, and this AOT artifact path. A new backend
    /// plugs in at this `target` boundary and must be differentially verified
    /// against the interpreter (byte-identical writes and events) before it is
    /// trusted. See `tasks/todo.md`.
    pub target: u16,
    /// Content identity of the artifact (RFC-0035), derived from the module and
    /// target so identical inputs yield identical artifacts.
    pub artifact_hash: Hash256,
}

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

impl AotProgram {
    /// COMPILE: validate the module once (deterministic/pure) and freeze it.
    /// Compilation is a compile-time-only cost; execution never re-validates.
    pub fn compile(module: &EirModule, target: u16) -> Result<Self> {
        module.validate(true)?;
        module.verify_linear_dominance()?;
        // The artifact hash is SHA-256 over the canonical artifact bytes with
        // the hash field itself zeroed (mirrors RFC-0021's module envelope).
        let zeroed = serialize_artifact(module, target, Hash256([0; 32]))?;
        let artifact_hash = digest(&zeroed);
        Ok(Self {
            eir: module.clone(),
            target,
            artifact_hash,
        })
    }

    /// EXECUTE: run the frozen program with a no-op runtime, returning ordered
    /// world writes identical to the interpreter.
    pub fn execute(&self, world: WorldId, version: WorldVersion) -> Result<Vec<WorldWrite>> {
        self.eir.interpret(world, version)
    }

    /// EXECUTE against a concrete world runtime (reads component fields).
    pub fn execute_with(
        &self,
        rt: &mut dyn EirRuntime,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        self.eir.interpret_with(rt, world, version)
    }

    /// Encodes the AOT artifact (RFC-0021-style envelope + embedded EIR module)
    /// so it can be persisted and loaded later.
    pub fn encode(&self) -> Result<Vec<u8>> {
        serialize_artifact(&self.eir, self.target, self.artifact_hash)
    }

    /// Decodes and verifies an AOT artifact. Recomputes the content hash over
    /// the bytes (with the hash field zeroed) and rejects on mismatch, then
    /// re-validates the embedded EIR.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < AOT_HASH_OFFSET + 32 + 4 {
            return Err(error(Status::Invalid, 1));
        }
        if bytes[0..8] != AOT_MAGIC {
            return Err(error(Status::Invalid, 2));
        }
        let major = u16::from_le_bytes([bytes[8], bytes[9]]);
        let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
        if major != AOT_MAJOR || minor != AOT_MINOR {
            return Err(error(Status::SchemaUnsupported, 3));
        }
        let target = u16::from_le_bytes([bytes[12], bytes[13]]);
        let stored_hash = Hash256(
            bytes[AOT_HASH_OFFSET..AOT_HASH_OFFSET + 32]
                .try_into()
                .unwrap(),
        );
        // Recompute the hash over the bytes with the hash field zeroed.
        let mut hashed = bytes.to_vec();
        hashed[AOT_HASH_OFFSET..AOT_HASH_OFFSET + 32].copy_from_slice(&[0u8; 32]);
        let computed = digest(&hashed);
        if computed != stored_hash {
            return Err(error(Status::HashCollision, 4));
        }
        let len_bytes: [u8; 4] = bytes[AOT_HASH_OFFSET + 32..AOT_HASH_OFFSET + 36]
            .try_into()
            .unwrap();
        let module_len = u32::from_le_bytes(len_bytes) as usize;
        let module_start = AOT_HASH_OFFSET + 36;
        let module_bytes = bytes
            .get(module_start..module_start + module_len)
            .ok_or(error(Status::Invalid, 5))?;
        let module = EirModule::decode(module_bytes)?;
        module.validate(true)?;
        module.verify_linear_dominance()?;
        Ok(Self {
            eir: module,
            target,
            artifact_hash: stored_hash,
        })
    }
}

/// Builds the canonical artifact bytes with the given hash placed in the hash
/// field.
fn serialize_artifact(module: &EirModule, target: u16, artifact_hash: Hash256) -> Result<Vec<u8>> {
    let module_bytes = module.encode()?;
    let mut out = Vec::with_capacity(AOT_HASH_OFFSET + 32 + 4 + module_bytes.len());
    out.extend_from_slice(&AOT_MAGIC);
    out.extend_from_slice(&AOT_MAJOR.to_le_bytes());
    out.extend_from_slice(&AOT_MINOR.to_le_bytes());
    out.extend_from_slice(&target.to_le_bytes());
    out.extend_from_slice(&artifact_hash.0);
    out.extend_from_slice(&(module_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&module_bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::{Function, Instruction, Opcode, EIR_EFFECT_WRITE_WORLD};
    use pwe_api::Status;

    fn const_instr(result_id: u32, value: u64) -> Instruction {
        Instruction {
            opcode: Opcode::Const,
            result_id,
            result_type: Some(crate::eir::ValueType::U64),
            operands: vec![],
            constant: Some(crate::eir::Immediate::U64(value)),
            target: None,
        }
    }
    fn store(address_id: u32, value_id: u32) -> Instruction {
        Instruction {
            opcode: Opcode::Store,
            result_id: 0,
            result_type: None,
            operands: vec![address_id, value_id],
            constant: None,
            target: None,
        }
    }
    fn ret() -> Instruction {
        Instruction {
            opcode: Opcode::Return,
            result_id: 0,
            result_type: None,
            operands: vec![],
            constant: None,
            target: None,
        }
    }

    fn module() -> EirModule {
        EirModule {
            module_hash: Hash256([1; 32]),
            schema_set_hash: Hash256([2; 32]),
            domain_ir_hash: Hash256([3; 32]),
            target_kind: 0,
            functions: vec![Function {
                id: 1,
                effect_mask: EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![const_instr(1, 0), const_instr(2, 42), store(1, 2), ret()],
            }],
        }
    }

    #[test]
    fn aot_execution_is_byte_identical_to_interpreter() {
        let program = AotProgram::compile(&module(), 0).unwrap();
        let writes = program.execute(WorldId(1), WorldVersion(0)).unwrap();
        let interpreted = module().interpret(WorldId(1), WorldVersion(0)).unwrap();
        assert_eq!(writes, interpreted);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].value, 42);
    }

    #[test]
    fn aot_validates_once_and_rejects_invalid_modules() {
        let mut bad = module();
        bad.functions[0].instructions.push(const_instr(2, 9)); // duplicate def
        assert_eq!(
            AotProgram::compile(&bad, 0).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn aot_compile_enforces_dominance_gate() {
        // Use-before-def (%2 used before %1 is defined) fails the RFC-0021
        // dominance gate even if it passes basic validation.
        let mut bad = module();
        bad.functions[0].instructions = vec![
            Instruction {
                opcode: Opcode::Add,
                result_id: 2,
                result_type: Some(crate::eir::ValueType::U64),
                operands: vec![1, 1],
                constant: None,
                target: None,
            },
            const_instr(1, 5),
            ret(),
        ];
        assert_eq!(
            AotProgram::compile(&bad, 0).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn aot_artifact_is_deterministic_and_target_sensitive() {
        let a = AotProgram::compile(&module(), 0).unwrap();
        let b = AotProgram::compile(&module(), 0).unwrap();
        let gpu = AotProgram::compile(&module(), 2).unwrap();
        assert_eq!(a.artifact_hash, b.artifact_hash);
        assert_ne!(a.artifact_hash, gpu.artifact_hash);
    }

    #[test]
    fn aot_artifact_round_trips_and_stays_interpreter_identical() {
        let program = AotProgram::compile(&module(), 0).unwrap();
        let bytes = program.encode().unwrap();
        let decoded = AotProgram::decode(&bytes).unwrap();
        assert_eq!(decoded.artifact_hash, program.artifact_hash);
        assert_eq!(decoded.target, program.target);
        // The loaded artifact executes identically to the interpreter.
        assert_eq!(
            decoded.execute(WorldId(1), WorldVersion(0)).unwrap(),
            module().interpret(WorldId(1), WorldVersion(0)).unwrap()
        );
    }

    #[test]
    fn aot_decode_rejects_corrupted_artifact() {
        let program = AotProgram::compile(&module(), 0).unwrap();
        let mut bytes = program.encode().unwrap();
        // Flip a byte in the embedded module -> hash no longer matches.
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(
            AotProgram::decode(&bytes).unwrap_err().status,
            Status::HashCollision
        );
    }

    #[test]
    fn aot_decode_rejects_tampered_target() {
        let program = AotProgram::compile(&module(), 0).unwrap();
        let mut bytes = program.encode().unwrap();
        bytes[12] ^= 1; // change target; hash (over the file) must then mismatch
        assert_eq!(
            AotProgram::decode(&bytes).unwrap_err().status,
            Status::HashCollision
        );
    }
}
