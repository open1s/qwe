# RFC-0044: Typed arrays / SoA user types
**Status:** Proposed. This RFC specifies first-class **fixed-length arrays** in
world state, generalizing today's `vecN name` and raw `s[i]` indexing into a
bounds-checked, named array type.

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
- `len(name)` — a compile-time integer literal `N` (usable in `for` bounds).

### Lowering

A new `Expr::Index(String, Box<Expr>)` resolves at lowering against the entity's
layout (`ctx.state_names` gives `name.0`; the length comes from `ctx.arrays`):
- constant index → `Expr::Slot(base + j)` (with a 0≤j<N check, detail 52);
- dynamic index → `Expr::SlotDyn(base + index)` (reusing `ReadSlotDyn` /
  `WriteSlotDyn`).

`for` ranges already accept literals, so `for j in 0..len(name)` unrolls with a
compile-time bound.

### Diagnostics

- constant index out of range → detail 52 (slot index out of range);
- `len(name)` on a non-array name → detail 95 (unknown array).

## Validation

- Cross-backend (`step_cross`) equality for `name[j]` static and dynamic forms.
- Determinism: unrolled `for j in 0..len(arr)` matches a hand-written sequence.
- Diagnostics for out-of-range constant indices and unknown array names.
- Conformance case exercising arrays on both backends.

## Backward compatibility

Additive: `vecN` and `s[i]` continue to work; `array`/`name[j]`/`len` are new
surface. `name[j]` with `name` a plain scalar is a compile error (detail 95).

## Alternatives

- **Only `vecN` (status quo)**: works but has no length and no bounds check.
- **Dynamic/flexible arrays (heap)**: rejected for the deterministic, fixed
  ABI/memory profile (AGENTS.md §3, §8); arrays are fixed-length by construction.
