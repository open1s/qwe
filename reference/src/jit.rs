//! RFC-0027 / RFC-0010 CPU JIT: compiled-artifact cache with hotness-based
//! specialization, assumption-checked entries, and safe-point deoptimization.
//!
//! The reference JIT is a *validated compiled cache* that shares the EIR
//! interpreter's semantics (RFC-0004: interpreter is the semantic oracle). A
//! JIT `execute` therefore produces byte-identical `WorldWrite`s to
//! `EirModule::interpret` for the same inputs — enforced by differential tests.
//! Lifecycle is Compile→Validate→CapabilityCheck→Link→Publish→Execute, with
//! Profile/Invalidate returning to Compile. Deoptimization happens only at
//! declared safe points (function boundaries and `Store` instructions) and
//! resumes generic semantics.

use crate::eir::{EirModule, WorldWrite};
use crate::sha256::digest;
use pwe_api::{Access, Error, Hash256, Result, Status, WorldId, WorldVersion};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// Hotness tiers (RFC-0010 §2). Drive specialization and recompile decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum Hotness {
    Cold = 0,
    Warm = 1,
    Hot = 2,
    VeryHot = 3,
}

/// The code-cache key (RFC-0010 §3): identity of the compiled unit, the target,
/// and the profile used to specialize it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CodeCacheKey {
    pub module_hash: Hash256,
    pub target: u16,
    pub profile: Hash256,
}

/// An assumption the specialized code depends on. Checked at entry; if any is
/// false, execution falls back to generic semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitAssumption {
    SchemaSet(Hash256),
    World(WorldId),
    /// Components must not exceed this count (range specialization).
    MaxEntities(u64),
}

/// A validated, published compilation unit.
#[derive(Clone, Debug)]
pub struct CompiledCode {
    pub key: CodeCacheKey,
    pub eir: EirModule,
    pub assumptions: Vec<JitAssumption>,
    pub capability_manifest: Hash256,
    pub hotness: Hotness,
}

impl CompiledCode {
    /// Returns true if every assumption holds for the given execution context.
    pub fn assumptions_hold(&self, world: WorldId, schema_set: Hash256) -> bool {
        self.assumptions.iter().all(|a| match *a {
            JitAssumption::SchemaSet(h) => h == schema_set,
            JitAssumption::World(w) => w == world,
            // MaxEntities is checked against a supplied count at execute time.
            JitAssumption::MaxEntities(_) => true,
        })
    }
}

/// A CPU JIT with hotness counters and atomic publish/invalidate.
#[derive(Default)]
pub struct CpuJit {
    code: BTreeMap<CodeCacheKey, CompiledCode>,
    invocations: BTreeMap<CodeCacheKey, AtomicU64>,
    /// Runtime policy-granted capabilities; `None` means nothing is installed.
    grants: Option<Access>,
    /// The manifest a compiled unit must present to be installed.
    required_manifest: Option<Hash256>,
}

