# RFC-0019: Schema and Canonicalization

**Status:** Normative — frozen for v0.2.0  
**Depends on:** RFC-0034, RFC-0036

## Scope and identifiers

A schema is declarative; it is not native struct layout. Every WIR component, EIR typed value, snapshot value, ABI component descriptor, and artifact is interpreted against a `SchemaSetHash`.

`ComponentSemanticId` is UTF-8 `namespace`, ASCII `stable_name`, and non-zero `major_version`. Both names match `[a-z][a-z0-9_.-]{0,127}`. `ComponentTypeId` is the first 16 bytes of `SHA-256("pwe.component/v2\\0" || ComponentSemanticIdCanonicalBytes)`. Collisions are fatal (`PWE_E_HASH_COLLISION`); readers compare complete semantic IDs.

`SchemaHash = SHA-256(SchemaCanonicalBytes)`. `SchemaSetHash = SHA-256("pwe.schema-set/v2\\0" || sorted(SchemaHash))`; duplicates are invalid. `WorldId` and `EntityId` are non-zero opaque `u128`. Entity generation is a non-zero `u32` and changes on reuse.

## Canonical bytes

```text
u16 schema_format=2 | ComponentSemanticId | u32 field_count | fields
ComponentSemanticId := u16 namespace_len | namespace | u16 name_len | name | u32 major
field := u32 field_id | u16 name_len | name | Type | u16 flags | DefaultValue
```

Strings are UTF-8, NUL-free, Unicode NFC. `field_id` is non-zero and unique. Fields are ascending `field_id`; names, type parameters, map keys, dependencies, resources, systems, and entity IDs use bytewise ascending order. Flags: `REQUIRED=1`, `TRANSIENT=2`, `REPLICATED=4`, `READ_ONLY=8`; other bits fail.

Type tags are `1 Bool`, `2 I8`, `3 I16`, `4 I32`, `5 I64`, `6 U8`, `7 U16`, `8 U32`, `9 U64`, `10 F32`, `11 F64`, `12 Bytes`, `13 String`, `14 EntityRef`, `15 Struct`, `16 Array`, `17 Map`, `18 Optional`, `19 FixedBytes`. Parameters recurse canonically. Maps require a totally orderable key. Booleans are exactly 0/1; floats canonicalize `-0` to `+0` and every NaN to quiet `0x7fc00000`/`0x7ff8000000000000`.

## Evolution and validation

Within a major version a field may be appended with a default; changing a field ID/type/meaning, removing a required field, or changing semantic identity requires a new major. Migrations are pure bounded functions from `(old SchemaHash, canonical value)` to `(new SchemaHash, canonical value)` registered before data is accepted.

Validation order is bounds → UTF-8/NFC → identifier syntax → ordering/uniqueness → type validity → defaults → hash. Mismatch is `PWE_E_SCHEMA_HASH`; unknown major is `PWE_E_SCHEMA_UNSUPPORTED`. Fixtures MUST prove equal semantics give identical bytes and NaN/negative-zero normalization is stable.
