# PWE Language Usage

[中文版](lang-usage.zh.md)

The PWE language is the textual front end of `pwe-reference` (`src/lang.rs`,
grammar `src/lang.pest`). A source program declares a world model and systems,
compiles to low-level EIR, and runs cross-backend — the interpreter and the CPU
JIT must produce byte-identical writes on every step.

```
PWE source ──parse──▶ WorldModel + system decls ──lower──▶ EIR ──interpret/JIT──▶ writes
```

```rust
let compiled = pwe_reference::lang::compile(SOURCE)?;      // parse → lower → EIR
let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
rt.step_cross()?;                                          // interpreter == JIT, asserted
rt.step_cross_n(30)?;                                      // 30 steps at once
```

From the shell, the `pwe` command-line toolchain (crate `pwe-cli`) compiles,
runs, and presents programs:

```sh
pwe compile scene.pwe -o scene.pweb   # .pwe → verified .pweb artifact
pwe run     scene.pweb --steps 600    # deterministic run, cross-backend each step
pwe present scene.pweb --port 8000    # live browser 3D viewer
```

## Program structure

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

Comments: `#` or `//` to end of line. Whitespace is insignificant.

## World section

| Statement | Meaning |
| --- | --- |
| `gravity = (x, y, z)` | Global uniform gravity vector. |
| `chan <name> { value = v }` | A channel entity; holds its latest value in `state[0]`. |
| `entity <name> { fields }` | A body. Fields below. |
| `field <name> { width = w; height = h; dx = d }` | A deterministic scalar grid field (the PDE substrate): cells read/written by rules via `fget`/`fset`/`flap`. |

### Entity fields

| Field | Meaning |
| --- | --- |
| `position = (x, y, z)` | Initial position. |
| `velocity = (x, y, z)` | Initial linear velocity. |
| `state = (v0, v1, …)` | Generic state slots (positional). Max 16 slots (`s0…s15`). |
| `state = (x = 0, y = 0, …)` | Named state slots; assign by name in rules, read via `@self.x`. Mixing named and positional in one list is allowed. |
| `state = (vec3 pos, …)` | A vector element reserves N consecutive slots named `pos`, `pos.0` … `pos.{N-1}` (zero-initialized); `pos` reads component 0, `@self.state.pos.1` reads component 1, `s[i]` indexes dynamically. |
| `mass = v` | Mass (slot 6 semantics for visual size; drives `nbody`). |
| `dynamic = false` | Static body (default is dynamic). |
| `nbody = false` | Exclude from the mutual `nbody` system. |
| `restitution = v` | Bounciness for contacts. |
| `friction = v` | Tangential friction for contacts. |
| `box = (dx, dy, dz)` | Box collider. |
| `sphere = r` | Sphere collider. |
| `hull = [(x,y,z), …]` | Convex-hull collider; needs ≥ 4 points. |
| `camera = true` | Marks the entity as the viewer camera (excluded from simulation). |
| `color = 0xRRGGBB` | Presentation color for the 3D viewer. |

Entity ids are 1-based in declaration order; channels follow the bodies.

## Systems

| System | Params | Meaning |
| --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | Apply uniform gravity to dynamic bodies. |
| `integrate` | `dt` | Integrate velocity into position. |
| `damping` | `factor` | Scale velocities each step. |
| `ground_contact` | `restitution` | Resolve contact with the ground plane. |
| `wall` | `x`, `z`, `y_min?`, `restitution?` | Bounded domain: `|x|,|z| ≤ limit`, velocity reflects on impact. |
| `force` | `ax`, `ay`, `az`, `dt` | Constant acceleration on dynamic bodies. |
| `linear` | `slots`, `dt`, `row0 = (a0, …, c)` | Linear dynamical system: `s_N' = Σ_j a_j·s_j + c`. Each `rowN` has `slots + 1` entries (coefficients + constant). |
| `nbody` | `G`, `dt` | Mutual inverse-square force among dynamic bodies: `G > 0` gravity, `G < 0` Coulomb repulsion. |
| `send` | `chan = name`, `value = expr` | Each dynamic entity evaluates `value` and writes it to the channel entity. |
| `recv` | `chan = name`, `slot = n` | Reads the channel's latest value into each dynamic body's `sN`. |
| `update` | `on = name?`, `when = expr?`, `every = n?`, `substeps = n?`, `dt`, `let …`, slot rules | User-defined nonlinear dynamical system (explicit Euler, below). |
| `rk4` | `on = name?`, `when = expr?`, `every = n?`, `substeps = n?`, `dt`, `let …`, slot rules | Same rules as `update`, but integrated with the classic **4th-order Runge–Kutta** method — far tighter accuracy for oscillators and nonlinear ODEs at the same `dt`. |
| `invariant` | `on = name?`, `expr`, `let …` | Per-step assertion: `expr` must be non-zero for the checked entities as the systems leave the state; a violated invariant fails the step (detail 69) before any write is applied. |
| `watch` | `on = name?`, `expr`, `mem = slot`, `into = slot` | Zero-crossing detection: flags 1 when the watched expression changes sign between consecutive steps; the previous value lives in the `mem` slot (world state), the flag lands in `into`. |

