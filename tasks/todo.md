# PWE v0.2 Implementation Tasks

## Task 1: Canonical byte and validation kernel

- [x] Add bounded little-endian reader/writer, then a RFC-0019 SHA-256 canonical hash implementation.
- Acceptance: malformed lengths never panic or allocate beyond limits; canonical values round-trip; hashes are SHA-256.
- Verify: `cargo test --workspace canonical`.
- Depends on: none. Files: `reference/src/*`. Scope: M.

## Task 2: Schema registry and WIR slice

- [x] Implement schema registration plus SCHEMA/ENTITIES/COMPONENTS WIR encode/decode.
- Acceptance: canonical WIR re-encodes byte-identically; invalid directory/order is rejected.
- Verify: `cargo test --workspace wir`. Depends on: 1. Scope: M.

## Task 3: Snapshot and delta

- [x] Implement deterministic snapshot and base-version-checked delta apply.
- Acceptance: failed restore changes no state; replay hashes match.
- Verify: `cargo test --workspace snapshot`. Depends on: 1. Scope: M.

## Task 4: Capability claims and reference enforcement

- [x] Replace non-empty-token placeholder with bounded claims verifier.
- Acceptance: access/region/version failures are rejected before transaction creation.
- Verify: `cargo test --workspace capability`. Depends on: 1. Scope: S.

## Task 5: Domain IR and EIR validator/interpreter

- [x] Implement bounded typed EIR subset and deterministic interpreter.
- Acceptance: invalid SSA/effects reject; interpreter produces ordered transaction writes.
- Verify: `cargo test --workspace eir`. Depends on: 1, 4. Scope: M.

## Task 6: Artifact identity and compiled-cache contract

- [x] Implement artifact identity validation and interpreter-backed cache.
- Acceptance: incompatible ABI/schema/artifact fails closed; invalidation is atomic.
- Verify: `cargo test --workspace artifact`. Depends on: 5. Scope: S.

## Task 7: Scheduler and render frame

- [x] Implement declared-access schedule validation and immutable render frame acquisition.
- Acceptance: conflicts reject; one frame reads one WorldVersion.
- Verify: `cargo test --workspace schedule render`. Depends on: 3, 4. Scope: M.

## Task 8: Conformance runner and minimal scenario

- [x] Add fixtures/report and ground-vehicle-camera replay scenario.
- Acceptance: RFC-0029 required cases report passed with no skips.
- Verify: `cargo test --workspace` and `cargo run -p pwe-conformance`. Depends on: 2–7. Scope: M.

## TODO: GPU / NPU / SIMD execution backends

- [ ] **GPU backend** — lower EIR to a GPU compute target (Metal / CUDA / Vulkan
  compute) behind the existing `AotProgram.target` backend boundary. Unimplemented
  in this reference: no target toolchain/hardware is available, and the EIR is
  deliberately target-neutral.
- [ ] **NPU backend** — the same boundary with a vendor NPU toolchain.
  Unimplemented (no toolchain available).
- [ ] **SIMD backend** — a lane-based executor running one instruction stream
  across a batch of entities (SIMT-style) with platform vector intrinsics.
  Unimplemented: it would be a third execution implementation and must first
  prove differential equivalence against the interpreter (AGENTS.md: semantics
  first, performance last).
- Acceptance (all three): a backend produces byte-identical writes and identical
  event streams to the interpreter across a corpus, and is wired into
  `step_cross`-style differential verification.
- Note: the reference ships three differential-verified backends — the
  **interpreter** (semantic reference), the **JIT** (interpreter-backed, proven
  every step by `step_cross`), and the **AOT** (artifact identity + verification
  + execution; `cargo test aot`, conformance `aot_and_fence`).
