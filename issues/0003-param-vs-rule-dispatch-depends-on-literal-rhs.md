# 0003 — Param-vs-rule dispatch depends on system kind and literal RHS (High)

## Summary

Inside a system block, whether `name = …` configures a **parameter** or writes
a **state slot rule** is decided per system kind *and* on whether the RHS is a
scalar literal. There is no diagnostic when a slot name collides with a
parameter name.

## Evidence

- Kind-keyed parameter names: `reference/src/lang/parser.rs:159-181`
  (`numeric_param_keys`), e.g. `update|rk4 → ["dt","every","substeps"]`,
  `wall → ["x","z","y_min","restitution"]`, `damping → ["factor"]`,
  `joint → ["length","stiffness","damping","iterations"]`.
- Dispatch: `reference/src/lang/parser.rs:245-277` — RHS parsed as `expr`; if
  `parse_scalar_number` succeeds **and** the key is in the kind's param list →
  `decl.params` (parameter), otherwise → `decl.assigns` (rule).
- Spec: `docs/lang-usage.md:179-181` ("A numeric parameter is recognised by
  name for the system kind… while `x = 1.0` is a rule"), `:686-687`.

## Concrete failure modes

1. Same key, different meaning by RHS shape:
   `update { dt = 0.01 }` sets the timestep; `update { dt = x*2 }` is a rule
   for a slot named `dt` (literal parses, expression does not).
2. An entity with `state = (x = 0, factor = 0)` cannot write `x` from a `wall`
   system with a literal RHS (`x = 1.0` becomes the wall bound), but
   `x = 0.0 + 1.0` *does* write the slot — semantics hinge on literal-ness.
3. Slot name colliding with `dt`/`every`/`substeps` in `update` is unwritable
   in its literal form, silently (no rule, no error) — it lands in
   `decl.params` and the slot keeps its old value.

## Impact

Context-dependent, literal-dependent name resolution with zero feedback. Two
programs differing only in `1.0` vs `0.5 + 0.5` can compile to different
program shapes.

## Fix

1. At build time, reject (or warn) when a statement key is both a kind param
   and a resolvable slot of a targeted entity — error with both interpretations
   named, suggesting `s[i] = …` or qualification.
2. Prefer requiring params in a distinct syntactic position (e.g.
   `update dt = 0.01 { … }` header vs rules in the body), which removes the
   ambiguity entirely; if that is too invasive, at least make the param-vs-rule
   decision not depend on the RHS being a literal.

## Labels

language-design, diagnostics, high
