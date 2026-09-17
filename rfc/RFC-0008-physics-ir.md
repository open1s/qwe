# RFC-0008: Physics IR

**Status:** Draft \| **Version:** 0.1.0

## 1. Principle

Physics IR describes computation over physical state; it does not define
storage ownership.

## 2. Pipeline

``` text
BroadPhase -> NarrowPhase -> ConstraintBuild -> Solve -> Integrate -> Commit
```

## 3. Inputs

Transform, mass, inertia, velocity, acceleration, collider, material,
constraints, environment.

## 4. Outputs

Updated transform, velocity, acceleration, contact state, constraint
state, events.

## 5. Solver abstraction

WIR/Physics IR must not require a particular solver. Backend may select
impulse, iterative, projected, constraint, particle or specialized
methods.

## 6. Determinism

Stable entity ordering, stable contact ordering, deterministic
reductions and explicit floating-point policy are required for strict
deterministic mode.

## 7. Broad phase

BVH, grid, sweep-and-prune, spatial hash are interchangeable
implementations under the same semantic contract.

## 8. GPU

Physics kernels may lower to GPU EIR when parallelism and memory traffic
justify dispatch.

## 9. Acceptance

Physics must access authoritative World storage through PhysicsView and
commit through the world transaction model.
