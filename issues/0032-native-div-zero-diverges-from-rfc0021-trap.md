# 0032 — Native backend: f64 `Div`/`Rem` by zero returns inf/NaN instead of the RFC-0021 trap (Medium)

## Summary

The interpreter treats f64 division/remainder by (exactly) zero as a
**defined trap** — RFC-0021, implemented in `divrem`
(`reference/src/eir.rs`, issue #17): the step fails with
`EirInvalid detail 18`.

The native backend (`reference/src/native.rs`, commit `7cc2d5a5`)
emits plain C:

```c
r[res] = r[a] / r[b];      // Div
r[res] = fmod(r[a], r[b]); // Rem
```

`Div`/`Rem` are in `NativeProgram::eligible()`, and the C ABI
`fn(*const f64) -> f64` has **no channel to report a trap**. So native
code returns IEEE `inf`/`NaN` where the reference interpreter fails the
step.

## Evidence

Probe (commit `7cc2d5a5`, fresh process), pure `funcs { g(v, w) { v / w } }`:

```text
P2 div-by-zero: interp=Err detail=18; native=Ok(inf)
=> DIVERGES (RFC-0021 trap vs inf)
```

The differential test (`native_pure_function_matches_interpreter`)
exercises only `a*a + b*b + sin(a)` over a benign input grid — no
division, no zero divisors — so it cannot catch this. The module docs'
"bit-identical" claim is therefore untested exactly where semantics
differ by design (IEEE vs RFC-0021).

## Impact

Violates AGENTS §4: *"The EIR interpreter is the semantic reference …
JIT/AOT ≡ optimized implementations"* and RFC-0021's defined trap. A
pure function like `f(a, b) { a / b }` silently changes world outcomes
(inf propagates, later steps may or may not trap on finiteness check
88) depending on which backend executed it — a determinism split between
interpreter and native.

## Suggested fix

Pick one, deliberately:

1. **Exclude `Div`/`Rem` from `eligible()`** until the ABI can express
   traps (cheapest, honest).
2. Emit a zero-guard that routes to a trap channel: e.g. change the
   native signature to carry an error out-param
   (`double f(const double* a, int32_t* err)`), set `err = 18` on a zero
   divisor, and have `call()` map that to `Err(EirInvalid, 18)` —
   matching the interpreter exactly (`+0.0`/`-0.0` both trap).

Whichever is chosen, extend the differential test with div/rem by
zero, `NaN`/`±inf` inputs, and negative-zero cases so the bit-identity
claim is actually bounded.
