# ADR-0005: GPU field offload is experimental (f32; opt-in)

**Status:** Accepted (Phase 3).

## Context

Phase 3 asks for a GPU/NPU backend. The engine's authoritative state is on the
CPU in f64; GPU compute is f32. The WGSL map-kernel emitter and a Metal field
stencil were implemented.

## Decision

Ship the in-runtime GPU path (`pwe run --gpu`, feature `gpu`, macOS `metal`) as
an **experimental, correctness-verified f32 offload**, explicitly **not** an
accelerator. The default (CPU) path is authoritative and deterministic.

## Consequences

- Measured **slower than the CPU** (256²: CPU 129 µs vs GPU 244 µs; 1.34×–2.55×
  across sizes) — per-step `f64↔f32` conversion + upload/readback + sync
  dominate. Only device-resident field state would win.
- `metal` is target-gated (macOS only), so non-macOS CI never builds it.
- WGSL execution is verified on the GPU host via the standalone `gpu-verify`
  crate (Metal through `wgpu`).

## Alternatives

- **Device-resident field buffers** (the only change that can win) — deferred;
  requires dirty-tracking and on-demand readback in the CPU-authoritative scene.
