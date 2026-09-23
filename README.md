# PWE — Physical World Engine

PWE is a Rust microkernel runtime for programmable, distributed representation,
simulation, compilation, and rendering of a physical world.

The World is the single authoritative source of truth. Entities are identity,
components are data, and systems are behavior. Physics, render, sensor, AI, and
network are peer domains — never independent worlds — and all authoritative
mutation goes through a `WorldTransaction`.

```
World Model → WIR → Domain IR → EIR → Interpreter/JIT/AOT → Runtime → CPU/GPU/NPU/Edge/Cloud
```

Execution backend status: the reference implements the **interpreter**
(semantic reference), the interpreter-backed **JIT**, and the **AOT** artifact
path — all differential-verified. The **GPU / NPU / SIMD** backends are
**TODO** (not implemented; see `tasks/todo.md`); they plug in at the
`AotProgram.target` boundary and must be differentially verified against the
interpreter before use.

## Tools
- use zg to query and search
- use jj for versioning control


## Repository layout

| Path | Contents |
| --- | --- |
| `pwe-api` | `no_std`, dependency-free frozen Rust contract (types, traits, ABI, limits, detail codes). |
| `reference/` | `pwe-reference`: deterministic in-memory semantic oracle and the reference CPU JIT. |
| `conformance/` | `pwe-conformance`: RFC-0029 conforming report runner + RFC-0030 ground/vehicle/camera scenario. |
| `cli/` | `pwe`: the command-line toolchain — `pwe compile` (`.pwe` → verified `.pweb` artifact), `pwe run` (deterministic execution), `pwe present` (live browser 3D viewer). |
| `rfc/` | The frozen v0.2 contract (RFC-0019..RFC-0036) and superseded v0.1 drafts (RFC-0001..RFC-0018). |
| `include/pwe_abi.h` | Generated C11 ABI header locked to the Rust layouts by layout tests. |
| `docs/` | RFC alignment matrix and the Draft→Frozen supersession map. |

## Requirements

- Rust 1.81 or newer. `pwe-api` is `no_std` and dependency-free; `pwe-reference`
  depends only on `pwe-api` and PEST (`pest`/`pest_derive`) for its language
  grammar.

## Build, test, run

```sh
cargo build --workspace
cargo test --workspace                  # 241 unit + 17 law-conformance + 3 property tests
cargo run -p pwe-conformance            # RFC-0029 report: all PASS, no skips
cargo run -p pwe-reference --example vehicle_scenario   # live physics demo
cargo run -p pwe-reference --example language_demo      # PWE language → EIR → cross-backend
cargo run -p pwe-reference --example distributed_demo   # cluster + RFC-0024 field migration
cargo clippy --workspace --all-targets --all-features
```

Run the `no_std` check on the API crate:

```sh
cargo check -p pwe-api --no-default-features
```

## Command-line toolchain

Like `javac`/`java` for the PWE language:

```sh
cargo build -p pwe-cli

pwe compile scene.pwe -o scene.pweb      # source -> verified .pweb binary
pwe run     scene.pweb --steps 600       # run the binary (interpreter == JIT each step)
pwe present scene.pweb --port 8000       # live browser 3D viewer (http://localhost:8000)
```

Ready-to-run examples live in `cli/examples/`:

```sh
pwe compile cli/examples/solar.pwe  -o solar.pweb  && pwe present solar.pweb --port 8000
pwe compile cli/examples/bounce.pwe -o bounce.pweb && pwe run bounce.pweb --steps 300
pwe compile cli/examples/heat.pwe   -o heat.pweb   && pwe run heat.pweb --steps 400

# `present` shows a top-right legend (color, name, r from the central body) and
# a top-left run-info panel (per-body radii, satellite distances, step + title).
# For many-body runs build in release: cargo build --release -p pwe-cli
```

A `.pweb` artifact is a self-describing container (magic + version) holding the
verified canonical (RFC-0021) EIR module plus the world-model source the runtime
derives its initial scene from; `run`/`present` execute the artifact's compiled
EIR. Compile failures render the offending source line with a caret, javac-style.

## The simulation core

Beyond the frozen contract, PWE ships a real, deterministic rigid-body
simulation (`reference/src/`):

