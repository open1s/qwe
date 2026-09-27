# Example worlds

Every demo compiles with `pwe compile … -o out.pweb`, then `pwe present
out.pweb`. Sources live in [`cli/examples/`](https://github.com/open1s/qwe/tree/main/cli/examples).

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

The directory also holds newer experiments — `chain`, `cloth`, `courtyard`,
`jelly`, `particles`, `structs` — check the sources directly; every example is
short and readable.

## Run one

```sh
cargo build --release -p pwe-cli
./target/release/pwe compile cli/examples/solar.pwe -o solar.pweb
./target/release/pwe present solar.pweb --port 8000
```

Override parameters without recompiling:

```sh
./target/release/pwe present spring.pweb --port 8000 --param k=16
```

## Reading the output

The viewer renders entities from their own declaration — `shape`, `size`,
`color`, `opacity`, `glow`, `label` — and fields as an isosurface (2D/3D) or a
curve (1D). Controls: **⟳ Restart**, **⏸ Pause / ▶ Resume**, **🏷 Labels**.

Presentation-only fields never touch simulation state, so what you see can
never perturb what you prove.

See also: [Determinism](determinism.md) · [Language guide](../lang-usage.md)
