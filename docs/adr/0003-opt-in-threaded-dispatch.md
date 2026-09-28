# ADR-0003: Threaded-dispatch interpreter is opt-in; jump table stays default

**Status:** Accepted (Phase 3).

## Context

"Threaded dispatch" (function-pointer/computed dispatch) is a classic
interpreter optimization. The interpreter currently dispatches with a Rust
`match` over `Opcode` (a jump table).

## Decision

Implement a genuine threaded interpreter (static handler table indexed by
opcode), but keep it **opt-in** (`pwe run --threaded` /
`LangRuntime::enable_threaded_dispatch`). Whole-module gating: modules with
opcodes outside the supported subset fall back to the jump table.

## Consequences

- Measured slower here: micro ~8%, end-to-end nbody-64 3086 µs vs 2264 µs
  (~36%) — the well-predicted jump table beats an indirect call per opcode.
  Hence not the default.
- Differentially verified (`threaded_dispatch_matches_jump_table`, and via
  `step_cross` against the JIT).

## Alternatives

- **Make it the default** — rejected: a measured regression.
- **Don't implement it** — rejected: the user asked for it as a selectable
  backend; it also serves as a second execution strategy for portability.
