# RFC-0047: Toward a general-purpose, top-tier simulation language
**Status:** Proposed (direction + capability matrix). This RFC does not change
any frozen contract. It fixes **what "general-purpose simulation language"
means for PWE**, maps each industrial simulation domain to the language feature
it needs, and marks what is Done / Partial / Missing today, so the remaining
work is scoped and sequenced. Each Missing row becomes (or reuses) a concrete
RFC before code lands.

## Motivation

PWE already has the hard part of a *platform*: a microkernel boundary, layered
IRs (WIR → Domain IR → EIR), an interpreter that is the semantic oracle,
byte-exact cross-backend determinism, a capability model, a frozen ABI, and a
live viewer. What it does **not** yet have is a *general* language surface: it
is a strong "programmable physical world" DSL whose expressiveness stops before
what MATLAB/Simulink, Modelica, AnyLogic/Simio, and the physics engines assume.

The goal of "general-purpose, replace every industrial simulation product" is a
multi-year program, not a single change. This RFC makes it tractable by
separating three layers and refusing to conflate them:

1. **Substrate** (what PWE already owns): one authoritative world, layered IRs,
   determinism, capabilities, cross-backend equivalence.
2. **Language surface** (the gap): value kinds, collections, modules, discrete
   events, hybrid (continuous + discrete) semantics, stochastic modeling.
3. **Domain libraries + backends** (breadth): control, signal, chemical/
   biochemical, electrical, thermal-hydraulics, queues/agent-based, PDE/CAD/
   robotics, plus an FMI/co-simulation boundary so PWE can *host* other tools
   before it *replaces* them.

A language "replaces" an industrial tool only when a **fixture** from that tool
runs in PWE and matches a reference result. So every domain row below is gated
on a named fixture + conformance case, not on a syntax checklist.

## Capability matrix (honest status)

Legend: **Done** = implemented + conformance/test evidence; **Partial** = a
subset exists; **Missing** = not implemented (RFC or sub-RFC required).

| Domain / product class | The feature it actually requires | Status | Evidence / next gate |
| --- | --- | --- | --- |
| General numerics (MATLAB core) | typed values incl. exact `int`/`bool`, arrays, linear algebra | Partial | `int`/`bool` casts + `let` annotations, `array N name` (RFC-0044, Done); typed EIR registers RFC-0043 (Proposed); no matrix type / BLAS |
| Control systems (Simulink) | continuous + discrete blocks, zero-crossing, algebraic loops | Partial | `update`/`rk4`, `at`/`periodic`/`schedule`, `when` gate; no zero-crossing detection, no algebraic-loop solver, no block-diagram IR |
| Equation-based / multi-physics (Modelica, Dymola) | acausal connectors, DAE (not just ODE), index reduction | Missing | PWE is causal ODE/difference only; a DAE layer would be a new Domain IR (needs RFC) |
| Agent-based / DES (AnyLogic, Simio, Arena) | discrete-event queue, resources, entities, process flow | Partial | pools, `emit`/`last_event`, `schedule`, `gillespie`; no queue/resource/statistics primitives, no event calendar as a first-class value |
| Stochastic chemical kinetics (COPASI, BioNetGen) | SSA, rule-based reactions, compartments | Done (core) | `gillespie` SSA with `channel` reactions; rule-based (graph) rewriting and compartments Missing |
| Molecular dynamics (LAMMPS, GROMACS) | force fields, thermostats, PBC, neighbor lists | Partial | `nbody`, `bonds` nets, `neighbor_*`; no PBC, no thermostat/barostat, no long-range (Ewald/PME) |
| Rigid-body / robotics (Gazebo, MuJoCo, Bullet) | joints, articulations, contacts, sensors | Partial | joints, soft bodies, colliders, hulls, broadphase; no contact solver tuning, no articulated-body dynamics, no sensor model library |
| CFD / thermal (OpenFOAM, Fluent) | conservative PDE, meshes, turbulence | Partial | grid fields + `diffuse`/`wave`/`poisson`, stability checks; uniform grids only, no unstructured mesh, no turbulence closures |
| Signal / DSP (Simulink, scipy) | filters, FFT, sampled-data timing | Missing | no filter/FFT primitives; `every`/`substeps` only |
| Electrical / power (Simulink, PLECS) | circuit DAE, switch events | Missing | DAE + event detection (same gap as Modelica) |
| Multibody / vehicles (Adams, CarSim) | constrained multibody, tires, road | Partial | joints + soft bodies + fields; no tire/road contact models |
| Digital-twin / co-simulation (FMI/FMU, DDS) | import/export FMU, sync protocols | Missing | a `pwe` FMU export + a co-sim boundary would let PWE host tools first (RFC needed) |

## What "top-tier" means here (non-negotiables)

Any domain library added on top MUST preserve the substrate guarantees or it
does not ship:

- **Determinism**: interpreter ≡ JIT/AOT byte-for-byte; stochastic paths use the
  seeded `random`/`noise`; discrete-event calendars are ordered by
  `(time, seq)` deterministically.
- **One authoritative world**: every domain mutates through `WorldTransaction`;
  no side-channel state.
- **Layer discipline**: new domain behavior is a Domain IR lowered to EIR; no
  undocumented execution path (`AGENTS.md` §3).
- **Verification**: each domain adds a **fixture** with a reference result
  (analytic, or a recorded trace from the tool being replaced) and a
  conformance case.

## Sequence (dependency-ordered)

1. **Value kinds → EIR registers** (RFC-0043). Exact integers/booleans unlock
   counts, indices, and event logic without float aliasing. (small, in flight)
2. **Collections** (RFC-0044 typed arrays — Done; plus `len`, dynamic-index
   checks, and a future record/array-of-record path).
3. **Discrete-event + hybrid core**: an event calendar as a first-class value,
   zero-crossing detection, and a queue/resource library — the shared need of
   DES, control, power, and chemical domains. (new sub-RFC)
4. **DAE / acausal layer**: connectors + `der`/`when` semantics over a DAE
   Domain IR, with index reduction. Unlocks Modelica-class multi-physics.
   (new sub-RFC; largest)
5. **Numeric kernels**: dense linear algebra, `fft`, filter primitives — with a
   SoA/deterministic contract.
6. **Domain libraries**: control, signal, electrical, MD (PBC + thermostats),
   CFD (meshes), robotics (articulated bodies + sensors).
7. **Co-simulation boundary**: FMI/FMU import/export so PWE interoperates
   before it replaces.

Each of steps 3–7 requires its own RFC; this RFC is the index and the honesty
contract (status column) they must update.

## Validation

- The capability matrix is the scoreboard; a row moves to **Done** only with a
  named fixture + conformance case, the same rule `docs/rfc-alignment.md` uses.
- `docs/rfc-alignment.md` gains a "general-simulation" section linking this RFC
  and each domain sub-RFC.

## Non-goals

- Claiming PWE *replaces* a tool before that tool's fixture passes.
- A visual block-diagram editor, or a GUI; PWE stays a text language + viewer.
- Weakening determinism or the kernel boundary for convenience (Decision Test,
  `AGENTS.md` §17: semantics first).

## Alternatives

- **Breadth-first syntax** (add keywords per product): rejected — it produces
  surface without fixtures and breaks the RFC/aligned-everything discipline.
- **Host-tools-first forever** (only co-simulate): rejected as the end state,
  but adopted as step 7 because it de-risks step 6.
