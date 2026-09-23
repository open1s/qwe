//! RFC-0021 block and dominance verification.
//!
//! RFC-0021 requires, before execution/link/JIT, that EIR verify "one
//! definition/value, dominance, block targets, types" and reject "dominance
//! violations". This module is that static check: it builds a control-flow
//! graph from EIR instructions grouped into blocks, verifies branch targets,
//! computes block dominators, and checks that every SSA use is dominated by its
//! single definition.
//!
//! It consumes the same [`crate::eir::Instruction`] type as the interpreter, but
//! is purely a static verifier: it does not execute anything, so the interpreter
//! (the semantic oracle) is untouched. A straight-line single block is a valid
//! CFG and passes exactly as before.

use crate::eir::{Instruction, Opcode};
use pwe_api::{Error, Result, Status};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32, offset: usize) -> Error {
    Error {
        status,
        detail,
        byte_offset: offset as u64,
    }
}

/// The terminator that ends a block. Mirrors the RFC-0021 binary terminators
/// (`RET=0x8000`, `BR=0x8001`, `COND_BR=0x8002`, `TRAP=0x8003`,
/// `UNREACHABLE=0x8004`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminator {
    Return,
    Trap,
    Unreachable,
    Br(u32),
    CondBr(u32, u32),
}

/// A basic block: `instructions[start..end)` plus one terminator.
#[derive(Clone, Debug)]
pub struct Block {
    pub id: u32,
    pub start: usize,
    pub end: usize,
    pub terminator: Terminator,
}

impl Block {
    pub fn new(id: u32, start: usize, end: usize, terminator: Terminator) -> Self {
        Self {
            id,
            start,
            end,
            terminator,
        }
    }
}

/// A control-flow graph over a function's instructions and blocks.
#[derive(Clone, Debug)]
struct Cfg {
    blocks: Vec<Block>,
    /// successor indices per block.
    succ: Vec<Vec<usize>>,
    entry: usize,
}

fn build_cfg(blocks: &[Block]) -> Result<Cfg> {
    if blocks.is_empty() {
        return Err(error(Status::EirInvalid, 40, 0));
    }
    let mut index_of: BTreeMap<u32, usize> = BTreeMap::new();
    for (i, b) in blocks.iter().enumerate() {
        if index_of.insert(b.id, i).is_some() {
            return Err(error(Status::EirInvalid, 41, b.start));
        }
    }
    // The entry block is the lowest id (deterministic, like function order).
    let entry_id = blocks.iter().map(|b| b.id).min().unwrap_or(0);
    let entry = index_of[&entry_id];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for b in blocks {
        let from = index_of[&b.id];
        let targets: Vec<u32> = match b.terminator {
            Terminator::Br(t) => vec![t],
            Terminator::CondBr(t1, t2) => vec![t1, t2],
            _ => Vec::new(),
        };
        for t in targets {
            let to = index_of
                .get(&t)
                .copied()
                .ok_or(error(Status::EirInvalid, 42, b.start))?;
            succ[from].push(to);
        }
    }
    Ok(Cfg {
        blocks: blocks.to_vec(),
        succ,
        entry,
    })
}

/// Verifies RFC-0021 dominance and block targets over `instructions` grouped
/// into `blocks`. The blocks' ranges must partition the instruction list, each
/// block must end in a valid terminator, every branch must target an existing
/// block, and every SSA use must be dominated by its single definition.
pub fn verify_dominance(instructions: &[Instruction], blocks: &[Block]) -> Result<()> {
    verify_dominance_impl(instructions, blocks, 0)
}

/// Verifies dominance with `argument_count` leading SSA slots treated as
/// pre-defined function arguments (defined at the entry block, which dominates
/// every other block). Used by modules that call functions with arguments.
pub fn verify_dominance_with_args(
    instructions: &[Instruction],
    blocks: &[Block],
    argument_count: u32,
) -> Result<()> {
    verify_dominance_impl(instructions, blocks, argument_count)
}

