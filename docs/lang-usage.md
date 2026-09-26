# PWE Language Guide

[中文版](lang-usage.zh.md)

This guide is **self-contained**: you can write, run, and debug complete PWE
programs from it alone. It targets PWE 0.0.x (`pwe-reference` + `pwe-cli`).

> **If you read only one section, read §0.3 (the two execution models) and §6
> (rules that bite).** Almost every non-obvious bug comes from those two.

---

## 0. Orientation

### 0.1 What PWE is

PWE is a small, deterministic simulation language. A program declares a **world
model** (entities, fields, parameters) and **systems** (the behaviour). It
compiles to a low-level IR (EIR) and runs **cross-backend**: the interpreter and
the CPU JIT must produce **byte-identical writes** every step.

```
source ──parse──▶ world model + systems ──lower──▶ EIR ──interpret ≡ JIT──▶ committed writes
```

Consequences you can rely on:
* **Determinism is first-class.** Same source + same seed ⇒ same trajectory,
  every run, on either backend. `random()`/`noise()` are seeded.
* The world is the single source of truth; a step is
  **Observe → Compute → (Prepare) → Commit → Publish**. Writes are applied
  atomically at the end of a step; invariants fail the step *before* any write.

### 0.2 Quick start

```sh
cargo build --release -p pwe-cli

./target/release/pwe compile scene.pwe -o scene.pweb   # source → verified artifact
./target/release/pwe run     scene.pweb --steps 600    # run N steps
./target/release/pwe present scene.pweb --port 8000    # live 3D viewer
# optional: --param K=V overrides a declared model parameter, on run/present
```

```rust
// Embedding API
let mut rt = pwe_reference::lang::LangRuntime::compile(src)?;  // or ::compile_file(path)
rt.step_cross()?;        // one step, interpreter == JIT asserted
rt.step_cross_n(600)?;   // many steps
let frame = rt.present_frame(None);   // render snapshot
```

Minimal program:

```pwe
world {
  gravity = (0, -9.81, 0)
  entity ball { position = (0, 5, 0) velocity = (3, 0, 0) sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (20, 1, 20); color = 0x557755 }
}
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.6 }
}
```

### 0.3 The two execution models (read this)

A body carries its dynamics in **one** of two independent stores. Mixing them on
one body is possible but is the usual source of "nothing moves" bugs.

| | **Component body** | **State-slot body** |
| --- | --- | --- |
| Declared with | `position = (…)`, `velocity = (…)`, `mass`, `dynamic`, colliders (`box`/`sphere`/`hull`) | `state = (…)` |
| Driven by systems | `gravity`, `integrate`, `damping`, `force`, `wall`, `ground_contact` | `update`, `rk4`, `linear`, `nbody` |
| Position lives in | `Transform.position` | `state[0..2]` (render fallback) |
| Velocity lives in | `Velocity.linear` | `state[3..6]` (your choice) |
| Read/written from rules as | `@name.position.x`, `@name.velocity.x` | `x`, `@name.x`, `s0`, … |

* **Component bodies** use the built-in physics systems; you generally do *not*
  write `update` rules for them.
* **State-slot bodies** are integrated by `update`/`rk4` rules; `position` is
  optional and purely visual if you also drive `state[0..2]`.
* `nbody` is a state-slot system: it expects `state = (px, py, pz, vx, vy, vz, m)`
  (mass in **slot 6**, not the `mass` field). See `cli/examples/solar.pwe`.
* `joint`/`soft` read **`Transform` positions and `rigid_body.mass`**, so their
  bodies are component bodies (`position`, `mass`, `dynamic`).

### 0.4 Determinism, rendering, and state hash

* World state = entities (transform/velocity/state/`active`) + fields + params +
  clock. Snapshots and `state_hash` cover exactly this.
* Presentation attributes (`color`, `shape`, `size`, `opacity`, `glow`,
  `label`, `orient`, `vector`, custom shapes) are **not** part of the state hash.

---

## 1. Lexical rules, keywords, syntax

### 1.1 Lexical rules

| Token | Form | Notes |
| --- | --- | --- |
| comment | `# …` or `// …` | to end of line; skipped anywhere |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | names of entities, slots, params, functions, shapes |
| `number` | `-? digits ("." digits)? (("e"\|"E") "-"? digits)?` | f64 literal |
| `value` | `number` (`/ number`)? | a literal **or a ratio** (`dt = 1/60`) |
| `boolean` | `true` \| `false` | |
| `string` | `"…"` (no escapes) | titles, SVG path data |
| `color` | `0x` hex | `0xRRGGBB` |
| `unit` | `[ m/s^2 ]` | base `m kg s A K mol cd`, ops `* / ^`; compile-time only |
| `slot` | `s` digits | own state slot by position (`s0`, `s1`, …) |
| constants | `t`, `pi`, `e` | clock (s), π, Euler's number |

Whitespace is insignificant; `;` between statements/params is **optional**. Units
must be bracketed so `s[0]` is unambiguous.

