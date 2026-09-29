# RFC-0046: EIR as an explicit SSA CFG
**Status:** Accepted (implemented, representation + wire form). EIR's CFG is now first-class: the *view* is explicit (`EirModule::blocks` returns each function's basic blocks with terminators; `verify_cfg` runs the CFG dominance gate, and `verify_linear_dominance` now uses real blocks instead of one block per function), and the **RFC-0021 FUNCTIONS section encodes explicit basic blocks** for branched functions (`block_count > 1`; a straight-line function keeps the legacy single-block layout, so older artifacts are byte-identical and still decode). The decoder concatenates blocks deterministically, so interpreter == JIT == AOT and re-encoding is byte-identical. Remaining (future, needs no format change): SSA block **parameters** (the field is reserved) and cross-block optimizations (DCE/GVN).

## Motivation

EIR is a per-function, essentially flat list: `Function { instructions:
Vec<Instruction> }`, where `Br`/`CondBr` carry **instruction-index** targets
(`reference/src/eir.rs`, opcodes `BR=0x8001` / `COND_BR=0x8002`). A structural
CFG verifier already exists — `dominance::verify_dominance_with_args` partitions
the list into basic blocks at every branch target (`blocks_from_instructions`),
checks targets/terminators, computes dominators, and enforces that every SSA use
is dominated by its definition. The interpreter walks the list with a `pc`.

Consequences of the flat form:

- **Blocks are implicit** (recomputed by every consumer: validator, dominance,
  AOT gate). `verify_linear_dominance` treats a whole function as one block for
  the AOT path — a coarser gate than the CFG verifier.
- **SSA at block joins is implicit.** Values that "exist" across a `CondBr` are
  valid only because of list order + the dominance check; there are no block
  parameters (φ), so optimizations that rename/move across blocks are unsafe to
  add.
- **Optimizations are limited** to the peephole passes in `optimize()` (const
  fold + `Mul/Add→Fma`). DCE, GVN, and loop opts need an explicit CFG.

## Design

### Representation

```
Function { id, argument_count, effect_mask,
           blocks: Vec<Block>,     // entry = block 0
           entry: u32 }
Block { params: Vec<u32>,          // SSA block parameters (φ as params)
        ops:    Vec<Instruction>,  // no branch/terminator inside `ops`
        term:   Terminator }       // Return | Br(bb) | CondBr(cond, bb_t, bb_f)
                                   // | Trap | Unreachable
```

- Edges are **block indices** (not instruction indices).
- A block "produces" values via `params`; a predecessor passes them on its
  terminator edge. This is standard SSA-with-block-parameters and removes the
  need for φ ordering subtleties.

### Verification

`dominance::verify` operates directly on blocks: every target is in range, each
block ends in exactly one terminator, the entry is block 0, and every use is
dominated by its def (block-parameter uses are dominated by the corresponding
edge from the predecessor). The existing single-block linear form is the
degenerate case (one block, `Return`).

### Wire format (RFC-0021) — as shipped

The FUNCTIONS section encodes the CFG **inline** (no separate section): a
straight-line function keeps the original single-block layout
(`block_count == 1`, byte-identical to legacy artifacts, `EIR_MINOR` unchanged),
while a branched function is written as explicit blocks (`block_count > 1`:
`argument_count`, then per block `block_id | param_count(reserved) |
instruction_count | instruction*`). The decoder concatenates the blocks
deterministically, so decoding reproduces the flat stream exactly and re-encoding
is byte-identical. A reader that does not implement `block_count > 1` fails
closed. RFC-0021 carries the concrete layout and the compatibility rationale.


### Execution

The interpreter, threaded dispatcher, JIT, and AOT all consume the block form;
the interpreter may keep a `(block, op)` program counter, and the flattened
instruction order is recoverable for the existing `CallIndex`/`Fma` machinery.

## Validation

- `validate` rejects: out-of-range block targets, a block with zero or >1
  terminators, unreachable-from-entry blocks (advisory), and any use not
  dominated by its def — each a named conformance case.
- Round-trip: an artifact encoded in the flat form decodes to the **same
  blocks** as one encoded in the block form, and both interpret identically.
- Equivalence: interpreter == JIT == AOT on the block form (`step_cross`).
- Optimizations enabled afterward (DCE/GVN) must be differentially verified
  against the interpreter (AGENTS §4).

## Backward compatibility

Reader-first: the decoder accepts both forms; `PWEEIR2` artifacts remain valid.
No interpreter/JIT semantics change (the flat single-block case is unchanged).

## Alternatives

- **Keep deriving blocks** — fine for verification, but blocks any cross-block
  optimization and duplicates the CFG in each consumer. Rejected as the
  long-term shape.
- **Full φ-node SSA with a separate dominance frontier build** — heavier than
  block parameters for this IR; block params give the same guarantees with less
  machinery.
