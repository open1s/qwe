# 0034 — Native `CondBr`/`Select` conditions use IEEE `!= 0.0`; interpreter uses bit-test `as_u64 != 0` (−0.0 diverges); differential-test coverage gap (Medium)

## Summary

Interpreter condition semantics are **bit-level**:

```rust
// reference/src/eir.rs:1331 (CondBr), :2548 (Select), :1041 (eval_const)
pcs[depth - 1] = if as_u64(cond) != 0 { ... }
```

`as_u64(-0.0)` is `0x8000_0000_0000_0000` → **nonzero → true**.

The native backend (`reference/src/native.rs`, commit `7cc2d5a5`)
emits IEEE comparisons:

```c
if (r[c] != 0.0) goto Lt; else goto Lf;   // CondBr
r[res] = (r[c] != 0.0) ? r[t] : r[f];     // Select
```

`-0.0 != 0.0` is **false** in IEEE → native takes the *other* branch
than the reference interpreter whenever a condition evaluates to
negative zero.

## Evidence

Code-level, both sides cited above (no lang probe: the exact
reachability of `Select`/`CondBr` with a `-0.0` condition depends on
which lowering sites can produce it — `lower.rs:339,451,481,968`,
`systems.rs:443,489` emit `Select`; every `CondBr` flows through the
same test). The divergence is unconditional in the generated code; only
its reachability varies.

Contributing root cause: the differential test
(`native_pure_function_matches_interpreter`) covers exactly one
function shape — `a*a + b*b + sin(a)` — over 64 benign grid inputs. It
missed #32 (div-by-zero), #33 (sign of zero) and this condition
semantics difference, while the module docs claim general
"bit-identical" verification.

## Impact

Branch divergence flips control flow: which arm's value lands in world
state, whether a loop guard proceeds. Combined with AGENTS §4
(interpreter ≡ reference) this is a silent-wrongness class bug the
moment branches are natively lowered. The unbounded "bit-identical"
claim overstates verification and hid three divergences.

## Suggested fix

- Emit the interpreter's exact test in C (reinterpret the `double`'s
  bits as `uint64_t` via a union/memcpy and compare `!= 0`), **or**
  decide the bit-test is the anomaly (RFC wording?) and change the
  interpreter + `eval_const` consistently — RFC decides, tests follow.
- Harden the differential test into a small **matrix**: op coverage
  (every `eligible()` opcode), input classes (`0.0`, `-0.0`, `NaN`,
  `±inf`, `DBL_MIN`, subnormals), and control flow (nested calls,
  `CondBr`/`Select` both arms). Assert bit-equality per case.
