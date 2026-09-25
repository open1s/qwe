# PWE Language Usage

[中文版](lang-usage.zh.md)

The PWE language is the textual front end of `pwe-reference` (`src/lang.rs`,
grammar `src/lang.pest`). A source program declares a world model and systems;
it compiles to low-level EIR and runs **cross-backend** — the interpreter and
the CPU JIT must produce byte-identical writes on every step.

```
PWE source ──parse──▶ WorldModel + system decls ──lower──▶ EIR ──interpret/JIT──▶ writes
```

```rust
let compiled = pwe_reference::lang::compile(SOURCE)?;      // parse → lower → EIR
let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
rt.step_cross()?;                                          // interpreter == JIT, asserted
rt.step_cross_n(30)?;                                      // 30 steps at once
```

```sh
pwe compile scene.pwe -o scene.pweb   # .pwe source → verified .pweb artifact
pwe run     scene.pweb --steps 600    # run the artifact (cross-backend each step)
pwe present scene.pweb --port 8000    # live browser 3D viewer
```

---

# Part I — Detailed language reference

## 1. Keywords, lexical rules & syntax

### 1.1 Lexical rules

| Token | Form | Notes |
| --- | --- | --- |
| comment | `# …` or `// …` | to end of line; skipped anywhere |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | names of entities, slots, params, functions, shapes |
| `number` | `-? digits ("." digits)? (("e"\|"E") "-"? digits)?` | f64 literal |
| `value` | `number` or `number "/" number` | a literal **or a ratio** (`dt = 1/60`) |
| `boolean` | `true` \| `false` | |
| `string` | `"…"` (no escapes) | titles, SVG path data |
| `color` | `0x` hex | `0xRRGGBB` |
| `unit` | `[` unit `]` | base `m kg s A K mol cd`, ops `*` `/` `^` — e.g. `[m/s^2]`, `[1/s]` |
| `slot` | `s` digits | own state slot by position (`s0`, `s1`, …); reserved |
| constants | `t`, `pi`, `e` | clock (s), π, Euler's number |

Whitespace is insignificant and `;` between statements/params is **optional**
(the grammar makes it `";"?`), so `dt = 0.1; x = 1.0` and `dt = 0.1` ⏎ `x = 1.0`
both parse. Units must be bracketed so `s[0]` stays unambiguous. A `let` may not
shadow `t`/`pi`/`e`/`sN` (detail 67).

### 1.2 Keywords

Genuine grammar tokens (cannot be used as identifiers):

* **sections**: `world`, `funcs`, `systems`
* **world**: `gravity`, `title`, `params`, `chan`, `value`, `entity`, `field`,
  `width`, `height`, `depth`, `dx`, `shape`, `part`
* **entity fields**: `position`, `velocity`, `state`, `vec`, `mass`, `dynamic`,
  `nbody`, `parent`, `restitution`, `friction`, `box`, `sphere`, `hull`,
  `camera`, `color`, `size`, `opacity`, `glow`, `label`
* **custom shapes**: `point`, `sphere`, `box`, `capsule`, `svg`, `hull`, `poly`,
  `at`, `depth`, `scale`, `faces`
* **functions / control**: `return`, `let`, `repeat`, `until`, `while`, `for`,
  `in`, `break`, `continue`, `if`
* **logic / booleans**: `and`, `or`, `not` (also `&&`, `||`, `!`), `true`, `false`
* **atoms**: `pi`, `e`, `t`

**Contextual (not reserved):** system *kinds* (`update`, `rk4`, `nbody`,
`diffuse`, `wave`, …) and system *parameters* (`on`, `when`, `every`, `substeps`,
`dt`, `field`, `prev`, `velocity`, `rate`, `iters`, `source`, `scale`, `damping`,
`absorb`, `mem`, `into`, …) are ordinary identifiers matched at build time — an
unknown kind is detail 49. `import` / `as` / `from` are handled by the module
loader; built-in function names (`sin`, `min`, `if`, `random`, `emit`, …) are
ordinary calls special-cased in lowering.

### 1.3 Syntax (EBNF)

