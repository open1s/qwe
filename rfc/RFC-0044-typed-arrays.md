# RFC-0044: Typed arrays / SoA user types
**Status:** Accepted (implemented). First-class **fixed-length arrays** in world
state, generalizing `vecN name` and raw `s[i]` indexing into a bounds-checked,
named array type. Shipped surface (this build):
`array N name [{ v0, … }]` in an entity body (composes with `state = (…)` in
either order; an entity may declare several `array` fields);
`name[j]` read (constant index → static slot; runtime index → `ReadSlotDyn`);
`name[j] = expr`, `name[j] += expr`, and `inte name[j] = rate` writes;
constant indices are bounds-checked at compile time (detail 52) and a
**runtime** index (`name[k]` for non-constant `k`) is bound-checked at
execution time by the `BoundsCheck` EIR opcode: a non-finite, fractional, or
out-of-range index is a load-class trap (detail 18) instead of a silent read or
write of an arbitrary State slot. An unknown array name is detail 109 — all
fail loudly instead of silently reading 0.0.
`vecN` and `s[i]` are unchanged (`s[i]` has no declared length and stays
unchecked, exactly as before). A `for` bound may be `len(name)` — the
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
`typed_array_unknown_array_in_ode_rule_is_rejected`,
`typed_array_runtime_index_out_of_range_traps`,
`array_reductions_fold_named_arrays`, `array_reduction_unknown_array_is_detail_109`,
`array_dot_needs_two_array_names`; EIR:
`eir::tests::eir_bounds_check_maps_and_traps`.

Deferred (future, see **Deferred** below): array element unit annotations and a
general scalar `len(name)`. The `EntityDecl.arrays` map records each array's
length, so these are additive. (`len(name)` in a `for` bound is implemented; as
a general scalar expression it is not — it resolves at parse time against the
world's array layout, so it is only meaningful as a bound.)

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
  `BoundsCheck(j, base, N)` followed by a runtime-indexed read
  `ReadSlotDyn(check(j))`, where `BoundsCheck` yields the absolute slot
  `base + j` or traps (detail 18).
- `name[j] = expr` — write, same rule (constant index → static slot write;
  runtime index → the same `BoundsCheck` then `WriteSlotDyn`).
- `len(name)` — a compile-time integer literal `N`, usable in `for` bounds
  (as a general scalar expression it is deferred; see below).
- **Array reductions** — `sum(a)`, `mean(a)`, `norm(a)` (Euclidean),
  `asum(a)` (`Σ|aᵢ|`), `prod(a)`, `min_of(a)`, `max_of(a)`, and the two-array
  `dot(a, b)`. The argument is a literal array name (its length is
  compile-time), so each unrolls to native EIR over the elements — **no new
  opcode**, cross-backend, and a kernel primitive for statistics / DSP /
  linear-algebra rules. An unknown name is detail 109; `dot` of unequal lengths
  (or a non-name argument) is rejected (52 / 59).

### Lowering

A new `Expr::Index(String, Box<Expr>)` resolves at lowering against the target
entity's layout (`ctx.state_names` carries the run `name.0 … name.k`; the
compile-time check re-derives the length from that run, which equals the `N`
recorded in `EntityDecl.arrays`):
- constant index → the static slot `name.j` (0≤j<N checked at compile time,
  detail 52);
- dynamic index → `BoundsCheck(index, base, N)` then `ReadSlotDyn` /
  `WriteSlotDyn` on the checked absolute slot, with `base`/`N` taken from the
  layout of the entity the system targets.

`BoundsCheck = 252` (operands `index, base, len`) is the RFC-0021 load-class
check: it traps (detail 18) when `index` is not a finite integer in `[0, len)`,
else yields `base + index`. It is interpreter-only today (not in
`native.rs::eligible`), so array programs run on the interpreter while the
`step_cross` cross-check treats the interpreter as the reference.

### Diagnostics

- constant index out of range → detail 52 (slot index out of range);
- runtime index out of range / fractional / non-finite → detail 18 (load-class
  trap, `BoundsCheck`);
- unknown array name (no `name.0 … name.k` run on the entity) → detail 109.

## Validation

- Cross-backend (`step_cross`) equality for `name[j]` static and dynamic forms.
- Diagnostics for out-of-range constant indices and unknown array names.
- Runtime out-of-range / fractional indices trap (detail 18), proving no silent
  read/write of an arbitrary State slot.
- Conformance case exercising an in-range dynamic array read/write on both
  backends; `lang::tests` covers the OOB trap (a trap aborts the step, so it is
  not a cross-backend success case).

## Deferred

Specified here but **not** part of this revision; `EntityDecl.arrays` already
records the length, so each item is additive:

- Bounds checks on a raw runtime `s[i]` index (`s[i]` has no declared length, so
  it stays unchecked, exactly as before).
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
