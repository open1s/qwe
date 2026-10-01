# RFC-0044: Typed arrays / SoA user types
**Status:** Accepted (implemented). First-class **fixed-length arrays** in world
state, generalizing `vecN name` and raw `s[i]` indexing into a bounds-checked,
named array type. Shipped surface (this build):
`array N name [{ v0, … }]` in an entity body (composes with `state = (…)` in
either order; an entity may declare several `array` fields);
`name[j]` read (constant index → static slot; runtime index → `ReadSlotDyn`);
`name[j] = expr`, `name[j] += expr`, and `inte name[j] = rate` writes;
constant indices are bounds-checked at compile time (detail 52) and an unknown
array name is detail 109 — both fail loudly instead of silently reading 0.0.
`vecN` and `s[i]` are unchanged. A `for` bound may be `len(name)` — the
declared length of a named array, a compile-time integer (`for j in
0..len(samples)`); an unknown array name is detail 109. Conformance:
`pwe-conformance` "RFC-0044 typed arrays (static + runtime index + len bound,
cross-backend)"; tests
`lang::tests::typed_arrays_read_write_and_bounds`,
`lang::tests::for_bound_can_be_array_len`,
`lang::tests::array_len_unknown_name_is_detail_109`,
`typed_array_runtime_index_read_matches_static`,
`typed_array_constant_index_out_of_range_is_rejected`,
`typed_array_initializer_arity_is_checked`, `typed_array_fields_compose_and_allow_several_per_entity`,
`typed_array_dynamic_write_lands_on_the_target_layout`,
`typed_array_unknown_array_in_ode_rule_is_rejected`.

Deferred (future, see **Deferred** below): dynamic-index bounds checks and
array element unit annotations. The `EntityDecl.arrays` map records each
array's length, so these are additive. (`len(name)` in a `for` bound is
implemented; as a general scalar expression it is not — it resolves at parse
time against the world's array layout, so it is only meaningful as a bound.)

## Motivation

SoA ("structure of arrays") is the storage default (AGENTS.md §9), and the
language already has `vecN pos` (N consecutive named slots `pos.0 … pos.{N-1}`,
with `pos` aliasing `pos.0`) plus a runtime-indexed `s[i]`. What is missing is a
**named, length-aware** array so that `arr[j]` is checked and self-documenting
rather than requiring the author to know the base slot and the length.

## Design

### Declaration

```
world {
  entity swarm {
    state = ( x = 0.0 )
    array 8 samples        # 8 f64 slots named `samples.0 … samples.7`
  }
}
```
`array N name` is equivalent to `vecN name` but records the **length** `N` with
the name (`EntityDecl.arrays: BTreeMap<String, usize>`), which `vecN` does not.

### Access

- `name[j]` — read. If `j` is an integer constant in `[0, N)` it lowers to the
  slot `name.j` directly (compile-time); otherwise it lowers to a
  runtime-indexed read `s[base + j]` (bounds are the caller's responsibility and
  documented; a future revision may add a checked variant).
- `name[j] = expr` — write, same rule (constant index → static slot write;
  runtime index → `WriteSlotDyn`).
- `len(name)` — **deferred** (see below): a compile-time integer literal `N`,
  usable in `for` bounds.

### Lowering

A new `Expr::Index(String, Box<Expr>)` resolves at lowering against the target
entity's layout (`ctx.state_names` carries the run `name.0 … name.k`; the
compile-time check re-derives the length from that run, which equals the `N`
recorded in `EntityDecl.arrays`):
- constant index → the static slot `name.j` (0≤j<N checked at compile time,
  detail 52);
- dynamic index → `base + index` (reusing `ReadSlotDyn` / `WriteSlotDyn`),
  with `base` taken from the layout of the entity the system targets.

### Diagnostics

- constant index out of range → detail 52 (slot index out of range);
- unknown array name (no `name.0 … name.k` run on the entity) → detail 109.

## Validation

- Cross-backend (`step_cross`) equality for `name[j]` static and dynamic forms.
- Diagnostics for out-of-range constant indices and unknown array names.
- Conformance case exercising arrays on both backends.

## Deferred

Specified here but **not** part of this revision; `EntityDecl.arrays` already
records the length, so each item is additive:

- Bounds checks on **runtime** indices (today unchecked, exactly like `s[i]`).
- Per-element unit annotations.
- `len(name)` outside a `for` bound (a general scalar expression).

## Backward compatibility

Additive: `vecN` and `s[i]` continue to work; `array`/`name[j]` are new
surface (`len` is deferred). `name[j]` with `name` a plain scalar is a compile
error (detail 109).

## Alternatives

- **Only `vecN` (status quo)**: works but has no length and no bounds check.
- **Dynamic/flexible arrays (heap)**: rejected for the deterministic, fixed
  ABI/memory profile (AGENTS.md §3, §8); arrays are fixed-length by construction.
