# PWE — Program the physical world in 4D.

**A Rust microkernel runtime + language for deterministic, distributed simulation
of a 4D world — 3D space, plus time.**

[中文](README-ZH.md)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.81%2B-orange.svg)](https://www.rust-lang.org)
[![tests](https://img.shields.io/badge/tests-304%20passing-brightgreen.svg)](#tests--conformance)
[![conformance](https://img.shields.io/badge/conformance-17%2F17%20%C2%B7%200%20skips-brightgreen.svg)](#tests--conformance)
[![RFCs](https://img.shields.io/badge/frozen%20contract-37%20RFCs-purple.svg)](#the-frozen-contract)
[![repo](https://img.shields.io/badge/github-open1s%2Fqwe-181717.svg)](https://github.com/open1s/qwe)

PWE is not another physics toy. It is a **substrate**: one authoritative world,
one typed intermediate representation, and one language that models *anything*
expressible as coupled differential or difference equations — gravity and
collisions, chemical kinetics, population dynamics, electromagnetism, heat,
sound, waves, robot arms, and machines that walk.

And it is **deterministic you can prove**: the interpreter is the semantic
oracle, the JIT must agree with it **byte-for-byte on every step**, and 300+
tests plus 17 conformance checks enforce it — with zero skips.

```
World Model → WIR → Domain IR → EIR → Interpreter / JIT / AOT → Runtime → CPU / GPU / NPU / Edge / Cloud
```

---

## The 60-second tour

```sh
cargo build --release -p pwe-cli

# A person-like figure walks (62-part humanoid: capsule limbs, eyes, fingers)
./target/release/pwe compile cli/examples/humanoid.pwe -o humanoid.pweb
./target/release/pwe present humanoid.pweb --port 8000   # open http://localhost:8000

# A pulse radiates through a 3-D cube — ⟳ Restart, then ▶ Resume
./target/release/pwe compile cli/examples/wave3d.pwe -o wave3d.pweb
./target/release/pwe present wave3d.pweb --port 8000

# A whole solar system, live
./target/release/pwe compile cli/examples/solar.pwe -o solar.pweb
./target/release/pwe present solar.pweb --port 8000
```

Every artifact is a **self-describing, verified binary** (`.pweb`): recompile,
rerun, restage — the same bytes, the same world.

---

## Why PWE is different

| Typical engine | PWE |
| --- | --- |
| A fixed set of built-in behaviors you configure | **A language** — model any law as rules on state |
| "Deterministic-ish," trusted by convention | **Provably deterministic** — interpreter ≡ JIT, asserted every step |
| Physics *or* chemistry *or* rendering, in separate tools | **Peer domains on one world** — physics, render, sensor, AI, network share one source of truth |
| 2D/3D scenes | **4D substrate** — 3D space + time; grid fields are volumetric |
| A black box you hope is stable | **A frozen, versioned contract** — 37 RFCs, canonical bytes, content hashes |
| Closed visuals | **Open live viewer** — 3D web viewport, restart / pause / label controls |

**The World is the single authoritative source of truth.** Entity is identity,
component is data, system is behavior. All authoritative mutation goes through a
`WorldTransaction`.

---

## One language, every domain

PWE's language is the same for every discipline — only the equations change.

```pwe
world {
  gravity = (0, 0, 0)
  field heat { width = 32; height = 32; depth = 32; dx = 1.0 }   # a 3D grid field
  entity body { state = (x = 1.0, vx = 0.0, temp = 353.15) }
}
systems {
  diffuse { field = heat; rate = 0.16 }        # PDE:  T += rate·∇²T
  update  { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.radiative_cooling(temp, 293.15, 0.9, 1.0, 1.0, 700.0) + 0.0
  }
}
```

| Domain | In PWE |
| --- | --- |
| **Mechanics / gravity / collisions** | `nbody`, `gravity`, `integrate`, `ground_contact`, `wall`, `force`, `linear`, `update` / `rk4` |
| **PDE fields (4D)** | `field` + `diffuse` / `poisson` / `wave` (heat, potential, sound, waves) |
| **Chemistry** | `std/chemistry` — full periodic table (Z = 1–118), Arrhenius, kinetics, pH |
| **Thermodynamics** | `std/thermal` — Newton cooling, Stefan–Boltzmann, conduction |
| **Acoustics & optics** | `std/acoustics`, `std/optics` — SPL, Doppler, Beer–Lambert, Fresnel |
| **Electromagnetism** | `std/em` + `poisson` — Coulomb, Lorentz, cyclotron, Poynting |
| **Robotics** | `std/robotics` — forward/inverse kinematics, PID, differential drive |
| **Anything stochastic** | `random()`, `noise()`, `emit()` — seeded and replay-stable |

And the **standard library** is just PWE: `std/` ships `math`, `particles`,
`forces`, `mechanics`, `chemistry` (1–118), `thermal`, `acoustics`, `optics`,
`em`, `robotics`, `units`, `control` — pure functions keyed by namespaced
physical constants you can override with `--param`.

---

## Showcase — see it move

Each compiles with `pwe compile … -o out.pweb`, then `pwe present out.pweb`.

| Demo | What it is |
| --- | --- |
| `humanoid.pwe` | A **62-part humanoid** walking — capsule limbs, facial detail, knuckled fingers. |
| `robot.pwe` | A **2-link robot arm** waving, driven by `std/robotics` forward kinematics. |
| `shapes.pwe` | **Custom shapes**: composites, a convex octahedron, an explicit-face gem, an extruded **SVG** star. |
| `wave3d.pwe` | A **3D wave** radiating through a cube — a translucent energy-coloured shell that expands and fades. |
| `acoustics.pwe` | **2-D sound** from a driven monopole; probes register arrival delay and level in dB. |
| `solar.pwe` | The **solar system**, live: 8 planets + Moon, orbiting and self-rotating. |
| `flock.pwe` | A **flock** coalescing and aligning via neighbourhood queries. |
| `domains.pwe` | One probe under a **damped spring** while **radiatively cooling** — forces + thermal + EM + chemistry composed. |
| `spring/spring.pwe` | Multi-file **modules**, parameters, units, and scheduled events (`--param k=16`). |
| `wave.pwe` | A **sine standing wave** on a 1-D line, drawn as an energy-coloured curve. |
| `heat.pwe` | **Heat diffusion** on a 2-D grid (an unrolled Gauss–Seidel sweep). |
| `bounce.pwe` | The classic: **gravity + ground contact** with restitution. |

The viewer renders entities from their own declaration — `shape`, `size`,
`color`, `opacity`, `glow`, `label` — and fields as an isosurface (2D/3D) or a
curve (1D). Controls: **⟳ Restart**, **⏸ Pause / ▶ Resume**, **🏷 Labels**.

---

## The language at a glance

```pwe
world {
  gravity = (0, -9.81, 0)
  entity vehicle {
    position = (0, 8, 0)  velocity = (4, 0, 0)
    mass = 4  dynamic = true  box = (1, 0.5, 0.7)
  }
  entity ground { position = (0, -5, 0)  dynamic = false  box = (50, 5, 50) }
}
systems {
  gravity { gravity_y = -9.81; dt = 1/60 }
  integrate { dt = 1/60 }
  ground_contact { restitution = 0.6 }
}
```

* **Rules are equations.** `slot = expr` means `slot += dt·expr` (Euler); `rk4`
  integrates the same rules at 4th order.
* **State, not scripts.** Named slots, cross-entity reads (`@other.state.x`),
  properties (`@other.mass`), spatial queries (`neighbor_count`, `nearest_dist`,
  `neighbor_mean`).
* **Modules, Python-style.** `import "std/forces"`, `import "m" as x`,
  `from "m" import f`; packages are directories; circular imports resolve.
* **Units, checked.** Opt-in `[m/s^2]` annotations with gradual dimensional
  analysis — unannotated stays a wildcard.
* **Events & scheduling.** `at(T)`, `periodic(P)`, `schedule(gate, delay, …)`,
  `emit` / `last_event` — exact-once, deterministic, on the step grid.
* **Control flow, bounded.** `repeat` / `for` / `break` / `continue` unroll to
  straight-line EIR; Newton iteration inside a step is a one-liner.
* **Diagnostics that teach.** Compile failures print the detail code and the
  offending line with a caret, javac-style.

The full reference — lexical rules, **every keyword**, EBNF, operator
precedence, and a complete **intrinsic function** table — lives in
[`docs/lang-usage.md`](docs/lang-usage.md).

---

## Determinism you can prove

Determinism is not a slogan here; it is a **compile-and-run gate**:

* **Interpreter ≡ JIT** — `step_cross` runs both backends on identical state and
  requires byte-identical writes *and* identical event/queue streams, every step.
* **Analytic law conformance** — every simulation is asserted against its closed
  form (Newton cooling, radioactive decay, logistic growth, Kepler orbits,
  harmonic energy, action–reaction, reversible kinetics, wall reflection, …).
* **Snapshot / replay** — state hashes reproduce bit-for-bit across runs and
  snapshots.
* **Frozen contract** — 37 RFCs define canonical bytes, schemas, and protocols;
  conformance runs with **zero skips**.

---

## Architecture

```
application  →  world  →  IR  →  compiler  →  runtime  →  kernel  →  platform
```

| Crate | Role |
| --- | --- |
| `pwe-api` | `no_std`, dependency-free **frozen contract** (types, traits, ABI, limits, detail codes). |
| `reference/` (`pwe-reference`) | The **semantic oracle**: deterministic world, interpreter, interpreter-backed JIT, AOT, language front end. |
| `conformance/` | RFC-0029 report runner + RFC-0030 scenario. |
| `cli/` (`pwe`) | The toolchain: `compile` / `run` / `present`. |

Inside `pwe-reference`:

| Module | Purpose |
| --- | --- |
| `lang` | The **PWE language**: PEST grammar → EIR, cross-backend execution, diagnostics. |
| `physics` / `physics_eir` | Deterministic rigid-body simulation; reusable `EirSystem`s lowered to typed EIR. |
| `field` / `continuum` | The generic 4D scalar field: storage, Laplacian, Poisson, diffusion, content hash. |
| `present` | The 3D web presentation layer: frames → JSON → a live Three.js viewer. |
| `eir` / `dominance` / `fence` / `aot` | Typed SSA IR, dominance verification, explicit fences, AOT artifact codec. |
| `broadphase` / `cluster` / `channel` / `distributed` | Interchangeable broad phase; a BEAM-like cluster; cross-runtime channels; distributed continuum. |
| `simulation` | `Input → Physics → Commit → RenderPrepare`, tracking camera, hashed snapshots. |

---

## Quick start

```sh
git clone git@github.com:open1s/qwe.git && cd qwe

cargo build --workspace
cargo test  --workspace          # 304 tests
cargo run -p pwe-conformance     # RFC-0029: all PASS, no skips

# the language, end to end:
cargo run -p pwe-reference --example language_demo
```

**Requirements:** Rust 1.81+. `pwe-api` is `no_std` and dependency-free;
`pwe-reference` depends only on `pwe-api` and PEST.

---

## Tests & conformance

| Suite | Count |
| --- | --- |
| Runtime / language unit tests | 266 |
| Analytic law-conformance | 17 |
| Property tests | 3 |
| Standard-library tests | 3 |
| Integration / other | 15 |
| **Total** | **304** |

Plus `pwe-conformance`: **17 / 17, zero skips**. The `no_std` check:
`cargo check -p pwe-api --no-default-features`.

---

## The frozen contract

The full frozen v0.2 contract (RFC-0019 … RFC-0036): schema & canonicalization,
WIR, EIR (typed SSA + dominance), memory & fences, world transactions,
distributed ownership, snapshots & deltas, the runtime ABI (`include/pwe_abi.h`),
JIT/AOT, render frames, conformance & the minimal profile, component ABI,
Domain IR, capabilities, errors/limits, artifact identity, and the interchange
envelope. See [`rfc/`](rfc) and [`docs/rfc-alignment.md`](docs/rfc-alignment.md).

---

## Honest limits & roadmap

Bounded, deliberate gaps behind the kernel boundary — **not** missing contracts:

* **Backends**: the reference implements the interpreter (the oracle), the
  interpreter-backed JIT, and the AOT path — all differential-verified. GPU /
  NPU / SIMD plug in at `AotProgram.target` and are **on the roadmap**. The
  reference "JIT" locks the JIT *contract* rather than emitting native code.
* **Rigid bodies**: AABB, sphere, convex-hull (exact SAT), compound and
  heightfield colliders, impulses, friction, distance joints, a ground plane. No
  soft bodies or a full joint family yet.
* **Dynamic entity sets are fixed at compile time** (object-pool activation is
  planned).

See [`tasks/todo.md`](tasks/todo.md) for the live list.

---

## Repository layout

| Path | Contents |
| --- | --- |
| `pwe-api` | Frozen, `no_std` contract. |
| `reference/` | Semantic oracle + JIT/AOT + the PWE language. |
| `conformance/` | Conformance runner. |
| `cli/` + `cli/examples/` | The `pwe` toolchain and ready-to-run worlds. |
| `std/` | The standard library (pure-function modules). |
| `rfc/` · `docs/` | The frozen contract and the usage guide. |
| `include/pwe_abi.h` | Generated C11 ABI header, layout-locked. |

---

## License

Apache-2.0. Copyright 2026 Open1s. See [LICENSE](LICENSE).

**The world is the single source of truth. Program it. Prove it. Watch it move.**
