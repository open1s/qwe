# 0018 — A state slot named `dt` cannot be written; the assignment silently rebinds the timestep (Low)

## Summary

Follow-up to the residual acknowledged in #3's closing comment ("a slot
literally named `dt` remains a param — tracked separately if needed").

Name-based dispatch in `update`/`rk4` always treats `dt` as the system's
timestep parameter, so a rule `dt = <expr>` in a system body never writes the
entity's `dt` slot — it **silently rebinds the simulation timestep** instead.
The slot is undeclarable-as-writable with no diagnostic.

## Repro

```pwe
world { gravity = (0, 0, 0)
  entity m { state = (dt = 7.0, x = 0.0) }
}
systems {
  update { on = m; dt = 0.1
    dt = 2.0
    x = t
  }
}
```

```
$ pwe run p6_dt_param_vs_slot.pweb --steps 3
  sim time:  6.000000 s            ← timestep became 2.0 (3 × 2.0), not 0.3
  #1  m  state = [7.0000, 4.0000]  ← slot dt still 7.0 (rule never applied)
```

Expected: either the rule writes the slot (`dt = 2.0` → slot 2.0, sim time
0.3), or compile fails with a diagnostic naming the collision. Today the
program compiles, runs, and does neither.

## Evidence

- Repro: `_probe_out/batch1/p6_dt_param_vs_slot.pwe` (verified still
  reproduces on `main` at `1df656bb`).
- #3 closing comment (2026-09-27): "Residual: a slot literally named `dt`
  remains a param — closing as fixed; tracked separately if needed."
- Dispatch: `reference/src/lang/parser.rs` `numeric_param_keys` /
  param-vs-rule decision per system kind; `dt` is always a param name in
  `update`/`rk4` bodies.

## Impact

`dt` is a natural slot name (elapsed time, delta storage). A model using it
compiles cleanly and then runs with a **different timestep** — silent,
systemic wrongness (every step's `x = t`, integration rate, `when`/`at`
windows all shift). Same class of silent-wrongness as #2/#12.

## Suggested fix

When an `update`/`rk4` system contains a rule whose LHS name is `dt` (or any
reserved param) **and** the targeted entity declares a slot of that name,
fail compilation with a dedicated diagnostic (e.g. new detail code:
"`dt` names both a system parameter and an entity slot"). Alternatively,
prefer the slot when one is declared — but that must be documented.
