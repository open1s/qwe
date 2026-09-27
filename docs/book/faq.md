# FAQ

## Is this a game engine?

No. PWE is a **substrate**: a microkernel runtime plus a language for
deterministic, distributed simulation of a 4D world. It happens to ship a live
3-D viewer so you can watch what you modeled.

## Why a language instead of a scene description?

Because the interesting cases are not a fixed set of behaviors you configure —
they are equations. `slot = expr` means `slot += dt·expr`; `rk4` integrates the
same rules at 4th order. Chemistry, acoustics, robotics and population dynamics
use the **same language**, only the equations change.

## How is determinism proven, not promised?

* The **interpreter is the semantic reference**; the JIT must produce
  byte-identical writes *and* identical event/queue streams **on every step**
  (`step_cross`).
* Every simulation is checked against a closed-form solution (Newton cooling,
  Kepler orbits, radioactive decay, …).
* State hashes reproduce bit-for-bit across runs and snapshots.
* The conformance suite runs with **zero skips**.

See [Determinism](determinism.md).

## What does `.pweb` mean?

A self-describing, verified binary artifact — the analogue of `.class` for
`java`. Compile once, then `pwe run` / `pwe present` execute the compiled EIR.
Recompile and you get the same bytes for the same world.

## Does it need a GPU?

No. The reference implements the interpreter (the oracle), the interpreter-backed
JIT, and the AOT path — all differentially verified on CPU. GPU / NPU / SIMD
plug in at `AotProgram.target` and are on the roadmap.

## Is it distributed?

Yes, by design: exactly one authoritative owner per mutable entity, an
ownership state machine with epochs that reject stale commands, and replaceable
transport (QUIC initially). Split-brain authority is never silently allowed.

## What does "4D" mean here?

3D space plus time — with time explicit and domain-based. Grid fields are
volumetric, so a `field` with `depth` is a 3-D continuum evolving on the step
clock: a 4D substrate.

## Can I override physical constants?

Yes. Standard-library functions are keyed by namespaced constants,
overridable with `--param` against the **same artifact** — no recompile.

## What are the honest limits right now?

* The reference "JIT" locks the JIT **contract**; it does not yet emit native
  code.
* Rigid bodies cover AABB, sphere, convex-hull (exact SAT), compound and
  heightfield colliders, impulses, friction, distance joints and a ground plane
  — see [RFC-0039](../../rfc/RFC-0039-constraint-joints.md) and
  [RFC-0040](../../rfc/RFC-0040-soft-bodies.md) for joints and soft bodies.
* Rendering is a first-class peer domain consuming a `RenderView`, but there is
  no offline renderer.

The live assessment lives in [Review & roadmap](../review-and-roadmap.md).

## Where do I start?

[Quick start](getting-started.md) — build, test, and run a world in under a
minute.