### 1.2 Keywords (reserved — cannot be identifiers)

* sections: `world`, `funcs`, `systems`
* world: `gravity`, `title`, `params`, `chan`, `value`, `entity`, `field`,
  `pool`, `soft`, `width`, `height`, `depth`, `dx`, `nx`, `ny`, `nz`, `spacing`,
  `origin`, `shape`, `part`
* entity fields: `position`, `velocity`, `state`, `vec`, `mass`, `dynamic`,
  `nbody`, `parent`, `restitution`, `friction`, `box`, `sphere`, `hull`,
  `rotation`, `camera`, `color`, `size`, `opacity`, `glow`, `label`, `orient`,
  `vector`
* shapes: `point`, `sphere`, `box`, `capsule`, `svg`, `hull`, `poly`, `at`,
  `depth`, `scale`, `faces`
* control: `return`, `let`, `repeat`, `until`, `while`, `for`, `in`, `break`,
  `continue`, `if`
* logic: `and`, `or`, `not`, `&&`, `||`, `!`, `true`, `false`
* atoms: `pi`, `e`, `t`

**Contextual (not reserved):** system *kinds* and their *params* (`update`,
`on`, `when`, `every`, `substeps`, `dt`, `field`, `pool`, `body`, `type`, …) are
ordinary identifiers matched at build time. Builtin function names (`sin`, `min`,
`random`, …) are ordinary calls special-cased in lowering. `import`/`as`/`from`
are handled by the module loader.

### 1.3 A rule of thumb for `slot = …`

Inside a system, a statement `name = <number>` (a *bare* number) is parsed as a
**system parameter**, not a rule. To write a constant expression, make it
non-trivial: `name = 0.0 + 3.0`. See §6.

### 1.4 Operator precedence

Highest → lowest: unary `-`, `not`/`!` → `* / %` → `+ -` →
comparisons `< <= > >= == !=` → `and`/`&&` → `or`/`||`.
Comparisons and logical operators yield `1.0` / `0.0`; any nonzero operand is
true. `not` binds to the following factor — write `not (x > 0)`.

---

## 2. Program structure and modules

```pwe
world   { # required: entities, fields, params, shapes, pools, softs }
funcs   { # optional: pure scalar functions }
systems { # optional: behaviour }
```

A `.pwe` file is a **module**. Imports follow Python:

```pwe
import "physics"               # physics.G, physics.thrust(m)
import "physics" as ph         # ph.G
from "physics" import thrust   # thrust(m)  (bare name)
```

**Import paths are resolved relative to the importing file** (e.g. from
`cli/examples/` use `import "../../std/forces"`; `import "std/forces"` works only
when a `std/` directory sits beside your file). A package is a directory
(`import "shapes"` → `shapes/__init__.pwe`). Functions
and parameters are namespaced by module; a module's own rules resolve their bare
names in their namespace first, then globally. Entities/systems/fields/channels
merge flatly (duplicate names are an error, detail 76). Std modules live under
`std/` (import as `"std/forces"` etc.).

---

## 3. `world` — the world model

| Statement | Meaning |
| --- | --- |
| `gravity = (x, y, z)` | global uniform gravity (component bodies) |
| `title = "…"` | run title shown by the viewer |
| `params { G = 1.0; k = 3.0 }` | model parameters; readable by name in rules, overridable with `--param G=2` |
| `chan <name> { value = v }` | a channel entity (`state[0]` holds the value) |
| `entity <name> { … }` | a body (§3.1) |
| `shape <name> { part … }` | a custom render shape (§3.2) |
| `field <name> { width=w; height=h; dx=d; depth? }` | a deterministic scalar grid (§3.3) |
| `pool <name>[N] { … }` | N initially-inactive slots for dynamic entities (§3.4) |
| `soft <name> { … }` | a mass-spring soft body (§3.5) |
| `import "…"` | module import (§2) |

**Entity id order** (used everywhere, incl. pool/soft names): declared entities
`1..E`, then channels, then pool slots (`<pool>#k`), then soft particles
(`<soft>#k`).

### 3.1 Entity fields

| Field | Meaning |
| --- | --- |
| `position = (x, y, z)` | initial position (Transform). |
| `velocity = (x, y, z)` | initial linear velocity. |
| `state = (v0, v1, …)` | positional state slots (max 16: `s0…s15`). |
| `state = (x = 0, y = 0, …)` | named state slots: assign by name, read via `@self.x`. May mix with positional. |
| `state = (vec3 pos, …)` | a vector element reserves N consecutive slots named `pos`, `pos.0`…`pos.{N-1}`. |
| `mass = v` | mass (drives `nbody` and visual size; required by `joint`/`soft`). |
| `dynamic = false` | static body (default dynamic). |
| `nbody = false` | exclude from the mutual `nbody` system. |
| `parent = <name>` | satellite (relative display). |
| `restitution = v` / `friction = v` | contact bounciness / tangential friction. |
| `box = (dx,dy,dz)` / `sphere = r` / `hull = [(x,y,z), …]` | collider (hull ≥ 4 points). |
| `rotation = (rx, ry, rz)` | **static** euler rotation (radians, XYZ), e.g. a tilted ground plane. |
| `camera = true` | viewer camera (excluded from simulation). |

