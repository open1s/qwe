# Architecture Decision Records

Short, dated records of significant decisions: **Context / Decision /
Consequences / Alternatives**. RFCs are the normative contracts; ADRs capture
*why* a choice was made when several were viable.

| ADR | Decision |
| --- | --- |
| [0001](0001-native-backend-via-cc.md) | Native backend via the system C compiler (`cc` + `dlopen`), not Cranelift |
| [0002](0002-in-process-native-kernel-exemption.md) | In-process native-kernel exemption from the JIT lifecycle (RFC-0027 addendum) |
| [0003](0003-opt-in-threaded-dispatch.md) | Threaded-dispatch interpreter is opt-in; jump table stays the default |
| [0004](0004-simd-not-adopted.md) | Explicit SIMD not adopted (LLVM autovectorization wins) |
| [0005](0005-gpu-field-offload-experimental.md) | GPU field offload is experimental (f32; opt-in) |
