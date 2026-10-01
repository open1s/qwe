# PWE Language Guide

[中文版](lang-usage.zh.md)

A complete, self-contained guide to writing **PWE** models and simulations. It
assumes **no prior experience** with PWE (or with simulation code). Read it
front to back once; afterwards use it as a reference.

**How to read it**

* **Part 0–1 (Tutorial)** — learn by building, one runnable program at a time.
  If you are new, start here.
* **Part 2 (Reference)** — every keyword, field, system, builtin, and rule.
* **Part 3 (Semantics & pitfalls)**, **Part 4 (Cookbook)**, **Part 5 (Tools)**,
  **Part 6 (Diagnostics)**, **Part 7 (std)**, **Part 8 (Examples)**.
* Appendices: a copy-paste **skeleton** and a **stability checklist**.

Every code block here compiles and runs as written.

---

# Part 0 — Getting started

## 0.1 What is a simulation, in one paragraph

A simulation is **some numbers that change over time according to rules**. The
numbers are the **state** (a ball's position and velocity; a temperature field;
…). You advance time in fixed **steps**: each step you read the current state,
compute how it changes, and write the new state. The size of a step is `dt`
(seconds). A **system** is one such rule; the **world** is the collection of
everything that has state. That is the whole idea — PWE just makes it precise,
fast, and reproducible.

## 0.2 Install and the three commands

```sh
cargo build --release -p pwe-cli          # build the `pwe` tool (once)

pwe compile scene.pwe -o scene.pweb       # source  → checked artifact
pwe run     scene.pweb --steps 600        # run N steps, print final state
pwe present scene.pweb --port 8000        # live 3D viewer at localhost:8000
```

`compile` catches mistakes with the offending line and a caret. `--param K=V`
(at `run`/`present`) overrides a model parameter — handy for experiments.

## 0.3 Your first program: a falling ball

```pwe
world {
  gravity = (0, -9.81, 0)
  entity ball   { position = (0, 5, 0) velocity = (3, 0, 0) sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (20, 1, 20); color = 0x557755 }
}
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.6 }
}
```

Run it: `pwe run ball.pweb --steps 300`. Observed tail:

```text
ran 300 steps (interpreter == JIT, every step)
  sim time:  5.000000 s
  #1   ball   pos = ( 14.4000,  0.0000, 0.0000)
  #2   ground pos = (  0.0000, -0.5000, 0.0000)
```

**Line by line**

* `world { … }` declares *what exists*.
* `gravity = (0, -9.81, 0)` sets world gravity (downward, y).
* `entity ball { … }` declares a **body**. `position`/`velocity` are its initial
  transform/velocity; `sphere = 0.3` is a collider (radius 0.3); `color` is
  presentation.
* `entity ground { … dynamic = false … }` is a **static** body (never moves);
  `box = (20,1,20)` is a box collider 20×1×20.
* `systems { … }` declares *behaviour*, applied **in order each step**:
  * `gravity` adds `gravity_y·dt` to every dynamic body's y-velocity.
  * `integrate` adds `velocity·dt` to position.
  * `ground_contact` stops/reflects bodies that would pass through `y = 0`
    (with `restitution = 0.6`, so it bounces and loses energy).

**What you saw**: the ball accelerated downward, bounced (each bounce lower),
settled on the ground, and drifted in x because of its initial `velocity.x = 3`.

## 0.4 Make it yours (change one thing at a time)

* Set `restitution = 1.0` — it bounces forever (elastic).
* Set `restitution = 0.0` — it lands and stops.
* Set `velocity = (6, 2, 0)` — faster and upward.
* Change `dt` in **both** systems to `0.008` — same physics, smaller steps.

Then re-run (`pwe compile … && pwe run …`). This edit → run → observe loop is
how you build every model.

## 0.5 The five ideas you now know

1. **world** = the state (entities, fields, parameters).
2. **entity** = a body with state; ids are 1-based in declaration order.
3. **systems** = rules that run once per step, in order.
4. **dt** = the step size; every system uses it.
5. **run** = repeat the systems over and over, committing new state each step.

## 0.6 The one idea that trips everyone up: two ways to store a body

PWE has **two independent stores** for a body's dynamics, and a body uses one of
them. If you put numbers in the wrong one, "nothing happens".

| | **Component body** | **State-slot body** |
| --- | --- | --- |
| Written as | `position`/`velocity`/`mass`/colliders | `state = (…)` |
| Driven by | `gravity`, `integrate`, `damping`, `force`, `wall`, `ground_contact` | `update`, `rk4`, `linear`, `nbody` |
| Position is | `Transform.position` | `state[0..2]` (render fallback) |
| Read from rules as | `@name.position.x`, `@name.velocity.x` | `x`, `@name.x`, `s0`, … |

The ball above is a **component body** — that's why `gravity`/`integrate` move it.
The next lesson uses a **state-slot body** so *you* write the physics. A body can
have both, but keep it simple: **pick one**.

---

# Part 1 — Tutorial: build simulations step by step

Each lesson is a complete, runnable program with what to expect.

## L1 — Write your own physics: a damped oscillator

Component systems give you Newtonian motion for free. For anything else, put the
numbers in `state` and write the rules yourself.

```pwe
world {
  gravity = (0, 0, 0)
  entity m { state = (x = 1.0, vx = 0.0) }
}
systems {
  update { on = m; dt = 0.01
    let k = 12.0       # spring constant
    let c = 0.4        # damping
    vx = vx + inte(0.0 - k*x - c*vx)  # vx' = -k·x - c·vx
    x  = x + inte(vx)                 # x'  = vx
  }
}
```

Run: `pwe run osc.pweb --steps 200` → `state = [0.6302, -1.5751]`. The mass swings
and slowly loses energy (damped), converging to 0.

**The crucial rule.** `slot = expr` is a plain **assignment**: each step the
slot takes that value. To *integrate* a derivative, make the step explicit with
`dt`:

* `vx = vx + inte(0.0 - k*x - c*vx)` ⇒ `vx += dt·(−k·x − c·vx)` (acceleration).
* `x = x + inte(vx)` ⇒ `x += dt·vx` (velocity).

The **`inte(E)` operator** is the increment `dt·E`, so `x = x + inte(vx)` equals
`x = x + dt*(vx)`; the **`inte slot = rate`** statement (also `integrate` and
`+=`) integrates without spelling out `dt`. The **`deriv(E)` operator** is the
backward difference `(E − E_prev)/dt` — it *differentiates* (0 on the first
step). In `rk4`, `inte slot = rate` integrates that rate with the classic
4th-order Runge–Kutta method (much more accurate than Euler at the same `dt`).