Unknown system kind → error (detail code 49).

## The `update` / `rk4` systems

Both systems take identical rules of the form `slot = expr`. `update` means
**`slot += dt · expr(state)`** (explicit Euler); `rk4` evaluates the same
derivative four times per step and combines them (Runge–Kutta 4), giving
4th-order accuracy — the same `dt` drifts far less. All reads happen first —
own slots, cross-entity references, and properties are read once per step, so
updates within one step are simultaneous (no ordering bias between rules).

```pwe
systems {
    update { on = target; dt = 0.02
        let omega = 0.5
        tx = -@self.ty * omega        # tx' = -ty·ω  (circular motion)
        ty = @self.tx * omega
    }
}
```

* `on = <name>` restricts the rule to one entity (default: all dynamic bodies).
* `let name = expr` computes a reusable local before the slot rules run.
* `when = expr` gates every rule's write: the state is untouched when it
  evaluates to 0 — mode/state-machine semantics (`when = mode == 1`).
* `every = n` runs the system only when `step % n == 0` (scheduling; the step
  counter is read via the `STEP` opcode, deterministic).
* `substeps = n` runs the integration n times with `dt/n` each; each substep
  re-reads the state and recomputes the `let` locals, so finer integration
  tracks the analytic solution better. Cross-entity references and properties
  are sampled once per step.
* The LHS is a positional slot `sN` or a named slot from that entity's
  `state = (x = 0, …)` layout; rules resolve per-entity.
* Unknown entity names in references produce no coupling; unresolved slots
  read `0`.

## The `invariant` system

```pwe
systems {
    update { on = reactor; dt = 0.0005
        let k = 3.0 * exp(-900.0 / temp)
        na = -k * na * water
        naoh = k * na * water
    }
    invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
}
```

The expression is evaluated per entity **after the systems run** and must be
non-zero. A zero (or NaN) value fails the step — `step_*` returns an error
(detail 69 for zero; the EIR's own NaN comparison rejection for NaN) **before
any write is applied**, so the scene keeps its pre-step state and never
silently proceeds past a broken one.

* `on = <name>` restricts the check to one entity (default: all dynamic bodies).
* Supports `let` locals like `update`.
* Each `invariant` system owns its own verdict field; several coexist.

## The `watch` system

```pwe
systems {
    update { on = ball; dt = 0.01
        x = vx
    }
    watch { on = ball; expr = x; mem = 5; into = 6 }
}
```

Each step the watch evaluates `expr` **after the systems run**, compares it
with the previous step's value (kept in the `mem` state slot — ordinary world
state, persisted and deterministic), and writes a 0/1 flag into `into`: 1 when
the value changed sign (a strict zero crossing; a zero memory is the initial
state and never counts). The entity's own rules read the flag and react —
bounce, reinitialize, switch mode. NaN values fail the step via the EIR's own
comparison rejection.

## Expressions

| Form | Meaning |
| --- | --- |
| `s0`, `s1`, … | Own state slots. |
| `x` (named slot), `@self.x` | Own named state slot. |
| `@name.sN`, `@name.state.x`, `@name.x` | Another entity's state slot. |
| `@name.mass`, `@name.is_dynamic` | Another entity's properties. |
| `@name.position.x/y/z`, `@name.velocity.x/y/z` | Another entity's transform/velocity. |
| `+ - * /`, unary `-` | Arithmetic (f64). |
| `< <= > >= == !=` | Comparisons, yielding `1.0` / `0.0`. |
| `and`/`&&`, `or`/`\|\|`, `not`/`!` | Logical connectives over nonzero = true, yielding `1.0`/`0.0`. Precedence: `not` > `and` > `or` > comparison. |
| `pi`, `e` | Constants. |
| `t` | Global simulation clock (seconds). |

### Builtin functions

