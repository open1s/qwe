# 0011 — `send`/`recv` systems run for every entity and silently ignore `on` (High)

## Summary

`send` and `recv` systems are lowered for **every entity in the program**, not
just the entity named by `on`. Two consequences:

1. `recv { on = dst; chan = c; slot = 0 }` overwrites **slot 0 of every
   entity** with the channel value each step — silently corrupting sibling
   systems' state.
2. `send { on = src; chan = c; value = v }` evaluates `value` once **per
   entity** and the **last declared entity wins**, so the channel payload is
   whatever the last entity's slot holds, not the sender's `value`.

`on = <name>` is parsed for these systems (the grammar accepts it, the program
compiles) but is never read — no `detail 62`, no warning.

## Repro (recv scope)

```pwe
world { gravity = (0, 0, 0)
  chan c { value = 555.0 }
  entity src { state = (v = 0.0, pv = 0.0) }
  entity dst { state = (r = 99.0, pr = 0.0) }
}
systems {
  update { on = src; dt = 1.0  v = v + 10.0  pv = print(1000.0 + v) }
  recv  { on = dst; chan = c; slot = 0 }
  update { on = dst; dt = 1.0  pr = print(2000.0 + r) }
}
```

```
$ pwe run m5_recv_scope.pweb --steps 3
  #1   src  ...  state = [555.0000, 1555.0000]   ← recv wrote into src!
  #2   dst  ...  state = [555.0000, 2555.0000]
```

`src` is never named by the `recv`, yet its `v` is set to the channel value
555 every step, and the `v = v + 10.0` update is never observable (recv runs
after it and clobbers the write). Correct behavior: `src = [30, 1030]`,
`dst = [555, 2555]`.

## Repro (send payload)

```pwe
world { gravity = (0, 0, 0)
  chan c { value = 0.0 }
  entity s1 { state = (v = 1.0) }
  entity s2 { state = (v = 2.0) }
  entity dst { state = (r = 0.0, pr = 0.0) }
}
systems {
  send { on = s1; chan = c; value = v }
  send { on = s2; chan = c; value = v }
  recv { on = dst; chan = c; slot = 0 }
  update { on = dst; dt = 1.0  pr = print(r) }
}
```

```
$ pwe run ch6_two_senders.pweb --steps 2
  prints: 0  0          ← channel payload became 0 (dst's slot), not 2.0
  s1 = [0], s2 = [0]    ← recv clobbered the senders too
```

Expected: `r` reflects `s2`'s `v = 2.0` (last send in program order).

## Evidence

- `reference/src/lang/compile.rs:103-150` — the `send`/`recv` compile block
  never reads `s.string_params.get("on")`. Contrast `update` at
  `compile.rs:229`, `rk4` at `compile.rs:291`, `fix` at `compile.rs:335`,
  `invariant` at `compile.rs:381`, which all resolve `on` via `resolve_on`.
- `reference/src/lang/systems.rs:2385-2400` — `ChanSystem` has no `only`
  field. Every other state system has `only: Option<BTreeSet<u128>>`
  (e.g. `UpdateSystem` at `systems.rs:30`) and guards the top of
  `lower_entity` (`systems.rs:51-52`).
- `reference/src/lang/systems.rs:2412-2534` — `ChanSystem::lower_entity`
  unconditionally reads/writes for whatever entity it is handed.
- `reference/src/physics_eir.rs:1828` — the lowering driver calls
  `sys.lower_entity(entity, …)` for **every** entity × system.
- `docs/lang-usage.md:597` — the parameter table lists only
  `chan,value` / `chan,slot` for `send`/`recv`; `on` is silently accepted
  anyway instead of rejected.

## Impact

Silent cross-entity state corruption: a single `recv` zeroes/overwrites an
unrelated entity's slot every step, and channel payloads are wrong whenever
more than one entity exists (i.e., always — channels are declared before
entities in every example). No diagnostic is produced.

## Suggested fix

Parse `on` for `send`/`recv` (detail 62 for unknown names) and store it as
`only` on `ChanSystem`, guarding `lower_entity` exactly like
`UpdateSystem` does; require `on` (or document broadcast semantics
explicitly, which the current payload behavior does not implement either way).
