# ADR-0002: In-process native-kernel exemption from the JIT lifecycle

**Status:** Accepted (Phase 3). Normative text: RFC-0027 addendum.

## Context

RFC-0027 (Normative) requires Compile→Validate→CapabilityCheck→Link→Publish→
Execute and "no executable artifact runs without a capability manifest". The
native C backend is in-process and world-access-only.

## Decision

Define a **narrow exemption** for in-process native kernels: Validate is never
skipped; effect bits are limited to world read/write + barrier; no capability
surface beyond the caller's own world; not reachable from a production entry
point. The **supported production route** is execution *through the CPU JIT*,
whose `ready()` enforces Validate/CapabilityCheck/Publish before promoting a hot
unit to native code (ADR-0002 enables `pwe run`'s default-on native JIT). The
standalone `NativeProgram` API stays exempt and test-only.

## Consequences

- Native code runs only after the JIT lifecycle gate; deopt is reserved for
  native being unavailable (execution traps propagate — see issue #40).
- Artifacts are content-hash identified (RFC-0035 style).

## Alternatives

- **Full lifecycle for the standalone API** (manifest/Publish) — deferred; not
  needed while it is not a production entry point.
