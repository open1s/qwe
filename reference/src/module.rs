//! The compiled PWE module and its runtime loader — the Erlang↔BEAM analog.
//!
//! Erlang:   `erlc` compiles `.erl` to a `.beam` module; the BEAM VM loads it
//!           (`code:load_binary/3`) and calls its functions.
//! PWE:      `compile_module` fuses a `WorldModel` + `PhysicsProgram` into a
//!           `PweModule` (WIR data + EIR bytecode); `PweRuntime::load` then
//!           executes the module's EIR functions against a `Scene` each step.
//!
//! A module is the unit of compilation and caching: identical `WorldModel` +
//! `PhysicsProgram` hash to an identical module identity, so a runtime can
//! cache and reload modules deterministically.

use crate::dsl::WorldModel;
use crate::eir::{EirModule, WorldWrite};
use crate::physics_eir::{apply_writes, PhysicsProgram, SceneRuntime};
use crate::scene::Scene;
use pwe_api::{Hash256, Result, WorldId, WorldVersion};

/// The unit of compilation: world data (WIR) + compiled systems (EIR).
pub struct PweModule {
    pub name: String,
    pub model: WorldModel,
    pub program: PhysicsProgram,
    pub eir: EirModule,
    pub identity: Hash256,
}

/// Compiles a `WorldModel` + `PhysicsProgram` into a `PweModule`.
pub fn compile_module(name: &str, model: WorldModel, program: PhysicsProgram) -> PweModule {
    let identity = module_identity(&model, &program.module);
    let eir = program.module.clone();
    PweModule {
        name: name.to_string(),
        model,
        program,
        eir,
        identity,
    }
}

/// Deterministic module identity from world model + compiled EIR.
pub fn module_identity(model: &WorldModel, eir: &EirModule) -> Hash256 {
    let mut bytes = model.model_hash().0.to_vec();
    bytes.extend_from_slice(&eir.domain_ir_hash.0);
    bytes.extend_from_slice(&eir.module_hash.0);
    crate::sha256::digest(&bytes)
}

/// The runtime: loads compiled modules and executes them against world state
/// (BEAM `code:load_binary` + calling functions).
pub struct PweRuntime {
    modules: std::collections::BTreeMap<Hash256, PweModule>,
    /// The world state the loaded modules execute against.
    pub scene: Scene,
}

impl PweRuntime {
    /// Boot a runtime with a starting scene.
    pub fn boot(scene: Scene) -> Self {
        Self {
            modules: std::collections::BTreeMap::new(),
            scene,
        }
    }

    /// LOAD a compiled module (BEAM `code:load_binary`). Returns its identity.
    pub fn load(&mut self, module: PweModule) -> Hash256 {
        let id = module.identity;
        self.modules.insert(id, module);
        id
    }

    /// Build a scene from the module's world model and load it, returning the
    /// loaded identity. Equivalent to loading a module whose data defines the
    /// initial world.
    pub fn load_with_world(&mut self, module: PweModule) -> Hash256 {
        let id = module.identity;
        self.scene = module.model.build_scene();
        self.modules.insert(id, module);
        id
    }

    /// CALL the module's systems for one step: execute the EIR against the
    /// current scene and commit the writes. Returns the writes produced.
    pub fn run_step(&mut self, id: &Hash256) -> Result<Vec<WorldWrite>> {
        let module = self.modules.get(id).ok_or(pwe_api::Error {
            status: pwe_api::Status::HandleStale,
            detail: 1,
            byte_offset: 0,
        })?;
        let mut rt = SceneRuntime::new(&self.scene);
        let writes = module
            .eir
            .interpret_with(&mut rt, WorldId(0), WorldVersion(0))?;
        let overlays = rt.take_overlays();
        apply_writes(&mut self.scene, &writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        Ok(writes)
    }

    pub fn run_steps(&mut self, id: &Hash256, n: u64) -> Result<()> {
        for _ in 0..n {
            self.run_step(id)?;
        }
        Ok(())
    }

    /// UNLOAD (BEAM `code:purge`): removes a module.
    pub fn unload(&mut self, id: &Hash256) {
        self.modules.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;
    use crate::physics_eir::PhysicsProgram;
    use pwe_api::EntityId;

    #[test]
    fn compile_load_run_mirrors_erlang_pipeline() {
        // world! = module data; program! = module functions (compiled to EIR).
        let model = crate::world! {
            gravity = [0.0, -9.81, 0.0];
            entity vehicle { position = [0.0, 8.0, 0.0]; dynamic = true; box = [1.0, 0.5, 0.7]; }
            entity ground { position = [0.0, -5.0, 0.0]; dynamic = false; box = [50.0, 5.0, 50.0]; }
        };
        let program = crate::program! {
            entities = [1];
            gravity = -9.81; dt = 1.0 / 60.0;
            systems = [
                gravity { gravity_y: -9.81, dt: 1.0 / 60.0 },
                integrate { dt: 1.0 / 60.0 },
                ground_contact { restitution: 0.6 },
            ];
        };

        // "erlc": compile to a module.
        let module = compile_module("physics", model.clone(), program);
        assert!(!module.eir.functions.is_empty());

        // "code:load_binary": boot a VM and load the module with its world.
        let mut vm = PweRuntime::boot(Scene::new(Vec3::ZERO));
        let id = vm.load_with_world(module);

        // "call": step the module's systems and observe the world advance.
        vm.run_steps(&id, 120).unwrap();
        let y = vm.scene.position(EntityId(1)).unwrap().y;
        // The vehicle falls under gravity (and is kept on the ground).
        assert!(y < 8.0, "vehicle should fall, got y={y}");
        assert!(y >= 0.0, "vehicle should not tunnel, got y={y}");

        // Loading the identical module twice yields the same identity (dedup).
        let m2 = compile_module("physics", model, rebuild_program());
        assert_eq!(m2.identity, id);
    }

    fn rebuild_program() -> PhysicsProgram {
        crate::program! {
            entities = [1];
            gravity = -9.81; dt = 1.0 / 60.0;
            systems = [
                gravity { gravity_y: -9.81, dt: 1.0 / 60.0 },
                integrate { dt: 1.0 / 60.0 },
                ground_contact { restitution: 0.6 },
            ];
        }
    }
}
