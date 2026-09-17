# RFC-0018: Versioning and Compatibility

**Status:** Draft \| **Version:** 0.1.0

## 1. Version domains

World schema, Component ABI, WIR, Domain IR, EIR, Runtime ABI, Plugin
ABI, Network protocol, Resource schema are versioned independently.

## 2. Semantic versioning

Major: incompatible semantics/layout. Minor: backward-compatible
additions. Patch: compatible corrections.

## 3. Hashes

Canonical schema and IR serialization produce stable hashes. Artifacts
reference the hashes they were compiled against.

## 4. Compatibility matrix

A runtime must explicitly advertise accepted major/minor versions.
Unknown major versions are rejected.

## 5. Migration

Schema migration is explicit and testable. Migration must declare
source/target hashes and preserve required identity/state invariants.

## 6. Network

Peers exchange protocol version and SchemaSetHash before state
synchronization.

## 7. Cache invalidation

Changing any semantic input included in a compilation key invalidates
the corresponding executable artifact.

## 8. Acceptance

No implicit downgrade may silently change world semantics. Incompatible
artifacts fail closed.
