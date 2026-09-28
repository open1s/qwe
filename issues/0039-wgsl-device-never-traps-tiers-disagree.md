# 0039 — WGSL device backend never traps: div/rem-by-zero and NaN compares diverge from interpreter + native (tiers disagree)

**Severity:** Low
**Reported:** 2026-09-28
**Component:** reference (wgsl backend)
**Discovered during:** verification of #37/#38 (commit `20ba90be`)

## Summary

The interpreter and the native backend both trap `EirInvalid 18` on
divide/remainder by `±0.0` and on any NaN-operand comparison. The WGSL
device backend (and its CPU oracle) evaluates both with IEEE semantics
and never traps. `20ba90be` documents this as an inherent f32
divergence (module doc table, "divide/remainder by ±0.0" row) and its
test matrix explicitly skips interpreter-trap inputs with a comment
pointing at "the div/rem trap umbrella" — but no issue tracked that
umbrella (#36 was closed for the native side only). Tracked here so the
decision lands in the RFC before the backend is wired.

## Evidence (probes at `20ba90be`)

| input | interpreter | native | WGSL oracle |
|---|---|---|---|
| `1.0 % 0.0` | `Err(EirInvalid d18)` | `Err(EirInvalid d18)` | `Ok(NaN)` |
| `1.0 % -0.0` | `Err(EirInvalid d18)` | `Err(EirInvalid d18)` | `Ok(NaN)` |
| `1.0 / 0.0` (#32) | `Err(EirInvalid d18)` | `Err(EirInvalid d18)` | IEEE `inf` |
| `(a+1.0) % (a+0.5)` at `a=-0.5` | `Err(EirInvalid d18)` | — | `Ok(NaN)` |
| `a < 1.0` at `a=NaN` | `Err(EirInvalid d18)` | `Err(EirInvalid d18)` (#36) | `Ok(0.0)`, emits `r[3] = select(0.0f, 1.0f, r[1] < r[2]);` |

Emitted shader has no trap channel for these paths (no `PweCtx`
analogue), so once wired, a kernel that fails the step on tiers 1–2
would silently produce IEEE results on tier 3.

## Why Low

The backend is not wired to any production entry point (RFC-0027
exemption), and the divergence is already documented in the module doc
table. This issue exists to force the semantics decision (trap, gate
the input class, or capability-flag the backend) before wiring.

## Fix direction

Decide in the RFC one of: (a) emit `PweCtx`-style trap calls in WGSL
impossible without host support → gate such kernels out of `eligible()`
(div/rem-by-zero and compares with NaN-unprovable operands), (b) an
explicit "device-tolerant" language mode where IEEE results are legal
and the interpreter/native match it under a flag, or (c) host-side
validation that rejects trap-inputs at dispatch. Update the module doc
table and the skipped test rows to the chosen contract.
