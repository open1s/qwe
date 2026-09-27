# 0023 — `rk4` silently drops plain assignment rules: wrong physics, self-reproducing error-55 advice, detail 73 not enforced on the assign path (High)

## Summary

The `rk4` compile arm builds its rule set **exclusively from
`s.update`** — the `inte slot = rate` / `+=` statements
(`reference/src/lang/compile.rs:286-306`). Plain assignments
(`slot = expr`, `s[i] = expr`) land in `s.assigns`
(`reference/src/lang/parser.rs` `store_param` → `decl.assigns`), and
`Rk4System` has **no `assigns` field at all**
(`reference/src/lang/systems.rs:559-581`); the construction at
`compile.rs:319-337` cannot pass one. Assignments are therefore never
lowered for `rk4` — silently, or behind an error whose advice cannot work.

Three observable symptoms:

**(a) Assignment-only `rk4` → error 55 whose suggested fix re-triggers
error 55.**

```
$ pwe compile adv.pwe        # systems { rk4 { on=e; dt=0.1  s0 = 0.0 + 1.0 } }
error 55: rk4 system has no slot rules (a pure-number RHS such as `s0 = 1.0` becomes a scalar parameter — write `s0 = 0.0 + 1.0` instead)
```

The message literally advises `s0 = 0.0 + 1.0` — which *is* the program
above, and it fails with the same message. The premise is also inaccurate:
a non-param key goes to `assigns` regardless of scalar RHS
(`numeric_param_keys("rk4") = ["dt", "every", "substeps"]`,
`parser.rs`), not to "a scalar parameter". The taught integration idiom
`v = v + inte(…)` (docs/lang-usage.md:155-157) is an assignment too, so it
also dies here (`r4tut.pwe`), even though `inte a = 1.0` alone compiles
fine (`adv2.pwe`).

**(b) Assignment mixed with `inte` compiles and silently computes the
wrong physics.**

```pwe
# r4osc.pwe — x written with plain `=`
systems { rk4 { on = e; dt = 0.1  inte v = 0.0 - 3.0*x  x = 0.0 + v } }
# r4plus.pwe — same system, x written with `+=`
systems { rk4 { on = e; dt = 0.1  inte v = 0.0 - 3.0*x  x += v } }
```

```
$ pwe run r4osc.pweb --steps 10
  #1  e  state = [1.0000, -3.0000]      ← x frozen at 1.0; v decays linearly (−0.3/step)
$ pwe run r4plus.pweb --steps 10
  #1  e  state = [-0.1605, -1.7096]     ← matches a reference RK4 integrator exactly
```

Reference RK4 for `x'=v, v'=-3x, x0=1, v0=0, dt=0.1, 10 steps` =
`[-0.1605, -1.7096]` (analytic at t=1: `[-0.1606, -1.7096]`). The `=`
program compiles with **no warning** and integrates a different (wrong)
system: `x` never updates, so `v` integrates against a stale `x`.

**(c) Detail 73 is bypassed on the plain-assign path.**
docs/lang-usage.md:798 (detail 73 row) promises: *Dynamic slot LHS is
`update`-only (rejected in `rk4`).* It does fire for the statement form:

```
$ pwe compile d73.pwe        # … rk4 { inte v = 1.0  let i = 1.0  inte s[i] = 3.0 }
error 73: dynamic slot LHS is update-only (rk4 stages need compile-time slots)
```

but `s[i] = 3.0 + 0.0` (plain form, same system otherwise) goes to
`assigns`, is dropped, and compiles clean (`r4dyn.pwe` → rc 0).

## Evidence

- `reference/src/lang/compile.rs:286-306` — rk4 arm iterates
  `&s.update` only; `:319-337` — `Rk4System { rules, lets, … }` built with
  no assigns; `:306-313` — the 55 check and its advice.
- `reference/src/lang/systems.rs:559-581` — `struct Rk4System` fields
  (`rules`, `lets`, `slots_hint`, `dt`, `every`, `when`, `substeps`,
  `entity_map`, `only`, `func_ids`, `field_dims`, `namespace`,
  `param_names`, `state_names_by_id`) — no assignment storage; compare
  `UpdateSystem`, which consumes `s.assigns`
  (`compile.rs:210-236`).
- Parser dispatch: `store_param` — `ode_stmt`/`add_stmt` → `s.update`;
  plain `ident = expr` → `s.assigns`.
- Docs: docs/lang-usage.md:693-694 ("**`slot = expr` assigns** … in
  `rk4`, `inte slot = rate` integrates …"), :798 (detail 73),
  :155-157 (the `inte(E)` operator idiom that also breaks here).
- Probes: `/tmp/pwe_final/{adv,r4tut,adv2,r4osc,r4plus,r4dyn,d73}.pwe`,
  re-verified on main `9d6277fe`.

## Impact

The most severe class: a program that compiles cleanly produces wrong
dynamics with zero diagnostics — a literal frozen coordinate while the
derivative keeps integrating. Users following the error-55 advice are led
into a loop; users following the docs' `slot = slot + inte(rate)` phrasing
get 55 instead of an rk4 system; and a documented diagnostic (73) is only
half implemented. `rk4` is the accuracy-selling integrator, and its
plain-assignment form is a trap.

## Suggested fix

Pick one contract and enforce it everywhere:

1. **Support it** (matches docs "`slot = expr` assigns"): add
   `assigns` (+ `dyn_assigns`) to `Rk4System`, apply them as
   instantaneous writes alongside the RK4 stage integration (same
   semantics as `update`), and lower them in the rk4 step loop.
2. **Reject it** (matches detail 73's spirit): diagnose any
   `s.assigns` entry in an `rk4` system at compile time — extend 73 (or
   55) with "`slot = expr` is not supported in `rk4`; use `+=` or
   `inte slot = rate`".

Either way, rewrite the 55 advice so the suggested code actually compiles,
and add regression tests for: assignment-only rk4, `=`-vs-`+=` equivalence
(r4osc must equal r4plus), and plain `s[i] =` in rk4.