```ebnf
program        = world_section funcs_section? systems_section?

world_section  = "world" "{" world_item* "}"
world_item     = gravity_stmt | title_stmt | params_stmt | chan_stmt
               | entity_stmt | field_stmt | shape_stmt
gravity_stmt   = "gravity" "=" vec3
title_stmt     = "title" "=" string
params_stmt    = "params" "{" (ident "=" value unit? ";")* "}"
chan_stmt      = "chan" ident "{" "value" "=" value ";" "}"
field_stmt     = "field" ident "{" param* "}"

entity_stmt    = "entity" ident "{" entity_field* "}"
entity_field   = position | velocity | state | mass | dynamic | nbody | parent
               | restitution | friction | box | sphere | hull | camera | color
               | shape | size | opacity | glow | label
state_field    = "state" "=" ( "(" state_item ("," state_item)* ")" | vecN )
state_item     = "vec" digits ident | ident "=" value unit? | value unit?
shape_stmt     = "shape" ident "{" shape_part* "}"
shape_part     = "part" shape_kind "=" part_value opt* ";"
part_value     = string | hull_list | vec3 | value
opt            = "at" vec3 | "depth" value | "scale" value | "faces" faces_list
shape_kind     = "point"|"sphere"|"box"|"capsule"|"svg"|"hull"|"poly"

funcs_section  = "funcs" "{" func_def* "}"
func_def       = ident "(" (ident ("," ident)*)? ")" "{" func_body "}"
func_body      = (func_item ";")+ "return" expr | expr
func_item      = let_stmt | repeat_stmt | for_stmt

systems_section= "systems" "{" system* "}"
system         = ident "{" param* "}"
param          = let_stmt | repeat_stmt | for_stmt | slot_lhs "=" expr | call
               | ident "=" ( vecN | expr | ident ) unit? ";"
slot_lhs       = "s" "[" expr "]"
let_stmt       = "let" ident "=" expr
repeat_stmt    = "repeat" number (("until"|"while") "(" expr ")")? "{" loop_item* "}"
for_stmt       = "for" ident "in" number ".." number "{" loop_item* "}"
loop_item      = let_stmt | repeat_stmt | for_stmt | break_stmt | continue_stmt
break_stmt     = "break" ("if" "(" expr ")")?
continue_stmt  = "continue" ("if" "(" expr ")")?

expr           = logical_or
logical_or     = logical_and (("or" |"||") logical_and)*
logical_and    = comparison  (("and"|"&&") comparison)*
comparison     = additive    (("<"|"<="|">"|">="|"=="|"!=") additive)*
additive       = term        (("+"|"-") term)*
term           = factor      (("*"|"/"|"%") factor)*
factor         = unary | number | call | "(" expr ")" | slot | slot_dyn
               | entity_ref | "t" | constant | namespaced | state_name
unary          = "-" factor | ("not"|"!") factor
call           = func_name "(" (expr ("," expr)*)? ")"
func_name      = ident ("." ident)*
slot           = "s" digits
slot_dyn       = "s" "[" expr "]"
entity_ref     = "@" ident "." (slot | prop_path)
prop_path      = ident ("." (ident | digits))*
namespaced     = ident "." ident ("." ident)*

vec3           = "(" value "," value "," value ")"
vecN           = "(" value ("," value)* ")"
hull_list      = "[" vec3 ("," vec3)* "]"
faces_list     = "[" face ("," face)* "]"
face           = "[" digits ("," digits)* "]"
unit           = "[" unit_atom (("*"|"/") unit_atom)* "]"
unit_atom      = ("mol"|"kg"|"cd"|"K"|"A"|"s"|"m") ("^" "-"? digits)? | digits
string         = '"' (any - '"')* '"'
number         = "-"? digits ("." digits)? (("e"|"E") "-"? digits)?
value          = number ("/" number)?
boolean        = "true" | "false"
color          = "0x" hex+
ident          = [A-Za-z_][A-Za-z0-9_]*
```

### 1.4 Operator precedence

Highest → lowest: unary `-` and `not`/`!` → `* / %` → `+ -` →
comparisons `< <= > >= == !=` → `and`/`&&` → `or`/`||`. Comparisons and logical
operators yield `1.0` / `0.0`; any nonzero operand is true. `not` binds to the
following factor — write `not (x > 0)` for a negated comparison.

## 2. Program structure

```pwe
world {
    # world items (required section)
}
funcs {
    # optional: user-defined pure functions
}
systems {
    # optional: systems that act on the world
}
```

Comments run from `#` or `//` to end of line and are skipped as whitespace
anywhere, including inside rule blocks. Whitespace is otherwise insignificant.
A `.pwe` file is also a **module** (see §4.8).

## 3. `world` — the world model

The world model describes *what the world is* (entities, fields, parameters).
It never depends on a CPU/GPU/OS.

