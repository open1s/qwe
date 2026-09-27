# 0035 — Native backend bypasses RFC-0027's normative JIT/AOT lifecycle (no Validate, no CapabilityCheck, no capability manifest, no artifact identity) (Medium)

## Summary

Commit `7cc2d5a5` loads and executes generated machine code through a
path that skips the contract both governing RFCs define:

**RFC-0027 (Status: Normative)** requires:

> Lifecycle is **Compile→Validate→CapabilityCheck→Link→Publish→Execute**,
> with Profile/Invalidate returning to Compile. Artifact identity is
> RFC-0035 identity plus EIR hash, target, Runtime ABI major,
> SchemaSetHash, feature and profile hashes. …
> **No executable artifact runs without a capability manifest.**

`NativeProgram::compile` does: emit C → `cc` → `dlopen` → `call()`.
There is no validation stage, no capability check, no capability
manifest, no artifact identity (no EIR hash / ABI-major / schema hash
in the artifact or even in its filename — see also #31), and no
Publish step.

**RFC-0010 (Draft) §7** names Cranelift (JIT) / LLVM (AOT) as reference
backends — this uses system `cc`; **§8**: *"Unvalidated generated code
cannot enter production execution."*

## Evidence

- `rfc/RFC-0027-jit-contract.md` (quoted above, Normative).
- `rfc/RFC-0010-jit-aot.md` §7–§8.
- `reference/src/native.rs`: `compile()` → `Command::new("cc")` →
  `dlopen` → `call()`; no other stage exists in the file.
- `AotProgram::compile(module, target)` (the boundary the module docs
  claim to sit at) does serialize `target` + artifact hash, but
  `NativeProgram` is never constructed through it and `NATIVE_TARGET`
  (= 3) is not referenced anywhere outside `native.rs` — so there is
  also no end-to-end identity chain today.

AGENTS §16: for execution/ABI-adjacent changes, *RFC → conformance
tests → implementation; if the RFC is wrong, update it deliberately.*

## Impact

If/when this backend is wired into `pwe run`/runtime (the roadmap's
Phase 3 `[x]` marks it as landed), executable code enters execution
without the validation and capability gates the normative contract
requires — a capability/security hole by RFC terms, and a
conformance-test gap (RFC-0029 suites won't see this path). Landing
the contract question *before* integration is much cheaper than after.

## Suggested fix

One of, deliberately:

1. **Implement the contract for this class of artifact**: add a
   Validate stage (structural checks on the emitted module — exactly
   what `eligible()` half-does), a CapabilityCheck/manifest input
   (e.g. `NativeProgram::compile(module, manifest)` refusing to compile
   without one), artifact identity in the filename (hash-keyed — which
   also fixes #31), and route through `AotProgram` with
   `target = NATIVE_TARGET`.
2. **Amend RFC-0027** with a defined, narrower exemption for
   in-process pure-function kernels (no world access → no world
   capability surface), stating exactly which stages are skipped and
   why, plus conformance coverage for it.

Until then, keep `native.rs` unreachable from production entry points
(it currently is — no caller outside its own tests) and say so in the
roadmap entry.
