# RFC-0003: Execution IR

**Status:** Draft \| **Version:** 0.1.0

## 1. Purpose

EIR is the common executable representation for interpreter, JIT and
AOT.

## 2. Structure

``` rust
pub struct Module {
    pub functions: Vec<Function>,
    pub globals: Vec<Global>,
    pub resources: Vec<Resource>,
}
pub struct Function {
    pub id: FunctionId,
    pub signature: Signature,
    pub blocks: Vec<Block>,
}
pub struct Block {
    pub id: BlockId,
    pub args: Vec<Value>,
    pub operations: Vec<Operation>,
    pub terminator: Terminator,
}
```

EIR uses SSA-like values.

## 3. Operations

Scalar: Load, Store, Add, Sub, Mul, Div, FMA, Compare, Select, Cast,
Call. Vector: VecLoad, VecStore, Shuffle, Reduce. Memory: Alloc, Free,
Copy, Prefetch. Parallel: ParallelFor, Map, Reduce, Scan, Pipeline,
Barrier. Device: Dispatch, Copy, Fence. Distributed: RemoteCall,
RemoteData, Migration, Replication.

Domain operations may exist temporarily but must lower to base EIR
before production code generation.

## 4. Memory spaces

ReadOnly, ReadWrite, Atomic, Shared, GPU.

## 5. Determinism

Operations that participate in deterministic execution must declare
deterministic semantics; unordered floating-point reductions are
forbidden in strict deterministic mode.

## 6. Validation

SSA dominance, type correctness, memory-space legality, dependency
correctness, bounded resource use, and capability requirements must be
validated before execution.

## 7. Invariant

The same valid EIR must have semantically equivalent interpreter, JIT
and AOT execution.