| Statement | Meaning |
| --- | --- |
| `gravity = (x, y, z)` | Global uniform gravity vector. |
| `params { G = 1.0; k = 3.0 }` | Runtime-settable model parameters; rules read them by name, overridable with `--param G=2` on the same artifact. |
| `title = "..."` | Human-readable run title, shown by `pwe present`. |
| `entity <name> { fields }` | A body (§3.1). |
| `shape <name> { part … }` | A user-defined **custom shape** (§3.2). |
| `chan <name> { value = v }` | A channel entity; holds its latest value in `state[0]`. |
| `field <name> { width = w; height = h; dx = d }` | A deterministic scalar grid field — the PDE substrate (§3.3). |
| `import "pkg/mod"` | Python-style module import (§4.8). |

### 3.1 Entity fields

Entity ids are 1-based in declaration order; channels follow the bodies.

| Field | Meaning |
| --- | --- |
| `position = (x, y, z)` | Initial position. |
| `velocity = (x, y, z)` | Initial linear velocity. |
| `state = (v0, v1, …)` | Generic state slots (positional). Max 16 slots (`s0…s15`). |
| `state = (x = 0, y = 0, …)` | Named state slots; assign by name in rules, read via `@self.x`. Named and positional may be mixed. |
| `state = (vec3 pos, …)` | A vector element reserves N consecutive slots named `pos`, `pos.0` … `pos.{N-1}`; `pos` reads component 0, `@self.state.pos.1` component 1, `s[i]` indexes dynamically. |
| `mass = v` | Mass (drives `nbody`; the viewer derives visual size from it). |
| `dynamic = false` | Static body (default is dynamic). |
| `nbody = false` | Exclude from the mutual `nbody` system. |
| `restitution = v` / `friction = v` | Contact bounciness / tangential friction. |
| `box = (dx,dy,dz)` / `sphere = r` / `hull = [(x,y,z), …]` | Collider (hull needs ≥ 4 points). |
| `camera = true` | Marks the entity as the viewer camera (excluded from simulation). |

**Presentation-only fields** (they never affect simulation state, determinism,
or the state hash):

| Field | Meaning |
| --- | --- |
| `color = 0xRRGGBB` | Render color. |
| `shape = point \| sphere \| box \| capsule \| <custom>` | Render shape (overrides the collider-derived one; `<custom>` names a §3.2 shape). |
| `size = v \| (dx,dy,dz)` | Marker diameter / sphere radius / box edge, or per-axis box dims; for a custom shape, scales the whole shape. |
| `opacity = v` | Opacity in `[0, 1]`. |
| `glow = v` | Emissive glow intensity (0 = matte). |
| `label = false` | Hide the floating name label (default `true`). |

### 3.2 Custom shapes (primitives, polyhedra, SVG)

A world can declare named **custom shapes**; entities reference them by name.
A shape is a list of parts, each an offset primitive.

```pwe
world {
  shape drone {                       # composite primitives
    part capsule = (0.06, 0.30, 0.06);
    part sphere  = 0.09 at (0, 0.20, 0);
    part box     = (0.54, 0.02, 0.02) at (0, 0.20, 0);
  }
  shape octa {                        # convex polyhedron from vertices
    part hull = [(0,0.9,0), (0.9,0,0), (0,-0.9,0), (-0.9,0,0), (0,0,0.9), (0,0,-0.9)];
  }
  shape gem {                         # arbitrary polyhedron: vertices + faces
    part poly = [(0,0.9,0), (0.7,0,0.7), (-0.7,0,0.7), (-0.7,0,-0.7), (0.7,0,-0.7), (0,-0.9,0)]
      faces = [[0,1,2],[0,2,3],[0,3,4],[0,4,1],[5,1,4],[5,4,3],[5,3,2],[5,2,1]];
  }
  shape star {                        # extruded SVG path
    part svg = "M 0,-1 L 0.224,-0.309 L 0.951,-0.309 L 0.363,0.118 L 0.588,0.809 L 0,0.382 L -0.588,0.809 L -0.363,0.118 L -0.951,-0.309 Z" depth 0.22 scale 0.8;
  }
  entity craft { state = (x = 0.0, y = 0.0, z = 0.0) shape = drone; color = 0x4AC3FF }
}
```

`part <kind> = <params> [at (x,y,z)] [scale s]`:

* `sphere = r`, `box = (dx,dy,dz)`, `capsule = (r_bottom, length, r_top)` —
  a sphere / box / smooth (optionally tapered) surface of revolution;
