# 0014 — `pwe run` claims "(interpreter == JIT, every step)" but cross-checks 1 step in 16 (Medium)

## Summary

Every `pwe run` ends with

```
ran 60 steps (interpreter == JIT, every step)
```

but the CLI batches cross-verification: `step_cross_batched(k)` runs `k-1`
interpreter-only steps and one cross-verified step, and the CLI uses
`CROSS_BATCH = 16`. So for a default run **4 of 60 steps** (for `--steps 3`,
**1 of 3**) are actually cross-checked — the printed claim, and the README /
book guarantees it mirrors, are false for CLI runs.

## Evidence

- `cli/src/main.rs:575` — `println!("ran {steps} steps (interpreter == JIT, every step)");`
  printed unconditionally by `report()`.
- `cli/src/main.rs:416-418` — `const CROSS_BATCH: u32 = 16;` with a comment
  that admits batching ("Interactive/demo runs favor throughput").
- `cli/src/main.rs:399-408` — run loop calls `rt.step_cross_batched(k)`.
- `reference/src/lang/runtime.rs:437-444` — doc comment: "Batched
  cross-backend stepping: `k-1` interpreter-only steps followed by one
  cross-verified step, so the JIT runs once per `k` steps rather than every
  step."
- Public claims that the CLI output echoes but does not honor:
  - `README.md:21` — "the JIT must agree with it **byte-for-byte on every step**"
  - `README.md:58` — "Provably deterministic — interpreter ≡ JIT, **asserted every step**"
  - `docs/book/determinism.md:23` — section "Interpreter ≡ JIT, every step"
  - `docs/lang-usage.md:65` — sample output showing the same line.

## Impact

Determinism/equivalence is the project's headline guarantee. The CLI both
*claims* a per-step assertion it does not perform and *prints* a claim users
cannot distinguish from the strict API (`step_cross_n`, which really is
per-step). A JIT divergence occurring in 15 of 16 steps would go unnoticed
while the run reports success.

## Suggested fix

Either cross-verify every step by default in `pwe run` (expose
`--cross-every N` for throughput) or make the output honest, e.g.
`ran 60 steps (interpreter == JIT, cross-checked every 16 steps)`, and align
README/book wording with what the shipped CLI actually does.
