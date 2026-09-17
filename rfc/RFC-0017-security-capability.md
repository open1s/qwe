# RFC-0017: Security and Capability Model

**Status:** Draft \| **Version:** 0.1.0

## 1. Principle

Authority is explicit. Possession of a pointer or handle does not imply
permission.

## 2. Capabilities

READ_WORLD, WRITE_COMPONENT, CREATE_ENTITY, DESTROY_ENTITY,
EXECUTE_SYSTEM, GPU_ACCESS, NETWORK_SEND, NETWORK_RECEIVE,
RESOURCE_READ, RESOURCE_WRITE, JIT_INSTALL.

## 3. Scope

Capabilities are scoped to World, Region, Entity set, Component type and
lifetime.

## 4. Validation

Every privileged operation checks capability before effect.

## 5. Plugin isolation

Untrusted plugins should run out-of-process or in a sandboxed execution
environment. In-process plugins are trusted code.

## 6. Generated code

JIT/AOT artifacts carry capability manifests. The runtime refuses
installation if required capabilities exceed policy.

## 7. Audit

Security-sensitive mutations produce audit events containing
actor/module identity, capability, WorldVersion and operation.

## 8. Acceptance

No component mutation, ownership transfer, resource write or network
side effect can occur without an explicit authority path.
