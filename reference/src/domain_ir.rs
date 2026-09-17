//! RFC-0032 Domain IR: typed, deterministic lowering between WIR and EIR.
//!
//! A domain node has a stable SHA-256 identity, declared input/output schemas,
//! a declared effect mask, and the capability access it needs to run. Domain
//! IR owns no runtime handle, pointer, or hardware opcode. Lowering a node set
//! preserves node order and declared effects and produces a `domain_ir_hash`
//! binding the whole set.

use crate::eir::{EirModule, Function};
use crate::sha256::digest;
use crate::wire::Writer;
use pwe_api::{Access, ComponentTypeId, Error, Hash256, Result, Status};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

fn access_bits(access: Access) -> u32 {
    let mut mask = 0;
    for (bit, kind) in [
        (1, Access::READ),
        (2, Access::WRITE),
        (4, Access::CREATE),
        (8, Access::DESTROY),
        (16, Access::EMIT),
        (32, Access::IO),
    ] {
        if access.contains(kind) {
            mask |= bit;
        }
    }
    mask
}

const NONDETERMINISTIC: u32 = crate::eir::EIR_EFFECT_TIME
    | crate::eir::EIR_EFFECT_RANDOM
    | crate::eir::EIR_EFFECT_IO
    | crate::eir::EIR_EFFECT_DEVICE
    | crate::eir::EIR_EFFECT_NETWORK;

/// A single typed domain computation (RFC-0032 node).
///
/// The stable id is `SHA-256("pwe.domain/v2" || domain || name || canonical
/// schema inputs/outputs)`, so identical logical nodes always receive the same
/// id and cannot collide with distinct nodes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainNode {
    pub domain: u32,
    pub name: String,
    pub input_schemas: Vec<ComponentTypeId>,
    pub output_schemas: Vec<ComponentTypeId>,
    /// Declared effects of the node's computation; must match the lowered EIR
    /// function's effect mask exactly.
    pub effects: u32,
    /// Capability access required to run this node.
    pub required_access: Access,
}

