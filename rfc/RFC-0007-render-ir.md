# RFC-0007: Render IR

**Status:** Draft \| **Version:** 0.1.0

## 1. Principle

Rendering is a first-class projection of World State. Renderer never
owns authoritative world state.

## 2. Pipeline

``` text
WorldView
 -> Visibility
 -> Culling
 -> LOD
 -> RenderGraph
 -> RenderIR
 -> GPU commands/shaders
 -> Present
```

## 3. RenderGraph

A graph consists of resources, passes, dependencies, synchronization and
device constraints.

Typical passes: Depth -\> Shadow -\> GBuffer -\> Lighting -\> Reflection
-\> PostProcess -\> Present.

## 4. Render entities

Renderable, MaterialBinding, Camera, Light, Skin, Instance, LOD.

## 5. Geometry

The same world entity may expose RenderMesh, CollisionSDF,
SensorPointCloud, NavigationGeometry.

## 6. Frame consistency

A frame references a specific WorldVersion and RenderTime. Renderer may
interpolate between simulation states but must identify the source
versions.

## 7. GPU

RenderIR must lower to backend-specific APIs such as
Metal/Vulkan/DX12/WebGPU without leaking those APIs into WIR.

## 8. Acceptance

Physics and rendering of the same entity must resolve from the same
World State rather than synchronized duplicate objects.
