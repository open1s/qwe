# 0022 — User `funcs` calls are never arity-checked: extra arguments silently ignored, missing arguments fail at runtime with undocumented detail 17 (Medium)

## Summary

Builtins advertise arity checking (docs/lang-usage.md:633 "Builtins
(arity checked; detail 59)"), but **user-defined `funcs` calls are not
checked at all**. The call site lowering resolves the name and pushes
operands without ever comparing against the definition's parameter list:

```rust
// reference/src/lang/lower.rs:394-411
if let Some(target) = qualified.or_else(|| ctx.func_ids.get(*name).copied()) {
    let mut operands = vec![target as u32];
    for a in args { operands.push(lower_expr(a, ctx, next_id, out)); }
    out.push(instr(Opcode::Call, out_reg, Some(ValueType::F64), operands, …));
```

`func_ids` is just `name → id` (`reference/src/lang/compile.rs:1928`),
even though the parser does collect each function's parameter names
(`reference/src/lang/parser.rs:1230-1237`) — the arity exists at compile
time and is thrown away.

Consequences:

- **Too many arguments**: silently ignored; the call runs with the
  declared parameters only.
- **Too few arguments**: compiles clean, then the EIR stack is short at
  execution and the run dies with `PWE EirInvalid (17) at 0` —
  "missing stack value" (`reference/src/eir.rs:1311`), a detail code with
  no entry in `detail_name` and no row in the docs Part 6 table (which
  starts at 48, docs/lang-usage.md:798).

## Repro

Too many:

```pwe
world { entity e { state=(x=1.0) } }
funcs { f(a) { a * 2.0 } }
systems { update { on=e; dt=0.01  x = f(1.0, 2.0) } }
```

```
$ pwe compile uf_over.pwe -o uf_over.pweb
compiled uf_over.pwe -> uf_over.pweb
$ pwe run uf_over.pweb --steps 10
  #1   e   pos = ( 2.0000, …)  state = [2.0000]      ← 2.0 = f(1.0); `2.0` vanished
```

Too few:

```pwe
world { entity e { state=(x=1.0) } }
funcs { f(a, b) { a * b } }
systems { update { on=e; dt=0.01  x = f(5.0) } }
```

```
$ pwe compile uf_under.pwe -o uf_under.pweb
compiled uf_under.pwe -> uf_under.pweb
$ pwe run uf_under.pweb --steps 10
pwe: step 0 failed: PWE EirInvalid (17) at 0
```

## Evidence

- `reference/src/lang/lower.rs:394-411` — call lowering, no arity data.
- `rg 'arity|params.len' reference/src/lang/lower.rs` → no matches;
  `reference/src/lang/parser.rs:630-647` shows the builtin check that was
  never extended to user functions.
- `reference/src/lang/compile.rs:1928` — `func_ids.insert(name, id)` (no
  signature); `reference/src/lang/parser.rs:1230-1237` — params parsed
  into a `Vec` at definition.
- `reference/src/eir.rs:1311` — detail 17 emitted on stack underflow;
  absent from `detail_name` (`reference/src/lang/diagnostics.rs:70-105`)
  and from docs Part 6 (docs/lang-usage.md:795-800).

## Impact

A typo'd call either silently computes with missing inputs (same class of
silent wrongness as #2/#12) or blows up at step 0 with an undocumented
code. This is the exact failure mode detail 59 was invented to prevent —
it just never got wired up for `funcs`, which is where arity mistakes are
most likely (refactoring a function's signature).

## Suggested fix

Carry `params.len()` (or the full signature) next to `func_ids` — the
parser already produces it — and at the call site compare
`args.len()`:

```rust
if args.len() != expected {
    return Err(error_at(Status::Invalid, 59, call_offset,
        format!("`{name}` expects {expected} argument(s), got {}", args.len())));
}
```

Separately, add detail 17 to the Part 6 table regardless (it is reachable
from more than this bug), and consider a clearer runtime message for
stack underflow in user calls.
