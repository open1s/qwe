# RFC-0013: Time and Determinism

**Status:** Draft \| **Version:** 0.1.0

## 1. Time

Kernel time supports Real, Simulation, Replay and Prediction domains.

``` rust
pub struct SimTime {
    pub tick: u64,
    pub nanos: i64,
}
```

## 2. Clock rules

World simulation must not read wall-clock time implicitly. Wall-clock
access is an explicit external input.

## 3. Fixed-step

Deterministic physics uses a declared fixed timestep. Variable-step
execution is legal only where semantics permit it.

## 4. Determinism levels

BestEffort, Reproducible, StrictDeterministic.

## 5. Strict mode

Requires stable iteration order, deterministic scheduling/reduction,
explicit random seeds, controlled floating-point behavior and
deterministic message ordering.

## 6. State hash

A canonical state hash is computed from schema, entity IDs, selected
components and canonical numeric representation.

## 7. Replay

Replay consists of initial snapshot + schema set + external input/event
stream + configuration.

## 8. Acceptance

Strict replay of the same artifact and inputs must reproduce the same
state hash at every declared checkpoint.