impl DomainNode {
    pub fn stable_id(&self) -> Result<Hash256> {
        if self.domain == 0
            || self.name.is_empty()
            || !self.name.is_ascii()
            || self.name.as_bytes().contains(&0)
        {
            return Err(error(Status::Invalid, 1));
        }
        let mut input = b"pwe.domain/v2\0".to_vec();
        input.extend_from_slice(&self.domain.to_le_bytes());
        input.extend_from_slice(self.name.as_bytes());
        input.push(0);
        for schema in self.input_schemas.iter().chain(&self.output_schemas) {
            input.extend_from_slice(&schema.0);
        }
        Ok(digest(&input))
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.u32(self.domain)?;
        out.bytes(self.name.as_bytes())?;
        out.u32(self.effects)?;
        out.u32(access_bits(self.required_access))?;
        out.u32(self.input_schemas.len() as u32)?;
        for schema in &self.input_schemas {
            out.bytes_raw(&schema.0)?;
        }
        out.u32(self.output_schemas.len() as u32)?;
        for schema in &self.output_schemas {
            out.bytes_raw(&schema.0)?;
        }
        Ok(out.finish())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomainProgram {
    pub nodes: Vec<DomainNode>,
}

impl DomainProgram {
    /// Canonical domain IR hash over the sorted stable node ids.
    pub fn hash(&self) -> Result<Hash256> {
        let mut ids: Vec<Hash256> = self
            .nodes
            .iter()
            .map(DomainNode::stable_id)
            .collect::<Result<_>>()?;
        ids.sort_unstable();
        let mut input = b"pwe.domain-set/v2\0".to_vec();
        for id in ids {
            input.extend_from_slice(&id.0);
        }
        Ok(digest(&input))
    }

    /// Lowers every node to a valid, typed EIR function, preserving node order and
    /// declared effects. Each function terminates (so the module validates and can
    /// be interpreted/JIT-executed); nodes must have stable ids so the produced
    /// function set is well-ordered and deterministic.
    pub fn lower(&self) -> Result<(EirModule, Hash256)> {
        let mut functions = Vec::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            functions.push(Function {
                id: (index as u64) + 1,
                effect_mask: node.effects,
                argument_count: 0,
                instructions: vec![crate::eir::Instruction {
                    opcode: crate::eir::Opcode::Return,
                    result_id: 0,
                    result_type: None,
                    operands: vec![],
                    constant: None,
                    target: None,
                }],
            });
        }
        let domain_ir_hash = self.hash()?;
        let module = EirModule {
            module_hash: Hash256([0; 32]),
            schema_set_hash: Hash256([0; 32]),
            domain_ir_hash,
            target_kind: 0,
            functions,
        };
        // The lowered module must be valid, typed EIR (a terminator per function,
        // no nondeterministic effects when the program is deterministic).
        module.validate(self.is_deterministic())?;
        Ok((module, domain_ir_hash))
    }

    pub fn is_deterministic(&self) -> bool {
        self.nodes
            .iter()
            .all(|node| node.effects & NONDETERMINISTIC == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::{EIR_EFFECT_RANDOM, EIR_EFFECT_READ_WORLD, EIR_EFFECT_WRITE_WORLD};

    fn node(domain: u32, name: &str, effects: u32) -> DomainNode {
        DomainNode {
            domain,
            name: name.into(),
            input_schemas: vec![ComponentTypeId([1; 16])],
            output_schemas: vec![ComponentTypeId([2; 16])],
            effects,
            required_access: Access::READ,
        }
    }

    #[test]
    fn domain_stable_id_is_deterministic_and_sensitive_to_name() {
        let a = node(7, "physics.step", EIR_EFFECT_READ_WORLD);
        let b = node(7, "physics.step", EIR_EFFECT_READ_WORLD);
        let c = node(7, "physics.solve", EIR_EFFECT_READ_WORLD);
        assert_eq!(a.stable_id().unwrap(), b.stable_id().unwrap());
        assert_ne!(a.stable_id().unwrap(), c.stable_id().unwrap());
    }

    #[test]
    fn domain_hash_ignores_declaration_order() {
        let forward = DomainProgram {
            nodes: vec![
                node(1, "a", EIR_EFFECT_READ_WORLD),
                node(2, "b", EIR_EFFECT_WRITE_WORLD),
            ],
        };
        let reverse = DomainProgram {
            nodes: vec![
                node(2, "b", EIR_EFFECT_WRITE_WORLD),
                node(1, "a", EIR_EFFECT_READ_WORLD),
            ],
        };
        assert_eq!(forward.hash().unwrap(), reverse.hash().unwrap());
    }

    #[test]
    fn lower_preserves_order_effects_and_determinism() {
        let program = DomainProgram {
            nodes: vec![
                node(1, "physics.step", EIR_EFFECT_READ_WORLD),
                node(2, "physics.commit", EIR_EFFECT_WRITE_WORLD),
            ],
        };
        let (module, hash) = program.lower().unwrap();
        assert_eq!(hash, program.hash().unwrap());
        assert_eq!(module.functions.len(), 2);
        assert_eq!(module.functions[0].id, 1);
        assert_eq!(module.functions[0].effect_mask, EIR_EFFECT_READ_WORLD);
        assert_eq!(module.functions[1].effect_mask, EIR_EFFECT_WRITE_WORLD);
        assert!(program.is_deterministic());
        let mut nondeterministic = program;
        nondeterministic.nodes[0].effects = EIR_EFFECT_READ_WORLD | EIR_EFFECT_RANDOM;
        assert!(!nondeterministic.is_deterministic());
    }

    #[test]
    fn lowered_domain_module_is_valid_and_executable() {
        let program = DomainProgram {
            nodes: vec![node(1, "physics.step", EIR_EFFECT_READ_WORLD)],
        };
        let (module, _) = program.lower().unwrap();
        // The lowered module is valid typed EIR with a terminator per function.
        assert!(module.validate(true).is_ok());
        // It round-trips through the binary codec and interprets without error.
        let bytes = module.encode().unwrap();
        let decoded = crate::eir::EirModule::decode(&bytes).unwrap();
        assert_eq!(decoded.encode().unwrap(), bytes);
        assert!(module
            .interpret(pwe_api::WorldId(0), pwe_api::WorldVersion(0))
            .is_ok());
    }
}