* `hull = [(x,y,z), …]` — a **convex polyhedron** from its vertices;
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]` — an **arbitrary polyhedron**
  from explicit vertices and index faces (non-convex allowed; faces are
  triangulated by the viewer);
* `svg = "<path d>" depth <d>` — an SVG path extruded along Z.

`at` offsets the part in the entity's local frame; `scale` sets the part's size
(amplitude). An entity can scale the whole shape with `size = s`. Custom shapes
are **presentation only**.

### 3.3 Grid fields — the 4D continuum substrate

Declared `field <name> { width = w; height = h; dx = d }` (2D) or with
`depth = d` (3D). Cells are deterministic world state (snapshot/replayable).
Space is 3D — with the simulation clock, fields are the 4D substrate
(3D space + time).

* `fget(f, i, j)` / `fget(f, i, j, k)` — the cell value; sees same-step writes.
* `fset(f, i, j, v)` / `fset(f, i, j, k, v)` — writes a cell (bare call statement).
* `flap(f, i, j)` / `flap(f, i, j, k)` — the zero-flux discrete Laplacian scaled
  by `1/dx²` (5-point 2D, 7-point 3D).
* Unknown field names read `0` (unresolved-reference convention).

### 3.4 Entity pools — dynamic entities (RFC-0038)

A `pool` is a fixed block of pre-allocated entity slots that start **inactive**
and are brought into and out of existence at runtime, keeping the EIR static:

```
world {
  entity emitter { state = (x = -6.0, vx = 1.5) }
  pool p[24] { state = (x = 0.0, vx = 0.0) shape = sphere size = 0.18 }
}
systems {
  spawn   { on = emitter; pool = p }          # one free slot per step
  update  { on = p; dt = 0.1 x = 0.0 + vx }   # runs only on active slots
  despawn { on = p; when = x > 6.0 }          # recycle past the edge
}
```

* Slots are named `<pool>#0`, `<pool>#1`, … and their ids follow the declared
  entities and channels (a pool of `n` slots declared after `P` earlier slots
  starts at the next id).
* All slots start inactive. Inactive slots are skipped by user systems (their
  per-entity functions begin with `active()` guard) and hidden from the render
  view; `despawn` still runs on them to clear the flag.
* `spawn` activates the lowest-id free slots and copies the caller's state:
  `count = n` emits up to `n` slots per step (batch), and `every = k` /
  `phase = m` restrict emission to steps where `step % k == m` (phased).
  `active()` reads the current entity's flag (0/1).
* `active` is world state: it is hashed and snapshotted (old snapshots restore
  with every entity active). Everything is deterministic and byte-identical
  across the interpreter and JIT.

## 4. Systems — the behaviour

### 4.1 Built-in system kinds

| System | Params | Meaning |
| --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | Uniform gravity on dynamic bodies. |
| `integrate` | `dt` | Integrate velocity into position. |
| `damping` | `factor` | Scale velocities each step. |
| `ground_contact` | `restitution` | Resolve contact with the ground plane. |
| `wall` | `x`, `z`, `y_min?`, `restitution?` | Bounded domain; velocity reflects on impact. |
| `force` | `ax`, `ay`, `az`, `dt` | Constant acceleration on dynamic bodies. |
| `linear` | `slots`, `dt`, `row0 = (a0, …, c)` | Linear system `s_N' = Σ_j a_j·s_j + c`; each `rowN` has `slots+1` entries. |
| `nbody` | `G`, `dt` | Mutual inverse-square force among dynamic bodies (`G>0` gravity, `G<0` Coulomb). |
| `send` / `recv` | `chan`, `value` / `slot` | Go-style channel send/receive. |
| `update` / `rk4` | see §4.2 | User-defined ODE rules (Euler / Runge–Kutta 4). |
| `invariant` | `on?`, `expr`, `let …` | Per-step assertion (§4.3). |
| `watch` | `on?`, `expr`, `mem`, `into` | Zero-crossing detection (§4.4). |
| `diffuse` | `field`, `rate` | Explicit diffusion `T += rate·∇²T` (Jacobi sweep, exactly conservative). |
| `poisson` | `field`, `source?`, `iters`, `scale?` | Gauss–Seidel relaxation of `∇²φ = ρ·scale`. |
| `wave` | `field`, `prev`, `velocity`, `dt`, `damping?`, `absorb?`, `absorb_width?` | Second-order leapfrog wave equation (§4.5). |
| `spawn` | `on`, `pool`, `count?`, `every?`, `phase?` | Activate free pool slots per caller/step and copy the caller's state; `count` = slots per emission (batch), `every`/`phase` gate emission to `step % every == phase` (§3.4). |
| `despawn` | `on`, `when` | Deactivate every pool slot where `when` holds (§3.4). |
| `joint` | `on`, `other`, `type`, `length?`, `stiffness?`, `damping?`, `axis?`, `anchor?`, `limit?`, `iterations?` | Pairwise position-relaxation constraint (RFC-0039). `type` = `distance`/`spring`, `weld`/`hinge`/`revolute`/`ball`/`spherical`, or `prismatic`/`slider`. Rotational/ratio types (`cone`, `universal`, `gear`, `rack`, `pulley`) are rejected. |

