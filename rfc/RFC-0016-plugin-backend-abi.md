# RFC-0016: Plugin and Backend ABI

**Status:** Draft \| **Version:** 0.1.0

## 1. Principle

Extensions are loaded through a stable C ABI and PWE schema.

## 2. Plugin types

Component, System, CompilerPass, PhysicsBackend, RenderBackend,
SensorBackend, DeviceBackend.

## 3. Plugin API

``` rust
#[repr(C)]
pub struct PwePluginApi {
    pub version: u32,
    pub register_component: unsafe extern "C" fn(*const ComponentDescriptor),
    pub register_system: unsafe extern "C" fn(*const SystemDescriptor),
}
```

## 4. Isolation

Plugins do not receive unrestricted World pointers. They receive
capabilities, views and handles.

## 5. Backend contract

A backend declares supported IR features, data layouts, devices,
determinism level and required capabilities.

## 6. Failure

Backend failure must be surfaced as an explicit runtime error. A backend
may be quarantined without corrupting World State.

## 7. Compatibility

Plugin ABI uses versioned function tables with size fields for
forward-compatible extension.

## 8. Acceptance

A backend can be replaced without changing WIR semantics.
