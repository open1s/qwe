# 0037 — WGSL `Select` condition uses IEEE `!= 0.0f`; interpreter uses the bit-test → `-0.0` flips the branch (and the CPU oracle shares the bug) (Medium)

## Summary

Same class as #34, now in the new WGSL device backend
(`reference/src/wgsl.rs`, commit `aee7ae30`):

```rust
// reference/src/wgsl.rs:233-236 (emitted shader)
Opcode::Select => format!(
    "  r[{res}] = select(r[{}], r[{}], r[{}] != 0.0f);\n",
    o[2], o[1], o[0]
),
```

```rust
// reference/src/wgsl.rs:331-337 (CPU oracle eval_f32)
Opcode::Select => {
    if get(0) != 0.0 { get(1) } else { get(2) }
}
```

The reference interpreter is **bit-level**: `as_u64(cond) != 0`
(`eir.rs:1331` CondBr, `:2548`/`:2550` Select) — `as_u64(-0.0) =
0x8000_0000_0000_0000` → nonzero → **true**. IEEE `-0.0 != 0.0` is
**false**. Both the emitted shader and the CPU oracle therefore take
the opposite branch from the interpreter whenever a condition is
negative zero.

Because the oracle shares the bug, the module's own verification axis
("a CPU f32 oracle … is checked against the interpreter within f32
tolerance") **cannot catch it** — it would fail outright at `-0.0`
inputs, but the test grid (`wgsl.rs:435`, `a = i * 0.4 - 6.0`) never
produces one.

## Evidence

Program (single-arg map kernel — `eligible()` accepts it):

```
world { gravity=(0,0,0) entity e { state=(c=0.0, x=0.0) } }
funcs { f(c) { if(c, 11.0, 22.0) } }
systems { update { on=e; dt=1.0  x = f(c) } }
```

Probe at commit `aee7ae30` (`LangRuntime::step_jit` vs `eval_f32`;
emitted line asserted from `emit_compute_shader`):

| c        | interpreter | `eval_f32` oracle (= shader semantics) | result    |
|----------|-------------|------------------------------------------|-----------|
| `0.0`    | 22.0        | 22.0                                     | match     |
| `-0.0`   | **11.0**    | **22.0**                                 | **diverges** |
| `1.0`    | 11.0        | 11.0                                     | match     |
| `NaN`    | 11.0        | 11.0                                     | match     |

Emitted shader line: `r[4] = select(r[3], r[2], r[1] != 0.0f);`

(NaN happens to agree: both bit-test and `!= 0.0f` are true for NaN.
Only negative zero flips.)

## Coverage gap

`f32_oracle_tracks_the_interpreter_within_tolerance` (wgsl.rs:426)
exercises 32 benign points and compares **oracle vs interpreter** —
never the emitted WGSL text. The input grid contains no `-0.0`
condition and no control-flow-free Select edge. #34's fix for the
native backend (`pwe_truthy`, reference/src/native.rs) did not
propagate to this backend.

## Suggested fix

1. Emit a bit-test condition, e.g.
   `select(r[f], r[t], bitcast<u32>(r[c]) != 0u)` (WGSL has no NaN/±0
   ambiguity at the bit level).
2. Fix the oracle (`eval_f32`) to match (`get(0).to_bits() != 0`).
3. Add `0.0`, `-0.0`, `1.0`, `NaN` condition inputs to the oracle ↔
   interpreter test (the same matrix #34 got for the native backend).

## Scope note

Trap-vs-value semantics (div/rem by zero → device returns inf/NaN,
interpreter traps `EirInvalid 18`) is the separate umbrella tracked in
#36; `eligible()` also admits always-trapping kernels such as
`f(a) { a / 0.0 + a }`.

## Environment

commit `aee7ae30` (main), macOS arm64; probe via
`pwe_reference::wgsl::{emit_compute_shader, eval_f32}` +
`LangRuntime::step_jit`.