* 1-arg: `sin cos exp ln sqrt abs floor ceil round sign log10 log2 sinh cosh tanh asin acos atan`
* 2-arg: `pow atan2 hypot min max`
* `if(c, a, b)` — select; `random()` — seeded, reproducible draw in `[0,1)`;
  `noise()` — seeded standard normal (Box–Muller over two draws), finite always;
  `print(x)` — logs `x`, yields it back, never changes world state;
  `emit(kind, payload)` — emits an ordered event, yields `0.0`;
  `last_event(kind)` — the payload of the most recent event of `kind` emitted
  so far in this step (0 when none) — in-language event consumption. Events
  are cleared each step (the host reads the step's events via
  `emitted_events()`); the kind is the numeric value, not a bit pattern.

### Vector helpers

* `vlen(x, y, z)` — √(x²+y²+z²); `vdot(x1,y1,z1, x2,y2,z2)` — component dot
  product; `vdist(x1,y1,z1, x2,y2,z2)` — distance between two points. Pure
  arithmetic (no new opcodes); combine with `neighbor_count`/`nearest_dist`
  for spatial models.

### Grid fields (PDE substrate)

Declared with `field <name> { width = w; height = h; dx = d }` in the world
section; the cells are deterministic world state (snapshot/replayable like any
other world state).

* `fget(f, i, j)` — the cell value at `(i, j)`; sees same-step writes.
* `fset(f, i, j, v)` — writes the cell (a bare call statement; yields `0.0`).
* `flap(f, i, j)` — the discrete Laplacian with the Field's zero-flux stencil,
  scaled by `1/dx²` — the PDE operator heat/diffusion/Poisson rules compose.
* Coordinates may be any expression (slots, locals, arithmetic). Unknown field
  names read `0` (the documented unresolved-reference convention).

### Dynamic slot indexing

* `s[i]` reads the State slot at a runtime index; `s[i] = expr` writes it
  (`s[i] += dt·expr`, like every rule). The index may be any expression
  (slots, locals, arithmetic). Arrays up to the 16-slot cap without extra
  state. Valid in `update` rules and their `let` blocks; a dynamic LHS in
  `rk4` is rejected (detail 73: RK4's working states are compile-time register
  chains, so a runtime-index LHS cannot feed them — dynamic *reads* work
  everywhere).

### Spatial queries

