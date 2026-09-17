//! RFC-0008 Physics IR (folded into Domain IR per the frozen v0.2 contract).
//!
//! Physics IR describes computation over physical state; it does not define
//! storage ownership and does not require a particular solver. A physics domain
//! node declares its input/output component schemas and effects and lowers to
//! EIR exactly like any other `DomainNode`.

use crate::domain_ir::DomainNode;
use crate::eir::{EIR_EFFECT_READ_WORLD, EIR_EFFECT_WRITE_WORLD};
use pwe_api::Access;
use std::collections::BTreeMap;

/// The canonical physics pipeline (RFC-0008 §2). Solver selection happens after
/// lowering and never changes World semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum PhysicsStage {
    BroadPhase = 0,
    NarrowPhase = 1,
    ConstraintBuild = 2,
    Solve = 3,
    Integrate = 4,
    Commit = 5,
}

/// A solver-agnostic physics domain node. Fixed schedule order is the stage
/// order; within a stage, backend may pick impulse/iterative/projected/etc.
#[derive(Clone, Debug)]
pub struct PhysicsNode {
    pub stage: PhysicsStage,
    pub node: DomainNode,
}

impl PhysicsNode {
    /// Builds the default pipeline node set. `domain` constants are namespaced
    /// under `pwe.physics` by the caller's `DomainNode` construction.
    pub fn pipeline(
        domain: u32,
        input_schemas: Vec<pwe_api::ComponentTypeId>,
        output_schemas: Vec<pwe_api::ComponentTypeId>,
    ) -> BTreeMap<PhysicsStage, PhysicsNode> {
        let stages = [
            PhysicsStage::BroadPhase,
            PhysicsStage::NarrowPhase,
            PhysicsStage::ConstraintBuild,
            PhysicsStage::Solve,
            PhysicsStage::Integrate,
            PhysicsStage::Commit,
        ];
        let mut map = BTreeMap::new();
        for stage in stages {
            let writes = stage == PhysicsStage::Commit;
            let node = DomainNode {
                domain,
                name: format!("physics.{:?}", stage).to_lowercase(),
                input_schemas: input_schemas.clone(),
                output_schemas: output_schemas.clone(),
                effects: if writes {
                    EIR_EFFECT_WRITE_WORLD
                } else {
                    EIR_EFFECT_READ_WORLD
                },
                required_access: if writes { Access::WRITE } else { Access::READ },
            };
            map.insert(stage, PhysicsNode { stage, node });
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::ComponentTypeId;

    #[test]
    fn physics_pipeline_is_stage_ordered_and_solver_agnostic() {
        let pipeline = PhysicsNode::pipeline(
            7,
            vec![ComponentTypeId([1; 16])],
            vec![ComponentTypeId([2; 16])],
        );
        assert_eq!(pipeline.len(), 6);
        // Only Commit declares WRITE_WORLD; the rest are read-only stages.
        assert_eq!(
            pipeline.get(&PhysicsStage::Commit).unwrap().node.effects,
            EIR_EFFECT_WRITE_WORLD
        );
        assert_eq!(
            pipeline
                .get(&PhysicsStage::BroadPhase)
                .unwrap()
                .node
                .effects,
            EIR_EFFECT_READ_WORLD
        );
    }

    #[test]
    fn physics_nodes_lower_like_any_domain_node() {
        let pipeline = PhysicsNode::pipeline(7, vec![], vec![]);
        for node in pipeline.values() {
            // stable_id is derivable (name/domain valid), proving it is a
            // well-formed DomainNode.
            assert!(node.node.stable_id().is_ok());
        }
    }
}