**Joints (RFC-0039).** `joint` keeps a pairwise constraint by iterated position
relaxation (mass-weighted; a `dynamic = false` side acts as an infinite-mass
anchor). `distance`/`spring` hold `|a − b| = length` (`spring` adds velocity
`damping`); `weld`/`hinge`/`ball` coincide an `anchor` point offset from `a`;
`prismatic` keeps `b` on the line through `a` along `axis`, with an optional
`limit = (lo, hi)` on the along-axis separation. The engine is point-mass, so
the rotational DOF of `hinge`/`ball` is simply unconstrained. `gravity` +
`integrate` drive the motion. Example: `cli/examples/chain.pwe`.

Unknown system kind → detail code 49.

### 4.2 `update` / `rk4`

Both take rules `slot = expr`. `update` means **`slot += dt · expr(state)`**
(explicit Euler); `rk4` evaluates the same derivative four times per step and
combines them (4th-order). All reads happen first — own slots, cross-entity
refs, properties are read once per step, so rules within a step are
simultaneous.

```pwe
systems {
    update { on = target; dt = 0.02
        let omega = 0.5
        tx = -@self.ty * omega        # tx' = -ty·ω  (circular motion)
        ty = @self.tx * omega
    }
}
```

* `on = <name>` restricts to one entity (default: all dynamic bodies).
* `let name = expr` computes a reusable local before the rules run.
* `when = expr` gates every write (state untouched when 0) — mode/state-machine.
* `every = n` runs only when `step % n == 0`.
* `substeps = n` integrates n times with `dt/n` each.
* The LHS is `sN` or a named slot from the entity's `state = (…)` layout.
* Since `slot = expr` **integrates**, "assign" with `slot = (target - slot)` (so
  `slot += dt·(target-slot) = target`).

### 4.3 `invariant` — assertions

Evaluated per entity **after the systems run**; must be non-zero. A zero (or
NaN) fails the step (`detail 69`) **before any write is applied**, so the scene
keeps its pre-step state.

```pwe
invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
```

### 4.4 `watch` — zero-crossing detection

Each step it compares `expr` with the previous value (kept in the `mem` state
slot — persisted world state) and writes a 0/1 flag into `into`: 1 on a strict
sign change. The entity's own rules read the flag and react (bounce, switch
mode).

```pwe
watch { on = ball; expr = x; mem = 5; into = 6 }
```

### 4.5 Continuum solvers (`diffuse` / `poisson` / `wave`)

```pwe
systems {
  diffuse { field = heat; rate = 0.2 }                          # T += 0.2·∇²T
  poisson { field = phi; source = rho; iters = 20 }             # ∇²φ = ρ
  wave    { field = u; prev = um; velocity = 1.0; dt = 0.5 }    # u_tt = c²∇²u
}
```

* `diffuse` — one **Jacobi** sweep per step; under the zero-flux stencil the
  total is conserved exactly. Stable for `rate ≤ 1/4` (2D) / `≤ 1/6` (3D).
* `poisson` — `iters` in-place Gauss–Seidel sweeps; boundary cells are fixed
  potentials.
* `wave` — leapfrog over two fields (`prev` = `u(t−h)`); Courant
  `c·h/dx ≤ 1/√2` (2D) / `≤ 1/√3` (3D). `damping` (default `1.0` = lossless)
  scales the temporal term; `absorb` + `absorb_width` add a graded sponge layer
  that absorbs outgoing waves at the boundary instead of reflecting them.
* All solvers iterate in 3D when `depth > 1`, run **once per step**, are
  deterministic, and lower to the existing field opcodes (interpreter = JIT).

### 4.6 Expressions