* `neighbor_count(r)` — number of other entities within distance `r` of the
  current entity's position. Participants: all non-camera scene bodies;
  position is `Transform` or `state[0..2]` (the viewer's convention).
  Deterministic (sorted id scan).
* `nearest_dist()` — distance to the nearest other entity; `f64::MAX` when the
  current entity is alone. Deterministic.
* Valid only inside system rules and their `let` blocks — not function bodies
  (no entity context there; detail 70).

### User-defined functions

```pwe
funcs {
    clamp(a, lo, hi) { if(s0 < s1, s1, if(s0 > s2, s2, s0)) }
}
```

### Branch control

Conditions combine via the logical connectives and branch via `if`:

```pwe
systems {
    update { on = heater; dt = 0.1
        # bang-bang thermostat with hysteresis (18–20 °C)
        let on = if(temp < 18.0, 1.0, if(temp > 20.0, 0.0, h))
        temp = on * 1.2 - (temp - 16.0) * 0.06
        h = on - h
    }
}
```

`not` binds to the following factor: write `not (x > 0)` for a negated
comparison.

### Loops

`repeat n { … }`, `for i in lo..hi { … }`, and the `break`/`continue`
statements unroll at lowering time (bounded), so they stay within the EIR's
straight-line (SSA, no back-edges) contract:

```pwe
systems {
    update { on = solver; dt = 1.0
        let g = s1
        repeat 100 until (abs(g * g - s0) < 1e-12) {
            let g = (g + s0 / g) * 0.5       # Newton's method for sqrt(s0)
        }
        s1 = g - s1                          # write the converged value back
    }
}
```

* Loop bodies contain only `let`, nested loops, and `break`/`continue`;
  slot rules stay outside the loop (the grammar enforces this).
* `repeat n until (cond)` checks the condition **after** each iteration
  (exits when true); `repeat n while (cond)` checks **before** (exits when
  false). `break`/`continue` accept an optional `if (cond)`.
* `break` exits only the innermost loop. Guarded expressions are evaluated
  unconditionally and their results discarded by the gate — IEEE f64 has no
  traps, so this is safe in straight-line EIR.
* Caps: one loop iterates at most 1000 times and unrolls at most 10000
  statements.
* `let` names may not shadow reserved tokens (`t`, `pi`, `e`, `s0`, `s1`, …)
  — the grammar resolves those before bare idents, so such a binding could
  never be read back.

The body is a scalar expression; parameters are referenced by slot `s0`,
`s1`, … (param *i* is slot `s_i`). Functions lower to EIR `CALL`s and are
callable from any `update` rule and from a `send` value. Arity is validated at
compile time.

## Semantics notes

* Time is explicit: `dt` multiplies every rule's expression; the simulation
  clock `t` advances by `dt` each step.
* Determinism is first-class: `random()` is seeded and replay-stable; the
  interpreter and JIT agree byte-for-byte every step (`step_cross`).
* State slots are capped at 16 per entity (`MAX_STATE_SLOTS`).

## A general simulation substrate

Because rules are plain scalar ODEs over named/positional state slots, with
`let` locals, user functions, cross-entity references, `random()`/`emit()`, and
RK4 or Euler integration, the language is not tied to rigid-body physics. Any
law expressible as (coupled) differential or difference equations — physical,
chemical, or biological — can be modeled, simulated, and visualized. See
`reference/examples/scientific_laws_demo.rs` for a live gallery running five
laws at once (harmonic motion, Kepler orbit, reversible kinetics, logistic
growth, radioactive decay), each proven by `reference/tests/laws.rs`.
* Reads-before-writes: within one step, every referenced value is the value
  from the *start* of the step.

## Error detail codes

| Code | Meaning |
| --- | --- |
| 48 | Missing required system param. |
| 49 | Unknown system kind. |
| 51 | Convex hull needs ≥ 4 points. |
| 52 | State slot count out of range (1..=16). |
| 53 | Missing `linear` row. |
| 54 | `linear` row length ≠ `slots + 1`. |
| 55 | Invalid `funcs` body / empty `update` rule / bad slot LHS. |
| 56 | Expression parse failure. |
| 57 | Number parse failure. |
| 58 | Slot / reference parse failure. |
| 59 | Call arity violation. |
| 60 | Program parse failure. |
| 62 | Unknown entity or channel name. |
| 64 | Invalid color literal. |

## Diagnostics

Compile failures carry a human message, a detail code, and a source offset.
The reference surfaces them through the `lang` module:

```rust
match pwe_reference::lang::LangRuntime::compile(source) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(source, &e)),
}
```

`lang::diagnose` renders a multi-line report with the offending source line
and a caret, e.g. a missing parameter:

```text
error 48: system 'update' is missing required parameter 'dt'
  --> line 13, column 9
    |
  13 |         update { on = reactor; dt = 0.0005
    |         ^
```

* `lang::clear_diagnostics` / `lang::take_diagnostics` expose the raw
  `Diagnostic` list (`detail`, `message`, `byte_offset`) for a failed compile.
* `lang::detail_name(detail)` maps a code to its canonical phrase.
* Comments (`#` / `//` to end of line) are skipped as whitespace anywhere in a
  program, including inside `update` / `rk4` rule blocks.

## Recipes

Minimal falling body (`language_demo`):

```pwe
world {
    gravity = (0, -9.81, 0)
    entity vehicle { position = (0, 8, 0); velocity = (4, 0, 0); mass = 4; dynamic = true; box = (1, 0.5, 0.7) }
    entity ground  { position = (0, -5, 0); dynamic = false; box = (50, 5, 50) }
}
systems {
    gravity { gravity_y = -9.81; dt = 1 / 60 }
    integrate { dt = 1 / 60 }
    ground_contact { restitution = 0.6 }
}
```

Radioactive decay (linear):

```pwe
world { gravity = (0,0,0) entity isotope { state = (100, 0) } }
systems { linear { slots = 2; dt = 1; row0 = (-0.05, 0, 0); row1 = (0, 0, 0) } }
```

Nonlinear pendulum (update + `sin`):

```pwe
world { gravity = (0,0,0) entity pend { state = (1.2, 0) } }
systems { update { dt = 0.0005
    s0 = s1
    s1 = -9.81 * sin(s0) } }
```

Orbital system (nbody, `solar_demo`):

```pwe
world {
    gravity = (0, 0, 0)
    entity sun  { state = (0, 0, 0, 0, 0, 0, 1000000, 0); color = 0xFFD24A }
    entity earth{ state = (4, 0, 0, 0, 500, 0, 1, 0);     color = 0x4aa8ff }
}
systems { nbody { G = 1.0; dt = 0.0001 } }
```

`state[0]` is the orbital radius vector's x, `state[3]` the initial orbital
velocity; slot 6 is the mass (visual size in the viewer).
