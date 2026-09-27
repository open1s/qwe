# 0016 — Most dimensional-mismatch (77) errors print without any source location (Low)

## Summary

`detail 77` diagnostics are emitted from two different paths with very
different quality:

- `error_at(… 77, sys.byte_offset, "rule `…` is not dimensionally consistent: …")`
  for rule/slot and `when`-gate mismatches — these record a diagnostic and the
  CLI renders a caret + the specific rule text.
- a bare `error(Status::Invalid, 77)` closure inside `DimEnv::of_expr` for any
  mismatch found *within* an expression (e.g. `x + t`, `vx + dt*vx`) — **no
  byte offset, no message, no recorded diagnostic**, so `pwe compile` prints
  only the generic fallback:

```
$ pwe compile u2_unit_control_bad.pwe -o u2.pweb
Invalid (77): dimension mismatch (see declared units)
```

with no `-->` caret, no line number, and no indication of *which rule* or
*which units* conflicted — while a parse error two lines earlier renders a
full caret block. The expression-internal path is the common one (any
mismatch between two named operands surfaces inside `of_expr`'s `unify`
calls first).

## Repro

```pwe
world { gravity = (0, 0, 0)
  params { k = 4.0 [1/s^2] }
  entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
}
systems {
  update { on = e; dt = 0.1 [s]
    x = x + dt*(  0.0 - k * x )
    vx = vx + dt*(  vx ) }
}
```

```
$ pwe compile u2_unit_control_bad.pwe -o u2.pweb
Invalid (77): dimension mismatch (see declared units)
```

Contrast a `detail 55` from the same compile stage, which renders with caret
and message:

```
$ pwe compile m2_let_only_update.pwe -o m2.pweb
error 55: update system has no rules; add `slot = <expr>`
  --> line 6, column 3
```

## Evidence

- `reference/src/lang/compile.rs:1408` — `let err = || error(Status::Invalid, 77);`
  used throughout `DimEnv::of_expr` (e.g. `Add`/`Sub` unification failures);
  plain `error()` carries `byte_offset = 0` and does not `push_diag`
  (`reference/src/lang/diagnostics.rs:54-67` — only `error_at` pushes).
- `reference/src/lang/diagnostics.rs:134-155` — `diagnose()` falls back to
  `detail_name(err.detail)` when no diagnostic was recorded and offset is 0,
  discarding any richer information.
- `reference/src/lang/compile.rs:1564-1568, 1631-1640, 1678-1688` — the
  `error_at` sites that *do* produce good output, proving the rendering path
  exists.

## Impact

The most frequent class of dimension errors is the one users cannot act on:
no rule text, no line, no expected/got units. Users must bisect rules by hand.

## Suggested fix

Give `of_expr` the source offset of the enclosing rule (or of the offending
sub-expression) and record a `Diagnostic` with the same message format as the
`error_at` sites — `unify` failures should say `got `m`, expected `m/s``.
