# Implementation Plan: PWE v0.2 Reference System

## Outcome

Deliver a dependency-free Rust API plus deterministic reference implementation
that can load canonical world data, execute a bounded EIR subset transactionally,
produce/restore state snapshots, enforce ownership, and prove the required
semantics by version-controlled conformance tests.

## Architecture decisions

- `pwe-api` remains `no_std`, dependency-free, and defines only stable contracts.
- `pwe-reference` is a `std` semantic oracle using ordered collections; it owns
  authoritative mutable state and never exposes a second authoritative cache.
- Wire formats are hand-written bounded readers/writers before any compression or
  JIT backend. No third-party dependency is added without a separate decision.
- The interpreter is the first executable EIR backend. A "JIT" starts as a
  validated compiled-artifact cache delegating semantics to the interpreter.

## Dependency graph

```text
types/errors -> canonical bytes -> schema -> WIR -> reference world
                                           -> snapshot/delta -> replay fixtures
schema + capability -> Domain IR -> EIR validation/interpreter -> artifact cache
reference world -> ownership -> scheduler/render view -> conformance runner
```

## Phases

1. Foundation: complete canonical byte/limit/error helpers and reference-world
   read/write/event semantics.
2. Interchange: schema registry, WIR reader/writer, extension validation.
3. State portability: snapshot/delta transactional restore and replay hash.
4. Execution: Domain IR, EIR validator/interpreter, artifact identity/cache.
5. Runtime boundaries: deterministic scheduler, render frame and transfer tests.
6. Conformance: fixtures, report runner, minimal ground/vehicle/camera scenario.

## Checkpoints

- After phases 1–2: canonical WIR round-trip and malformed input rejection pass.
- After phases 3–4: restore/replay and interpreter/artifact equivalence pass.
- After phases 5–6: all required profile tests pass and report contains no skip.

## Risks

| Risk | Mitigation |
| --- | --- |
| RFCs leave some binary records underspecified | Keep implementation bounded to declared subset; add fixtures and record every extension point. |
| Premature JIT complexity | Use interpreter semantics and cache identity first; backend is replaceable. |
| Hidden nondeterminism | Ordered containers, explicit event ordering, replay-hash tests. |
| ABI drift | Keep API additive and assert descriptor/version compatibility in tests. |
