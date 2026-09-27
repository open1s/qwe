# 0002 — Unknown identifiers and refs silently read 0.0 (High)

## Summary

A typo in an expression is never a diagnostic: an undeclared bare name and an
unknown `@entity.slot` both lower to a read that yields 0.0. The detail-code
table (48–80) has no "unknown identifier" code at all.

## Evidence

- Bare name: `reference/src/lang/mod.rs:1316-1331` — comment *"An undeclared
  bare name: read 0.0 (via an unset parameter)"*; it emits `ReadView` against
  `param_component_id(name)` for a name that was never declared.
- `@ent.sN` / `@ent.prop`: `reference/src/lang/mod.rs:1247-1253` —
  `entity_map.get(name).copied().unwrap_or(u128::MAX)` then
  `ref_regs.get(…).unwrap_or(0)` → register id 0, never allocated by this
  lowering pass (all `next_id` seeds start ≥ 1).
- Named cross-entity refs: `reference/src/lang/mod.rs:1305-1314` —
  `unwrap_or(u128::MAX)` / `unwrap_or(0)`.
- Collection silently skips them: `reference/src/lang/mod.rs:884-889`
  (`if let Some(id) = id { … }`), and `Expr::Ref` records any name without
  validating it (`mod.rs:873-874`).
- Detail 62 ("unknown entity or channel name") is raised only for structural
  positions — `on =`, `chan =`, field names (`mod.rs:4360,4478,4540,4584,4652…`)
  — never inside expressions.
- Documented as intended behavior: `docs/lang-usage.md:624` ("An unresolved bare
  name reads 0.0"), `:460` ("the LHS names no slot" → rule silently does
  nothing).

## Impact

Contradicts AGENTS.md §13 ("avoid silent fallback"). A mistyped slot name turns
into constant-zero physics with a successful compile and a plausible-looking
run; the only symptom is "my body never moves". The register-0 fallback
(`mod.rs:1249`) additionally makes unknown `@refs` behavior accidental rather
than designed (default value or a later runtime error), not a specified 0.0.

## Fix

1. Add a detail code (e.g. 81) "unknown identifier in expression" and emit it
   for: undeclared bare names, `@entity` not in `entity_map`, `@entity.slot` not
   in that entity's layout, `prop` not a known property.
2. Offer an escape hatch for intentional dynamic lookups — an explicit
   `defined(x)` / `@ent?.slot` — so optional references stay possible.
3. Keep a `--lenient` mode that restores 0.0-fallback for exploratory work.

## Labels

diagnostics, language-semantics, determinism, high
