# PWE Architecture

PWE is a Rust microkernel runtime + language for deterministic, distributed
simulation of a 4D world (3D space + time). This document is the map; the
normative contracts live in `rfc/` and the binding rules in `AGENTS.md`.

## Pipeline

```
World Model → WIR → Domain IR → EIR → Interpreter / JIT / AOT → CPU / GPU
```

- **World Model** is the single authoritative source of truth (`reference/src/dsl.rs`).
- **WIR** describes *what the world is*; **Domain IR** (Physics/Render/Sensor/AI)
  describes domain computation; **EIR** is the typed, SSA-like, verifiable,
  capability- and determinism-aware executable form (`reference/src/eir.rs`,
  RFC-0021).
- Production execution preserves `WIR → Domain IR → EIR → execution`; there is
  no undocumented bypass.

The EIR **interpreter is the semantic oracle** (RFC-0004); JIT/AOT/GPU are
optimized implementations that must agree with it.

## Crates

| Crate | Role |
| --- | --- |
| `pwe-api` (repo root) | Frozen v0.2 ABI: handles, `WorldId`/`WorldVersion`, capabilities, errors. `no_std`, zero dependencies. |
| `pwe-reference` | The reference implementation: language front end (`lang/`), IRs, interpreter/JIT/AOT, backends, scene, rendering adapter. |
| `pwe-cli` | The `pwe` toolchain: `compile`, `run`, `present`, `migrate`. |
| `pwe-conformance` | RFC conformance harness (release gate). |
| `gpu-verify` (standalone) | Host-only GPU execution verification (Metal via `wgpu`); kept out of the workspace so CI never builds it. |

## Execution tiers (EIR)

| Tier | Where | Notes |
| --- | --- | --- |
| Interpreter (jump table) | `eir.rs` `run_call_tree` | Default; dense `Regs`, flat instruction stream (effectively pre-decoded). |
| Threaded dispatch | `eir.rs` `execute_threaded_with_index` | Opt-in (`--threaded`); static handler table; **slower** here, kept selectable + verified. |
| Optimizing pass | `eir.rs` `optimize()` | Constant folding + `Mul/Add → Fma` (bit-exact). Applied by AOT/interpreter. |
| Native JIT | `jit.rs` + `native.rs` | Hotness→native C (`cc` + `dlopen`); deopt on unavailability, propagates traps. **Default on** in `pwe run`. |
| Native AOT | `aot.rs` | Optimize + freeze + content-hash artifact. |
| GPU offload | `gpu.rs` (feature `gpu`, macOS) | Metal field-sweep offload; **experimental**, f32-approximate. |
| WGSL device backend | `wgsl.rs` | Lowers map kernels to WGSL; `naga`-validated; trap via an atomic flag. |

The interpreter and the JIT run in lockstep when configured (`step_cross`), and
their outputs must be byte-identical — this is the differential contract.

## Boundaries

Kernel mechanisms (`memory / handles / scheduling / task / sync / time /
capability / transaction / event / resource / ABI / device`) are domain-neutral;
physics/render/AI/sensor policy lives outside the kernel. Platform/backend
details stay behind backend boundaries. See `AGENTS.md` §2 and §14.

## Determinism & correctness

- Time is explicit and domain-based; `WorldTransaction` is the only
  authoritative mutation path; successful commit increments `WorldVersion`.
- Stable ABIs use `#[repr(C)]`, fixed-width types, opaque handles, explicit
  status codes; Rust ABI types never cross stable boundaries.
- Runtime paths avoid `panic`/`unwrap`; typed `Result<T, E>` throughout.
- Verification: `cargo test --workspace`, `pwe-conformance`, differential
  (`step_cross`) tests, and recorded benchmarks (`reference/benches`).

## Related

- `AGENTS.md` — binding engineering rules.
- `rfc/` — normative contracts (see `docs/rfc-alignment.md` for the matrix).
- `docs/review-and-roadmap.md` — the improvement roadmap and status.
- `docs/adr/` — architecture decision records.
