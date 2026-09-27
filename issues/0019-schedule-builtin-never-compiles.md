# 0019 — The documented `schedule()` builtin can never compile (High)

## Summary

`schedule(gate, delay, kind, payload)` is documented (README:161,
docs/lang-usage.md:372, docs/lang-usage.md:638) but **every parseable call
fails to compile**. Lowering emits the `ScheduleEvent` instruction with a
fresh result register and an `F64` result annotation
(`reference/src/lang/lower.rs:547-569`), while the validator's
`ScheduleEvent` arm deliberately computes `declared_type = None` — the
opcode's own spec says "**Yields nothing**"
(`reference/src/eir.rs:246-249`). With a nonzero `result_id`, the generic
result-record check then fires:

```rust
// reference/src/eir.rs:1075
let ty = declared_type.ok_or(error(Status::EirInvalid, 6, index))?;
```

so every call dies with detail 6 ("unspecified compile error") whose
"source location" is really an instruction index rendered as line 1
(see #20). The alternative spelling — a string `kind` — does not even
parse: the expression grammar has no string literals.

No test or example anywhere in the repo calls `schedule(` (only a comment
in `lower.rs:549`), so CI has never exercised it.

## Repro

```pwe
world { entity e { state=(x=1.0) } }
systems { update { on=e; dt=0.01  let z = schedule(1.0, 0.5, 2.0, 1.0) } }
```

```
$ pwe compile sc2.pwe -o sc2.pweb
error 6: unspecified compile error
  --> line 1, column 6
    |
   1 | world { entity e { state=(x=1.0) } }
    |      ^
```

String `kind` form (`let z = schedule(1.0, 0.5, "k", 1.0)`):

```
$ pwe compile sc.pwe -o sc.pweb
error 60: failed to parse program:  --> 2:62
  |
2 | systems { update { on=e; dt=0.01  let z = schedule(1.0, 0.5, "k", 1.0) } }
  |                                                              ^---
  |
  = expected factor
```

Contrast: `at(T)` / `periodic(P)` share the same lowering arm but their
validator returns `Some(ValueType::F64)` (`reference/src/eir.rs:931-941`)
and compile fine.

## Evidence

- `reference/src/lang/lower.rs:547-569` — `"at" | "periodic" | "schedule"`
  arm: allocates `out_reg`, pushes `instr(op, out_reg, Some(F64), …)`.
- `reference/src/eir.rs:246-249` — `ScheduleEvent = 223` doc: "Yields
  nothing." `reference/src/eir.rs:942-948` — verify returns `None`.
- `reference/src/eir.rs:1075` — `declared_type.ok_or(detail 6)` when
  `result_id != 0`.
- Docs: README.md:161, docs/lang-usage.md:372, docs/lang-usage.md:638.
- Coverage: `rg 'schedule\(' --glob '!docs' --glob '!book'` → only
  `lower.rs:549` comment; no test/example/fixture calls it.

## Impact

A headline "Events & scheduling" feature is completely unusable, and the
failure mode points users at line 1 of their file with "unspecified
compile error" — the worst possible diagnostic. The `kind` representation
(number vs string) is also unspecified in the docs, so even after the type
mismatch is fixed the intended call shape is unclear.

## Suggested fix

Decide the contract first:

1. If `schedule` yields nothing (current spec), lowering must not allocate
   a result register — and `let z = schedule(…)` / `x = schedule(…)` should
   be a clear compile error ("`schedule` returns no value; call it as a
   statement"), not detail 6 at line 1.
2. If it should yield a handle/time (like `at`), make the validator return
   the matching type and align the `Some(F64)` annotation with it.

Also specify `kind` in the docs (string literal needs grammar support), and
add a test that compiles and runs `schedule(…)` in both call positions.