| Form | Meaning |
| --- | --- |
| `s0`, `s1`, … | Own state slots. |
| `x` (named slot), `@self.x` | Own named state slot. |
| `@name.sN`, `@name.state.x`, `@name.x` | Another entity's state slot. |
| `@name.mass`, `@name.is_dynamic` | Another entity's properties. |
| `@name.position.x/y/z`, `@name.velocity.x/y/z` | Another entity's transform/velocity. |
| `+ - * / %`, unary `-` | Arithmetic (f64). |
| `< <= > >= == !=` | Comparisons → `1.0` / `0.0`. |
| `and`/`&&`, `or`/`\|\|`, `not`/`!` | Logical connectives (nonzero = true). Precedence: `not` > `and` > `or` > comparison. |
| `pi`, `e` | Constants. |
| `t` | Global simulation clock (seconds). |

A bare name that resolves to no local, slot, or parameter reads `0.0`
(unresolved-reference convention). System parameters such as `dt` are *not* in
expression scope.

**Builtins** — 1-arg: `sin cos exp ln sqrt abs floor ceil round sign log10 log2
sinh cosh tanh asin acos atan`; 2-arg: `pow atan2 hypot min max`; plus
`if(c,a,b)`, `random()` (seeded), `noise()` (seeded normal), `print(x)`,
`emit(kind,payload)`, `last_event(kind)`. Vector helpers: `vlen`, `vdot`,
`vdist`. Spatial: `neighbor_count(r)`, `nearest_dist()`,
`neighbor_mean(slot,r)`, `nearest_dx/dy/dz()` (rules only).

**Scheduling**: `at(T)` — 1.0 in the one step whose window `[t,t+dt)` contains
`T`; `periodic(P[,phase])` — 1.0 once per period.

**Units** are opt-in and compile-time checked (wildcards never error):

```pwe
params { k = 4.0 [1/s^2] }
entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = e; dt = 0.1 [s]
    vx = 0.0 - k * x     # 1/s^2 · m · s = m/s  matches vx
}
```

**Dynamic slots**: `s[i]` reads / `s[i] = expr` writes the slot at a runtime
index (update rules only; rejected in `rk4`, detail 73). **Loops**:
`repeat n { … }`, `for i in lo..hi { … }`, `break`/`continue` — unrolled at
lowering (≤ 1000 iterations, ≤ 10000 statements); loop bodies hold only `let`,
nested loops and `break`/`continue`.

### 4.7 Built-in (intrinsic) functions — complete reference

Arity is checked at compile time (mismatch → detail 59). A call whose name is
not listed below is a user-defined function resolved from `funcs` (existence and
arity validated at lowering). There is **no `tan`** — write `sin(x)/cos(x)`.

**Elementary math**

| Call | Result |
| --- | --- |
| `sin(x)`, `cos(x)` | trigonometric, radians |
| `tanh(x)`, `sinh(x)`, `cosh(x)` | hyperbolic |
| `asin(x)`, `acos(x)`, `atan(x)` | inverse trigonometric, radians |
| `atan2(y, x)` | angle of the point `(x, y)` in `(-π, π]` |
| `exp(x)` | eˣ |
| `ln(x)`, `log10(x)`, `log2(x)` | natural / base-10 / base-2 logarithm |
| `sqrt(x)` | √x (negative → NaN; halves unit exponents) |
| `pow(a, b)` | aᵇ |
| `hypot(a, b)` | √(a²+b²) |
| `abs(x)` | absolute value |
| `floor(x)`, `ceil(x)`, `round(x)` | integer, downward / upward / half-away-from-zero |
| `sign(x)` | −1, 0 or 1 |

**Selection**

| Call | Result |
| --- | --- |
| `if(c, a, b)` | `a` when `c ≠ 0`, else `b` (both arms are evaluated; the unused one is discarded) |
| `min(a, b)`, `max(a, b)` | smaller / larger of two values |

**Randomness (seeded, replay-stable)**

| Call | Result |
| --- | --- |
| `random()` | uniform draw in `[0, 1)` |
| `noise()` | standard normal (Box–Muller over two draws); always finite |

**I/O & events**

| Call | Effect |
| --- | --- |
| `print(x)` | logs `x`, returns it unchanged (never mutates world state) |
| `emit(kind, payload)` | appends an ordered event `(kind, payload)`, returns `0.0` |
| `last_event(kind)` | payload of the most recent event of that `kind` emitted so far this step (`0` when none). Events are cleared each step; the host reads them via `emitted_events()`. `kind` is a numeric value, not a bit pattern. |

