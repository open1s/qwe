# 0010 — `print(x)` semantics are undocumented (Low)

## Summary

`print(x)` is listed among the builtins with no explanation of where output
goes, whether it is part of the deterministic contract, or that the log is
capped.

## Evidence

- Listed only by name: `docs/lang-usage.md:631` ("`print(x)`, `emit(…)`").
- Implementation: `reference/src/eir.rs:807` — `Opcode::Print` validates one
  f64 operand and "Logs to the execution context"; `eir.rs:1870+` — the log is
  a **debug side-channel that persists across steps and is capped** so a
  per-step print cannot grow memory unboundedly in a long live run.
- Opcode lowering: `reference/src/lang/mod.rs:1505` — `print → Opcode::Print`.

## Impact

Two ambiguities for users:

1. Is `print` part of what `step_cross` compares (interpreter ≡ JIT)? It reads
   like an observable side effect; if it is not hashed/compared, say so —
   otherwise users will debug determinism divergences through it.
2. The cap means output can be truncated mid-run with no warning; in
   `present`/live runs this is invisible.

`emit`/`last_event`, by contrast, *are* part of the deterministic contract
(`docs/lang-usage.md:369-373`) — the distinction should be stated once, in one
place.

## Fix

Add one sentence to §2.9 and the Part 3 semantics list: `print` is a capped,
non-contract debug trace (host-visible, excluded from cross-backend
comparison); `emit`/`last_event` are contract events.

## Labels

docs, determinism, low
