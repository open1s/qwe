# 0040 — Native JIT deopt re-executes a step on partially-committed native writes: trap swallowed, state double-applied

**Severity:** Medium
**Reported:** 2026-09-28
**Component:** reference (jit.rs + native.rs write path)
**Introduced by:** `70ca8d7f` (phase-3 hotness→native JIT); the write-path
mechanism predates it but only becomes load-bearing with the deopt fallback.

## Summary

`pwe_write_view` commits world writes to the runtime **immediately**
(`ctx.rt.write_field(...)`) *and* records them in the returned write list.
When a natively-executed step fails mid-way (div/rem-by-zero, NaN compare —
the `EirInvalid 18` trap paths verified in #32/#36), `execute_entries` returns
`Err` and the recorded write list is dropped, **but the writes already applied
via `write_field` are not rolled back**. The JIT's deopt fallback then
re-executes the *whole* step with the interpreter on top of that partially
mutated scene:

- statements before the trapping one are applied **twice** (once by the failed
  native run, once by the interpreter), and
- the trap can **disappear entirely** (the re-executed step no longer divides
  by zero, because the divisor was already mutated by the failed run) — the
  step reports **success with wrong state and no error**.

The interpreter path is atomic in the same repro (evaluation traps before any
`WriteView` commits; scene stays pristine at error), so `enable_native_jit(true)`
+ `step_jit` silently diverges from the interpreter, defeating the differential
contract of the new backend.

## Evidence (probe against pristine `70ca8d7f`)

Source:

```
world { gravity=(0,0,0) entity e { state=(v=7.0,w=100.0,x=0.0) } }
systems { update { on=e; dt=1.0
 w = w - 1.0
 x = v / w
 } }
```

`w` reaches 0 after step 100; step 101 evaluates `7.0 / 0.0`.

| run (150 steps) | step 101 outcome | scene after step 101 | `native_executions` |
|---|---|---|---|
| `step_jit`, native OFF | `Err(EirInvalid d18)` | pristine (`w=0 x=7`) | 0 |
| `step_jit`, native **ON** | **no error**, continues to 150 | **`w=-2 x=-7`** (w applied twice, trap gone) | 37 (frozen at 101) |
| `step_cross`, native ON | `Err(EirInvalid d18)` at 101 (interp side runs first on its clone and traps before the JIT side is compared) | n/a | 37 |

Trace of the corrupted run:

```
ON-jit step 100: w=0  x=7    (nat_exec=37)   # native ok
ON-jit step 101: w=-2 x=-7   (nat_exec=37)   # native failed (not counted), step SUCCEEDS
ON-jit step 102: w=-3 x=-3.5 (nat_exec=38)
```

What happens at step 101: the failed native run commits `w = -1` through
`write_field` before its division traps; `execute_entries` returns `Err`;
`CpuJit::execute_with_env_validated` falls through to
`code.eir.execute(...)` **with the same runtime**, which re-runs the entry on
`w = -1` → `w = -2`, `x = 7 / -1 = -7`, no division by zero → the trap and the
error both vanish.

Additional leaked side effect on the same path: `execute_entries` calls
`rt.commit_barrier()` for barrier entries *before* running them and has no
rollback for it either.

## Current blast radius

- **`step_jit` / `step_jit_n` with `enable_native_jit(true)`** (public API):
  silent state corruption + swallowed trap. Reproduced above.
- **`pwe run` today:** masked — native code only executes inside `step_cross`,
  which evaluates the interpreter side first on a pristine clone and propagates
  its `?` error before the JIT side runs; a native-only failure on the clone is
  caught by the byte-compare (`EirInvalid 50`) instead of corrupting the live
  scene. The masking is incidental (evaluation order + clones), not a designed
  guarantee.
- **Standalone `NativeProgram::execute_entries`:** partial commits also persist
  after `Err` (pre-existing; was untested — probes only asserted the `Err`).
- This blocks the RFC-0027 endgame: any future wiring that steps through the
  JIT without the cross-check (the whole point of a production JIT) would
  corrupt state.

## Fix direction

Make the native step atomic:

1. Stage native `write_field` calls into a scratch view that native reads also
   consult (preserving read-your-writes), merge into the runtime **only after
   every entry returns OK**, discard on `Err`; and/or keep an undo log in
   `NativeCtx` and roll back on the error path; likewise defer
   `commit_barrier()` until the entry succeeds.
2. Alternatively (minimal): on native `Err`, **do not re-execute** — propagate
   the error (a failed step is a failed step). This keeps the trap visible but
   still needs the rollback of partial commits so the runtime is left in the
   pre-step state, matching the interpreter's atomic failure.
3. Add a regression test: the repro above with `enable_native_jit(true)` +
   `step_jit` must fail with `EirInvalid 18` at step 101 with the scene pristine
   (`w=0`), identical to native OFF; and a mid-step `commit_barrier` case.
