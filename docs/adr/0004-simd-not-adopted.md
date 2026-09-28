# ADR-0004: Explicit SIMD not adopted (LLVM autovectorization wins)

**Status:** Accepted (Phase 3).

## Context

Phase 3 lists SIMD. The field-stencil interior is unit-stride and branch-free —
a natural vectorization target.

## Decision

Do **not** ship explicit 2-lane SIMD for the stencil. Measured an explicit
SSE2/NEON 2-lane kernel as **slower** than the scalar loop (256-wide row:
0.128 µs vs 0.096 µs); LLVM already autovectorizes the scalar loop better than a
narrow hand-written vector.

## Consequences

- The interior stays scalar and is left to the compiler's autovectorizer.
- No arch-specific `unsafe` (or its maintenance/verification burden).
- Recorded in the roadmap with the measurement (AGENTS §15).

## Alternatives

- **Wider hand-written SIMD** (AVX2/NEON 4×): plausible but heavier and
  arch-specific, with uncertain gain over LLVM; revisit only with evidence.