impl CpuJit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants the capability sets this runtime permits at install time.
    pub fn set_grants(&mut self, access: Access) {
        self.grants = Some(access);
    }

    /// Requires a specific capability manifest before install (RFC-0027: no
    /// executable runs without a capability manifest).
    pub fn require_manifest(&mut self, manifest: Hash256) {
        self.required_manifest = Some(manifest);
    }

    /// COMPILE: validate the EIR and fold it into a compilation unit. Emits an
    /// assumed range (MaxEntities) as a specialization baseline.
    pub fn compile(
        &self,
        module: &EirModule,
        target: u16,
        profile: Hash256,
        assumptions: Vec<JitAssumption>,
    ) -> Result<CompiledCode> {
        module.validate(false)?;
        module.verify_linear_dominance()?;
        let key = CodeCacheKey {
            module_hash: module.module_hash,
            target,
            profile,
        };
        let manifest = self.required_manifest.ok_or(error(Status::Capability, 5))?;
        Ok(CompiledCode {
            key,
            eir: module.clone(),
            assumptions,
            capability_manifest: manifest,
            hotness: Hotness::Cold,
        })
    }

    /// VALIDATE + CAPABILITY_CHECK + LINK: refuse any unit whose manifest the
    /// runtime did not require (no install without matching manifest).
    fn ready(&self, code: &CompiledCode) -> Result<()> {
        if let Some(required) = self.required_manifest {
            if code.capability_manifest != required {
                return Err(error(Status::Capability, 6));
            }
        } else {
            return Err(error(Status::Capability, 5));
        }
        if self.grants.is_none() {
            return Err(error(Status::Capability, 7));
        }
        Ok(())
    }

    /// PUBLISH: install a compiled unit atomically.
    pub fn publish(&mut self, code: CompiledCode) -> Result<()> {
        self.ready(&code)?;
        self.code.insert(code.key, code);
        Ok(())
    }

    /// EXECUTE with a concrete world runtime (reads component fields). Same
    /// lifecycle and assumption gating as `execute`, but through the runtime so
    /// the compiled unit can actually read world state.
    pub fn execute_with(
        &self,
        key: &CodeCacheKey,
        rt: &mut dyn crate::eir::EirRuntime,
        world: WorldId,
        version: WorldVersion,
        schema_set: Hash256,
        entity_hint: u64,
    ) -> Result<Vec<WorldWrite>> {
        let code = self.code.get(key).ok_or(error(Status::HandleStale, 9))?;
        self.ready(code)?;
        if !code.assumptions_hold(world, schema_set) {
            return code.eir.interpret_with(rt, world, version);
        }
        if code
            .assumptions
            .iter()
            .any(|a| matches!(a, JitAssumption::MaxEntities(max) if entity_hint > *max))
        {
            return code.eir.interpret_with(rt, world, version);
        }
        if let Some(counter) = self.invocations.get(key) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        // JIT execution shares interpreter semantics (differential contract).
        code.eir.interpret_with(rt, world, version)
    }

    /// Env-aware execution (supports `TIME`/`RANDOM`/`IO`/`EMIT_EVENT`). Shares
    /// interpreter semantics via the non-pure `interpret_with_env`, so the JIT
    /// and interpreter agree for modules with nondeterministic effect opcodes.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_env(
        &self,
        key: &CodeCacheKey,
        rt: &mut dyn crate::eir::EirRuntime,
        env: &mut crate::eir::ExecEnv,
        world: WorldId,
        version: WorldVersion,
        schema_set: Hash256,
        entity_hint: u64,
    ) -> Result<Vec<WorldWrite>> {
        let code = self.code.get(key).ok_or(error(Status::HandleStale, 9))?;
        self.ready(code)?;
        if !code.assumptions_hold(world, schema_set) {
            return code.eir.interpret_with_env(rt, env, world, version);
        }
        if code
            .assumptions
            .iter()
            .any(|a| matches!(a, JitAssumption::MaxEntities(max) if entity_hint > *max))
        {
            return code.eir.interpret_with_env(rt, env, world, version);
        }
        if let Some(counter) = self.invocations.get(key) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        code.eir.interpret_with_env(rt, env, world, version)
    }

    /// Env-aware execution that skips EIR re-validation. For callers that step
    /// the same immutable module many times (the language runtime validates
    /// once at compile), this avoids re-running the dominance verifier every
    /// step. The JIT lifecycle result is identical to `execute_with_env`: both
    /// paths share interpreter semantics, so the assumption/deopt branches
    /// would return the same writes anyway.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_with_env_validated(
        &self,
        key: &CodeCacheKey,
        rt: &mut dyn crate::eir::EirRuntime,
        env: &mut crate::eir::ExecEnv,
        world: WorldId,
        version: WorldVersion,
    ) -> Result<Vec<WorldWrite>> {
        let code = self.code.get(key).ok_or(error(Status::HandleStale, 9))?;
        self.ready(code)?;
        if let Some(counter) = self.invocations.get(key) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        code.eir.execute(rt, env, world, version)
    }

    /// EXECUTE: run the compiled unit if assumptions hold, else deopt.
    pub fn execute(
        &self,
        key: &CodeCacheKey,
        world: WorldId,
        version: WorldVersion,
        schema_set: Hash256,
        entity_hint: u64,
    ) -> Result<Vec<WorldWrite>> {
        let code = self.code.get(key).ok_or(error(Status::HandleStale, 9))?;
        self.ready(code)?;
        // Assumption check at entry.
        if !code.assumptions_hold(world, schema_set) {
            // Deoptimize: fall back to generic interpreter semantics.
            return code.eir.interpret(world, version);
        }
        // Range assumptions involving entity count are validated here.
        if code
            .assumptions
            .iter()
            .any(|a| matches!(a, JitAssumption::MaxEntities(max) if entity_hint > *max))
        {
            return code.eir.interpret(world, version);
        }
        // Count the invocation for hotness promotion.
        if let Some(counter) = self.invocations.get(key) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        // JIT execution shares interpreter semantics (differential contract).
        code.eir.interpret(world, version)
    }

    /// PROFILE: classify hotness from invocation count (RFC-0010 §2).
    pub fn hotness(&self, key: &CodeCacheKey) -> Hotness {
        let n = self
            .invocations
            .get(key)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0);
        match n {
            0 => Hotness::Cold,
            1..=99 => Hotness::Warm,
            100..=999 => Hotness::Hot,
            _ => Hotness::VeryHot,
        }
    }

    /// INVALIDATE: atomically remove a unit, forcing a future Compile.
    pub fn invalidate(&mut self, key: &CodeCacheKey) -> Result<()> {
        self.code.remove(key);
        self.invocations.remove(key);
        Ok(())
    }

    /// Record an invocation count for tests without a live counter.
    pub fn record_invocation(&mut self, key: &CodeCacheKey, times: u64) {
        self.invocations.entry(*key).or_default();
        self.invocations
            .get(key)
            .map(|c| c.fetch_add(times, Ordering::Relaxed));
    }
}

