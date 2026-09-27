# Architecture

```
application  →  world  →  IR  →  compiler  →  runtime  →  kernel  →  platform
```

## The pipeline

```
World Model → WIR → Domain IR → EIR → Interpreter/JIT/AOT → Runtime → CPU/GPU/NPU/Edge/Cloud
```

* **World Model** describes intent.
* **WIR** serializes the model plus initial authoritative state. It must not
  depend on CPU / GPU / LLVM / Cranelift / Metal / Vulkan / CUDA / OS.
* **Domain IR** lowers domain computation (physics, render, sensor, AI) without
  choosing hardware.
* **EIR** is the typed, SSA-like, verifiable, capability-aware, target-neutral
  executable representation.
* **Runtime** is the sole owner of mutable authoritative World State.

## Microkernel boundary

The kernel provides **mechanisms only**:

```
memory / handles / scheduling / task / sync / time /
capability / transaction / event / resource / ABI / device
```

It MUST NOT depend on physics, rendering, AI, sensors, or application logic.
Domain policy lives outside the kernel.

## Crates

| Crate | Role |
| --- | --- |
| `pwe-api` | `no_std`, dependency-free **frozen contract** (types, traits, ABI, limits, detail codes). |
| `reference/` (`pwe-reference`) | The **semantic oracle**: deterministic world, interpreter, interpreter-backed JIT, AOT, language front end. |
| `conformance/` | RFC-0029 report runner + RFC-0030 scenario. |
| `cli/` (`pwe`) | The toolchain: `compile` / `run` / `present`. |

Dependency direction — never reversed:

```
application → world → IR → compiler → runtime → kernel → platform
```

## Inside `pwe-reference`

| Module | Purpose |
| --- | --- |
| `lang` | The **PWE language**: PEST grammar → EIR, cross-backend execution, diagnostics. |
| `physics` / `physics_eir` | Deterministic rigid-body simulation; reusable `EirSystem`s lowered to typed EIR. |
| `field` / `continuum` | The generic 4D scalar field: storage, Laplacian, Poisson, diffusion, content hash. |
| `present` | The 3D web presentation layer: frames → JSON → a live Three.js viewer. |
| `eir` / `dominance` / `fence` / `aot` | Typed SSA IR, dominance verification, explicit fences, AOT artifact codec. |
| `broadphase` / `cluster` / `channel` / `distributed` | Interchangeable broad phase; a BEAM-like cluster; cross-runtime channels; distributed continuum. |
| `simulation` | `Input → Physics → Commit → RenderPrepare`, tracking camera, hashed snapshots. |

## World state

```
Observe → Compute → Prepare → Commit → Publish
```

* Entity = identity; Component = data; System = behavior.
* ECS / data-oriented storage; SoA by default.
* Authoritative mutation goes through `WorldTransaction`; a successful commit
  increments `WorldVersion`.
* Structural changes are deferred to commit; authoritative state is never
  mutated opportunistically during Compute.

## Distributed world

Exactly one authoritative owner per mutable entity:

```
Owned → PrepareTransfer → Frozen → Transferred → Confirmed → Owned
```

Epochs reject stale commands. Split-brain authority is never silently allowed.
Transport is replaceable — QUIC is the initial implementation.

## Memory & concurrency

Prefer SoA, chunks, contiguous data, batching, SIMD, zero-copy. Avoid heap
allocation, pointer chasing, locks, and virtual dispatch where unjustified. Use
`ReadView` / `WriteView` with explicit access declarations; atomics require
explicit justification.

## Capabilities

World, resource, device, network and JIT access are **capability-controlled**.
Untrusted plugins should be isolated; plugins never get unrestricted World
memory access.

See also: [Determinism](determinism.md) ·
[RFC-0001: Architecture](../../rfc/RFC-0001-architecture.md) ·
[RFC alignment](../rfc-alignment.md)