**Recursion.** A function body may use the control-flow form
`if cond { return a } else { return b }` (lazy): each call pushes a frame on the
call stack, so functions may recurse — e.g.
`fact(n) { if n < 1.0 { return 1.0 } else { return n * fact(n - 1.0) } }`. The
expression `if(c, a, b)` evaluates *both* branches, so it cannot be used for
recursion.

**Assign vs integrate.** Since `=` assigns, a constant write is just
`slot = target` — no idiom needed.

**One gotcha, right away:** `let` names may not be `t`, `pi`, `e`, or `sN`
(detail 67). (A rule `x = 1.0` is fine — see below.)

**Parameters vs rules.** A numeric parameter is recognised **by name** for the
system kind: `update { dt = 0.01 }` sets `dt`, while `update { x = 1.0 }` is a
rule for the slot `x` (no need for the old `0.0 + 1.0` trick).

Exercises: raise `k` (faster); raise `c` (dies out sooner); add a driving term
`+ 3.0*sin(2.0*t)` to `vx` for a driven oscillator.

## L2 — Parameters: run experiments without editing code

```pwe
world {
  gravity = (0, 0, 0)
  params { k = 12.0; c = 0.4 }
  entity m { state = (x = 1.0, vx = 0.0) }
}
systems {
  update { on = m; dt = 0.01
    vx = vx + dt*(0.0 - k*x - c*vx)
    x  = x + dt*(vx)
  }
}
```

Now the constants live in `params`, readable by name in rules. Sweep them without
recompiling:

```sh
pwe run osc.pweb --steps 200 --param k=40
pwe run osc.pweb --steps 200 --param k=40 --param c=0.0   # undamped
```

Parameters are part of world state and recorded in snapshots, so a run is fully
reproducible from its parameters.

## L3 — Many bodies and interactions

**Read another body** with `@name.<slot>`:

```pwe
world { gravity = (0, 0, 0)
  entity a { state = (x = 1.0,  vx = 0.0) }
  entity b { state = (x = -1.0, vx = 0.0) }
}
systems {
  update { on = a; dt = 0.01 vx = vx + dt*(0.0 - 8.0*(x - @b.x))   x = x + dt*(vx) }
  update { on = b; dt = 0.01 vx = vx + dt*(0.0 - 8.0*(x - @a.x))   x = x + dt*(vx) }
}
```

Two masses on a spring between them: they oscillate in anti-phase (equal and
opposite). `@b.x` is b's slot `x`; reads are sampled at the start of the step, so
both rules see each other's previous value.

**Gravity between bodies** is built in — `nbody` expects
`state = (px, py, pz, vx, vy, vz, m)` (mass in **slot 6**, *not* the `mass` field):

```pwe
world { gravity = (0, 0, 0)
  entity sun   { state = (0, 0, 0,  0,    0,    0,  1000) sphere = 1.2 }
  entity earth { state = (4, 0, 0,  0,    15.8, 0,  0.05) sphere = 0.3 }
}
systems { nbody { G = 1.0; dt = 0.001 } }
```

`earth` orbits `sun`: radius stays ≈ 4 (at 500 steps it is near `(-1.60, 3.65)`,
a quarter-turn; a full orbit is ≈ 1571 steps). This is `solar.pwe` in miniature.

## L4 — Fields and PDEs (heat, waves)

A `field` is a grid of numbers (the continuum substrate). Systems `diffuse`,
`poisson`, `wave` solve PDEs on it; rules read/write cells with `fget`/`fset`.

Heat: seed the centre once and let it spread (total is conserved).

```pwe
world { gravity = (0, 0, 0)
  field heat { width = 16; height = 16; dx = 1.0 }
  entity probe { state = (x = 0.0) }
}
systems {
  diffuse { field = heat; rate = 0.2 }          # T += 0.2·∇²T
  update { on = probe; dt = 1.0
    # seed the centre at t=0; every later step writes the same value back
    let _ = fset(heat, 8, 8, if(at(0.0), 100.0, fget(heat, 8, 8)))
    x = fget(heat, 8, 8) - x                     # sample the centre
  }
}
```

Observed: `field heat: total=100.000000` at every step (conserved), while the
centre cools `100 → 2.05` (20 steps) `→ 0.40` (200 steps) as heat spreads out.

A wave is a second-order PDE solved by a leapfrog over two fields:

```pwe
world { gravity = (0, 0, 0)
  field u  { width = 41; height = 1; dx = 1.0 }   # height=1 ⇒ a 1-D line
  field um { width = 41; height = 1; dx = 1.0 }   # u(t-dt)
  entity probe { state = (x = 0.0) }
}
systems { wave { field = u; prev = um; velocity = 1.0; dt = 0.5 } }
```

Stability limits matter: `rate ≤ 1/4` (2D) for `diffuse`; Courant
`c·dt/dx ≤ 1/√2` (2D) for `wave`. Exceed them and the field blows up.

## L5 — Dynamic entities: a particle system (pools)

Entities are fixed at compile time. To have entities appear and disappear, declare
a **pool** of initially-inactive slots and `spawn`/`despawn` them.

```pwe
world { gravity = (0, 0, 0)
  entity emitter { state = (x = 0.0, y = 0.0, vx = 0.0, vy = 0.0) }
  pool p[64] { state = (x = 0.0, y = 0.0, vx = 0.0, vy = 0.0); shape = sphere; size = 0.1 }
}
systems {
  spawn  { on = emitter; pool = p; count = 2; every = 1 }   # 2 slots/step
  update { on = p; dt = 0.1
    vx = vx + dt*(2.0*(neighbor_mean(0, 0.5) - x))         # simple cohesion
    vy = vy + dt*(0.0 - 9.81*0.1)                                # gravity
    x = x + dt*(vx)
    y = y + dt*(vy)
  }
  despawn { on = p; when = y < -10.0 }                       # recycle
}
```

* `pool p[64]` makes 64 inactive slots named `p#0`…`p#63`.
* `spawn` copies the **caller's** (`emitter`) state into the lowest free slots.
* `update { on = p; … }` runs **only on active** slots (`on = <pool>` expands to
  all slots).
* `despawn` turns matching slots off so they can be reused.
* `neighbor_count(r)` / `neighbor_mean(slot, r)` / `nearest_dist()` work here —
  they are deterministic, id-sorted scans.

Observed: 64 particles active after 40 steps, falling and cohering.

## L6 — Constraints: a pendulum chain (joints)

Joints are pairwise, position-based constraints between component bodies.

