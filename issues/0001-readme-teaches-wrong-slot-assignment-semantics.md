# 0001 — README teaches `slot = expr` as Euler integration (High)

## Summary

The READMEs describe the language's central operator with the wrong semantics.
`slot = expr` is a **plain assignment**; only `inte slot = rate` and `slot += …`
integrate.

## Evidence

- `README.md:150`: "*Rules are equations.* `slot = expr` means `slot += dt·expr`
  (Euler); `rk4` …"
- `README-ZH.md:144`: same claim in Chinese.
- Reality, lowering side: plain `ident = expr` reaches `decl.assigns`
  (`reference/src/lang/parser.rs:245-277`) → plain write; `ode_stmt` /
  `add_stmt` (`inte`, `+=`) reach `decl.update`
  (`reference/src/lang/parser.rs:212-226`) → integration.
- Reality, spec side: `docs/lang-usage.md:152,159-164,683` — "`slot = expr`
  assigns"; integrate with `slot = slot + inte(rate)` or `inte slot = rate`.
- Residue of the old increment semantics survives in Appendix B:
  `docs/lang-usage.md:877` — "Rules are `slot = <expression>`; assign with
  `slot = (target - slot)`" — correct only under increment semantics; under the
  real semantics it writes the error term into the slot.

## Impact

The flagship README is the first thing a user reads. Following it produces
integrators that never integrate (a body that "never moves"), which is already
the #1 entry in the common-mistakes table (`docs/lang-usage.md:459-461`).

## Fix

1. Rewrite `README.md:150` / `README-ZH.md:144` to match `docs/lang-usage.md`
   §L1: assignment by default; integration via `inte` / `+=` / explicit `dt`.
2. Reword `docs/lang-usage.md:877` to the correct idiom, e.g. "to approach a
   target: `slot = slot + k*(target - slot)*dt` (or `inte slot = …`)".
3. Grep for other copies of the claim (wiki `The-PWE-Language`, `docs/book/*`).

## Labels

docs, language-semantics, high
