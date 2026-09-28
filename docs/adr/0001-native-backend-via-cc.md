# ADR-0001: Native backend via the system C compiler (`cc` + `dlopen`)

**Status:** Accepted (Phase 3).

## Context

Phase 3 asks for a real JIT/AOT ("Cranelift etc."). Adding a code generator
in-crate means a heavy dependency (compile time, binary size, MSRV) that must be
vetted under AGENTS §12, and EIR world access goes through `dyn EirRuntime`, so
a Cranelift JIT would need an extern-ABI callback layer regardless.

## Decision

Emit C for eligible EIR functions and compile it with the **system `cc`**
(`-O2 -ffp-contract=off`), load it with `dlopen`, and call it through a
`#[repr(C)]` function-pointer table (`PweCtx`) into the runtime. No new Rust
dependency. This is the `NativeProgram` backend (`native.rs`) and the AOT path
(`aot.rs`).

## Consequences

- Real machine code today, dependency-free; unavailable without a C compiler
  (returns an error, never a hard build/runtime requirement).
- Not sandboxed (runs in-process); gated by the lifecycle/exemption (ADR-0002).
- `-ffp-contract=off` preserves the interpreter's two-rounding `Fma` semantics.

## Alternatives

- **Cranelift/LLVM**: heavier dependency and ABI work for the same semantics.
- **No native code**: keeps only the interpreter (rejected for Phase 3).