```pwe
world { gravity = (0, -9.81, 0)
  entity anchor { position = (0, 6, 0) mass = 1.0 dynamic = false }
  entity b1     { position = (0, 5, 0) mass = 1.0 dynamic = true }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  joint { on = anchor; other = b1; type = distance; length = 1.0; iterations = 8 }
}
```

`type = distance` keeps `|anchor − b1| = 1.0`. `dynamic = false` on `anchor`
makes it an infinite-mass pivot. Add a `b2` and a second joint for a longer chain
(see `chain.pwe`). Other kinds: `spring` (adds `damping`), `hinge`/`ball`/`weld`
(pin an `anchor` point), `prismatic` (slide along `axis`, optional `limit`).

## L7 — Soft bodies (a sheet or a gel)

```pwe
world { gravity = (0, -9.81, 0)
  soft cloth { nx = 6; ny = 6; spacing = 0.4; origin = (-1, 5, 0); mass = 0.1 }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  soft      { body = cloth; stiffness = 1.0; iterations = 6 }
}
```

`soft` creates an `nx × ny` (× `nz`) grid of particles joined by structural,
shear, and bend springs; the `soft` system relaxes them so the sheet keeps its
spacing while sagging. Mesh edges render as bonds. `nz > 1` gives a 3-D gel
(`jelly.pwe`).

## L8 — Events and scheduling

```pwe
world { gravity = (0, 0, 0) entity e { state = (x = 0.0) } }
systems {
  update { on = e; dt = 0.1
    let fired = at(0.5)     # 1.0 in the one step whose [t, t+dt) contains 0.5
    x = x + dt*(fired)      # integrate the trigger into x
    emit(1.0, x)            # append an event (kind=1, payload=x)
  }
}
```

* `emit(kind, payload)` records an ordered event; the host reads them via the API,
  and rules can read the latest with `last_event(kind)`.
* `at(T)` fires once when the clock reaches `T`; `periodic(P[,phase])` fires once
  per period; `schedule(gate, delay, kind, payload)` enqueues a future event.
* Events and their queue are part of the deterministic cross-backend contract.

## L9 — Make it look right (presentation)

Presentation never affects the simulation. Attributes:

```pwe
entity ball { position = (0, 5, 0) sphere = 0.3; color = 0xFF6B4A; opacity = 1.0; glow = 0.3; label = true }
```

* `color = 0xRRGGBB` (exactly 6 hex digits), `opacity` `[0,1]`, `glow` (emissive), `label = false` hides
  the name, `size = v | (dx,dy,dz)` sets marker/box size. These are checked:
  an `opacity` outside `[0,1]` is detail 105, a malformed colour detail 64, and every field of one
  `entity` / `pool` body may appear **at most once** (a repeat is detail 107 — `tag` is the
  exception, it accumulates — and an entity has at most one collider, `box` / `sphere` / `hull`).
* `shape = point | sphere | box | capsule | <custom>` overrides the collider shape.
* **Custom shapes** compose primitives, `hull`/`poly` polyhedra, SVG, and each
  other:

  ```pwe
  world {
    shape head { part sphere = 0.115 at (0, 0, 0); }
    shape person {
      part box  = (0.34, 0.46, 0.20) at (0, 0.57, 0);
      part head at (0, 0.92, 0);          # include another shape
    }
    entity e { state = (x=0,y=0,z=0) shape = person; color = 0x4AC3FF }
  }
  ```

* `rotation = (rx,ry,rz)` tilts a body statically (e.g. a ramp).
* `orient = true` lets the **simulation** steer a state body's facing/lean: state
  slots 7/8/9 are euler `(pitch, yaw, roll)`. Example (yaw from heading + lean):

  ```pwe
  entity walker { state = (x=0,y=0,z=0, vx=1.0, vz=0.0, dx=1.0, dz=0.0, rx=0.0, ry=0.0, rz=0.0)
                  orient = true; shape = person }
  systems { update { on = walker; dt = 1.0
    ry = (atan2(vx, vz) - ry)     # face the heading
    rx = (0.12 - rx)              # lean forward
    x = 0.05*vx
    z = 0.05*vz
  } }
  ```

* `vector = false` hides the viewer's velocity arrow / orbit ring.
* **Triangles, no seams:** build one `poly` mesh from a sampled heightfield for a
  smooth ground (`courtyard.pwe`).

## L10 — Organise: functions, modules, units

Pure helper functions live in `funcs`:

```pwe
funcs { accel(k, x) { 0.0 - k * x } }
systems { update { on = m; dt = 0.01 vx = vx + dt*(accel(k, x))   x = x + dt*(vx) } }
```

Functions take arguments and return a scalar; they may call other functions and
builtins, but have **no access to the world** (no entities, no `@refs`, no
spatial queries).

**Modules** split a model across files. A module is a `.pwe` file (with an empty
`world { }` if it only defines funcs); import paths are **relative to the
importing file**:

```pwe
# pkg/util.pwe
world { }
funcs { twice(v) { 2.0 * v } }

# main.pwe
import "pkg/util"                      # from cli/examples/ you'd write "../../std/…"
world { gravity = (0,0,0) entity e { state = (x=0.0) } }
systems { update { on = e; dt = 0.1 x = util.twice(1.0) } }
```

**Units** are optional but catch mistakes at compile time:

```pwe
params { k = 12.0 [1/s^2] }
entity m { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = m; dt = 0.01 [s] vx = vx + dt*(accel(k, x)) }
```

## L11 — Exact stochastic kinetics (`gillespie`)

Some systems are not smooth at all: a few molecules reacting one at a time. An
ODE there describes the *mean*, not the thing itself — the discreteness is the
whole point. `gillespie` runs the exact stochastic simulation algorithm
(Gillespie SSA):

```pwe
world {
  gravity = (0, 0, 0)
  params { k = 0.15 [1/s] }
  entity e { state = (A = 40.0, C = 0.0) tag = rx }
}
systems {
  gillespie {
    tag = rx
    dt = 0.1
    events = 8
    channel decay = k * A => (A = A - 1, C = C + 1)
  }
}
```

* Each tagged entity carries **integer species counts** in its state slots;
  `tag` selects the reacting entities, `dt` the step, `events` (1..=512,
  default 64) the per-step reaction budget.
* Every `channel <name> = <propensity> => (<slot> = <expr>, …)` is one
  reaction. The propensity is its current rate — `k * A` here, so this is
  first-order decay with rate `k·A`.
* Each step draws the exact waiting time `τ ~ Exp(a)`, fires the reactions
  that land inside `dt`, and stops at the step boundary. Nothing is Euler-ised:
  a step is a *truncation* of a continuous-time Markov chain, not an
  approximation of one.
