# RFC-0042: `struct` record types over flat state slots
**Status:** Normative. The language gains user-defined **record types**
(`struct`): a named group of fields that, when used in a `state` layout, is
flattened at compile time to **flat, dotted scalar state slots**. It is
zero-cost sugar over the existing slot model — no new runtime representation, no
new opcodes — so it stays deterministic and byte-identical across backends.

## Motivation

Position/velocity/state triplets and other records are used everywhere, but the
language only offered positional slots (`s0…`) and the `vecN` shorthand. Named
records make rules self-documenting (`pos.x = pos.x + inte(vel.x)`), let several
records coexist without index bookkeeping, and keep the SoA slot storage.

## Declaration

```
world {
  struct Vec3 { x = 0.0; y = 0.0; z = 0.0 }
  struct Body { pos = Vec3; vel = Vec3; mass = 1.0 }

  entity a { state = Body }                              # a record type
  entity b { state = (p = Vec3, hp = 10.0) }             # record + a scalar
  entity c { state = (pos = (x = 7.0, y = 8.0, z = 9.0)) }# inline record
}
```

- `struct Name { field = <default>; field = <Type> }` — a field is a scalar
  (with a default) or another declared `struct` (nested).
- A `state` item may be a `type_ref` (`Body`), a named item whose RHS is a
  `type_ref` (`p = Vec3`), or an inline nested record (`pos = (x = 1, …)`).

## Rules

1. **Flattening.** Each leaf scalar becomes one ordinary state slot, named by
   its dotted path (`pos.x`, `pos.y`, `vel.z`, `mass`). Fields are laid out in
   declaration order, recursively; the resulting slot indices are exactly what a
   hand-written flat `state = (pos.x = 0, …)` would produce.
2. **Access.** Own fields are read/written as `pos.x` (a dotted rule LHS or an
   expression name); another entity's as `@a.pos.x`; both are usable in rules
   and `funcs`. The bare record name (`pos`) aliases its first field (`pos.x`),
   mirroring `vecN`.
3. **`vecN` alignment.** `vec3 pos` (the built-in shorthand) yields `pos.0 …
   pos.2`, with `pos` aliasing `pos.0`; dotted struct access and `vecN` share
   the same dotted-slot resolution.
4. **Compile-time, zero-cost.** A `struct` has no runtime footprint beyond its
   flattened slots and is part of world state (serialized, versioned, hashed).

## Diagnostics

Detail 80 covers: an unknown `struct` type, a `struct` reference cycle, and a
flattened layout exceeding the state-slot limit.

## Validation

- `struct_record_types_layout_and_rules` (lang tests): layout, dotted rules,
  and cross-entity `@a.pos.x`.
- `cli/examples/structs.pwe` compiles and runs cross-backend.
- Determinism: a `struct`-based program and its hand-flattened equivalent produce
  identical writes.

## Backward compatibility

Additive: flat positional states, `vecN`, and named scalars are unchanged. New
grammar: `struct_stmt`, `struct_field`, `type_ref`, nested `state_rhs`, and
`dot_lhs`.
