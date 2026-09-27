# 0009 — Malformed unit annotations silently drop dimensional checking (Low)

## Summary

A unit annotation that fails to parse is ignored without a diagnostic, so a
typo silently turns dimensional checking off for that declaration — the check
the user wrote the annotation to get.

## Evidence

- `reference/src/lang/parser.rs:284-296` — trailing unit is parsed with
  `if let Ok(d) = …parse::<crate::units::Dim>() { decl.param_units.insert(…) }`.
  The `Err` arm does nothing: no `push_diag`, no `error(…, 77)`.
- Same pattern for state annotations (units are compile-time only,
  `docs/lang-usage.md:640-641`, `:447-453`), which promise "mismatches are
  detail 77" — a promise that is void the moment the annotation is unparseable.
- Detail 77 exists for genuine mismatches
  (`reference/src/lang/diagnostics.rs:98`).

## Impact

`state = (x = 1.0 [meter])` compiles, checks nothing, and looks identical to a
correctly annotated program. Silent failure of the safety mechanism itself —
the same class as 0002, on the checking path.

## Fix

Emit a new detail code (e.g. 82 "invalid unit annotation") on parse failure
instead of ignoring it; keep accepting only base units
`m kg s A K mol cd` with `* / ^` as documented (`docs/lang-usage.md:499`).

## Labels

diagnostics, units, low
