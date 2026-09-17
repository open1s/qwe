# RFC-0012: Memory and Data Layout

**Status:** Draft \| **Version:** 0.1.0

## 1. Memory regions

Static, Dynamic, Frame, Scratch, Shared, GPU.

## 2. Ownership

World Runtime owns authoritative storage. Systems borrow through scoped
views. Allocations on hot paths should be eliminated or bounded.

## 3. Layout

Default ECS layout is SoA. Compiler may choose AoS/AoSoA per access
pattern.

## 4. Alignment

ABI-visible types specify size/alignment explicitly. SIMD/GPU alignment
is a backend concern exposed through layout metadata.

## 5. Zero-copy

Views reference existing storage whenever safe. Cross-device transfer is
explicit and represented as an EIR resource operation.

## 6. Memory ordering

Atomic accesses declare ordering. Non-atomic accesses require
scheduler-established happens-before relationships.

## 7. GPU residency

GPU buffers have ownership/state transitions and synchronization fences;
CPU code cannot assume GPU visibility without synchronization.

## 8. Acceptance

A race detector/validator can derive legal concurrent access from system
declarations and memory effects.
