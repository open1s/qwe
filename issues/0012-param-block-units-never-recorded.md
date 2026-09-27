# 0012 — `params { … [unit] }` annotations are never recorded, silently disabling dimensional checks (High)

## Summary

Unit annotations on **model parameters** are parsed by the grammar but never
stored: in the `params { … }` block handler the pending key is consumed by the
`value` arm before the `unit_expr` arm runs, so `model.param_units` is never
inserted into for any `name = value [unit]` entry. Every reference to such a
parameter then resolves to an *unknown* dimension (wildcard), and any rule
whose mismatch involves only parameters passes dimensional analysis silently.

This makes the documented gradual-typing contract ("annotate params with
`[m]`, `[m/s]`; mismatches are `detail 77`") a no-op for params, while state
slots keep working — so users get inconsistent enforcement with no warning.

Related to #9 (which covers the *system-level* `if let Ok` drop at
`parser.rs:287`); this bug is worse: it fires for **every valid unit** in a
`params` block.

## Repro

```pwe
world { gravity = (0, 0, 0)
  params { k = 4.0 [1/s^2] }
  entity e { state = (x = 1.0 [m]) }
}
systems {
  update { on = e; dt = 0.1 [s]  x = k }
}
```

```
$ pwe compile m1_param_unit_slash.pwe -o m1.pweb
  compiled ...          ← should be: Invalid (77): dimension mismatch
$ pwe run m1.pweb --steps 1
  #1   e   state = [4.0000]   ← [1/s^2] value written into an [m] slot
```

Same for `x = x + dt*(0.0 - k*x)` with `k = 4.0 [kg]` (`_probe_out/batch5/`,
`batch2/u1_unit_kg`, `batch2/u3_unit_assign_only`) — all compile.

Control (mismatch not involving a param **does** error, so the checker itself
runs and state units work):

```pwe
world { gravity = (0, 0, 0)
  params { k = 4.0 [1/s^2] }
  entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
}
systems {
  update { on = e; dt = 0.1 [s]
    x = x + dt*(  0.0 - k * x )      ← vacuously passes (k is wildcard)
    vx = vx + dt*(  vx )             ← m + s·(m/s) mismatch → Invalid (77)
  }
}
```

## Evidence

- `reference/src/lang/parser.rs:1080-1107` — params-block loop:

  ```rust
  Rule::value => {
      if let (Some(k), Some(v)) = (pending_key.take(), ...) {   // ← takes the key
          model.params.insert(k, v);
      }
  }
  Rule::unit_expr => {
      if let (Some(k), Ok(d)) = (pending_key.clone(), ...) {    // ← key is None here
          model.param_units.insert(k, d);                       // ← unreachable
      }
  }
  ```

  `pending_key.take()` at `parser.rs:1089` clears the key, so the `unit_expr`
  arm at `parser.rs:1096-1106` never inserts. The `if let (Some(k), Ok(d))`
  shape also means an unparsable unit would be dropped without any diagnostic.

- `reference/src/lang/compile.rs:1416-1420` — `DimEnv::of_expr` falls back to
  `self.params.get(name)` → `None` (wildcard) when the map is empty.
- `reference/src/lang/compile.rs:1610/1656/1661` — `params:
  &parsed.model.param_units` is passed as the (always empty) param env.
- System-level units *do* work (`parser.rs:181`, `decl.param_units`), which is
  why `dt = 0.1 [s]` behaves correctly — only the `params` block path is dead.
- Test gap: `reference/src/lang/tests.rs:1394-1415`
  (`units_check_consistent_and_reject_mismatch`) passes **vacuously** with
  respect to param units — its `bad` case errors on the state-slot rule
  (`vx = vx + dt*(vx)`), never on a `k`-referencing rule.

## Impact

- Documented dimensional checking silently does not apply to model params —
  the exact case units are most useful for (constants like `g`, `k`, `m`).
- False sense of safety: users annotating `params { g = 9.8 [m/s^2] }` get
  zero enforcement.
- Existing unit tests mask the bug (vacuous pass).

## Suggested fix

- In the `Rule::value` arm use `pending_key.clone()` (or restructure so the
  key survives until the `unit_expr` arm), and emit a `detail 77` (or a
  dedicated detail) when `Dim::from_str` fails instead of dropping silently.
- Add a regression test asserting `params { k = 4.0 [1/s^2] }` +
  `x = k` (with `x` in `[m]`) is rejected with detail 77.
