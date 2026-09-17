# RFC Supersession Map (Draft v0.1 → Frozen v0.2)

`rfc/README-v0.2.md` states the v0.2 contract is **exactly RFC-0019 through
RFC-0036**. RFC-0001 through RFC-0018 are earlier `Draft` (v0.1.0) documents
whose normative content has been folded into the frozen set. This file records
that mapping so no requirement is silently dropped and no Draft RFC is
re-implemented as if it were still current.

## Direct supersession

| Draft (v0.1) | Frozen (v0.2) | Notes |
| --- | --- | --- |
| 0001 Architecture | `AGENTS.md` layers + invariants | Layers and downward-dependency rule live in `AGENTS.md` §1/§14. |
| 0002 World IR | 0020 WIR | Binary + validation superseded the draft model; EntityId is `u128`, not `u64` (frozen §README). |
| 0003 Execution IR | 0021 EIR | Typed SSA, ops, validation, interpreter oracle. |
| 0004 Component ABI | 0031 Component ABI | Descriptor tuple frozen; flags/align/size. |
| 0005 Execution/Consistency | 0023 World Transaction | Observe→Compute→Prepare→Commit→Publish; conflict policies; WorldVersion. |
| 0006 Distributed World | 0024 Ownership State Machine | Prepare→Freeze / prepare/reserve/install/release; epoch; split-brain. |
| 0007 Render IR | 0028 Render-frame consistency | Frame references one WorldVersion + RenderTime. |
| 0009 Compiler | 0032 Domain IR + 0027 JIT + 0035 Artifact | Validate→lower→optimize folded into Domain IR/JIT contract. |
| 0010 JIT/AOT | 0027 JIT Contract | Hotness→specialize→invalidate→deopt; assumptions; cache key. |
| 0011 Runtime ABI | 0026 Runtime ABI | Versioned C function table; size field; no unwind. |
| 0012 Memory layout | 0022 Memory/concurrency | SoA default; ReadView/WriteView; declared access. |
| 0017 Security/capability | 0033 Capability | Capabilities scoped; verifier; enforcement before transaction. |
| 0018 Versioning | 0034 Errors/limits + 0035 Artifact | Independent version domains; hashes; fail closed. |

## Folded-in but worth surfacing

These Draft requirements had no exact frozen equivalent and are recorded here as
forward-looking refinements, implemented as **kernel-neutral data types** (no
physics/render/sensor engines live in `pwe_api` or the kernel — `AGENTS.md` §2):

| Draft | Forward-looking content | Implementation |
| --- | --- | --- |
| 0013 Time & determinism | `SimTime{tick,nanos}`, clock domains (Real/Sim/Replay/Prediction), determinism levels (BestEffort/Reproducible/StrictDeterministic), fixed-step, canonical state hash | `pwe_api::time` |
| 0018 Compatibility matrix | advertise accepted major/minor per domain; reject unknown major | `pwe_api::compat` |
| 0015 Resource/asset | `ResourceId` + content hash + schema version + residency (CPU/GPU/cache/remote) + content addressing | `pwe_api::resource` |
| 0016 Plugin/backend ABI | `PwePluginApi` (register_component/register_system), plugin kinds, isolation, quarantine | `pwe_api::ffi::PwePluginApi` |
| 0014 Spatial/interest | spatial index abstraction + `InterestQuery` (region/class/component/LOD/visibility) + no-authority-effect invariant | `pwe_api::spatial` |
| 0008 Physics IR | BroadPhase→…→Commit pipeline as **Domain IR** nodes; solver-agnostic | `pwe_reference::domain_ir` physics nodes |
| 0017 Audit | security-sensitive mutations emit audit events (actor/identity/capability/version/op) | `pwe_api::audit` |

## Explicit non-goals (unchanged)

Draft RFCs named here but intentionally out of the v0.2 minimal profile
(`AGENTS.md` §2 kernel boundary + RFC-0030 profile): real GPU backends
(Metal/Vulkan/wgpu), physics solvers, neural rendering, city-scale simulation.
These remain peer-domain concerns and are **not** re-entered into the frozen
contract.