**Presentation-only** (never affect simulation state, determinism, or the hash):

| Field | Meaning |
| --- | --- |
| `color = 0xRRGGBB` | render colour. |
| `shape = point\|sphere\|box\|capsule\|<custom>` | render shape (overrides the collider-derived one). |
| `size = v \| (dx,dy,dz)` | marker diameter / radius / box edge / per-axis box dims; for a custom shape, scales the whole shape. |
| `opacity = v` | `[0, 1]`. |
| `glow = v` | emissive intensity (0 = matte). |
| `label = false` | hide the floating name (default shown). |
| `orient = true` | for a state-only body, read state slots **7/8/9** as euler **(pitch, yaw, roll)** radians for the render orientation (yaw about Y, body-local pitch). Default: slot 7 is a spin about Z. |
| `vector = false` | hide the viewer's velocity arrow / orbit ring (for bodies whose slots 3–5 are not a velocity). |

### 3.2 Custom shapes

A shape is a list of parts; entities reference it by `shape = <name>`.

```pwe
world {
  shape head { part sphere = 0.115 at (0, 0, 0); }
  shape drone {
    part capsule = (0.06, 0.30, 0.06);
    part sphere  = 0.09 at (0, 0.20, 0);
    part box     = (0.54, 0.02, 0.02) at (0, 0.20, 0);
  }
  shape octa { part hull = [(0,0.9,0),(0.9,0,0),(0,-0.9,0),(-0.9,0,0),(0,0,0.9),(0,0,-0.9)]; }
  shape gem {
    part poly = [(0,0.9,0),(0.7,0,0.7),(-0.7,0,0.7),(-0.7,0,-0.7),(0.7,0,-0.7),(0,-0.9,0)]
      faces = [[0,1,2],[0,2,3],[0,3,4],[0,4,1],[5,1,4],[5,4,3],[5,3,2],[5,2,1]];
  }
  shape star {
    part svg = "M 0,-1 L 0.224,-0.309 L 0.951,-0.309 L 0.363,0.118 L 0.588,0.809 L 0,0.382 L -0.588,0.809 L -0.363,0.118 L -0.951,-0.309 Z" depth 0.22 scale 0.8;
  }
  shape person {                       # composition: include another shape
    part box  = (0.34, 0.46, 0.20) at (0, 0.57, 0);
    part head at (0, 0.92, 0);
  }
}
```

* `sphere = r`, `box = (dx,dy,dz)`, `capsule = (r_bottom, length, r_top)`.
* `hull = [(x,y,z), …]` — convex polyhedron.
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]` — arbitrary polyhedron (faces
  triangulated; use it for meshes such as a terrain — see §7.12).
* `svg = "<d>" depth <d>` — SVG path extruded along Z.
* `part <other-shape> [at (x,y,z)] [scale s]` — **include another shape**,
  inlined recursively (unknown name / cycle is a compile error).

`at` is a local offset; `scale` is a per-part amplitude; the entity's `size`
scales the whole shape. Shapes are **presentation only**.

### 3.3 Grid fields — the continuum substrate

```pwe
field heat { width = 16; height = 16; dx = 1.0 }          # 2D
field u { width = 17; height = 17; depth = 17; dx = 1.0 } # 3D
```

Cells are deterministic world state (snapshot/replayable). Access from rules:

| Call | Effect |
| --- | --- |
| `fget(f, i, j)` / `fget(f, i, j, k)` | read a cell (sees same-step writes) |
| `fset(f, i, j, v)` / `fset(f, i, j, k, v)` | write a cell (also a bare statement) |
| `flap(f, i, j)` / `flap(f, i, j, k)` | zero-flux discrete Laplacian, scaled by `1/dx²` (5-point 2D, 7-point 3D) |

The first argument must be a **literal field name**. Field reads see writes made
earlier in the same step (unlike entity reads, §6.4).

### 3.4 Entity pools — dynamic entities (RFC-0038)

A `pool name[N] { …entity fields… }` is a block of pre-allocated slots that
start **inactive**; the EIR stays static (one function per entity):

```pwe
world {
  entity emitter { state = (x = -6.0, vx = 1.5) }
  pool p[24] { state = (x = 0.0, vx = 0.0); shape = sphere; size = 0.18 }
}
systems {
  spawn   { on = emitter; pool = p }              # one free slot per step
  spawn   { on = emitter; pool = p; count = 3 }   # batch: up to 3 per step
  spawn   { on = emitter; pool = p; every = 2; phase = 1 }  # phased
  update  { on = p; dt = 0.1 x = 0.0 + vx }       # runs only on active slots
  despawn { on = p; when = x > 6.0 }              # recycle
}
```

* Slots are `<pool>#0`, `#1`, …; ids follow entities and channels.
* Inactive slots are skipped by user systems (an `active()` guard) and hidden in
  the viewer; `despawn` still runs on them to clear the flag.