**Scheduling (exactly-once, on the step grid)**

| Call | Fires |
| --- | --- |
| `at(T)` | `1.0` in the one step whose window `[t, t+dt)` contains `T`, else `0.0` |
| `periodic(P[, phase])` | `1.0` once per `P` seconds (optional phase offset); requires `P > dt` |
| `schedule(gate, delay, kind, payload)` | when `gate ≠ 0`, enqueue `(kind, payload)` to fire `delay` seconds later — a dynamic event queue, drained deterministically (part of the cross-backend contract) |

**Spatial queries** (valid only inside system rules and their `let` blocks; detail 70 in a function body)

| Call | Result |
| --- | --- |
| `neighbor_count(r)` | number of other bodies within `r` of self |
| `nearest_dist()` | distance to the nearest other body (`f64::MAX` when alone) |
| `neighbor_mean(slot, r)` | mean of state slot `slot` over neighbours within `r` (`0` when none) |
| `nearest_dx()`, `nearest_dy()`, `nearest_dz()` | per-axis offset `(nearest − self)` (`0` when alone) |

All are deterministic (sorted-id scans). Position is `Transform`, else `state[0..2]`.

**Vector helpers** (pure arithmetic over scalar components)

| Call | Result |
| --- | --- |
| `vlen(x, y, z)` | √(x²+y²+z²) |
| `vdot(x1,y1,z1, x2,y2,z2)` | x1·x2 + y1·y2 + z1·z2 |
| `vdist(x1,y1,z1, x2,y2,z2)` | distance between the two points |

**Grid-field access** — the first argument must be a literal field name (2-D or 3-D)

| Call | Effect |
| --- | --- |
| `fget(f, i, j)` / `fget(f, i, j, k)` | read a cell (sees same-step writes) |
| `fset(f, i, j, v)` / `fset(f, i, j, k, v)` | write a cell (also usable as a bare statement; yields `0.0`) |
| `flap(f, i, j)` / `flap(f, i, j, k)` | discrete Laplacian (zero-flux, scaled by `1/dx²`) |

### 4.8 Modules and packages

A `.pwe` file is a module. `import` follows Python:

```pwe
import "physics"                 # physics.G, physics.thrust(m)
import "physics" as ph           # ph.G
from "physics" import thrust     # thrust(m)  (bare)
```

A **package** is a directory (`import "shapes"` → `shapes/__init__.pwe`).
Functions and parameters are namespaced by module; a module's own rules resolve
their bare names in their namespace first, then globally. Entities / systems /
fields / channels merge flatly (duplicate names are an error, detail 76).
Circular imports resolve; members register under every alias.

## 5. Standard library (`std/`)

Pure-function modules: `math`, `particles`, `forces`, `mechanics`, `chemistry`
(periodic table 1–118), `thermal`, `acoustics`, `optics`, `em`, `robotics`,
`units`, `control`. Physical constants are params (`chemistry.R_gas`,
`thermal.sigma_sb`, `em.k_coulomb`, …), overridable with `--param`. See
`std/README.md`.

```pwe
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
}
```

---

# Part II — Usage

```sh
pwe compile <src.pwe> -o <out.pweb> [--param K=V]   # parse → verify → .pweb
pwe run     <out.pweb> [--steps N] [--param K=V]... # run the artifact
pwe present <out.pweb> [--port P] [--param K=V]...  # live 3D viewer
```

* An artifact is a self-describing container (magic + version) holding the
  verified canonical EIR plus the model source; `run`/`present` execute the
  compiled EIR. Compile errors render the offending source line with a caret.
* `--param K=V` overrides a declared model parameter (validated; aliases update
  together).
* The live viewer polls the runtime and offers **⟳ Restart** (reload the initial
  scene, paused at t=0), **⏸ Pause / ▶ Resume**, and **🏷 Labels** (show/hide all
  names). Fields render as an isosurface (2D/3D) or a colored curve (1D);
  entities render from their `shape`/`size`/`color`/`opacity`/`glow`/`label`.
* Build demos in release for smooth playback: `cargo build --release -p pwe-cli`.

---

# Part III — Examples: what they show, how to run, what you see

All under `cli/examples/`. Compile once, then run/present the `.pweb`.

