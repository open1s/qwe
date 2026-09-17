# RFC-0011: Runtime ABI

**Status:** Draft \| **Version:** 0.1.0

## 1. Purpose

Defines the ABI exposed to compiled EIR code.

## 2. Runtime context

A compiled function receives a stable runtime context containing time,
world handles, memory arenas, scheduler services, capabilities and
device handles.

## 3. Rule

Compiled code must not depend on private Rust object addresses,
allocator internals or trait-object layouts.

## 4. Calls

Runtime calls are versioned C-compatible function tables. Calls declare
side effects and capability requirements.

## 5. Handles

WorldHandle, ComponentHandle, ResourceHandle, DeviceHandle. Handles are
generation-checked.

## 6. Errors

Runtime errors use explicit status/result codes at the ABI boundary.
Panic/unwind across the ABI is forbidden.

## 7. ABI version

Major version may break ABI. Minor versions add compatible entries
through a size/versioned table.

## 8. Acceptance

A compiled module can execute against any runtime implementing its
declared ABI version and capabilities.