* Effects of one event are evaluated **together** — they all read the state
  *before* the event, so `(A = A - 1, C = C + 1)` is one simultaneous move, and
  the last writer wins if two effects name the same slot.
* If a step would need more than `events` reactions, the run fails with
  **detail 106** instead of silently dropping reactions: raise `events` or
  lower `dt`. A typo in a propensity or effect is a **compile error**
  (detail 48/52), never a silent `0.0` read.

Averaged over many independent vessels the survivor count follows the
deterministic law `A(t) = A0·e^{-kt}`: the stochastic and the ODE view agree in
bulk and disagree one molecule at a time. See `cli/examples/kinetics.pwe` — 24
atoms draining the reactant tray one hop at a time.

## Common mistakes (symptom → cause → fix)

| Symptom | Cause | Fix |
| --- | --- | --- |
| My body never moves | it's a state body but you used `gravity`/`integrate` (or vice-versa) | pick one model (§0.6) |
| A rule "does nothing" | `on =` missing (runs on every body) or the LHS names no slot | add `on = <entity\|pool>`; check the slot/field name |
| Value stays constant unexpectedly | `slot = expr` **assigns** | to integrate, write `slot = slot + dt*(rate)` (or `inte slot = rate`) |
| `update` runs on the wrong bodies | no `on =` → it runs on **every** dynamic body | add `on = <entity\|pool>` |
| Objects move oddly across a step | system order / read timing | order: forces → integrate → constraints; reads are start-of-step |
| `nbody` does nothing | mass not in `state[6]` (or body is component-only) | `state = (px,py,pz,vx,vy,vz,m)` |
| Field solver explodes | `dt`/stability exceeded | `rate ≤ 1/4` (diffuse 2D); `c·dt/dx ≤ 1/√2` (wave 2D) |
| Pool bodies ignored | inactive slots are skipped | they must be `spawn`ed first; `on = <pool>` |
| `let` fails to compile | name shadows `t`/`pi`/`e`/`sN` | rename it |
| Import not found | path is relative to the file | `import "../../std/forces"` etc. |
| Angle/spin looks wrong | `state[7]` is a Z-spin by default | set `orient = true` to use slots 7/8/9 as euler |

## A practice progression

1. Bounce a ball; tune restitution.
2. Oscillator; tune `k`, `c`; add forcing.
3. Two coupled oscillators; watch energy exchange.
4. A small orbit; vary `G`/velocity.
5. Heat diffusion; vary `rate`; plot at the probe.
6. A standing wave on a string.
7. A particle fountain with a pool.
8. A 3-link pendulum with joints.
9. A cloth sheet.
10. Rebuild `courtyard.pwe`: mesh ground + walking, turning figures.
11. Stochastic kinetics: tune `k`, watch `kinetics.pwe` drain one hop at a time.

---

# Part 2 — Language reference

## 2.1 Lexical rules

| Token | Form | Notes |
| --- | --- | --- |
| comment | `# …` or `// …` | to end of line; skipped anywhere |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | names of entities, slots, params, functions, shapes |
| `number` | `-? digits ("." digits)? (("e"\|"E") "-"? digits)?` | f64 |
| `value` | `number` (`/ number`)? | literal **or ratio** (`1/60`) |
| `boolean` | `true` \| `false` | |
| `string` | `"…"` (no escapes) | titles, SVG data |
| `color` | `0x` + 6 hex digits | `0xRRGGBB` |
| `unit` | `[ m/s^2 ]` | base `m kg s A K mol cd`; `* / ^`; compile-time only |
| `slot` | `s` digits | own state slot by position |
| constants | `t`, `pi`, `e` | clock (s), π, Euler's number |

Whitespace is insignificant; `;` is optional. Units are bracketed so `s[0]` is
unambiguous.

## 2.2 Keywords (reserved)

* sections `world` `funcs` `systems`
* world `gravity` `title` `params` `chan` `value` `entity` `field` `pool` `soft`
  `struct` `array` `width` `height` `depth` `dx` `nx` `ny` `nz` `spacing` `origin`
  `shape` `part`
* entity `position` `velocity` `state` `vec` `mass` `dynamic` `nbody` `parent`
  `restitution` `friction` `box` `sphere` `hull` `rotation` `camera` `color`
  `size` `opacity` `glow` `label` `orient` `vector`
* shapes `point` `sphere` `box` `capsule` `svg` `hull` `poly` `ring`
  `cylinder` `cone` `plane` `at` `depth` `scale` `faces`
* control `return` `let` `repeat` `until` `while` `for` `in` `break` `continue`
  `if` `else`
* rules `inte` `integrate` `+=` (and the operators `inte(…)`, `deriv(…)`)
* systems `send` `recv` `spawn` `despawn` `joint` `invariant` `watch`
* world `lang_version`
* logic `and` `or` `not` `&&` `||` `!` `true` `false`
* atoms `pi` `e` `t`

System *kinds* and *params* are ordinary identifiers matched at build time (an
unknown kind is detail 49). Builtins are ordinary calls special-cased in lowering.

Casts: `i64(x)`/`i32(x)`/`u64(x)`/`u32(x)` truncate toward zero,
`f64(x)` widens an exact integer (identity on a float), `bool(x)` is
`x != 0`. They make value-kind boundaries explicit.

**Integer values (RFC-0043).** A number literal with no `.`/exponent (`2`, `-7`)
is an exact **integer**; a literal with a fraction (`2.0`, `1e-3`) is an f64.
Integer arithmetic is exact and truncating: `7 / 2 == 3`, `7 % 3 == 1`, and
`1 / 2 == 0`. An expression is integer only when *both* operands are integers —
a single fractional operand widens the whole expression: `2.5 + 1 == 3.5`,
`1.0 / 2.0 == 0.5`. `let n: i64 = expr` binds an exact integer (truncating a
float `expr` toward zero and trapping on a non-finite one, detail 18); `n / 2`
then stays integer, while writing `n` to a state slot (`slot = n`) widens it to
f64. This is intentional and documented — old programs that relied on
`1 / 2 == 0.5` must write `1.0 / 2.0`.

`print(x)` is a debugging builtin: it records `x` in the run log (shown by
`pwe run` / the live viewer) and returns `x` unchanged — write `let _ = print(x)`.

Precedence (high → low): unary `-`, `not`/`!` → `* / %` → `+ -` → comparisons →
`and` → `or`.

## 2.3 `world` items