| Module | Purpose |
| --- | --- |
| `math` | `Vec3`, `Quat`, `Aabb` — lossless `f64`, deterministic. |
| `components` | Typed `Transform`, `Velocity`, `Force`, `RigidBody`, `Collider` (AABB + sphere), `Camera`, `DistanceJoint` with canonical byte encoding. |
| `scene` | The authoritative typed World State (ordered, deterministic). |
| `physics` | Fixed-step integration (gravity, damping, forces), interchangeable broad phase (uniform grid or BVH), sphere/box/convex-hull narrow phase (hull is exact SAT), impulse + friction resolution, ground contact, distance-joint constraint solver, anti-tunneling substepping. |
| `physics_eir` | **EIR-backed physics**: reusable `EirSystem` components (`GravitySystem`, `ForceSystem`, `IntegrateSystem`, `DampingSystem`, `GroundContactSystem`, `LinearSystem`, `UpdateSystem`) that lower to typed EIR and are composed into a `PhysicsProgram`; the interpreter executes them and produces ordered world writes. `EirSimulation` drives a scene purely through EIR (no bypass). A generic `State` component carries user-defined dynamical-system slots. |
| `present` | **3D web presentation layer**: snapshots a running `Scene` into frames, serializes them to JSON, and writes a self-contained HTML file with a **Three.js 3D viewport** (playback, orbit controls, timeline, state panel). Space-feel **starfield background**; each entity renders with a **distinct shape, color, size, and CSS2D name label**; the central body **glows** (emissive + point light); **orbit rings** and **velocity vectors** show motion; **click a body to highlight + inspect its full state**. **Molecules** (CO₂ / CH₄) render with **bonds** as lines between bonded atoms (`with_bonds`). Also serves a **live viewer** (`serve_live`) — the browser polls the running runtime's `LiveState` (frame + step + procedure log) in real time (`--example live_demo` / `solar_demo` at `http://localhost:8000`). |
| `lang` | **The PWE language**: a declarative **PEST grammar** front end that compiles `world { … } systems { … }` to low-level EIR and runs it cross-backend (interpreter + CPU JIT) with byte-identical write agreement. `chan`/`send`/`recv`/`nbody` are first-class grammar productions. Collider DSL: box, sphere, convex hull; systems: gravity, force, integrate, damping, ground_contact, **`wall` (bounded domain, velocity reflects on impact)**, linear, **`nbody` (mutual multi-body gravity/Coulomb for micro & macro — entities sense each other's forces and react)**, **`send`/`recv` (Go-style channels, cross-runtime over the wire)**; plus **generic `state` slots** (up to 16 per entity), a `linear` dynamical system, and nonlinear `update` **and `rk4`** systems with scalar expressions over `+ − × ÷`, parens, state slots `s0…`, **constants `π` and `e`**, **transcendental functions `sin cos exp ln sqrt pow`**, **comparisons `< > <= >= == !=`** and **`min`/`max`/`if` selection**, **cross-entity references `@name.sN`**, **`update { on = name }` targeting**, **`nbody = false` exclusion**, **simulation time `t`**, **`random()` (seeded, reproducible stochastic draws)**, **`emit(kind, payload)` (ordered events)**, **user-defined pure functions** (`funcs { clamp(a,b) { … } }` lowered to EIR `CALL`s, reusable from any `update` rule), **named state slots** (`state = (x = 0, y = 0)` — assign by name in rules, read via `@self.x` / `@name.x` / `@name.state.x`), **rich math builtins** (`abs floor ceil round sign log10 log2 sinh cosh tanh asin acos atan atan2 hypot`), and **cross-entity property access** (`@other.mass`, `@other.position.x/y/z`, `@other.velocity.x/y/z`, `@other.is_dynamic`), **`let` local variables** (compute intermediate values once in an `update` rule, reused by several rules — elegant, no inlining), **`print(x)` debugging** (a transparent log channel that logs `x` and yields it back, never changing world state), **bounded control flow** (`repeat N`, `for i in lo..hi`, `break`/`continue` with `if` conditions, `until`/`while` gates — unrolled to straight-line EIR with run/skip predication, no back-edges), **`invariant { expr }` per-step assertions** (lowered to a hidden verdict field; a violated invariant fails the step **before any write is applied**, so the scene never silently proceeds past a broken state), **spatial queries** (`neighbor_count(r)` / `nearest_dist()` — deterministic sorted-id scene scans served through EIR, valid in system rules), **`when = expr` mode gating** (every rule's write predicated on a mode expression — state-machine semantics), **`watch` zero-crossing detection** (sign-change flags across steps, with the watch memory in ordinary state slots), **`every = n` scheduling** and **`substeps = n` multi-rate integration** (the `STEP` opcode reads the host-advanced step counter; substeps re-integrate with dt/n), **`noise()` seeded Gaussian** (Box–Muller over two reproducible draws) and **vector helpers** (`vlen`/`vdot`/`vdist` over scalar components), and **dynamic slot indexing** (`s[i]` reads/writes the State slot at a runtime index — arrays up to the 16-slot cap), **grid fields (the PDE substrate)** (`field <name> { width; height; dx }` declarations; deterministic cells read/written by rules via `fget`/`fset`/`flap` — the zero-flux laplacian composing heat/diffusion/Poisson laws), **vector state sugar** (`state = (vec3 pos, …)` reserving N named slots), and **in-language event consumption** (`last_event(kind)` reads the step's events; per-step clearing, host observability via `emitted_events()`) — expressing pendulums, forced oscillators, waves, saturation/threshold laws, exact growth/decay, inverse-square laws, N-body gravity, coupled oscillators, reaction kinetics, logistic growth, predator–prey, RLC, damped motion, **stochastic/noisy dynamics**, and **multi-entity coupled / property-aware models** across micro and macro scales entirely in source. Compile failures carry rich **diagnostics** (`lang::diagnose` renders the code, message, and a source caret); comments are skipped anywhere in a program. |
| `field` + `continuum` | **A generic continuum runtime**: a domain-neutral scalar field (storage, Laplacian, Poisson relaxation, diffusion, content hash) on which peer domains simulate distinct phenomena — **electromagnetics** (electrostatic potential via Gauss–Seidel, verified against the analytic solution) and **chemistry** (reaction–diffusion with mass conservation). |
| `broadphase` | Deterministic broad phase with interchangeable backends: a uniform grid (conservative) and a median-split **BVH** (exact), both under one `pairs()` contract verified against a brute-force oracle. |
| `cluster` | **A BEAM-like runtime cluster**: processes with reduction budgets, a preemptive round-robin scheduler, multiple runtime nodes, and entity migration between nodes through the RFC-0024 ownership state machine. |
| `channel` | **Go-like channels across runtimes**: bounded FIFO channels (`send`/`recv`, deterministic non-blocking) plus a `ChannelRouter` that exchanges messages between independent runtimes via the portable `wire` format — cross-runtime, cross-platform communication (the same bytes that would travel over a network). |
| `distributed` | **Distributed continuum**: runs the generic EM/chemistry field on the cluster — one field per node evolved by the preemptive scheduler, with node-to-node field migration through RFC-0024 carrying state and ownership together (`--example distributed_demo`). |
| `aot` | **AOT compile path**: a validated, frozen `AotProgram` whose execution is byte-identical to the interpreter (RFC-0010/0027), with a self-authenticating RFC-0035 artifact codec (encode/decode with content-hash verification). |
| `dominance` | **RFC-0021 block/dominance verification**: a CFG built from EIR instructions, with branch-target validation and SSA dominance checking. Enforced as a compile gate by the AOT and JIT paths. |
| `fence` | **RFC-0022 explicit fences**: bounded `MemoryOrder` and a CPU/GPU `CpuGpuHandoff` that fails closed unless a Release/Acquire fence and an ownership transfer are present. |
| `simulation` | `Input → Physics → Commit → RenderPrepare` loop, tracking/look-at camera, content-addressed state hash, snapshot/restore with replay. |

Run `cargo run -p pwe-reference --example vehicle_scenario` to see a vehicle
fall under gravity, land and drift on a ground plane, tracked by a camera —
with replay determinism proven (two runs and a snapshot-replay both reproduce
the identical state hash).

## Demo examples

Each runs with `cargo run -p pwe-reference --example <name>` and shows a slice
of the runtime working end to end:

| Example | What it demonstrates |
| --- | --- |
| `language_physics_demo` | **Use the PWE language**: define a physics world (`world { … }` model + `systems { … }`) in source, parse it, compile to typed EIR, and run cross-backend with `interpreter == JIT` asserted every step. |
| `language_demo` | The same language pipeline on a single-vehicle scene. |
| `continuum_demo` | The generic field: electromagnetics (electrostatic potential vs analytic) and chemistry (diffusion conserving mass) on one scalar `Field`, with deterministic content hashes. |
| `broadphase_demo` | Uniform-grid vs BVH broad phase: interchangeable candidate pairs, and both backends drive physics to identical positions. |
| `physics_hull_demo` | Convex-hull collider in a live, deterministic rigid-body sim (pyramid + slab + sphere), with replay determinism. |
| `compile_stack_demo` | The compile stack: source → EIR → interpreter/JIT/AOT agreement, RFC-0021 dominance + RFC-0022 fence checks, and the RFC-0035 AOT artifact codec. |
| `pipeline_demo` | The full documented pipeline in one place: **World → WIR (RFC-0020) → Domain IR (RFC-0032 physics pipeline) → EIR (RFC-0021) → interpreter/JIT/AOT runtime**, with WIR and EIR binary round-trips and cross-backend agreement. |
| `distributed_demo` | BEAM-like cluster: a continuum field per node evolved by the scheduler, migrated between nodes through RFC-0024 ownership with identical state. |
| `state_dynamics_demo` | **User-defined phenomena in the language**: radioactive decay, a harmonic oscillator, logistic growth, a nonlinear pendulum (`sin`), and exact exponential growth — expressed purely as `state` + `linear`/`update` systems, with cross-backend execution. |
| `rk4_demo` | **4th-order Runge–Kutta (`rk4`) integration**: the same `slot = expr` rules as `update` but integrated with RK4 — oscillators stay accurate at coarse `dt` where explicit Euler drifts, and multi-variable models use >8 state slots (up to 16). |
| `na_water_demo` | **Water + sodium reaction, live**: `2Na + 2H₂O → 2NaOH + H₂` with mass-action kinetics and an **Arrhenius** rate constant `k(T) = A·e^{−Ea/T}`; the exotherm feeds back into temperature (which speeds the reaction) as the reactor **heats and swells** in the 3D viewport (`http://localhost:8000`). |
| `scientific_laws_demo` | **Scientific-laws gallery, live**: five independent laws run simultaneously in one 3D scene — simple **harmonic motion**, a **Keplerian orbit** (Newton gravity), **reversible chemical kinetics** `A ⇌ B` (mass conserved), **logistic growth** `N' = rN(1−N/K)`, and **radioactive decay** `N' = −λN` — each modeled in PWE source and visualized as bodies that move/grow/shrink (`http://localhost:8000`). |
| `carbon_atom_demo` | **Carbon atom (micro-scale), live**: a red nucleus + 6 electrons in the K(2)/L(4) shell configuration, each bound by inverse-square Coulomb force, served live over HTTP (`http://localhost:8000`) — orbit rings, velocity arrows, per-shell colors, stable shells. |
| `molecule_demo` | **Carbon molecular structure**: **CO₂** (linear O=C=O) and **CH₄** (tetrahedral, 109.5°) — atoms are `dynamic=false` entities (element color + mass→size) with bonds drawn as lines between bonded atoms via `present::with_bonds`, live at `http://localhost:8001`. |
| `stochastic_demo` | **General-purpose substrate proof (non-physics)**: a **stochastic ecology** model — noisy logistic population with carrying capacity, demographic noise via `random()`, event emission via `emit(...)`, and time via `t` — run cross-backend every step with **seeded, reproducible** trajectories (replay matches bit-for-bit). Demonstrates the engine simulates arbitrary stochastic requirements, not just physics. |
| `functions_demo` | **User-defined functions in the language**: reusable pure helpers (`clamp`/`smoothstep`/`logistic`) lowered to EIR `CALL`s and called from an `update` rule — run cross-backend (interpreter == JIT) every step, reproducible. Shows the language is a general modeling substrate, not a fixed rule DSL. |
| `expressive_demo` | **Live expressiveness showcase**: a target orbits and a glider chases it using named state slots, `let` locals, rich builtins, cross-entity property access, and user-defined functions — served **live** over HTTP (`http://localhost:8003`), run cross-backend (interpreter == JIT) every step, reproducible. |
| `solar_system_demo` | **N-body / orbital (macro-scale)**: a fixed `sun` anchoring a planet and a moon via **cross-entity `@sun.sN` coupling** under inverse-square gravity — circular orbits hold steady across backends. |
| `chemistry_reaction_demo` | **Chemical kinetics (mass action)**: reversible `A + B ⇌ C` with `dA/dt = −k_f[A][B] + k_r[C]`, relaxing to chemical equilibrium `K_eq = k_f/k_r` with mass conserved. |
| `channel_demo` | **Cross-runtime channels**: two independent `ChannelRouter`s (different regions / hosts) exchange messages Go-style over the portable wire format. |
| `chan_demo` | **Go-style channels in the language**: a `chan` system sends a value to a channel entity and reads it back — plain cross-entity EIR, interpreter == JIT enforced. |
| `nbody_demo` | **Micro & macro multi-body**: a planet in a stable circular orbit (macro, `G>0`) and like-charged particles repelling with conserved momentum (micro, `G<0`), via the `nbody` system. |
| `interaction_demo` | **Interaction & reaction, macro + micro**: a star and two planets mutually attract (`G>0`) and charged particles interact (`G<0`), each a distinct shape + color, written to a 3D viewer (`interaction_viewer.html`). Newton's third law verified (momentum conserved). |
| `solar_demo` | **Live solar system**: all 8 planets + Moon — every body **revolves** (公转) around the Sun via `nbody`, **self-rotates** (自转), and has a **distinct size** derived from its real relative mass (Sun 2.5, Jupiter 2.0, Earth 0.35, Moon 0.09), served live over HTTP (`http://localhost:8000`). |
| `science_demo` | **Breadth of scientific phenomena, verified**: Newton cooling, radioactive decay, logistic growth, harmonic-oscillator energy, projectile motion, and chemical equilibrium — each checked against its analytic solution. |
| `present_demo` | **3D web viewport**: runs a physics sim, snapshots frames, and writes `science_viewer.html` (Three.js playback) — open in a browser to watch the simulation. |
| `live_demo` | **Live runtime interface**: the browser connects to the running runtime over HTTP and shows the simulation **in real time** — 3D scene, step counter, and a procedure log (`http://localhost:8000`). The car drives in a **walled domain** and bounces off the walls. |
| `vehicle_scenario` / `terrain_scenario` | Full scenes with camera tracking, state hashing, and snapshot+replay determinism. |

## The PWE language

`pwe-reference` ships a small textual front end (`reference/src/lang.rs`). A
source program declares a world model and systems, compiles to low-level EIR,
and runs cross-backend (interpreter + CPU JIT) through `LangRuntime`, which
asserts the two backends produce byte-identical writes every step.

```pwe
world {
  gravity = (0, -9.81, 0)
  entity vehicle {
    position = (0, 8, 0); velocity = (4, 0, 0)
    mass = 4; dynamic = true; box = (1, 0.5, 0.7)
  }
  entity ground {
    position = (0, -5, 0); dynamic = false; box = (50, 5, 50)
  }
}
systems {
  gravity { gravity_y = -9.81; dt = 1 / 60 }
  integrate { dt = 1 / 60 }
  ground_contact { restitution = 0.6 }
}
```

`cargo run -p pwe-reference --example language_demo` runs it: the vehicle falls,
bounces, and settles while `interpreter == JIT` is enforced on every step.

Compile failures carry rich **diagnostics**: `lang::diagnose(source, &err)`
renders the detail code, a human message, and the offending source line with a
caret (e.g. `error 48: system 'update' is missing required parameter 'dt'`).

## Law conformance

`reference/tests/laws.rs` gates correctness: each PWE-language simulation is run
and asserted against its analytic / objective result, so every simulation obeys
the underlying physical, chemical, or biological law — not just spot-checked:

| Law | Checked result |
| --- | --- |
| Newton's law of cooling | `T(t) = T_env + (T0−T_env)(1−k·dt)^n` |
| Radioactive decay | `N = N0·(1−λ·dt)^n` |
| Exponential growth | `N = N0·(1+r·dt)^n` |
| Logistic growth | `N → 1/(1+e^{−rt})` |
| Projectile (ballistics) | `y(t) = v0·t − ½gt²` |
| Harmonic oscillator | total mechanical energy conserved |
| Reversible reaction | `[C]/([A][B]) = K_eq = k_f/k_r`, mass conserved |
| N-body gravity | stable circular orbit |
| **Kepler orbit** | **elliptical orbit conserves angular momentum** |
| N-body Coulomb | linear momentum conserved |
| **N-body action–reaction** | **Newton's third law: equal & opposite forces, total momentum conserved** |
| Wall boundary | body kept inside `|x|,|z| ≤ limit`, velocity reflects with restitution |
| Channel | deterministic round-trip |

## What is implemented

The full frozen v0.2 contract:

- **Schema & canonicalization** (RFC-0019): bounded little-endian wire primitives, SHA-256, schema registry keyed by canonical hash.
- **WIR** (RFC-0020): envelope, CRC32C, section kinds, full-consumption + duplicate/unknown-section rejection, validation order.
- **EIR** (RFC-0021): typed SSA subset, validator, deterministic interpreter (the semantic oracle), plus the `PWEEIR2` binary envelope / directory / module-hash / TYPES+FUNCTIONS codec and a **block/dominance verifier** (`EirModule::verify_linear_dominance`) enforced by the AOT and JIT compile gates (RFC-0021 "one definition/value, dominance, block targets"). Arithmetic includes the elementary functions `sin cos exp ln sqrt pow`.
- **Memory & concurrency** (RFC-0022): declared-access scheduler, view lifetime, and **explicit fences** for CPU/GPU resource handoff (Release/Acquire strength + ownership transfer, else fail-closed).
- **World transaction** (RFC-0023): create/destroy/write, read set, ordered events, atomic commit, `OPEN→PREPARED→VALIDATED→COMMITTED|ABORTED`, and all four conflict policies (`Reject`, `LastWriterByPriority`, `Merge`, `CommutativeMerge`) with a registered merge registry.
- **Distributed ownership** (RFC-0024): multi-region state machine, epochs, split-brain detection.
- **Snapshot & delta** (RFC-0025): deterministic snapshot (SimTime + resource refs), base-version-checked delta, transactional restore, resource updates.
- **Runtime ABI** (RFC-0026): C11 function table, `include/pwe_abi.h`, layout-locked.
- **JIT** (RFC-0027): compiled-cache CPU JIT with hotness, assumptions, safe-point deopt; differential with the interpreter. Refuses install without a capability manifest. **AOT** (RFC-0010/0027): a validated, frozen program object whose execution is byte-identical to the interpreter, with a self-authenticating RFC-0035 artifact codec.
- **Render frame** (RFC-0028): immutable frame reading one `WorldVersion`, two-tick interpolation, and an external `Present` path.
- **Conformance** (RFC-0029) & **minimal profile** (RFC-0030): `pwe-conformance` with zero skips, a single ground+vehicle+camera scenario, and a `PhysicsView` peer of `RenderView`.
- **Component ABI** (RFC-0031), **Domain IR** (RFC-0032), **Capability** (RFC-0033), **Errors/limits** (RFC-0034), **Artifact identity** (RFC-0035), **Interchange envelope** (RFC-0036).

Plus the kernel-neutral definitions folded in from the v0.1 drafts (see
`docs/rfc-supersession.md`): simulation time/clock domains/determinism levels,
version-compatibility matrix, resource identity/residency, spatial/interest
queries, security-audit events, and the plugin/backend C ABI.

## Current limits (v0.2)

The reference is deterministic and self-contained but intentionally scoped:

- **Rigid-body scope**: AABB, sphere, **convex-hull** (SAT narrow phase, exact
  for convex polyhedra), compound, and heightfield colliders; uniform gravity,
  impulses, restitution, friction, distance joints, and a ground plane. No soft
  bodies or a full joint family yet.
- **Broad phase** is deterministic and interchangeable: a uniform grid and a
  median-split **BVH** (RFC-0014) expose the same `pairs()` contract, proven
  against a shared brute-force oracle; the grid is conservative (no false
  negatives), the BVH is exact. The physics system can select either backend
  (`BroadPhaseKind`) and produces identical simulated positions.
- **No GPU backends** (Metal/Vulkan/wgpu), native codegen (Cranelift/LLVM), or
  neural rendering. The reference "JIT" is a validated compiled cache that locks
  the JIT *contract* without emitting native machine code.

These are bounded, deliberate gaps behind the kernel boundary, not missing
contracts.

## License

Apache-2.0. Copyright 2026 Open1s. See [LICENSE](LICENSE).