* `spawn` activates the lowest-id free slots and copies the **caller's** state.
  `count` = slots per emission, `every`/`phase` gate emission to
  `step % every == phase`. `active()` reads the current entity's flag (0/1).
* `on = <pool>` in any system (`update`, `despawn`, `invariant`, …) expands to
  all of the pool's slots.
* `active` is hashed/snapshotted world state.

### 3.5 Soft bodies (RFC-0040)

```pwe
world {
  soft cloth { nx = 8; ny = 8; nz = 1; spacing = 0.4; origin = (-1, 5, 0); mass = 0.1 }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  soft      { body = cloth; stiffness = 1.0; damping = 0.3; iterations = 6 }
}
```

Creates an `nx × ny × nz` grid (`nz` default 1) of dynamic particles joined by
structural/shear/bend distance springs; `soft` relaxes them by mass-weighted
position relaxation. Mesh edges render as bonds. Examples: `cloth.pwe`,
`jelly.pwe` (3D).

---

## 4. Systems

### 4.1 All system kinds (exact parameters)

`?` = optional. Required params missing ⇒ detail 48; unknown kind ⇒ detail 49.

| Kind | Parameters | Model | Meaning |
| --- | --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | component | `velocity.y += gravity_y·dt` |
| `integrate` | `dt` | component | `position += velocity·dt` |
| `damping` | `factor` | component | `velocity *= factor` |
| `force` | `ax`, `ay`, `az`, `dt` | component | `velocity += (ax,ay,az)·dt` |
| `wall` | `x`, `z`, `y_min?`, `restitution?` | component | reflect at bounds `±x`, `±z` |
| `ground_contact` | `restitution` | component | resolve contact with the `y=0` plane |
| `linear` | `slots`, `dt`, `row0 = (a0,…,c)`, … | state | `s_N' = Σ a_j s_j + c`; each `rowK` has `slots+1` values |
| `nbody` | `G`, `dt` | state | mutual inverse-square force; needs `state = (px,py,pz,vx,vy,vz,m)` |
| `send` / `recv` | `chan`, `value` / `chan`, `slot` | channel | Go-style channel send / receive |
| `update` | `dt`, `on?`, `when?`, `every?`, `substeps?`, rules | state | explicit-Euler ODE rules |
| `rk4` | `dt`, `on?`, `when?`, `every?`, `substeps?`, rules | state | 4th-order Runge–Kutta on the same rules |
| `invariant` | `expr`, `on?` | — | per-step assertion (§4.6) |
| `watch` | `expr`, `mem`, `into`, `on?` | state | zero-crossing flag (§4.7) |
| `diffuse` | `field`, `rate` | field | `T += rate·∇²T` (Jacobi, conservative) |
| `poisson` | `field`, `iters`, `source?`, `scale?` | field | Gauss–Seidel `∇²φ = ρ·scale` |
| `wave` | `field`, `prev`, `velocity`, `dt`, `damping?`, `absorb?`, `absorb_width?` | field | leapfrog `u_tt = c²∇²u` |
| `spawn` | `on`, `pool`, `count?`, `every?`, `phase?` | pool | activate free slots (§3.4) |
| `despawn` | `on`, `when` | pool | deactivate matching slots |
| `joint` | `on`, `other`, `type`, `length?`, `stiffness?`, `damping?`, `axis?`, `anchor?`, `limit?`, `iterations?` | transform | pairwise constraint (§4.8) |
| `soft` | `body`, `stiffness?`, `damping?`, `iterations?` | transform | mass-spring grid (§3.5) |

### 4.2 `update` / `rk4` — the core

Both take rules `slot = expr` meaning **`slot += dt · expr`** (integration).

```pwe
systems {
  update { on = target; dt = 0.02
    let omega = 0.5
    tx = -@self.ty * omega        # tx' = -ty·ω  (rotation in the xy-plane)
    ty =  @self.tx * omega
  }
}
```

* `on = <name>` restricts to one entity (or a pool, §3.4); default = all dynamic
  bodies. Note: with no `on`, the rule runs on **every** dynamic body, including
  unrelated ones — prefer `on`.
* `let name = expr` computes a reusable local **before** the rules run.
* `when = expr` gates every write (state unchanged when 0) — mode/state-machine.
* `every = n` runs only when `step % n == 0`. `substeps = n` integrates n times
  with `dt/n` each (each substep re-reads own state).
* The LHS is `sN` **or** a named slot from the entity's `state = (…)` layout.
* **Assign, don't integrate:** use `slot = (target - slot)` (where `dt=1` gives
  `slot = target` exactly), or `slot = (target - slot) / dt`.

### 4.3 Component physics

`gravity` → `integrate` are the usual pair (order matters: force/gravity first).
`force` adds a constant acceleration; `wall`/`ground_contact` resolve boundaries.
These operate on `position`/`velocity` components, **not** `state` slots.