fn verify_dominance_impl(
    instructions: &[Instruction],
    blocks: &[Block],
    argument_count: u32,
) -> Result<()> {
    let cfg = build_cfg(blocks)?;

    // Blocks must partition the instruction stream with no overlap/gap.
    let mut ranges: Vec<&Block> = blocks.iter().collect();
    ranges.sort_by_key(|b| b.start);
    let mut cursor = 0usize;
    for b in &ranges {
        if b.start != cursor {
            return Err(error(Status::EirInvalid, 43, b.start));
        }
        if b.end > instructions.len() {
            return Err(error(Status::EirInvalid, 44, b.end));
        }
        // The block must end in a terminator that matches its declared kind.
        if b.end == 0 || !is_terminator(&instructions[b.end - 1], b.terminator) {
            return Err(error(Status::EirInvalid, 45, b.end));
        }
        cursor = b.end;
    }
    if cursor != instructions.len() {
        return Err(error(Status::EirInvalid, 43, cursor));
    }

    // Dominators via iterative dataflow over the block graph.
    let n = cfg.blocks.len();
    let all: u64 = if n < 64 { (1u64 << n) - 1 } else { u64::MAX };
    let mut dom: Vec<u64> = vec![0u64; n];
    dom[cfg.entry] = 1u64 << cfg.entry;
    for (i, d) in dom.iter_mut().enumerate() {
        if i != cfg.entry {
            *d = all;
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if i == cfg.entry {
                continue;
            }
            let mut pred_dom = all;
            let mut any_pred = false;
            for (p, s) in cfg.succ.iter().enumerate() {
                if s.contains(&i) {
                    pred_dom &= dom[p];
                    any_pred = true;
                }
            }
            if !any_pred {
                continue; // unreachable block; leave as is
            }
            let new = (1u64 << i) | pred_dom;
            if new != dom[i] {
                dom[i] = new;
                changed = true;
            }
        }
    }

    // Map each instruction position to its enclosing block index.
    let mut block_of = vec![0usize; instructions.len()];
    for (bi, b) in cfg.blocks.iter().enumerate() {
        block_of[b.start..b.end].fill(bi);
    }

    // Reject multiple definitions of one value, and index the defining
    // position of every SSA value once. `def_of` then answers in O(log n)
    // instead of rescanning the whole stream per operand — the difference
    // between linear and quadratic on large unrolled systems (3D field sweeps).
    let mut result_ids: BTreeMap<u32, usize> = BTreeMap::new();
    for (pos, ins) in instructions.iter().enumerate() {
        if ins.result_id != 0 && result_ids.insert(ins.result_id, pos).is_some() {
            return Err(error(Status::EirInvalid, 46, pos));
        }
    }
    let def_of = |id: u32| -> Option<(usize, usize)> {
        result_ids.get(&id).map(|&pos| (block_of[pos], pos))
    };

    for (bi, b) in cfg.blocks.iter().enumerate() {
        for (idx, ins) in instructions[b.start..b.end].iter().enumerate() {
            let pos = b.start + idx;
            for (op_pos, &operand) in ins.operands.iter().enumerate() {
                if operand == 0 {
                    continue;
                }
                // Function arguments are pre-defined at the (dominating) entry.
                if operand <= argument_count {
                    continue;
                }
                // Branch-target operands are block ids, not SSA value uses. A CALL target
                // is a function id, not a value use either.
                let is_target = match ins.opcode {
                    Opcode::Br => true,
                    Opcode::CondBr => op_pos != 0,
                    Opcode::Call => op_pos == 0,
                    _ => false,
                };
                if is_target {
                    continue;
                }
                let (def_block, def_pos) =
                    def_of(operand).ok_or(error(Status::EirInvalid, 47, pos))?;
                if def_block == bi {
                    if def_pos >= pos {
                        return Err(error(Status::EirInvalid, 48, pos));
                    }
                } else if dom[bi] & (1u64 << def_block) == 0 {
                    // def block does not dominate the use block.
                    return Err(error(Status::EirInvalid, 49, pos));
                }
            }
        }
    }
    Ok(())
}

