# 0024 — Slot-index bounds are enforced for `+=`/`inte` (detail 52) but bypassed by plain assignments; out-of-range reads silently yield 0.0 (Medium)

## Summary

Positional slot access (`sN`) is checked inconsistently. The detail-52
bounds check lives only in the `s.update` loops of the compiler
(`reference/src/lang/compile.rs:176-192` for `update`, `:288-292` for
`rk4`); plain assignments (`sN = expr`) are built in a different loop that
performs **no check** (`compile.rs:210-236`), and **reads are never
checked anywhere**.

Worse, the compile-time guard on the lowering side cannot fire for
assignments: `slots` is computed by bumping with the assignment's own
index (`reference/src/lang/systems.rs:131-135
slots = slots.max(idx + 1)`), so the subsequent
`if idx >= slots { continue }` (`systems.rs:433-435`) is always false.
Out-of-range writes are emitted and then discarded by the bounded state
view at runtime — silently, with no diagnostic on either path.

## Repro (entity declares `a`, `b` — two named slots)

| probe | source | result |
| --- | --- | --- |
| d52b | `s16 += 1.0` | `Invalid (52): state slot index out of range (0..=15)` ✓ |
| d52 | `inte s16 = 1.0` | `Invalid (52)` ✓ |
| oob | `s16 = 42.0` | **compiles clean**; `pwe run --steps 10` → `state = [11.0000, 22.0000]` (write silently gone) |
| d99w | `s99 = 0.0 + 5.0` | **compiles clean**; run → `state = [1.0000, 2.0000]` (write silently gone) |
| d99 | `a = 0.0 + s99` | **compiles clean, no warning**; run → `state = [0.0000, 2.0000]` (read silently 0.0) |
| d5c | `s5 = 0.0 + 7.0` | **compiles clean**; run → `state = [1.0000, 2.0000, 0.0000, 0.0000, 0.0000, 7.0000]` — writes an *undeclared* slot inside the entity's spare state capacity |
| unk3 | `a = 0.0 + y` (unknown *name*) | `warning [85]: unknown identifier y — reads 0.0 (typo?)` ✓ contrast |

```pwe
# oob.pwe
world { entity e { state = (a=11.0, b=22.0) } }
systems { update { on=e; dt=0.1  s16 = 42.0 } }
```

```
$ pwe compile oob.pwe -o oob.pweb     ← no diagnostic
$ pwe run oob.pweb --steps 10
  #1  e  state = [11.0000, 22.0000]   ← s16 write vanished
```

## Evidence

- `reference/src/lang/compile.rs:176-192` — 52 check over `s.update`
  only; `:288-292` — same for rk4; `:210-236` — `s.assigns` loop, no
  `numeric_slot` bounds test.
- `reference/src/lang/parser.rs` `numeric_param_keys` / `store_param` —
  `s16 = 42.0` lands in `decl.assigns` (key is not a param name).
- `reference/src/lang/systems.rs:131-135` — `slots` inflated by the
  assignment's own index; `:433-435` — dead guard;
  `:454-462` — assigns emit `WriteView` into the state view.
- Physical capacity: `reference/src/components.rs:198`
  `MAX_STATE_SLOTS = 16` — in-range-but-undeclared writes (d5c) land in
  spare capacity and are even printed by the report.
- Read path: `reference/src/lang/lower.rs:115`
  `Expr::Slot(i) => ctx.slot_regs.get(*i).copied().unwrap_or(0)` —
  unconditional fallback, no `push_diag`.
- Warning 85 exists only for unknown *identifiers*
  (`lower.rs:229,246`), not for out-of-range positional slots.

## Impact

A typo in a slot number (`s5` vs `s15`, `s16` vs `s1`) either silently
drops a write, silently reads 0.0, or writes a slot the entity never
declared — all with a clean compile. This is precisely the silent-wrongness
class that #2/#52 fixes closed for identifiers and `+=`, re-opened by the
assign/read paths. `s16 += …` erroring while `s16 = …` sails through is
hard to defend in review.

## Suggested fix

1. Run the existing 52 check in the `s.assigns` loop too (one line), and
   in rk4's arm — the check machinery already exists.
2. Give out-of-range positional reads a diagnostic: either emit
   `push_diag(85, offset, "slot index 99 out of range (0..=15) — reads
   0.0")` at the `Expr::Slot` fallback, or reject at parse time (the
   index is a literal — the parser knows it is > 15).
3. Decide the policy for undeclared-but-in-range slots (d5c): warn or
   reject. The spare-capacity write is at best undocumented behavior.
4. Add regression tests covering all three paths (assign / read /
   undeclared in-range) alongside the existing 52 tests.
