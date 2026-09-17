# RFC-0004: Component ABI

**Status:** Draft \| **Version:** 0.1.0

## 1. ABI principle

The stable ABI is C-compatible metadata plus PWE Schema. Rust private
layout is never a public ABI.

## 2. Descriptor

``` rust
#[repr(C)]
pub struct ComponentDescriptor {
    pub type_id: u128,
    pub version: u32,
    pub size: u32,
    pub alignment: u32,
    pub flags: u64,
    pub schema: *const SchemaDescriptor,
}
```

## 3. Flags

POD, TrivialCopy, Send, Sync, GPUReadable, GPUWritable,
NetworkReplicated, Snapshotable, Deterministic, ZeroCopy.

## 4. Access

World owns storage. Domains receive ReadView/WriteView. Raw pointers
must not escape a safe access scope.

## 5. Storage

Default layout is SoA; AoS/AoSoA are legal compiler transformations.

## 6. Capability

READ, WRITE, EXECUTE, GPU_READ, GPU_WRITE, NETWORK_READ, NETWORK_WRITE.

## 7. Lifecycle

Registered -\> Allocated -\> Active -\> Migrating -\> Inactive -\>
Destroyed.

## 8. Migration

Schema major changes require an explicit migration function. Entity
identity must survive migration.

## 9. Hot reload

Only at a Migration Safe Point; no live code may observe partially
migrated component storage.

## 10. Invariant

A plugin cannot directly mutate authoritative World memory without a
valid capability and transaction.