| Example | Shows | Run | What you see |
| --- | --- | --- | --- |
| `bounce.pwe` | `gravity` + `ground_contact` | `pwe run bounce.pweb --steps 300` | A ball falls and bounces with restitution; the probe's state oscillates. |
| `heat.pwe` | 2-D grid field, unrolled Gauss–Seidel `flap` sweep | `pwe run heat.pweb --steps 400` | A held-hot centre spreads into a steady radial profile; the centre temperature stabilizes. |
| `solar.pwe` | `nbody` + `update` (sun + 8 planets + moon) | `pwe present solar.pweb` | A glowing sun with orbiting planets, orbit rings, a per-body legend. |
| `flock.pwe` | `neighbor_count` / `neighbor_mean` | `pwe present flock.pweb` | Many agents coalesce and align into a flock. |
| `spring/spring.pwe` | modules + params + units + `at`/`periodic` | `pwe run spring.pweb --steps 400 --param k=16` | A damped spring driven by a periodic kick; changing `k` changes the frequency. |
| `domains.pwe` | composing `std/forces` + `std/thermal` + `std/em` + `std/chemistry` | `pwe run domains.pweb --steps 200` | A probe on a damped spring cools radiatively toward ambient. |
| `wave.pwe` | 1-D `wave` solver, sine standing wave | `pwe run wave.pweb --steps 40` | The probe swings between −1 and +1 (total conserved at 0); `present` draws an energy-colored sine curve. |
| `wave3d.pwe` | 3-D `wave` + sponge absorption | `pwe run wave3d.pweb --steps 60` | Total rises to 1 on each pulse, then decays; `present` shows a translucent blue spherical shell expanding and fading. |
| `acoustics.pwe` | 2-D sound + `std/acoustics` dB | `pwe run acoustics.pweb --steps 80` | A driven monopole radiates; probes at increasing distance register arrival delay and level in dB. |
| `robot.pwe` | 2-link arm, `std/robotics` FK, render attributes | `pwe present robot.pweb` | A waving 2-link arm: thin-box links, sphere joints, a red end-effector. |
| `humanoid.pwe` | 62-part figure: capsule limbs, facial detail, fingers | `pwe present humanoid.pweb` | A person-like figure walking; labels off by default (press 🏷 to show). |
| `shapes.pwe` | custom shapes: composite, polyhedra, SVG | `pwe present shapes.pweb` | A drone (primitives), an extruded SVG star, a convex octahedron, and an explicit-face gem. |

Quick recipes:

```sh
# A pulse radiating through a 3-D cube, from t = 0
cargo build --release -p pwe-cli
./target/release/pwe compile cli/examples/wave3d.pwe -o wave3d.pweb
./target/release/pwe present wave3d.pweb --port 8000   # ⟳ Restart, then ▶ Resume
```

```
# observed: field u total=1.000000 at n=3, then decays (sponge absorbs it)
$ ./target/release/pwe run wave3d.pweb --steps 60   # ... field u: 17x17x17 dx=1 total=...
```

---

# Part IV — Diagnostics & error codes

Compile failures carry a human message, a detail code, and a source offset:

```rust
match pwe_reference::lang::LangRuntime::compile(source) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(source, &e)),
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
| 48 | Missing required system param. |
| 49 | Unknown system kind. |
| 51 | Convex hull needs ≥ 4 points. |
| 52 | State slot count out of range (1..=16). |
| 53 / 54 | `linear` row missing / wrong length. |
| 55 | Invalid `funcs` body / empty `update` rule / bad slot LHS. |
| 56 / 57 / 58 | Expression / number / slot-reference parse failure. |
| 59 | Call arity violation. |
| 60 | Program parse failure. |
| 62 | Unknown entity or channel name. |
| 64 | Invalid color literal. |
| 69 | Invariant violated. |
| 70 | Spatial query outside a rule. |
| 73 | Dynamic slot LHS in `rk4`. |
| 76 | Import/module error (missing file, duplicate entity/field). |
| 77 | Dimensional mismatch. |

---

# Part V — Semantics notes

* **Time is explicit**: `dt` multiplies every rule's expression; the clock `t`
  advances by `dt` each step.
* **Reads-before-writes**: within a step, every referenced value is the value
  from the *start* of the step.
* **Determinism is first-class**: `random()` is seeded and replay-stable; the
  interpreter and JIT agree byte-for-byte every step (`step_cross`).
* **State slots** are capped at 16 per entity.
* **A general substrate**: because rules are plain scalar ODEs over named state
  slots, with locals, functions, cross-entity references, `random()`/`emit()`,
  and RK4/Euler integration, the language is not tied to rigid-body physics —
  any (coupled) differential or difference law can be modeled, simulated, and
  visualized.
