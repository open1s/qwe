# 0038 — Emitted WGSL builtins diverge from the CPU oracle at edge inputs; the emitter is never executed or validated, and the promised `naga` check is absent (Medium)

## Summary

`reference/src/wgsl.rs` (commit `aee7ae30`) claims two-way
verification: a structural validator plus "a CPU f32 oracle (the same
lowering evaluated on the CPU) … checked against the interpreter
within f32 tolerance. `naga` validation is added as a dev-dependency
where available."

That structure verifies the **oracle**, never the **emitted shader**.
Where the emitted WGSL builtin has different semantics from the Rust
expression in `eval_f32`, the tests stay green while the shader is
wrong. Four confirmed cases (a fifth, `Select`, is #37):

## Evidence

All probes: `emit_compute_shader` output inspected, `eval_f32`
executed, interpreter (`step_jit`) executed, WGSL semantics cited
from the WGSL spec (w3.org TR) / quick references.

### 1. `sign` — WGSL returns 0 for zero; oracle/interpreter use `signum`

```
funcs { f(a) { sign(a) } }        // eligible (argc 1, straight-line)
emitted: r[2] = sign(r[1]);
```

| input  | interpreter (`f64::signum`) | `eval_f32` (`f32::signum`) | WGSL `sign()` per spec | oracle == shader |
|--------|------------------------------|-----------------------------|------------------------|------------------|
| `0.0`  | 1.0                          | 1.0                         | **0.0**                | **no**           |
| `-0.0` | -1.0                         | -1.0                        | **0.0/-0.0**           | **no**           |
| `1.5`  | 1.0                          | 1.0                         | 1.0                    | yes              |
| `-2.5` | -1.0                         | -1.0                        | -1.0                   | yes              |
| `NaN`  | NaN                          | NaN                         | NaN                    | yes              |

WGSL: "Returns `1.0` when `e > 0`, `0.0` when `e = 0`, and `-1.0`
when `e < 0`" (WGSL numeric quick reference; the WESL reference
implementation returns `n` unchanged when `n.is_zero()`).

### 2. `round` — WGSL is ties-to-even; Rust/interpreter are half-away-from-zero

```
funcs { f(a) { round(a) } }
emitted: r[2] = round(r[1]);
```

| input  | interpreter / `eval_f32` (`f32::round`) | WGSL `round()` per spec | oracle == shader |
|--------|-------------------------------------------|-------------------------|------------------|
| `0.5`  | 1.0                                       | **0.0**                 | **no**           |
| `2.5`  | 3.0                                       | **2.0**                 | **no**           |
| `-2.5` | -3.0                                      | **-2.0**                | **no**           |
| `1.5`  | 2.0                                       | 2.0                     | yes              |
| `0.4`  | 0.0                                       | 0.0                     | yes              |

WGSL: ties round "to the nearest even integer" (`round(2.5) = 2.0`,
`round(3.5) = 4.0`, `round(-2.5) = -2.0`).

### 3. `hypot` — shader formula overflows f32 where the oracle does not

```
funcs { f(a) { hypot(a, a) } }
emitted: r[2] = sqrt(r[1] * r[1] + r[1] * r[1]);   // wgsl.rs:219-222
```

| input    | interpreter | `eval_f32` (`f32::hypot`) | shader `sqrt(a*a+a*a)` | oracle == shader |
|----------|-------------|-----------------------------|------------------------|------------------|
| `3.0`    | 4.2426…     | 4.2426…                     | 4.2426…                | yes              |
| `2.0e20` | 2.8284e20   | 2.8284e20                   | **inf**                | **no**           |
| `1.0e20` | 1.4142e20   | 1.4142e20                   | **inf**                | **no**           |

`a * a` overflows f32 (`3.4e38`) long before `hypot` would.

### 4. `Rem` — the `trunc` emulation is not `fmod`

```
funcs { f(a) { … a % b … } }   // EIR Rem = C fmod (eir.rs divrem: "fmod semantics")
emitted (wgsl.rs:214-217):
  r[res] = r[a] - r[b] * trunc(r[a] / r[b]);
oracle: get(0) % get(1)   // Rust f32 % = libm fmodf — exact
```

A 4.1M-pair sweep (random f32 bit patterns + structured pairs at
quotient boundaries `a = b*n` and ±1 ulp around them) found
**1,851,047 divergences** between the shader formula and `fmodf`.
Catastrophic examples (not just ulp noise):

```
a=-9.357623e24  b=1.3386294e-21 : shader=inf   fmod=-6.107837e-22
a=6.5286154e9   b=7.617948e-38  : shader=-inf  fmod=1.3387114e-38
a=8.70385e-36   b=2.2155692e-36 : shader=2.0571424e-36 fmod=2.0571422e-36
```

Root cause: `f32(a/b)` rounds (relative error 2⁻²⁴), so `trunc` picks
the wrong integer part whenever the exact quotient is large or within
half an ULP of an integer; the large-quotient case then also suffers
catastrophic cancellation in `a - b*n`.

### 5. Doc claim: `naga` validation is not in the tree

Module doc (wgsl.rs:17): "`naga` validation is added as a
dev-dependency where available." — `naga` (and `wgpu`) appear
**nowhere** in `Cargo.toml` / `reference/Cargo.toml`, no test invokes
it, and `validate_shader` (wgsl.rs:265) is a bracket-balancer plus
substring check that cannot see any of the semantic bugs above. The
three wgsl tests pass (`cargo test wgsl` → 3 passed) — green tests
around an emitter that is never parsed by a real WGSL validator.

`emit_compute_shader`/`WGSL_TARGET` still have zero callers outside
`wgsl.rs` (not wired — consistent with the module's stated scope and
RFC-0027), so the divergences are latent; the verification claims are
what is wrong today.

## Suggested fix

1. Add `naga` as a dev-dependency and validate every emitted shader
   in the tests (it is pure Rust, CPU-only — no GPU needed); drop or
   deliver the doc claim.
2. Make the emitter match the oracle (and interpreter at f32
   granularity) on edge inputs:
   * `sign`: emit an explicit form reproducing `signum`
     (zero → ±1), e.g. `select(-1.0, 1.0, …)` with a zero/NaN guard;
   * `round`: emit `sign(x) * floor(abs(x) + 0.5)`-style
     half-away-from-zero (watch `-0.0`);
   * `hypot`: scale-by-max-abs form to avoid overflow
     (WGSL has no `hypot` builtin);
   * `Rem`: a `fmod`-equivalent that survives large quotients (the
     `trunc` form is not sufficient); document the chosen
     approximation and test it against `fmodf`.
3. Extend the oracle ↔ interpreter matrix with edge classes —
   `±0.0`, subnormals, `±inf`, `NaN`, halfway values, magnitudes near
   f32 overflow — mirroring the input matrix the native backend got
   in #34's fix.
4. Where an approximation cannot match, state the divergence in the
   module doc explicitly (input-class table), rather than a blanket
   "within f32 tolerance".

## Environment

commit `aee7ae30` (main), macOS arm64; probes via
`pwe_reference::wgsl::{emit_compute_shader, eval_f32, eligible}` +
`LangRuntime::step_jit`; WGSL semantics from the W3C WGSL
specification and WGSL builtin references.