/// Maps a block's declared terminator kind to the EIR opcode it must end with.
fn is_terminator(ins: &Instruction, t: Terminator) -> bool {
    let ok_op = match t {
        Terminator::Return => matches!(ins.opcode, Opcode::Return),
        Terminator::Trap => matches!(ins.opcode, Opcode::Trap),
        Terminator::Unreachable => matches!(ins.opcode, Opcode::Unreachable),
        Terminator::Br(_) => matches!(ins.opcode, Opcode::Br),
        Terminator::CondBr(_, _) => matches!(ins.opcode, Opcode::CondBr),
    };
    ok_op && ins.result_id == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eir::{Immediate, ValueType};

    fn ins(opcode: Opcode, result_id: u32, operands: Vec<u32>) -> Instruction {
        Instruction {
            opcode,
            result_id,
            result_type: None,
            operands,
            constant: None,
            target: None,
        }
    }
    fn cinst(result_id: u32, value: u32) -> Instruction {
        Instruction {
            opcode: Opcode::Const,
            result_id,
            result_type: Some(ValueType::U32),
            operands: vec![],
            constant: Some(Immediate::U32(value)),
            target: None,
        }
    }

    #[test]
    fn single_straight_line_block_passes() {
        // block0: const %1, add %2 = %1+%1, return
        let instructions = vec![
            cinst(1, 5),
            ins(Opcode::Add, 2, vec![1, 1]),
            ins(Opcode::Return, 0, vec![]),
        ];
        let blocks = vec![Block::new(0, 0, 3, Terminator::Return)];
        assert!(verify_dominance(&instructions, &blocks).is_ok());
    }

    #[test]
    fn def_used_before_definition_is_rejected() {
        // %2 used before %1 is defined in the same block.
        let instructions = vec![
            ins(Opcode::Add, 2, vec![1, 1]),
            cinst(1, 5),
            ins(Opcode::Return, 0, vec![]),
        ];
        let blocks = vec![Block::new(0, 0, 3, Terminator::Return)];
        assert_eq!(
            verify_dominance(&instructions, &blocks).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn diamond_cfg_with_dominating_def_passes() {
        // block0: %1=const 5, cond-br
        // block1: %2 = %1+%1, br end
        // block2: %3 = %1+%1, br end
        // block3: return
        let instructions = vec![
            cinst(1, 5),
            ins(Opcode::CondBr, 0, vec![1, 2, 4]), // cond=%1, true->blk1, false->blk2
            ins(Opcode::Add, 2, vec![1, 1]),
            ins(Opcode::Br, 0, vec![6]), // br end
            ins(Opcode::Add, 3, vec![1, 1]),
            ins(Opcode::Br, 0, vec![6]),    // br end
            ins(Opcode::Return, 0, vec![]), // return terminator
        ];
        let blocks = vec![
            Block::new(0, 0, 2, Terminator::CondBr(1, 2)),
            Block::new(1, 2, 4, Terminator::Br(3)),
            Block::new(2, 4, 6, Terminator::Br(3)),
            Block::new(3, 6, 7, Terminator::Return),
        ];
        // %1 dominates both block1 and block2 (defined in entry), and %2/%3 are
        // defined before use within their blocks, so this is valid.
        assert!(verify_dominance(&instructions, &blocks).is_ok());
    }

    #[test]
    fn branch_to_unknown_block_is_rejected() {
        let instructions = vec![cinst(1, 5), ins(Opcode::Return, 0, vec![])];
        let blocks = vec![Block::new(0, 0, 2, Terminator::Br(99))];
        assert_eq!(
            verify_dominance(&instructions, &blocks).unwrap_err().status,
            Status::EirInvalid
        );
    }

    #[test]
    fn multiple_definitions_of_one_value_are_rejected() {
        let instructions = vec![cinst(1, 5), cinst(1, 6), ins(Opcode::Return, 0, vec![])];
        let blocks = vec![Block::new(0, 0, 3, Terminator::Return)];
        assert_eq!(
            verify_dominance(&instructions, &blocks).unwrap_err().status,
            Status::EirInvalid
        );
    }
}