### 4.4 Channels (concurrency without threads)

`chan <name> { value = v }` declares a channel entity. `send { chan = c; value = e }`
publishes `e`; `recv { chan = c; slot = k }` reads the latest value into `state[k]`.
Cross-runtime routing is available via the embedding API (`set_peer`, regions).

### 4.5 Continuum solvers

```pwe
diffuse { field = heat; rate = 0.2 }                       # T += 0.2·∇²T
poisson { field = phi; source = rho; iters = 20 }          # ∇²φ = ρ
wave    { field = u; prev = um; velocity = 1.0; dt = 0.5 } # u_tt = c²∇²u
```

* `diffuse` — one Jacobi sweep/step; zero-flux ⇒ total conserved exactly.
  Stable for `rate ≤ 1/4` (2D) / `≤ 1/6` (3D).
* `poisson` — `iters` in-place Gauss–Seidel sweeps; boundary cells fixed.
* `wave` — leapfrog over two fields; Courant `c·h/dx ≤ 1/√2` (2D) / `≤ 1/√3`
  (3D). `damping` (default 1.0) scales the temporal term; `absorb` +
  `absorb_width` add a graded sponge layer instead of reflecting.
* All iterate in 3D when `depth > 1`, run once per step, and are deterministic.

### 4.6 `invariant` — assertions

Evaluated per entity **after** the systems run; must be non-zero (0/NaN fails the
step with detail 69 **before any write is applied**).

```pwe
invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
```

### 4.7 `watch` — zero-crossing detection

Compares `expr` with its previous value (kept in the `mem` state slot — persisted
world state) and writes 1 into `into` on a strict sign change, else 0.

```pwe
watch { on = ball; expr = x; mem = 5; into = 6 }
```

### 4.8 Joints (RFC-0039)

```pwe
joint { on = a; other = b; type = distance;  length = 1.0; stiffness = 1.0 }
joint { on = a; other = b; type = spring;    length = 1.0; stiffness = 20; damping = 0.5 }
joint { on = a; other = b; type = hinge;     anchor = (0,0,0) }
joint { on = a; other = b; type = prismatic; axis = (1,0,0); limit = (0.0, 2.0) }
```

Iterated, mass-weighted position relaxation over `Transform` positions
(`gravity`+`integrate` drive it). `dynamic = false` acts as an infinite-mass
anchor. Kinds: `distance`/`spring` (hold `|a−b| = length`), `weld`/`hinge`/
`revolute`/`ball`/`spherical` (coincide `a + anchor` and `b`), `prismatic`/
`slider` (keep `b` on the line through `a` along `axis`, optional `limit = (lo,hi)`).
Rotational/ratio joints (`cone`, `universal`, `gear`, `rack`, `pulley`) are
rejected (this engine is point-mass). Example: `chain.pwe`.

### 4.9 Expressions

| Form | Meaning |
| --- | --- |
| `s0`, `s1`, … | own state slots |
| `x`, `@self.x` | own named state slot |
| `@name.sN`, `@name.state.x`, `@name.x` | another entity's state slot |
| `@name.mass`, `@name.is_dynamic` | another entity's properties |
| `@name.position.x/y/z`, `@name.velocity.x/y/z` | another entity's transform/velocity |
| `+ - * / %`, unary `-` | arithmetic (f64) |
| `< <= > >= == !=` | comparisons → `1.0`/`0.0` |
| `and`/`&&`, `or`/`\|\|`, `not`/`!` | logic (nonzero = true) |
| `pi`, `e`, `t` | constants / clock |

A bare name resolving to no local/slot/param reads `0.0`. System params (e.g.
`dt`) are **not** in expression scope — but `let dt = 0.02` then use `dt`.

### 4.10 Builtin functions (complete; arity is compile-checked, detail 59)

There is **no `tan`** — write `sin(x)/cos(x)`.

* **1-arg math**: `sin cos exp ln sqrt abs floor ceil round sign log10 log2
  sinh cosh tanh asin acos atan`
* **2-arg math**: `pow atan2 hypot min max`
* **selection**: `if(c,a,b)` (both arms evaluated; the unused one discarded)
* **random (seeded)**: `random()` in `[0,1)`, `noise()` standard normal
* **I/O & events**: `print(x)`; `emit(kind,payload)`; `last_event(kind)`
  (payload of the latest matching event this step, else 0)
* **scheduling**: `at(T)` (1.0 in the step whose `[t,t+dt)` contains `T`);
  `periodic(P[,phase])` (1.0 once per P seconds; needs `P > dt`);
  `schedule(gate,delay,kind,payload)` (enqueue when `gate≠0`)
