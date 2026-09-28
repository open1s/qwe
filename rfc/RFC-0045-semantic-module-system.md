# RFC-0045: Semantic module system
**Status:** Accepted (implemented, RFC-0045 slice 1-2). A first-class module
system with an explicit surface. Implemented: `module <dotted.name>` (stable
identity + alias), `export a, b` surface with privacy enforcement (detail 102),
duplicate-module detection (101), and deterministic (layout-independent) merge
order. Remaining: `from … import …` privacy, module versions.

## Motivation

The language already has `import "path"` and `from <m> import <name>`
(`reference/src/lang/compile.rs`: `collect_module`, `merge_modules`). Today it is
**fuzzy** in three ways that hurt determinism and tooling:

1. **Namespace identity is derived from the file path** (its last/stem segment,
   `module_stem`), so renaming a file silently changes every reference, and two
   differently-placed files can collide.
2. **Everything is flattened** into one `ParsedProgram`; namespaced functions
   become `ns.f` and `from … import …` adds `bare -> ns.f` aliases. Collisions
   are resolved implicitly (aliases + merge order), not reported.
3. There is **no export surface**: every definition is visible to every importer,
   and there is no privacy boundary or module version.

A deterministic, distributable engine needs modules with **stable identity**,
**explicit exports**, and **defined collision behavior** — the same discipline
the rest of the IR/ABI layer already has.

## Design

### Declarations

```
module math.util            # stable module name (not the path)
export { clamp, lerp }       # the public surface (unexported items stay private)
import "std/forces"          # sugar: binds the module's name (see below)
from math.util import clamp  # binds `clamp` directly
```

- A module's **identity** is its declared `module <dotted.name>`; a file without
  one defaults to a name derived from its path **once**, recorded in the module
  metadata (so later path moves do not change references).
- `export` lists the public items (functions, structs, constants). Anything not
  exported is **private** to the module and invisible to importers.
- `import "path"` binds the module under its declared name (sugar for `import
  <name> "path"`); qualified references use that name (`math.util.clamp`).

### Resolution and merging

- Modules are loaded into a **module graph** keyed by canonical path
  (cycle-safe, as today) but their **names** come from declarations, and the
  graph is topologically merged in a **deterministic order** (dependency order,
  ties by name — not filesystem order).
- A name collision (two modules exporting the same bound name, or two `from`
  aliases colliding) is a **hard error** with a new detail code (module
  collision), never a silent last-wins.
- Cross-module references resolve through the exported surface only; an
  unexported name is "unknown identifier" (detail 85), not a silent 0.0.

### Identity and caching

The **module set** (each module's name + source hash) is part of the artifact
identity (RFC-0035), so changing any imported module changes the artifact hash
and invalidates caches. This also lets the LSP cache per-module diagnostics.

## Validation

- determinism: the same module graph yields the same merged program regardless
  of directory layout / merge order (`module_graph_is_order_independent`).
- collision → error (`duplicate_export_is_rejected`).
- privacy: referencing an unexported item is detail 85 (`private_item_is_invisible`).
- cycles rejected with a defined error, not a hang.
- artifact hash changes when an imported module changes.
- conformance: a case in `pwe-conformance` (cross-backend) using two modules.

## Backward compatibility

`import` / `from … import …` keep working (they become sugar); existing single-
file programs are unaffected. Path-derived namespaces remain the fallback when
no `module` declaration is present, so nothing breaks on day one; a deprecation
warning (new detail) nudges toward declaring names.

## Alternatives

- **Keep the flat merge** — simple, but non-deterministic under collisions and
  hostile to tooling; rejected as the long-term shape.
- **Path = identity** — the current behavior; rejected because moves are
  breaking and layouts leak into semantics.
