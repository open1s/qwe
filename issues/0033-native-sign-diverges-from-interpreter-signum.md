# 0033 — Native backend: `Sign` opcode diverges from interpreter `signum` at 0.0, −0.0 and NaN (Medium)

## Summary

The interpreter lowers `Opcode::Sign` to Rust `f64::signum()`
(`reference/src/eir.rs:1442` runtime, `:2536` const-fold):

| input | interpreter (`signum`) | native (emitted C) |
| --- | --- | --- |
| `+0.0` | **1.0** | **0.0** |
| `-0.0` | **-1.0** | **0.0** |
| `NaN` | **NaN** | **±1.0** (payload sign) |
| `1.0` / `-5.0` | 1.0 / -1.0 | 1.0 / -1.0 ✓ |

The native emitter (`reference/src/native.rs`, commit `7cc2d5a5`):

```c
r[res] = (r[x] == 0.0) ? 0.0 : copysign(1.0, r[x]);
```

That is a different function: it maps all zeros to `0.0` and NaN to
`±1.0`.

## Evidence

Probe (commit `7cc2d5a5`, fresh process), pure `funcs { f(v) { sign(v) } }`:

```text
P1 sign(0.0): interp=1 native=0 => DIVERGES
P1b sign(-0.0): interp=-1 native=0 => DIVERGES
```

The differential test's input grid (`i*0.31-4.0`, `i*-0.17+2.0`) never
produces `0.0`/`-0.0`/`NaN`, and its function body never calls `sign`,
so it passes while this divergence is live.

## Impact

Same class as #32: the interpreter is the semantic reference (AGENTS
§4). `sign()` of a zeroed quantity — a common state after reset or
cancellation — yields different world state per backend. Const-folded
expressions use `signum` (eval_const path) while native uses the other
rule, so even *within* one run, fold-time and native-time can disagree.

## Suggested fix

Emit the exact reference semantics:

```c
r[res] = (r[x] != r[x]) ? NAN : copysign(1.0, r[x]);
```

(`copysign(1.0, ±0.0)` gives `±1.0`; the `isnan` guard restores NaN.)
Add `sign(0.0)`, `sign(-0.0)`, `sign(NaN)` to the differential test.
