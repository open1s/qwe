# RFC-0009: Physical Compiler

**Status:** Draft \| **Version:** 0.1.0

## 1. Compilation pipeline

``` text
PWL/WIR
 -> Validate
 -> Normalize
 -> Lower Domain IR
 -> Optimize
 -> Partition
 -> Schedule
 -> Specialize
 -> EIR
 -> Target Code
```

## 2. Compiler passes

Schema validation, constant propagation, dead-code elimination,
common-subexpression elimination, inlining, loop optimization,
vectorization, data-layout transformation, memory planning, device
partitioning.

## 3. Partitioning

Compiler chooses CPU/GPU/NPU according to latency, arithmetic intensity,
parallelism, memory traffic and device capability.

## 4. Scheduling

The compiler and runtime share dependency semantics. Compiler may
precompute static DAGs; runtime resolves dynamic dependencies.

## 5. Profile-guided optimization

Profiles may include invocation count, execution time, branch rate,
memory traffic, cache miss, entity count and device utilization.

## 6. Compilation identity

A compilation artifact is identified by source/schema hash, compiler
version, target, feature set and relevant profile hash.

## 7. Acceptance

Compiler output must pass EIR validation before execution or
installation.
