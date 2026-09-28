# 0036 — f64 comparisons with NaN: interpreter traps `EirInvalid 18`, native returns IEEE result — tier divergence and unspecified semantics (Medium)

## Summary

The reference interpreter traps on **any f64 comparison with a NaN
operand**; the native backend returns the IEEE result. `pwe run`
(interpreter tier) and `NativeProgram::call` therefore disagree on the
same program, and no RFC defines which behavior is correct.

Interpreter — `compare()` orders f64 with `partial_cmp`:

```rust
// reference/src/eir.rs:2768
fn compare(op: Opcode, a: Immediate, b: Immediate) -> Option<Immediate> {
    ...
    (F64(a), F64(b)) => a.partial_cmp(&b),   // NaN -> None
```

`None` is converted to a hard trap at both execution and const-fold
sites:

```rust
// reference/src/eir.rs:1370 (execute loop)
| Opcode::Ge => compare(instruction.opcode, a, b)
      .ok_or(error(Status::EirInvalid, 18, 0))?,
// reference/src/eir.rs:2499 (eval_const) -> fold skipped -> runtime traps
```

Native — `cmp_op` (reference/src/native.rs:598) emits plain C
operators, which are IEEE-754 and never trap:

```c
r[res] = (r[a] < r[b]) ? 1.0 : 0.0;   // NaN < x  -> 0.0, no trap
```

## Evidence

Minimal program (`lt` probe, commit `bb7cf78a`, macOS):

```
world { gravity=(0,0,0) entity e { state=(a=0.0,b=1.0,x=0.0) } }
funcs { f(a, b) { a < b } }
systems { update { on=e; dt=1.0  x = f(a, b) } }
```

Direct comparison probe (`np.call(ID, &[a,b])` vs `LangRuntime::step_jit`):

| call              | interpreter            | native              | result     |
|-------------------|------------------------|---------------------|------------|
| `lt(0.0, 1.0)`    | `Ok(1.0)`              | `Ok(1.0)`           | match      |
| `lt(-0.0, 0.0)`   | `Ok(0.0)`              | `Ok(0.0)`           | match      |
| `lt(NaN, 1.0)`    | **`Err(EirInvalid d18)`** | `Ok(0.0)`         | **diverges** |
| `lt(1.0, NaN)`    | **`Err(EirInvalid d18)`** | `Ok(0.0)`         | **diverges** |

Reachability through plain lang control flow — every `if <cond> { … }`
lowers its condition through `truthy()`, which emits `Ne(reg, 0.0)`
(reference/src/lang/lower.rs:942-954), so **a NaN condition traps the
interpreter even without a user-written comparison**:

```
funcs { f(flag, v) { if flag { return v } return 0.0 - v } }
call with flag = NaN:
  interp = Err(EirInvalid d18)   native = Ok(3.0)      => DIVERGES
  (flag ∈ {0.0, -0.0, 1.0, -2.5}: both tiers MATCH — #34's bit-test fix verified)
```

The interpreter trap also fires on pure-interpreter runs (`pwe run`),
i.e. it is not only a native-backend question, and it reports
`byte_offset = 0` (no caret), same diagnostic class as #20/#21.

## Coverage gap

The fixed differential matrix (`native_pure_matches_interpreter_on_a_matrix`,
reference/src/native.rs:833) now covers div/sign/select/`%` over a
9-value input matrix **including NaN — but no body performs a
comparison**, so `Eq/Ne/Lt/Le/Gt/Ge` × NaN remains untested. Bodies
`a < b` and a control-flow `if flag { return v }` would have caught
this. (Related minor gap: no matrix body produces a `CondBr`; the
CondBr bit-test fix was probe-verified externally only.)

## Spec status

No RFC defines f64 comparison semantics with NaN:

* RFC-0021 defines the divide-by-zero trap only;
* RFC-0019/RFC-0027 mention NaN solely for canonical hashing;
* `compare()` carries no doc comment stating that unordered results
  are meant to be traps.

The current interpreter behavior (`partial_cmp` → trap) looks like an
incidental consequence of Rust's `partial_cmp` rather than a designed
contract; it is also non-IEEE (every mainstream VM — WASM, JVM, JS —
returns unordered results without trapping).

## Suggested fix

1. Decide the semantics: **IEEE-754 unordered results** (recommended —
   matches every other execution environment and keeps `arith`, which
   already propagates NaN without trapping, consistent) *or* a
   documented defined trap like RFC-0021's divide-by-zero rule.
2. Record the decision in the EIR execution contract (RFC-0021 table
   or the applicable RFC) — AGENTS §16.
3. Align both tiers: if IEEE, make `compare()` return the ordered/
   unordered result instead of `None` (F64 `Ne(NaN, x) = true`,
   `Lt/Le/Gt/Ge = false`, `Eq = false`); if trap, make the native codegen
   emit the same guard as `div_guard`.
4. Add comparison bodies (`a < b`, `a == a`, `if flag { return … }`)
   to the differential matrix with the existing NaN inputs, and check
   the const-fold path (`eval_const` at eir.rs:2499) matches.
5. Give the resulting diagnostic a real `byte_offset` if a trap is
   kept (currently `offset=0`, no caret).

## Environment

* commit `bb7cf78a` (main), macOS arm64, `cc` = Apple clang
* probe: `NativeProgram::compile` + `call` vs `LangRuntime::compile` +
  `step_jit`, fresh process per mode
