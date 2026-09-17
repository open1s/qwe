# RFC-0001: PWE Architecture

**Status:** Draft \| **Version:** 0.1.0

## 1. Scope

定义 Physical World Engine (PWE) 与 Physical World Runtime (PWR)
的总体架构、层次、依赖关系和不可违反的不变量。

## 2. Architecture

``` text
Application
  -> World Model
  -> World IR (WIR)
  -> Domain IR
  -> Execution IR (EIR)
  -> AOT/JIT
  -> Physical World Runtime
  -> CPU/GPU/NPU/Edge/Cloud
```

World 是唯一逻辑真相源；Physics、Render、AI、Sensor 都通过 View 访问
World State。

## 3. Core layers

-   Kernel: memory, scheduler, time, capability, task, synchronization.
-   World: entity/component/state/spatial/resource.
-   IR: WIR, Physics IR, Render IR, EIR.
-   Compiler: validation, lowering, optimization, partitioning, code
    generation.
-   Runtime: execution, memory, devices, distribution.
-   Domains: physics, rendering, sensors, AI.

## 4. Invariants

1.  World is the single logical source of truth.
2.  World Model and World State are distinct.
3.  Entity contains identity, not business behavior.
4.  Component is data; System is behavior.
5.  WIR is runtime-independent.
6.  EIR is hardware-independent.
7.  Domain subsystems do not own independent worlds.
8.  AOT and JIT share EIR semantics.
9.  Cross-module access uses ABI/View/Capability.
10. Production execution must be represented by validated IR.

## 5. Non-goals

PWE is not initially required to implement every physics model, every
GPU API, neural rendering, VR, or city-scale simulation.

## 6. Dependency rule

Dependencies point downward only, except explicit interfaces:
`Application -> World -> IR -> Compiler -> Runtime -> Platform`.

## 7. Acceptance

A conforming implementation must be able to compile one WIR world into
executable CPU code while rendering the same World State without
duplicating authoritative state.
