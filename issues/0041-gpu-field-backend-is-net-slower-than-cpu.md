# 0041 — GPU field-sweep backend is net-slower than the CPU at every tested size (Medium)

**Reported:** pass 16 (commit `55457ff1`)
**Severity:** Medium
**Component:** `reference/src/gpu.rs` (field_diffuse Metal path), `reference/src/physics_eir.rs`
**Status:** open

## Summary

`pwe run --gpu` ("Metal GPU acceleration for field sweeps", `reference/Cargo.toml`
comment; roadmap marks it as an opt-in 加速/accelerator) is **measurably slower
than the plain CPU interpreter at every workload size tested on the Apple M2
Max** — 2.55× slower at 0.26M cells down to 1.34× slower at 16.8M cells. The
feature as shipped cannot beat the loop it replaces.

## Reproduction

Fixture: 3-D `diffuse` field model (`width×height×depth`, one seed + one
diffuse sweep per step), release builds of `pwe-cli --features gpu`:

| field size | cells | steps | CPU real | `--gpu` real | ratio |
|---|---|---|---|---|---|
| 128×128×16 | 262,144 | 500 | 0.19–0.20 s | 0.50–0.51 s | **2.55× slower** |
| 256×256×32 | 2,097,152 | 200 | 0.62–0.66 s | 1.26–1.28 s | **2.0× slower** |
| 512×512×64 | 16,777,216 | 50 | 1.42–1.43 s | 1.90–1.95 s | **1.34× slower** |

(Run with `/usr/bin/time -p`, 2–3 repetitions, identical native-JIT settings,
same binary with and without `--gpu`. Cross-guard steps execute the GPU path
twice — both `rt_a`/`rt_b` attach the context — which is correct for tier
agreement but doubles dispatch on those steps.)

## Root cause

`Gpu::diffuse` rebuilds the whole device pipeline **per step**:

1. `f64 → f32` conversion of the full field in scalar Rust (host, per step),
2. `new_buffer_with_data` allocation + upload of the input,
3. encode → `commit` → `wait_until_completed` (fully synchronous),
4. full-device readback,
5. `f32 → f64` conversion back (host, per step).

The MSL kernel itself is microseconds; the per-step host conversion passes and
buffer churn dominate, and they scale linearly with cell count exactly like the
CPU loop they replace — so the fixed-cost amortization visible in the table
never flips the sign.

## Fix directions (in preference order)

1. **Device-resident field state**: keep the field buffer alive on the GPU
   across steps (double-buffered or in-place with a barrier), convert/upload
   once at attach or on CPU-side writes only, read back on demand
   (inspection, `total=` reporting, ownership transfer). This is the only
   change that can actually win.
2. Reuse pooled buffers sized to the field (kill per-step allocation), and
   overlap encode/compute with the next step's host work instead of
   `wait_until_completed` immediately.
3. If (1)/(2) are out of scope: relabel the backend as *experimental
   correctness-verified f32 offload* in `reference/Cargo.toml`, the CLI
   banner and the roadmap line, and add a benchmark record per AGENTS §15
   ("benchmark performance claims") showing the crossover — do not call it
   an accelerator until it accelerates.

## Notes

- Correctness itself is healthy: field totals CPU vs GPU —
  207.620740 vs 207.620935 (500 steps, rel Δ≈9e-7), 1969.339306 vs
  1969.359782 (5000 steps, rel Δ≈1e-5), 23.113062 vs 23.113063
  (16.8M cells, 50 steps) — bounded f32 drift, no blow-up.
- Default path unaffected (feature-gated, opt-in, CPU stays the oracle).
