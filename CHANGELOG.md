# Changelog

All notable changes to PWE are documented here. The project follows the frozen
v0.2 contract defined by RFC-0019 through RFC-0036.

## [0.2.0] — 2026-09-13

Initial release implementing the complete frozen v0.2 contract.

### Added

- `pwe-api` (`no_std`, dependency-free): frozen types/traits, ability to
  compute canonical hashes, advertised limits, stable detail-code catalog,
  time/clock-domain/determinism models, compatibility matrix, resource
  identity/residency, spatial/interest queries, audit events, and the
  C11 runtime + plugin ABI surfaces.
- `pwe-reference`: deterministic semantic oracle covering
  - schema registry + canonicalization (RFC-0019)
  - WIR encode/decode + rejection (RFC-0020)
  - typed SSA EIR validator + interpreter (RFC-0021)
  - declared-access scheduler (RFC-0022)
  - transactional world (RFC-0023)
  - distributed ownership state machine (RFC-0024)
  - snapshot/delta + transactional restore (RFC-0025)
  - CPU JIT with hotness/assumptions/safe-point deopt (RFC-0027)
  - immutable render frame (RFC-0028)
  - component ABI / domain IR / capability / errors / artifact identity /
    interchange envelope (RFC-0031..0036)
  - physics IR domain nodes (folded from draft RFC-0008)
  - `pwe-lang`: a textual PWE source language (lexer + parser) that compiles
    to low-level EIR and runs cross-backend (interpreter + CPU JIT) through a
    runtime with byte-identical write agreement (`reference/src/lang.rs`).
- `pwe-conformance`: RFC-0029 report runner and RFC-0030 minimal scenario;
  12 cases pass with no skips.
- `include/pwe_abi.h`: generated C11 header, layout-locked to the Rust ABI.
- `docs/rfc-alignment.md` and `docs/rfc-supersession.md`.

### Notes

- The JIT is a validated compiled cache sharing the interpreter's semantics,
  not a native machine-code emitter. A native backend (Cranelift/LLVM) is a
  deliberately-scoped future addition.