| Item | Meaning |
| --- | --- |
| `gravity = (x,y,z)` | global gravity (component bodies). |
| `title = "…"` | viewer title. |
| `params { K = v }` | model parameters (overridable with `--param`). |
| `chan <name> { value = v }` | a channel entity (`state[0]`). |
| `entity <name> { … }` | a body. |
| `shape <name> { part … }` | a custom render shape. |
| `struct <name> { field = <default> … }` | a named record type (§2.12). |
| `field <name> { width; height; dx; depth? }` | a scalar grid. |
| `pool <name>[N] { … }` | N inactive slots for dynamic entities. |
| `soft <name> { nx; ny; nz?; spacing; origin; mass }` | a mass-spring grid. |
| `bond <a> <b> [order=n] [polarity=p] [cloud=true] [min=d] [max=d]` | a render bond between two named entities (ball-and-stick). |
| `bonds { tag = <t>; other = <u>?; within = <d>; min = <d>?; axis = (x,y,z)?; angle = <rad>? }` | a **neighbourhood bond net**: auto-bond every pair of entities tagged `<t>` (with `other`, the cross `t`–`u` pairs) within `[min, within]`, recomputed each frame (bonds form/break by proximity — e.g. a reaction). `axis`+`angle` add a **bond-angle** gate. |
| `import "…"` | module import (path relative to the file). |

`bond` is **presentation only** (not a physics constraint): it draws a stick
between the centres of entities `<a>` and `<b>`. Options: `order` (1/2/3 =
single/double/triple, drawn as parallel sticks), `polarity` in `[0,1]` (tints the
bond from A's toward B's colour), `cloud=true` (a translucent shared-electron
region), `min`/`max` distances (the bond is drawn only while the two entities are
within that range — proximity-gated bonds that form/break, e.g. a chemical
reaction). It is the basis of `std/molecules/`, where each atom is an entity and
each chemical bond a `bond` — see L9 and `cli/examples/molecule.pwe`.

Both statements validate their options: each option may appear **at most once**,
a `bond` range must satisfy `0 ≤ min ≤ max`, and a `bonds` net requires `tag`
and `within` with `0 < min ≤ within`. An unknown or non-numeric key is an error
(details 107/108), so a typo like `withn` fails loudly instead of silently
leaving `within` unbounded and bonding every tagged pair regardless of distance.

**Entity id order**: declared entities `1..E`, then channels, then pool slots
(`<pool>#k`), then soft particles (`<soft>#k`).

## 2.4 Entity fields

| Field | Meaning |
| --- | --- |
| `position` / `velocity = (x,y,z)` | initial transform / velocity. |
| `state = (…)` | state slots: positional `(v0,…)`, named `(x=0,…)`, `vec3 pos` (slots `pos.0…`), a `struct` type, or a nested record `(p = (x=0,…))`. Max 16. |
| `mass` | mass (nbody, viewer size; required by joint/soft). |
| `dynamic = false` | static. |
| `nbody = false` | exclude from `nbody`. |
| `parent = <name>` | satellite display. |
| `restitution` / `friction` | contact response. |
| `box` / `sphere` / `hull` | collider. |
| `rotation = (rx,ry,rz)` | static euler rotation (radians). |
| `camera = true` | viewer camera. |

A neighbourhood bond net needs entities grouped by `tag = <name>` (an entity
field): `entity H { state=(…) tag = atom }` + `bonds { tag = atom; within = 1.0 }`.

Presentation-only: `color`, `shape = point\|sphere\|box\|capsule\|<custom>`,
`size = v|(dx,dy,dz)`, `opacity`, `glow`, `label = false`,
`orient = true` (state 7/8/9 = euler pitch/yaw/roll),
`vector = false` (hide arrows/rings).

## 2.5 Custom shapes

`part <kind> = <params> [at (x,y,z)] [scale s]`:

* `sphere = r`, `box = (dx,dy,dz)`, `capsule = (r0, len, r1)`;
* `ring = (radius, tube)` (a torus — e.g. an electron shell);
* `cylinder = (radius, height)`, `cone = (radius, height)`,
  `plane = (width, height)` (3D primitives; a `plane` renders double-sided);
