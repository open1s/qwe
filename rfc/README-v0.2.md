# PWE Specification v0.2.0

## Status and normative language

This directory is the v0.2 implementation contract. It contains exactly 18 RFCs, `RFC-0019` through `RFC-0036`. The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative. A decoder or runtime that cannot meet a MUST requirement fails closed: it returns a defined error and makes no authoritative mutation.

Unless an RFC says otherwise, all multi-byte wire integers are unsigned little-endian, all lengths are byte lengths, all counts are `u32`, and all hashes are 32 raw SHA-256 bytes. Reserved fields MUST be zero when written. `u128` values are two little-endian `u64` values, low then high. Native-language memory layout is never a wire format.

## Frozen v0.2 contracts

WIR (RFC-0020), EIR (RFC-0021), Component ABI (RFC-0031), Runtime ABI (RFC-0026), and the Distributed State Machine (RFC-0024) are frozen at v0.2.0. A compatible implementation MAY add an optional extension only through the relevant extension rules; it MUST NOT reinterpret a defined bit, opcode, field, transition, status code, or ABI table slot.

## Core pipeline

```text
World Model -> WIR -> Domain IR -> EIR -> Interpreter/JIT/AOT -> Runtime -> CPU/GPU/NPU
```

World Model describes intent. WIR serializes model plus initial authoritative state. Domain IR lowers domain systems without choosing hardware. EIR is the typed executable representation. The Runtime is the sole owner of mutable authoritative World State.

## Non-negotiable invariants

1. World is the single logical source of truth.
2. World Model and World State are separate.
3. Entity is identity; Component is data; System is behavior.
4. WIR is runtime-independent and EIR is hardware-independent.
5. Physics, Render, AI, and Sensor never own authoritative duplicate worlds.
6. Interpreter, JIT, and AOT share EIR observable semantics.
7. Cross-module mutation uses capability-bound views and the frozen ABIs.
8. A mutable entity has exactly one authoritative owner for one ownership epoch.
9. Incompatible schema, ABI, or artifact identities fail closed.
10. A successful transaction is the only way normal system writes become authoritative.

## RFC map

| RFC | Contract |
| --- | --- |
| 0019 | Schema and canonicalization |
| 0020 | WIR binary |
| 0021 | EIR binary and validation |
| 0022 | Memory and concurrency |
| 0023 | World transaction |
| 0024 | Distributed ownership state machine |
| 0025 | Snapshot and state delta |
| 0026 | Runtime ABI |
| 0027 | JIT contract |
| 0028 | Render-frame consistency |
| 0029 | Conformance |
| 0030 | Minimal reference profile |
| 0031 | Component ABI |
| 0032 | Domain IR |
| 0033 | Capability and view ABI |
| 0034 | Error, result, and limits |
| 0035 | Artifact identity and compatibility |
| 0036 | Interchange envelope and extension registry |

An implementation claiming `pwe-v0.2` MUST implement RFC-0019, 0020, 0021, 0023, 0026, 0029, 0031, 0033, 0034, 0035, and 0036. It MUST also implement RFC-0024 and 0025 when it exposes remote regions or persistence, and RFC-0027 when it exposes a JIT.
