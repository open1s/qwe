# RFC-0020: WIR Binary Format

**Status:** Normative — frozen for v0.2.0  
**Depends on:** RFC-0019, RFC-0034, RFC-0036

## Envelope

WIR carries a World Model and optional initial authoritative state. It is runtime-, pointer-, and host-layout-independent.

```text
0 [8] magic="PWEWIR2\\0" | 8 u16 major=2 | 10 u16 minor=0 | 12 u32 flags
16 [32] schema_set_hash | 48 u128 world_id | 64 u64 world_version | 72 i64 sim_time_ns
80 u32 section_count | 84 u32 header_crc32c(bytes 0..79) | 88 directory[section_count]
```

Flags are `INITIAL_STATE=1` and `DEBUG_NAMES=2`; other bits fail. Each 32-byte directory entry is `u16 kind | u16 flags | u32 reserved=0 | u64 offset | u64 length | u32 crc32c | u32 reserved=0`. Offsets are 8-byte aligned, non-overlapping, ascending, and after the directory. Flags are `OPTIONAL=1`, `AUTHORITATIVE=2`, `COMPRESSED=4`; compression follows RFC-0036.

## Sections and records

Kinds: `1 SCHEMA`, `2 ENTITIES`, `3 COMPONENTS`, `4 RESOURCES`, `5 SPATIAL`, `6 SYSTEMS`, `7 EVENTS`, `8 TIMELINES`, `9 EXTENSIONS`. `SCHEMA`, `ENTITIES`, and `COMPONENTS` are REQUIRED; a known kind occurs once. Unknown non-optional kinds fail. Each section starts `u32 record_count`; each variable record starts `u32 byte_len` and is consumed exactly.

`ENTITIES`: `EntityId | u32 generation | u32 flags`, sorted `(EntityId,generation)`. `COMPONENTS`: `ComponentTypeId | EntityId | u32 generation | u32 value_len | canonical_schema_value`, sorted `(ComponentTypeId,EntityId,generation)`. Duplicates fail. `INITIAL_STATE` determines whether values are authoritative; otherwise they are model defaults only.

## Canonicalization and validation

A pure model writes `world_version=0`. Writers use only zero alignment padding and CRC stored bytes. Readers validate magic/version, header CRC, limits, directory, section CRC, uniqueness, ordering, schema closure, references, then full consumption. Overflow, unsafe allocation, missing schema/entity, non-zero reserved field, or size beyond `PWE_LIMIT_MAX_DOCUMENT_BYTES` fails. Re-encoding decoded canonical WIR MUST reproduce identical bytes excluding a transport compression layer.