* **pools**: `active()` (current entity's flag)
* **vector helpers**: `vlen(x,y,z)`, `vdot(x1,y1,z1,x2,y2,z2)`,
  `vdist(x1,y1,z1,x2,y2,z2)`
* **spatial** (rules only; detail 70 elsewhere): `neighbor_count(r)`,
  `nearest_dist()`, `neighbor_mean(slot,r)`, `nearest_dx/dy/dz()`
  (position = Transform, else `state[0..2]`)
* **grid fields**: `fget`, `fset`, `flap` (§3.3)
* any other name = a user function from `funcs`

### 4.11 `funcs` — pure functions

```pwe
funcs {
  hooke(k, x) { 0.0 - k * x }        # expression body
  clamp01(v) { if (v < 0.0, 0.0, if (v > 1.0, 1.0, v)) }
}
```

Functions are **pure scalar functions of their arguments**: no world/entity/
spatial access, no writes. They may call other `funcs` and builtins. Namespaced
per module; call as `mod.fn(...)` or bare within the module.

### 4.12 Units (opt-in, compile-time)

```pwe
params { k = 4.0 [1/s^2] }
entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = e; dt = 0.1 [s]
    vx = 0.0 - k * x     # 1/s^2 · m · s = m/s  matches vx
}
```

Un-annotated values are wildcards and never error. A mismatch is detail 77.

### 4.13 Dynamic slots and loops

* `s[i]` reads / `s[i] = expr` writes the state slot at a runtime index
  (**`update` only**; rejected in `rk4` — detail 73).
* `repeat n { … }` (≤ 1000), `for i in lo..hi { … }` (ascending integers),
  `break`/`continue` (`break if (cond)`) — unrolled at lowering (≤ 10000
  statements). Loop bodies may contain only `let`, nested loops, and
  `break`/`continue`; slot rules stay outside. `for` binds the index as a local.

---

## 5. Presentation & the viewer

* Fields render as two nested isosurfaces (2D/3D) or a coloured curve (1D).
* Entities render from `shape`/`size`/`color`/`opacity`/`glow`/`label`.
  Custom shapes support primitives, `hull`, `poly` meshes, SVG, and composition.
* `rotation` (static), `orient` (simulation-driven yaw/lean), `vector=false`
  (hide arrows/rings), `label=false` (hide names).
* Soft bodies draw mesh bonds.
* Viewer controls: **⟳ Restart** (reload initial scene, paused at t=0),
  **⏸ Pause / ▶ Resume**, **🏷 Labels**.

---

## 6. Rules that bite (read before debugging)

1. **`slot = expr` integrates.** It is `slot += dt·expr`. To *assign*, write
   `slot = (target - slot)` (and use `dt = 1` if you want an exact write).
2. **A bare-number RHS is a system parameter, not a rule.** `x = 1.0` sets the
   parameter `x` (a no-op rule); write `x = 0.0 + 1.0` for a rule.
3. **Bare numbers as params must be `value`s**, or use an expression.
   `dt = 1/60` is fine; `dt = 0.016` is fine.
4. **Own reads are sampled once** at the start of a (sub)step: rules within one
   system are simultaneous. Across systems in a step, later systems **see**
   earlier writes (read-after-write), so order matters (`gravity` before
   `integrate`). Field reads see same-step writes; entity reads do not
   cross-system unless the earlier write is to the same component (§3.3).
5. **Pick one execution model per body** (§0.3). A `state`-driven body does not
   respond to `gravity`/`integrate`; a component body ignores `update` unless
   you also give it `state`.
6. **`nbody` mass is `state[6]`**, not the `mass` field.
7. **`on = <pool>` expands to all slots**; `active()` is only meaningful for pool
   bodies. `spawn` copies the caller's `state`.
8. **Reserved names**: `let` may not shadow `t`/`pi`/`e`/`sN` (detail 67).
9. **`state[7]` is the viewer's Z-spin** unless the body sets `orient = true`
   (then slots 7/8/9 are euler pitch/yaw/roll). Keep the gait/phase out of slot 7
   unless you mean a spin.
10. **Spatial queries work only in `update`/`rk4` rules and their `let`s** (detail
    70 in `funcs`). They are deterministic sorted-id scans.
11. **`fget/fset/flap` first argument is a literal field name**, arity 3/4 or 4/5.
12. **16 state slots max**; **loops ≤ 1000 iterations / 10000 statements**;
    dynamic-slot LHS is `update`-only.
13. **`invariant` fails the step before any write** — use it to keep bad states
    out rather than to "fix" them.
14. **Joints/soft need `mass`/`dynamic`** (a `rigid_body`) and use `Transform`
    positions; jointed bodies should be component bodies.
15. **Determinism**: never rely on hash-map order; ids are stable and ascending;
    `random()`/`noise()` are seeded per run.

---

## 7. Recipes (patterns for high-quality simulations)

**7.1 Harmonic oscillator (damped, driven)**
```pwe
entity m { state = (x = 1.0, vx = 0.0) }
update { on = m; dt = 0.01
  let k = 12.0 ; let c = 0.4 ; let F = 3.0
  vx = (0.0 - k*x - c*vx + F) + 0.0
  x  = vx
}
```
(Note the `+ 0.0` so each RHS is an expression.)

**7.2 Orbit / N-body** — use `nbody` with `state = (px,py,pz,vx,vy,vz,m)`
(see `solar.pwe`), or an explicit pairwise `update` for exotic forces.

**7.3 Diffusion (heat)**
```pwe
field heat { width = 16; height = 16; dx = 1.0 }
diffuse { field = heat; rate = 0.2 }
update  { on = probe; dt = 1.0 x = fget(heat, 8, 8) - x }   # sample the field
```

**7.4 Wave** — `wave { field = u; prev = um; velocity = v; dt = h; absorb = 0.08; absorb_width = 2 }`
(keep `c·h/dx ≤ 1/√2`).

**7.5 Particles (pool + forces + queries)**
```pwe
pool p[64] { state = (x=0.0, y=0.0, vx=0.0, vy=0.0); shape=sphere; size=0.1 }
spawn  { on = emitter; pool = p; count = 2; every = 1 }
update { on = p; dt = 0.1
  vx = 0.0 + 2.0*(neighbor_mean(0, 0.5) - x) + 0.0   # simple cohesion
  vy = 0.0 - 9.81*0.1 + 0.0
  x = vx ; y = vy
}
despawn { on = p; when = y < -10.0 }
```

**7.6 Chain / pendulum** — a `distance` `joint` per link (see `chain.pwe`);
make the top body `dynamic = false`.

**7.7 Soft body** — `soft` + `gravity` + `integrate` (§3.5).

**7.8 Events** — `emit(k, v)`; read with `last_event(k)`; schedule with
`at(T)`, `periodic(P)`, `schedule(gate,delay,k,v)`.

**7.9 State machine** — gate writes with `when = expr` (e.g. a cooldown slot) or
use `watch` to flip a mode slot on a zero crossing.

**7.10 Units** — annotate `state`/params; the compiler checks consistency (§4.12).

**7.11 Transforms & bodies** — component body (`position`/`velocity`/`mass`);
drive with `gravity`/`integrate`; add `joint`s for linkages.

**7.12 Triangle-mesh ground (no seams)** — build one `poly` shape from a sampled
heightfield and assign it to a static entity; people sample the same height
function:
```pwe
shape terrain { part poly = [ (x0,h,z0), … ] faces = [ [i,j,k], … ]; }
entity ground { position=(0,0,0); dynamic=false; shape = terrain; color = 0x6F8F4F; label=false }
```
See `courtyard.pwe` (mesh ground + winding path + turning/leaning people).

---

## 8. Diagnostics

Compile failures carry a message, a detail code, and a source offset:

```rust
match pwe_reference::lang::LangRuntime::compile(src) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(src, &e)),
}
```

```text
error 48: system 'update' is missing required parameter 'dt'
  --> line 13, column 9
    |
  13 |         update { on = reactor; dt = 0.0005
    |         ^
```

| Code | Meaning |
| --- | --- |
| 48 | Missing required system parameter. |
| 49 | Unknown system kind. |
| 50 | Cross-backend mismatch (runtime; a bug — please report). |
| 51 | Convex hull needs ≥ 4 points. |
| 52 | State slot index out of range (0..=15). |
| 53 / 54 | `linear` row missing / wrong length. |
| 55 | Invalid `funcs` body / empty `update` rule / bad slot LHS. |
| 56 / 57 / 58 | Expression / number / slot-reference parse failure. |
| 59 | Call arity mismatch. |
| 60 | Program parse failure. |
| 62 | Unknown entity or channel name. |
| 63 | `nbody` with no dynamic bodies. |
| 64 | Invalid color literal. |
| 65 | Invalid loop count (integer 1..=1000). |
| 66 | Loop unrolls beyond the statement limit. |
| 67 | `let` shadows a reserved token (`t`/`pi`/`e`/`sN`). |
| 68 | `for` range must be ascending integers. |
| 69 | Invariant violated. |
| 70 | Spatial query outside a system rule. |
| 71 | `every` must be an integer ≥ 1. |
| 72 | `substeps` must be an integer 1..=1000. |
| 73 | Dynamic slot LHS is `update`-only (rejected in `rk4`). |
| 75 | `field` needs `width` and `height` ≥ 1. |
| 76 | Import error (missing file, duplicate entity/field/channel/pool). |
| 77 | Dimensional mismatch. |
| 78 | Unknown pool name (`spawn`/`despawn`). |
| 79 | Unknown shape / shape-reference cycle. |

**Debugging workflow**: reduce to one entity + one system; check the model
(§0.3); check `slot = expr` integration (§6.1–6.2); add an `invariant` to catch
blow-ups; run `pwe run … --steps N` and read the printed state.

---

## 9. Standard library (`std/`)

Pure-function modules; constants are params (overridable with `--param`). See
`std/README.md` for full signatures.

| Module | Constants | Functions (representative) |
| --- | --- | --- |
| `math` | — | `clamp clamp01 lerp mix remap step smoothstep wrap sqr deg rad hypot2 hypot3 min3 max3 sgn deadzone ease_in/out` |
| `forces` | — | `hooke spring_accel damping_accel drag_linear/quadratic_accel coulomb_force gravity_force inverse_square_accel buoyancy_force thrust_accel damper_force` |
| `particles` | — | `terminal_velocity drag_step ballistic_x/y/vy bounce_vy reflect radius_from_mass stopping_distance freefall_time speed` |
| `mechanics` | — | `momentum kinetic_energy reduced_mass elastic_1d_v1/v2 impulse friction_force normal_impulse inertia_rod/disk/sphere torque angular_accel angular_kinetic` |
| `thermal` | `sigma_sb` | `celsius kelvin newton_cooling heat_capacity sensible_heat conduction_flux stefan_boltzmann radiative_cooling thermal_diffusivity thermostat_hysteresis mixing_temp` |
| `acoustics` | `rho_air`, `c_air`, `p_ref` | `speed_of_sound_air wavelength spl pressure_from_spl acoustic_impedance doppler inverse_square sound_intensity beat_frequency` |
| `optics` | `h_planck`, `c_light` | `inverse_square_intensity beer_lambert snell_angle critical_angle fresnel_reflectance reflect_axis wien_peak photon_energy focal_length` |
| `em` | `c_light`, `k_coulomb`, `mu0` | `coulomb_force electric_field potential lorentz_force cyclotron_radius biot_savart_wire poynting plane_wave_b/e impedance_free_space` |
| `chemistry` | `R_gas`, `avogadro` | `atomic_mass element_period/group shell_capacity valence_electrons neutrons mol_from_mass mass_from_mol molarity dilute ideal_pressure/volume arrhenius ph h_from_ph neutralization_volume half_life_decay radioactive_amount` |
| `robotics` | — | `planar2_x/y planar2_ik_q1/q2 pid joint_accel diff_drive_v_left/right trapezoid_peak reach rotate_x/y` |
| `control` | — | `first_order second_order low_pass complementary integrate derivative pid pid_clamped feedforward state_feedback bang_bang hysteresis rate_limit_delta lead within slew` |
| `units` | — | `kmh_to_ms`, `ev_to_j`, `atm_to_pa`, `deg_to_rad`, `g_to_ms2`, … |

```pwe
# paths are relative to this file; from cli/examples/ you would write "../../std/…"
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
}
```

---

## 10. Examples (`cli/examples/`)

Compile then run/present the `.pweb`.

| Example | Shows |
| --- | --- |
| `bounce.pwe` | `gravity` + `ground_contact` (component body). |
| `heat.pwe` | 2-D field + unrolled Gauss–Seidel `flap` sweep. |
| `solar.pwe` | `nbody` (state `px,py,pz,vx,vy,vz,m`) + orbit display. |
| `flock.pwe` | `neighbor_count` / `neighbor_mean` boids. |
| `spring/spring.pwe` | modules + params + units + `at`/`periodic`. |
| `domains.pwe` | composing `std/forces` + `std/thermal` + `std/em` + `std/chemistry`. |
| `wave.pwe`, `wave3d.pwe` | 1-D / 3-D `wave` solvers (3D with sponge absorption). |
| `acoustics.pwe` | 2-D sound + `std/acoustics` dB. |
| `robot.pwe`, `humanoid.pwe` | linkage / articulated figures with render attributes. |
| `shapes.pwe` | composite shapes, polyhedra, SVG. |
| `chain.pwe` | distance joints (pendulum chain). |
| `cloth.pwe`, `jelly.pwe` | soft bodies (sheet, 3D gel). |
| `particles.pwe` | pool + `spawn`/`despawn`. |
| `courtyard.pwe` | triangle-mesh ground + turning/leaning walkers (`orient`). |

---

## Appendix A — Canonical skeleton

```pwe
# 1. imports
import "std/forces"

# 2. world
world {
  title = "…"
  gravity = (0, -9.81, 0)
  params { k = 12.0 }
  entity a { position = (0, 5, 0) velocity = (1, 0, 0) mass = 1.0 sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (40,1,40); color = 0x557755 }
}

# 3. functions (pure)
funcs { accel(k, x) { 0.0 - k * x } }

# 4. systems (order matters)
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.5 }
  invariant      { on = a; expr = abs(x) < 1e6 }   # refuse blow-ups
}
```

## Appendix B — Stability checklist

* [ ] Each body uses one model (§0.3): component **or** state-slot.
* [ ] Rules are `slot = <expression>` (not bare numbers) and use the assign idiom
      when you mean assignment (§6.1–6.2).
* [ ] `on = <entity|pool>` set where intended.
* [ ] System order is force/gravity → integrate → constraints/boundaries.
* [ ] `dt` and solver stability limits respected (diffuse/wave).
* [ ] `invariant`s guard against NaN/blow-ups.
* [ ] Spatial queries only in rules; field ops take literal field names.
* [ ] State ≤ 16 slots; loops within limits; dynamic LHS only in `update`.
* [ ] Slot 7 reserved unless `orient = true`.
* [ ] `random()`/`noise()` acceptable (seeded, deterministic).