* `hull = [(x,y,z), …]` (convex);
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]` (arbitrary mesh);
* `svg = "<d>" depth <d>`;
* `part <other-shape> [at …] [scale …]` (composition, recursive).

Presentation-only part options (all optional; they never affect semantics):

* `color = 0xRRGGBB` — overrides the entity colour for this part (multi-colour
  shapes: protons vs neutrons vs electrons);
* `opacity = o` — overrides the entity opacity for this part (`0..1`); makes a
  part faint/transparent (e.g. a pale electron orbit);
* `spin s` — self-rotation speed in rad/s (the value follows the keyword
  directly; `spin = s` is accepted too);
* `orbit (radius, speed, phase)` — animates the part around the entity's local
  origin (electrons orbiting a nucleus), `speed` in rad/s; `axis (x,y,z)` sets
  the orbit-plane normal (default local `+Z`).

Each option may appear **at most once** on a part (detail 107); an `opacity`
outside `0..1` (detail 105) or a malformed colour literal (detail 64) is an
error rather than a silently dropped value.

`std/atoms/` uses these to draw atomic structure; `std/molecules/` for bonds.

## 2.6 Grid fields

`field f { width=w; height=h; dx=d; depth? }` (2D, or 3D with `depth`). Rules:
`fget(f,i,j[,k])`, `fset(f,i,j,[k,]v)`, `flap(f,i,j[,k])`. First arg is a literal
field name; cell writes are visible to later reads in the same step.

## 2.7 All system kinds

`?` = optional; missing required ⇒ detail 48; unknown kind ⇒ detail 49.

| Kind | Parameters | Model | Meaning |
| --- | --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | component | `velocity.y += gravity_y·dt` |
| `integrate` | `dt` | component | `position += velocity·dt` |
| `damping` | `factor` | component | `velocity *= factor` |
| `force` | `ax`,`ay`,`az`,`dt` | component | `velocity += (ax,ay,az)·dt` |
| `wall` | `x`,`z`,`y_min?`,`restitution?` | component | reflect at `±x`,`±z` |
| `ground_contact` | `restitution` | component | resolve the `y=0` plane |
| `linear` | `slots`,`dt`,`row0=(…)`,… | state | `ds/dt = A·s + c` (Euler) |
| `nbody` (velocity-Verlet) | `G`,`dt` | state | inverse-square; `state=(px,py,pz,vx,vy,vz,m)` |
| `pair` | `tag`,`other?`,`dt`,`law`,`coord?`,`cohort?` | state | general pairwise force: for each body in `tag`, sum `law(r)` (a force magnitude in the pair distance `r`, positive = repulsive) over the `other` group (default itself); **kicks** the velocity `v += a·dt`; `state=(px,py,pz,vx,vy,vz,m)`. With `coord = rc`, the law may also use `n` (the body's **coordination number**: how many bodies are within `rc`), `rmin` (its nearest-neighbour distance) and `rminj` (the partner's nearest-neighbour distance) — so a **symmetric** bond order is expressible. `cohort = <tag>` sets the group the coordination is measured over (default: `tag` ∪ `other`), so it can span all elements — pair an element-specific force with a global environment (`tag = H, atom` + `cohort = atom`) |
| `drift` | `tag`,`dt`,`damp?` | state | position drift `p += v·dt` for tagged bodies (optionally damped `v *= 1-damp·dt`); pairs with `pair` |
| `send`/`recv` | `on`(req),`chan`,`value` / `on`(req),`chan`,`slot` | channel | channel send/receive (scoped to `on`) |
| `update` | `dt`,`on?`,`when?`,`every?`,`substeps?`,rules | state | explicit Euler (rules read the *original* slot values — no intra-step chaining; give each slot one rule) |
| `rk4` | `dt`,`on?`,`when?`,`every?`,`substeps?`,rules | state | Runge–Kutta 4 |
| `gillespie` | `tag`(req),`dt`,`events?`,channels | state | exact stochastic kinetics (Gillespie SSA): each `channel <name> = <propensity> => (<slot> = …, …)` is one reaction; every step draws the exact waiting time from the propensity and fires at most `events` reactions (1..=512, default 64) — needing more fails with detail 106 |
| `invariant` | `expr`,`on?` | — | per-step assertion |
| `watch` | `expr`,`mem`,`into`,`on?` | state | zero-crossing flag |
| `diffuse` | `field`,`rate` | field | `T += rate·∇²T` |
| `poisson` | `field`,`iters`,`source?`,`scale?` | field | `∇²φ = ρ·scale` |
| `wave` | `field`,`prev`,`velocity`,`dt`,`damping?`,`absorb?`,`absorb_width?` | field | `u_tt = c²∇²u` |
| `spawn` | `on`,`pool`,`count?`,`every?`,`phase?` | pool | activate slots |
| `despawn` | `on`,`when` | pool | deactivate slots |
| `joint` | `on`,`other`,`type`,… | transform | pairwise constraint |
| `soft` | `body`,`stiffness?`,`damping?`,`iterations?` | transform | mass-spring grid |

> **Channels are a single-cell mailbox, not a Go-style queue.** A `chan` holds one
> value in `state[0]`; `send` overwrites it (**last writer wins**) and `recv` reads
> it into a slot. There is no queueing or blocking. Because systems run in
> declaration order, a `recv` placed *before* its `send` sees the **previous**
> step's value; place it after for same-step delivery.

## 2.8 Expressions

| Form | Meaning |
| --- | --- |
| `s0`, `s1`, … | own slots |
| `x`, `@self.x` | own named slot |
| `@name.sN`, `@name.x`, `@name.state.x` | another entity's slot |
| `@name.mass`, `@name.is_dynamic` | another entity's properties |
| `@name.position.x`, `@name.velocity.x` | another entity's transform/velocity |
| `+ - * / %`, `-x` | arithmetic |
| `< <= > >= == !=` | comparisons → 1.0/0.0 |
| `and`/`&&`, `or`/`\|\|`, `not`/`!` | logic |
| `pi`, `e`, `t` | constants / clock |

An unresolved bare name reads `0.0`. System params are not in expression scope
(bind `let dt = 0.02` to use `dt`).

## 2.9 Builtins (arity checked; detail 59)

1-arg math: `sin cos exp ln sqrt abs floor ceil round sign log10 log2 sinh cosh
tanh asin acos atan`; 2-arg: `pow atan2 hypot min max`; `if(c,a,b)`; `random()`,
`noise()`; `at(T)`,
`periodic(P[,phase])`, `schedule(gate,delay,kind,payload)`; `active()`;
`vlen vdot vdist`; spatial `neighbor_count(r)`, `nearest_dist()`,
`neighbor_mean(slot,r)`, `nearest_dx/dy/dz()`; field `fget fset flap`. No `tan`
(use `sin/cos`). Any other name is a `funcs` function.

## 2.10 `funcs`, units, loops

* Sections must appear in order: `world` (required) → `funcs` (optional) → `systems` (optional).

* `funcs { f(a,b) { expr } }` — pure scalar functions; no world access.
* `funcs` parameters and the return value may be unit-annotated
  (`f(x: [m]) : [m/s] { … }`); call sites unify argument/result dimensions
  (detail 77).
* `let name: <type> = expr` — optional type annotation (`f64`/`i64`/
  `i32`/`u64`/`u32`/`bool`); comparisons and `and`/`or`/`not` are `bool`, other
  expressions are numeric. A mismatch is detail 89.
* Units: annotate `state`/params with `[m]`, `[m/s]`, `[1/s^2]`; mismatches are
  detail 77; unannotated values are wildcards. Base units are
  `m kg s A K mol cd`; named coherent-SI **derived** units are also accepted
  (`N J W Pa Hz C V F Oh S Wb T H lm lx Bq Gy Sv kat`), expanded to base
  dimensions — e.g. `a = F / mass` with `F` in `[N]`, `mass` in `[kg]` checks as
  `m/s^2`.
  Add `units = "strict"` to the `world` block to **require** an annotation on
  every state slot and parameter (missing ones are detail 90).
* `s[i]` read / `s[i] = expr` write (runtime index; **`update` only**).
* `repeat n {…}` (≤1000), `for i in lo..hi {…}` (ascending), `break`/`continue`
  (`break if (…)`), unrolled (≤10000 statements). Loop bodies: `let`, nested
  loops, `break`/`continue` only.

## 2.11 Arrays (fixed-length, named)

`array N name [{ v0, … }]` declares N consecutive state slots named
`name.0 … name.{N-1}` and records the length, so `name[j]` is checked rather
than a raw slot index. It composes with `state = (…)` in either order.

```pwe
world {
  gravity = (0, 0, 0)
  entity swarm { state = (x = 0.0) array 4 samples { 1.0, 2.0, 3.0, 4.0 } }
}
systems {
  update { on = swarm; dt = 0.5
    samples[0] = samples[0] + dt*(1.0)   # constant index → static slot
    samples[1] += 2.0                    # `+=` integrates with dt
    inte samples[2] = 4.0                # `inte name[j] = rate`
    x = samples[x]                        # runtime index (x holds 0..3)
  }
}
```

* `name[j]` read: a **constant** `j` lowers to a static slot; a **runtime** `j`
  lowers to a runtime-indexed read (`s[base + j]`), unchecked like `s[i]`.
* `name[j] = expr`, `name[j] += expr`, and `inte name[j] = rate` are the write
  forms (the read set is sampled at system start, like every rule).
* A constant `j` outside `[0, N)` is **detail 52**; an unknown array name is
  **detail 109**; an initializer with more than `N` values is also detail 52.
  None of these silently read 0.0.
* `vecN pos` remains the anonymous, length-less vector form; `array` adds the
  length and the bounds check. Both flatten to flat state slots (zero-cost).

* A `for` bound may be `len(name)` — the declared length of a named array,
  resolved at compile time (`for j in 0..len(v) { … }`). An unknown array name is
  detail 109. Bounds on a **runtime** index remain unchecked (like `s[i]`).

## 2.12 Struct types (records)

A `struct` names a group of fields; using it in `state = …` lays the fields out
as dotted scalar slots:

```pwe
world {
  struct Vec3 { x = 0.0; y = 0.0; z = 0.0 }
  struct Body { pos = Vec3; vel = Vec3; mass = 1.0 }

  entity a { state = Body }                               # slots pos.x…mass
  entity b { state = (p = Vec3, hp = 10.0) }              # embed + a scalar
  entity c { state = (pos = (x = 7.0, y = 8.0, z = 9.0)) }# inline record
}
systems {
  update { on = a; dt = 0.1
    pos.x = pos.x + dt*(vel.x)   # dotted LHS: pos.x += dt·vel.x
    vel.y = vel.y + dt*(0.0 - 9.81)
  }
}
```

* Fields may be scalars (with a default) or other structs (nested).
* Access own fields as `pos.x`; another entity's as `@a.pos.x`; both in rules and
  `funcs`.
* A `struct` is compile-time sugar over flat slots — zero-cost, deterministic,
  and part of world state. Unknown types and cycles are errors (detail 80).
* `vecN name` is the built-in shorthand: `vec3 pos` → `pos.0`, `pos.1`, `pos.2`
  (with `pos` an alias for `pos.0`).

---

# Part 3 — Semantics and pitfalls

* **Time is explicit**: every rule's expression is multiplied by `dt`; `t`
  advances by `dt` each step.
* **Divide-by-zero traps** (detail 18) for integers **and** floats
  (RFC-0021) — never silent `inf`/`NaN`.
* **`slot = expr` assigns**. Integrate with `slot = slot + inte(rate)` (or the
  `inte slot = rate` / `+=` statement); in `rk4`, `inte slot = rate`
  integrates with 4th-order Runge–Kutta. `inte(E) = dt·E`, `deriv(E) = (E−E_prev)/dt`.
* **Language version**: `world { lang_version = "0.3" }` pins the semantics
  (v0.3 = the rules above). Absent means "current"; an unsupported value is
  rejected at compile time (detail 83). Legacy pre-v0.3 sources can be upgraded
  with `pwe migrate <src.pwe> [-o out.pwe]` (old implicit `=` integration and the
  old `deriv` form are rewritten to `inte`).
* **Conserved quantities**: `conserved { on = <entity>; expr = <expr>;
  tolerance = 1e-4 }` records `expr` each step; the run fails (detail 87) if its
  relative drift exceeds `tolerance`. `pwe run` prints the drift.
* **System parameters are recognised by name** per kind; any other
  `name = <expr>` (including `name = 1.0`) is a rule.
* **Reads**: within one system's function, all reads are sampled once at the
  start of the (sub)step (rules are simultaneous). Across systems, a later system
  sees an earlier system's writes — so order matters. Field reads see same-step
  writes.
* **Determinism**: seeded `random()`/`noise()`, sorted-id spatial scans,
  interpreter ≡ JIT byte-for-byte (`step_cross`).
* **State** ≤ 16 slots. **`invariant`** fails the step before any write.
* **Two body models** (§0.6). `nbody` mass is `state[6]`. `state[7]` is a Z-spin
  unless `orient = true`.
* **Pools**: inactive slots are skipped and hidden; `on = <pool>` expands.

---

# Part 4 — Cookbook (compact)

* **Damped/driven oscillator**: §L1/L2.
* **N-body**: `nbody` with `state=(px,py,pz,vx,vy,vz,m)`.
* **Exotic pairwise force**: an explicit `update` reading `@other`.
* **Diffusion / Poisson / wave**: §L4; respect stability limits.
* **Particle system**: pool + `spawn`/`despawn` + `neighbor_*` (§L5).
* **Linkage / pendulum**: one `distance` joint per link, top `dynamic=false` (§L6).
* **Cloth / gel**: `soft` with `nz` (§L7).
* **Events / scheduling**: `emit`/`last_event`/`at`/`periodic`/`schedule` (§L8).
* **State machine**: gate writes with `when = expr`; flip modes with `watch`.
* **Units & dimensional checks**: §L10.
* **Mesh ground (no seams)**: one `poly` shape from a heightfield (§L9).

---

# Part 5 — Tools

```sh
pwe compile <src.pwe> -o <out.pweb> [--param K=V]
pwe run     <out.pweb> [--steps N] [--param K=V]…
pwe present <out.pweb> [--port P] [--param K=V]…
```

The `.pweb` artifact is a self-describing container (magic + version) holding the
verified EIR plus the model source; `run`/`present` execute the compiled EIR.
Viewer controls: **⟳ Restart** (reload initial scene, paused at t=0), **⏸ Pause /
▶ Resume**, **🏷 Labels**. Build release for smooth playback.

Embedding API:

```rust
let mut rt = pwe_reference::lang::LangRuntime::compile(src)?; // or ::compile_file
rt.step_cross_n(600)?;                 // interpreter == JIT asserted
let frame = rt.present_frame(None);    // render snapshot
```

---

# Part 6 — Diagnostics

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
| 80 | Unknown struct type / struct cycle / state too large. |
| 83 | Unsupported `lang_version`. |
| 84 | Malformed unit annotation. |
| 85 | Unknown identifier (reads 0.0) — a warning. |
| 86 | Unstable solver setting (CFL / diffusion limit). |
| 87 | Conserved quantity drifted beyond `tolerance`. |
| 88 | Non-finite state (simulation diverged) — with `pwe run --check`. |
| 89 | `let` type annotation mismatch. |
| 90 | Missing unit annotation (`units = "strict"`). |
| 91 | `nbody` body has fewer than 7 state slots. |
| 92 | `nbody` body also has a `mass` field (ignored; mass is `state[6]`) — a warning. |
| 93 | Plain assignment in `rk4` (integrate with `inte slot = rate`). |
| 94 | State slot name collides with a system parameter name — a warning. |
| 95 | `nbody` body exposes slots 7/8/9 without `orient = true` — a warning. |
| 96 | A rule writes slots 7/8/9 without `orient = true` — a warning. |
| 100 | Assignment to an unknown state slot (ignored) — a warning. |
| 101 | Duplicate module name (two modules declare the same `module`). |
| 102 | Reference to a non-exported module item. |
| 103 | A field name is not a lowercase identifier. |
| 105 | An `opacity` outside `0..1` (per-part or entity-level). |
| 106 | A `gillespie` step exceeded its reaction budget (`events`) — raise `events` or lower `dt`. |
| 107 | A duplicate option or field: each option of a part, `bond` or `bonds`, and each non-`tag` field of an `entity` / `pool` body, may appear at most once. |
| 108 | An unknown or invalid `bond` / `bonds` option (bad key, non-numeric value, missing `tag`/`within`, inverted range). |
| 109 | An unknown array name in `name[j]` (RFC-0044): the entity declares no `name.0 … name.k` state run. |

**Workflow**: reduce to one entity + one system; check the model (§0.6); check
the integrate/assign trap; add an `invariant`; run with `--steps N` and read the
printed state.

---

# Part 7 — Standard library (`std/`)

Pure-function modules; constants are overridable params. Full signatures in
`std/README.md`.

| Module | Constants | Representative functions |
| --- | --- | --- |
| `math` | — | `clamp clamp01 lerp mix remap step smoothstep wrap sqr deg rad hypot2 hypot3 min3 max3 sgn deadzone ease_in/out` |
| `forces` | — | `hooke spring_accel damping_accel drag_linear/quadratic_accel coulomb_force gravity_force inverse_square_accel buoyancy_force thrust_accel damper_force` |
| `particles` | — | `terminal_velocity drag_step ballistic_x/y/vy bounce_vy reflect radius_from_mass stopping_distance freefall_time speed` |
| `mechanics` | — | `momentum kinetic_energy reduced_mass elastic_1d_v1/v2 impulse friction_force normal_impulse inertia_rod/disk/sphere torque angular_accel angular_kinetic` |
| `thermal` | `sigma_sb` | `celsius kelvin newton_cooling heat_capacity sensible_heat conduction_flux stefan_boltzmann radiative_cooling thermal_diffusivity thermostat_hysteresis mixing_temp` |
| `acoustics` | `rho_air`,`c_air`,`p_ref` | `speed_of_sound_air wavelength spl pressure_from_spl acoustic_impedance doppler inverse_square sound_intensity beat_frequency` |
| `optics` | `h_planck`,`c_light` | `inverse_square_intensity beer_lambert snell_angle critical_angle fresnel_reflectance reflect_axis wien_peak photon_energy focal_length` |
| `em` | `c_light`,`k_coulomb`,`mu0` | `coulomb_force electric_field potential lorentz_force cyclotron_radius biot_savart_wire poynting plane_wave_b/e impedance_free_space` |
| `chemistry` | `R_gas`,`avogadro` | `atomic_mass element_period/group shell_capacity valence_electrons neutrons mol_from_mass mass_from_mol molarity dilute ideal_pressure/volume arrhenius ph h_from_ph neutralization_volume half_life_decay radioactive_amount` |
| `robotics` | — | `planar2_x/y planar2_ik_q1/q2 pid joint_accel diff_drive_v_left/right trapezoid_peak reach rotate_x/y` |
| `control` | — | `first_order second_order low_pass complementary integrate derivative pid pid_clamped feedforward state_feedback bang_bang hysteresis rate_limit_delta lead within slew` |
| `units` | — | `kmh_to_ms ev_to_j atm_to_pa deg_to_rad g_to_ms2 …` |

```pwe
# from cli/examples/ the paths would be "../../std/…"
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = vx + dt*(forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0))
    temp = temp + dt*(thermal.newton_cooling(temp, 293.15, 0.05))
}
```

---

# Part 8 — Examples (`cli/examples/`)

Compile then run/present the `.pweb`.

| Example | Teaches |
| --- | --- |
| `bounce.pwe` | component body: `gravity` + `ground_contact`. |
| `heat.pwe` | 2-D field + `flap` sweep. |
| `solar.pwe` | `nbody` (state `px,py,pz,vx,vy,vz,m`). |
| `flock.pwe` | boids via `neighbor_count`/`neighbor_mean`. |
| `spring/spring.pwe` | modules + params + units + `at`/`periodic`. |
| `domains.pwe` | composing `std/*`. |
| `wave.pwe`, `wave3d.pwe` | 1-D / 3-D wave solvers. |
| `acoustics.pwe` | 2-D sound + dB. |
| `robot.pwe`, `humanoid.pwe` | linkages / articulated figures. |
| `shapes.pwe` | composite shapes, polyhedra, SVG. |
| `chain.pwe` | distance joints (pendulum). |
| `cloth.pwe`, `jelly.pwe` | soft bodies (sheet, 3-D gel). |
| `particles.pwe` | pool + `spawn`/`despawn`. |
| `courtyard.pwe` | mesh ground + turning/leaning walkers (`orient`). |
| `structs.pwe` | `struct` record types (`pos.x`, `@a.pos.y`). |
| `kinetics.pwe` | exact stochastic kinetics: `gillespie` channels + SSA. |

---

## Appendix A — Canonical skeleton

```pwe
# imports (paths relative to this file)
import "std/forces"

world {
  title = "…"
  gravity = (0, -9.81, 0)
  params { k = 12.0 }
  entity a { position = (0, 5, 0) velocity = (1, 0, 0) mass = 1.0 sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (40,1,40); color = 0x557755 }
}

funcs { accel(k, x) { 0.0 - k * x } }

systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.5 }
  invariant      { on = a; expr = abs(x) < 1e6 }
}
```

## Appendix B — Stability checklist

* [ ] Each body uses one model (§0.6).
* [ ] Rules are `slot = <expression>` (an **assignment**); integrate with
`slot = slot + inte(rate)` or `inte slot = rate`.
* [ ] `on = <entity|pool>` where intended.
* [ ] System order: forces/gravity → integrate → constraints/boundaries.
* [ ] Solver stability respected (`diffuse` rate, `wave` Courant).
* [ ] `invariant`s guard against NaN/blow-ups.
* [ ] Spatial queries in rules only; field ops take literal field names.
* [ ] ≤ 16 state slots; loops within limits; dynamic LHS only in `update`.
* [ ] Slot 7 reserved unless `orient = true`.
* [ ] `nbody` bodies declare `state=(px,py,pz,vx,vy,vz,m)` (detail 91).
* [ ] `random()`/`noise()` acceptable (seeded, deterministic).
