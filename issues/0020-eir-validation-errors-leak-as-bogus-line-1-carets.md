# 0020 — EIR validation failures reach users as `error 4/6: unspecified compile error` with a caret at line 1 (Medium)

## Summary

Every EIR-level validation failure is constructed with
`reference/src/eir.rs:13 fn error(status, detail, offset)` where `offset`
is the **instruction index**, not a source byte offset.
`lang::diagnose` (`reference/src/lang/diagnostics.rs:139`) treats any
nonzero `byte_offset` as a position in the source file and renders a caret
at `line 1, column index+1` — i.e. somewhere inside `world { … }` — plus
the generic `detail_name` fallback `"unspecified compile error"`
(`reference/src/lang/diagnostics.rs:105`). No diagnostic is ever
`push_diag`'d for EIR errors (`push_diag` is only called from
`lower.rs:229/246` and `error_at`), so the real reason is discarded.

Two further inconsistencies surface on the same paths:

- **Format split**: errors with `byte_offset == 0` print
  `Invalid (NN): <name>` with no location at all, while nonzero offsets
  print `error NN: <name> --> …`. Docs Part 6 only documents the second
  style (docs/lang-usage.md:755+).
- **§2.9's arity promise is broken for the 1-arg math family**:
  docs/lang-usage.md:633 says "Builtins (arity checked; detail 59)", but
  the parser only arity-checks `min`/`max`/`if`/`random`/`inte`/`deriv`
  (`reference/src/lang/parser.rs:630-647`); `sin/cos/exp/…` fall through
  (`parser.rs:624`) and wrong-arity calls fail later in EIR with detail 4.

## Repro

```pwe
world { entity e { state=(x=1.0) } }
systems { update { on=e; dt=0.01  x = sin(1.0, 2.0) } }
```

```
$ pwe compile sin2.pwe -o sin2.pweb
error 4: unspecified compile error
  --> line 1, column 5
    |
   1 | world { entity e { state=(x=1.0) } }
    |     ^
```

| probe | source | actual output |
| --- | --- | --- |
| sin2 | `x = sin(1.0, 2.0)` | `error 4` @ line 1 col 5 (EIR: `Sin` needs 1 operand) |
| tan1 | `x = tan(1.0)` | `error 6` @ line 1 col 4 (unknown name → `Opcode::Nop` fallback `lower.rs:443`, yields no type) |
| sc2 | `let z = schedule(1.0, 0.5, 2.0, 1.0)` | `error 6` @ line 1 col 6 (see #19) |
| min1 | `x = min(1.0)` | `Invalid (59): call arity mismatch` — no caret, no line at all |

Note the caret columns (4, 5, 6) are just instruction indices + 1; they
point into the `world` header regardless of where the offending call is.

## Evidence

- `reference/src/eir.rs:13` — `error()` stores the instruction index as
  `byte_offset`.
- `reference/src/lang/diagnostics.rs:105` — `_ => "unspecified compile
  error"` fallback; `:139` — `diagnose()` renders offset as source
  position; `:43` — `push_diag` unused by the EIR validator.
- `reference/src/eir.rs:942` (schedule → detail 6), `:1075` (detail 6
  site), and the detail-4 operand checks (e.g. `Sin` above `eir.rs:930`).
- `reference/src/lang/parser.rs:624,630` — arity check present for
  `min|max` only; docs/lang-usage.md:633-641 promises arity checking for
  the whole builtin table and says "No `tan` (use `sin/cos`)".
- `reference/src/lang/lower.rs:443` — `_ => (Opcode::Nop, false)` swallows
  unknown function names.

## Impact

Users get a location that is always wrong (line 1 of their file), a
message that says nothing, and — for the documented `sin(…)` family — a
different detail code than the docs promise. `min(1.0)` vs `sin(1.0,2.0)`
on the same mistake class print in two incompatible formats. This is the
output of the compiler's most common failure path (any EIR invariant).

## Suggested fix

1. Track a source span per instruction during lowering (or record the
   enclosing rule's `byte_offset` in the `Error`) so EIR failures render at
   the offending rule with a real message ("`sin` expects 1 argument, got
   2", "`tan` is not a builtin and no `funcs` function named `tan` exists",
   "`schedule` yields no value").
2. Route EIR failures through `error_at`/`push_diag` so the message
   survives; keep `detail_name` only as final fallback.
3. Add parser-side arity checks for the 1-arg/2-arg math builtins so
   §2.9's detail-59 contract holds.
4. Reconcile `Invalid (NN)` vs `error NN` with the Part 6 format.
