# 0013 — Side-effect-only `update` systems (`let` / bare call) are rejected with error 55 (Medium)

## Summary

An `update` system whose body contains only `let` bindings or bare call
statements (e.g. `print(...)`, `let _ = fset(...)`) fails to compile with

```
error 55: update system has no rules; add `slot = <expr>`
```

even though the parser deliberately supports both forms — bare calls are
mapped to `UpdateStmt::Let("_", …)` specifically so they can run for their
side effect. The `no rules` emptiness check only inspects parsed rules and
assignments and runs **before** `update_stmts` (lets/bare calls) are even
converted.

## Repro

```pwe
world { gravity = (0, 0, 0)
  entity a { state = (x = 1.0) }
}
systems {
  update { on = a; dt = 1.0  let _ = print(42.0) }
}
```

```
$ pwe compile m2_let_only_update.pwe -o m2.pweb
error 55: update system has no rules; add `slot = <expr>`
  --> line 6, column 3
    |
  6 |   update { on = a; dt = 1.0  let _ = print(42.0) }
    |   ^
```

Also reproduced with a bare call (no `let`):

```pwe
  update { on = dst; dt = 1.0  print(r) }   # → same error 55
```

(`_probe_out/batch3/ch2_send_recv_prints.pwe`, `ch3_recv_send_prints.pwe`)

## Evidence

- `reference/src/lang/compile.rs:216-225` — the guard
  `if rules.is_empty() && dyn_rules.is_empty() && assigns.is_empty() &&
  dyn_assigns.is_empty() { return Err(error_at(… 55 …)) }` does not consider
  `s.update_stmts`.
- `reference/src/lang/compile.rs:228` — `let lets = to_let_stmts(&s.update_stmts)?`
  runs only *after* the guard, so lets are provably invisible to it.
- `reference/src/lang/parser.rs:92` — parser comment: "A bare call statement
  (`fset(field, i, j, v)`) runs for its side effect", and the call arm stores
  it as `Let("_", …)`.
- `docs/lang-usage.md:262` — documents `let _ = fset(...)` inside `update`
  (that example survives only because it *also* has a rule); builtins
  `print(x)` / `emit(kind,payload)` are documented at `docs/lang-usage.md:631`.

## Impact

- The intended "side-effect-only system" idiom (logging, one-shot seeding,
  event emission) is impossible; users must smuggle a dummy rule
  (`scratch = 0.0`) into the system.
- Parser and compiler contradict each other: the parser's `Let("_")` mapping
  is unreachable in practice when it is the *only* content.
- Error message actively misleads ("add `slot = <expr>`") for a system that
  already contains statements.

## Suggested fix

Include `to_let_stmts(&s.update_stmts)` in the emptiness check (or check
`s.update_stmts.is_empty()` before erroring). If lets-only updates should
still be rejected for some reason, the parser should not accept bare calls,
and the error message should mention statements.
