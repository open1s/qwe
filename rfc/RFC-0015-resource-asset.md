# RFC-0015: Resource and Asset System

**Status:** Draft \| **Version:** 0.1.0

## 1. Scope

Defines immutable or versioned resources such as Mesh, Texture,
Material, SDF, Terrain, NavigationMap, Shader, WorldSnapshot.

## 2. Identity

Every resource has ResourceId, content hash, schema version and optional
dependency list.

## 3. Content addressing

Production assets should be content-addressed so identical resources can
be deduplicated across nodes.

## 4. Residency

CPU, GPU, local cache and remote storage are explicit residency states.

## 5. Streaming

Resources may be streamed according to Interest/LOD policies. Missing
optional render resources must not invalidate authoritative physics
state.

## 6. Versioning

Resource updates create a new version unless an explicitly compatible
in-place update is allowed.

## 7. Security

Resource loading must validate type, size, dependency graph and declared
capabilities.

## 8. Acceptance

A resource reference is stable independently of the physical storage
location.
