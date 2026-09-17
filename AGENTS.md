# AGENTS.md — PWE

PWE is a Rust-based microkernel runtime for programmable, distributed physical-world representation, simulation, compilation, and rendering.

**Read this file before changing code. For detailed semantics, follow the applicable RFC.**

## Rules
- When determining which codebase directory or microservice a bug report belongs to, use the `laya` MCP tool to instantly classify the issue into categories (e.g., [frontend, backend, database]) instead of generating a full text reasoning path.

## Tools
- use zg to query and search — prefer zg (zvec-grep) over rg/grep for all searching
- use jj for versioning control

## 1. Architecture

```text
World Model → WIR → Domain IR → EIR → Interpreter/JIT/AOT → Runtime → CPU/GPU/NPU/Edge/Cloud
```

Core rules:

* **World is the single authoritative source of truth.**
* Model ≠ State.
* Entity = identity; Component = data; System = behavior.
* ECS/data-oriented storage; SoA by default.
* Physics, Render, Sensor, AI, Network are peer domains, not independent worlds.
* Authoritative mutation goes through `WorldTransaction`.
* A mutable entity has exactly one authoritative owner.
* Time is explicit and domain-based.
* Determinism is a first-class execution mode.
* Rendering is a first-class domain and consumes a consistent `RenderView`.

## 2. Microkernel Boundary

Kernel provides mechanisms only:

```text
memory / handles / scheduling / task / sync / time /
capability / transaction / event / resource / ABI / device
```

Kernel MUST NOT depend on:

```text
physics / rendering / AI / sensors / application logic
```

Domain policy belongs outside the kernel.

## 3. IR Rules

### WIR

Describes **what the world is**.

WIR MUST NOT depend on:

```text
CPU / GPU / LLVM / Cranelift / Metal / Vulkan / CUDA / OS
```

### Domain IR

Describes domain computation:

```text
Physics IR
Render IR
Sensor IR
AI IR
...
```

### EIR

Describes **executable computation**.

EIR MUST be:

* typed
* SSA-like
* verifiable
* capability-aware
* deterministic-aware
* target-neutral

Production execution MUST preserve:

```text
WIR → Domain IR → EIR → execution
```

Do not bypass EIR with undocumented execution paths.

## 4. Execution

Interpreter, JIT and AOT MUST share EIR semantics.

The **EIR interpreter is the semantic reference**.

```text
Interpreter ≡ semantic baseline
JIT/AOT     ≡ optimized implementations
```

Compiler optimization MUST NOT change semantics.

JIT MUST support:

```text
hotness → specialization → invalidation → deoptimization
```

## 5. World State

Use:

```text
Observe → Compute → Prepare → Commit → Publish
```

Authoritative state MUST NOT be mutated opportunistically during Compute.

Successful commit increments `WorldVersion`.

Structural changes are deferred to commit.

## 6. Distributed World

Exactly one authoritative owner per mutable entity.

Ownership:

```text
Owned → PrepareTransfer → Frozen → Transferred → Confirmed → Owned
```

Use epochs to reject stale commands.

Never silently allow split-brain authority.

Transport is replaceable; QUIC is the initial implementation.

## 7. ABI

Stable ABI MUST use:

```text
#[repr(C)]
fixed-width types
opaque handles
explicit sizes
versioned function tables
explicit status codes
```

Never expose Rust ABI types across stable boundaries:

```text
String / Vec / Box / dyn Trait / references / closures / panic
```

No unwind across ABI.

## 8. Schema & Serialization

Schemas are versioned and hashed.

Breaking schema changes require explicit migration.

Production binary formats MUST be:

```text
versioned + schema-aware + canonical + deterministic
```

Do not invent incompatible ad-hoc formats.

## 9. Memory & Concurrency

Prefer:

```text
SoA / chunks / contiguous data / batching / SIMD / zero-copy
```

Avoid unnecessary:

```text
heap allocation / pointer chasing / locks / virtual dispatch
```

Use `ReadView` / `WriteView` and explicit access declarations.

Atomics require explicit justification.

## 10. Capabilities

World/resource/device/network/JIT access MUST be capability-controlled.

Never grant unrestricted World memory access to plugins.

Untrusted plugins SHOULD be isolated.

## 11. Unsafe

`unsafe` is allowed for:

```text
FFI / memory / lock-free / SIMD / devices / JIT / zero-copy
```

Keep unsafe isolated behind safe abstractions and document required invariants.

## 12. Dependencies

Before adding a dependency, check:

```text
license / maintenance / security / compile cost /
binary size / runtime overhead / platform support
```

Prefer small, mature dependencies.

## 13. Coding Rules

Prefer:

```text
explicit invariants
Result<T,E>
small APIs
data-oriented design
zero-copy where justified
```

Avoid:

```text
panic in runtime paths
global authoritative state
hidden side effects
invented APIs
silent fallback
TODO pretending to be implementation
```

Public architectural APIs MUST be documented.

Do not use `unwrap()` in production paths unless the invariant is locally proven.

## 14. Crate Boundaries

Keep the architectural dependency direction:

```text
application
    ↓
world
    ↓
IR
    ↓
compiler
    ↓
runtime
    ↓
kernel
    ↓
platform
```

Never introduce reverse domain dependencies.

Platform/backend details stay behind backend boundaries.

## 15. Agent Workflow

Before editing:

```text
inspect → identify RFC → understand invariants → plan → implement
```

After editing:

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features
```

For relevant changes also run:

```text
conformance
determinism
serialization
ABI
interpreter/JIT/AOT equivalence
distributed ownership
```

Benchmark performance claims.

## 16. Change Policy

When changing any:

```text
ABI / schema / WIR / EIR / network protocol /
snapshot / transaction / ownership semantics
```

check the corresponding RFC first.

Do not silently break a frozen contract.

If implementation and RFC disagree:

```text
RFC → conformance tests → implementation
```

If the RFC is wrong, update it deliberately.

## 17. Decision Test

Before accepting an architectural change, verify:

1. World remains the single source of truth.
2. WIR and EIR boundaries remain intact.
3. Kernel remains domain-neutral.
4. Deterministic execution remains possible.
5. Heterogeneous execution remains possible.
6. No stable ABI/schema/protocol is silently broken.

**When in doubt: preserve semantics first, architecture second, performance third, convenience last.**