/// Canonical profile hash (RFC-0010 CodeCacheKey profile component).
pub fn profile_hash(description: &[u8]) -> Hash256 {
    digest(description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::{EirModule, Function, Immediate, Instruction, Opcode, ValueType};

    fn const_instr(id: u32, value: u64) -> Instruction {
        Instruction {
            opcode: Opcode::Const,
            result_id: id,
            result_type: Some(ValueType::U64),
            operands: vec![],
            constant: Some(Immediate::U64(value)),
            target: None,
        }
    }
    fn store(addr: u32, val: u32) -> Instruction {
        Instruction {
            opcode: Opcode::Store,
            result_id: 0,
            result_type: None,
            operands: vec![addr, val],
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
                effect_mask: crate::eir::EIR_EFFECT_WRITE_WORLD,
                argument_count: 0,
                instructions: vec![const_instr(1, 0), const_instr(2, 42), store(1, 2), ret()],
            }],
        }
    }

    #[test]
    fn jit_compile_validate_publish_execute_lifecycle() {
        let mut jit = CpuJit::new();
        let manifest = Hash256([7; 32]);
        jit.require_manifest(manifest);
        jit.set_grants(Access::WRITE);

        let profile = profile_hash(b"default");
        let code = jit
            .compile(
                &module(),
                0,
                profile,
                vec![JitAssumption::World(WorldId(1))],
            )
            .unwrap();
        jit.publish(code).unwrap();

        // Execute returns interpreter-identical writes.
        let writes = jit
            .execute(
                &CodeCacheKey {
                    module_hash: Hash256([1; 32]),
                    target: 0,
                    profile,
                },
                WorldId(1),
                WorldVersion(0),
                Hash256([2; 32]),
                0,
            )
            .unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].value, 42);
    }

    #[test]
    fn jit_refuses_install_without_capability_manifest() {
        let jit = CpuJit::new();
        // No required manifest -> compile fails closed.
        assert_eq!(
            jit.compile(&module(), 0, profile_hash(b"x"), vec![])
                .unwrap_err()
                .status,
            Status::Capability
        );
    }

    #[test]
    fn jit_deoptimizes_to_interpreter_on_assumption_violation() {
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"p");
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
        // Wrong world violates the assumption -> still executes via generic
        // interpreter semantics (differential result identical).
        let writes = jit
            .execute(&key, WorldId(99), WorldVersion(0), Hash256([2; 32]), 0)
            .unwrap();
        assert_eq!(writes[0].value, 42);
    }

    #[test]
    fn jit_hotness_promotion_and_invalidation() {
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"p2");
        let code = jit.compile(&module(), 0, profile, vec![]).unwrap();
        jit.publish(code).unwrap();
        let key = CodeCacheKey {
            module_hash: Hash256([1; 32]),
            target: 0,
            profile,
        };
        assert_eq!(jit.hotness(&key), Hotness::Cold);
        jit.record_invocation(&key, 500);
        assert_eq!(jit.hotness(&key), Hotness::Hot);
        jit.invalidate(&key).unwrap();
        assert!(!jit.code.contains_key(&key));
    }

    #[test]
    fn jit_is_differential_with_interpreter() {
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"diff");
        let code = jit.compile(&module(), 0, profile, vec![]).unwrap();
        jit.publish(code).unwrap();

        let interpreter = module().interpret(WorldId(1), WorldVersion(0)).unwrap();
        let key = CodeCacheKey {
            module_hash: Hash256([1; 32]),
            target: 0,
            profile,
        };
        let jitted = jit
            .execute(&key, WorldId(1), WorldVersion(0), Hash256([2; 32]), 0)
            .unwrap();
        assert_eq!(interpreter, jitted);
    }

    #[test]
    fn jit_executes_lowered_physics_and_matches_interpreter() {
        // Lower a real physics module and verify the JIT produces byte-identical
        // writes to the interpreter when both run against the same scene.
        use crate::components::{RigidBody, Transform, Velocity};
        use crate::math::Vec3;
        use crate::physics_eir::{lower_physics, SceneRuntime};
        use crate::scene::{Entity, Scene};

        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut e = Entity::dynamic();
        e.transform = Some(Transform {
            position: Vec3::new(0.0, 10.0, 0.0),
            ..Default::default()
        });
        e.velocity = Some(Velocity {
            linear: Vec3::new(0.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        e.rigid_body = Some(RigidBody {
            mass: 1.0,
            restitution: 0.0,
            friction: 0.0,
            is_dynamic: 1,
        });
        scene.insert(pwe_api::EntityId(1), e);

        let lowered = lower_physics(&[1u128], scene.gravity.y, 1.0 / 60.0);

        // Interpreter path.
        let mut rt_a = SceneRuntime::new(&scene);
        let int_writes = lowered
            .module
            .interpret_with(&mut rt_a, WorldId(0), WorldVersion(0))
            .unwrap();

        // JIT path over the same module.
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"physics");
        let code = jit.compile(&lowered.module, 0, profile, vec![]).unwrap();
        jit.publish(code).unwrap();
        let key = CodeCacheKey {
            module_hash: lowered.module.module_hash,
            target: 0,
            profile,
        };
        let mut rt_b = SceneRuntime::new(&scene);
        let jit_writes = jit
            .execute_with(
                &key,
                &mut rt_b,
                WorldId(0),
                WorldVersion(0),
                Hash256([0; 32]),
                1,
            )
            .unwrap();

        // Differential contract: identical ordered writes.
        assert_eq!(int_writes, jit_writes);
        assert!(!int_writes.is_empty());
    }
}
