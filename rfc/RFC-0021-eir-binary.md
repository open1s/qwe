# RFC-0021: EIR Binary Format and Validation

**Status:** Normative — frozen for v0.2.0  
**Depends on:** RFC-0019, RFC-0032, RFC-0033, RFC-0034, RFC-0036

## Module envelope

```text
[8] "PWEEIR2\\0" | u16 major=2 | u16 minor=0 | u32 flags | [32] module_hash |
[32] schema_set_hash | [32] domain_ir_hash | u16 target_kind | u16 reserved=0 | u32 section_count | directory
```

Directory entries are RFC-0020 entries. Module hash is SHA-256 over the file with its hash bytes zeroed. Sections: `1 TYPES`, `2 IMPORTS`, `3 GLOBALS`, `4 FUNCTIONS`, `5 RESOURCES`, `6 DEBUG`, `7 EXTENSIONS`; TYPES and FUNCTIONS are REQUIRED. Target is `0 GENERIC`, `1 CPU`, `2 GPU`, `3 NPU`. Targets MAY constrain code generation but MUST NOT alter observable EIR semantics.

## Functions and operations

Functions are sorted by `FunctionId`: `u64 id | signature_type | u32 effect_mask | u32 block_count | blocks`. Blocks are sorted `u32 block_id`, start `u32 argument_count`, and finish with exactly one terminator. SSA IDs are monotonic `u32`; zero is invalid. Instruction: `u16 opcode | u16 flags | u32 result_id | u32 result_type | u32 operand_count | operand_ids`. Terminators have result 0: `RET=0x8000`, `BR=0x8001`, `COND_BR=0x8002`, `TRAP=0x8003`, `UNREACHABLE=0x8004`.

Core opcodes: `NOP=0`, `CONST=1`, `ADD..REM=16..31`, `EQ..GE=32..39`, `SELECT=40`, `CALL=48`, `READ_VIEW=64`, `WRITE_VIEW=65`, `LOAD=66`, `STORE=67`, `ATOMIC=68`, `EMIT_EVENT=69`, `TIME=70`, `RANDOM=71`, `IO=72`. Vendor operations are `0x4000..0x7fff` and require RFC-0036 declaration. Effect bits: `READ_WORLD=1`, `WRITE_WORLD=2`, `READ_RESOURCE=4`, `WRITE_RESOURCE=8`, `ATOMIC=16`, `IO=32`, `DEVICE=64`, `NETWORK=128`, `TIME=256`, `RANDOM=512`.

## Verification and semantics

Before execution/link/JIT, verify ordering, one definition/value, dominance, block targets, types, no implicit conversion, declared effects, capabilities, and no write/IO in pure functions. Time/random/IO/device/network are prohibited in deterministic functions. Failure is `PWE_E_EIR_INVALID` with byte offset/opcode.

Integer arithmetic wraps at width. Divide-by-zero, invalid shift/load/capability, and `TRAP` are defined traps. F32/F64 use RFC-0019 canonical output rules. Interpreter is semantic oracle; JIT/AOT preserve result, transaction, event order, and traps for identical inputs.
