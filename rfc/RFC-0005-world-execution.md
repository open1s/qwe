# RFC-0005: World Execution and Consistency

**Status:** Draft \| **Version:** 0.1.0

## 1. Execution model

Each simulation step follows:

``` text
Observe -> Compute -> Prepare -> Commit -> Publish
```

Systems compute against a declared snapshot/view. Mutations become
authoritative only during Commit.

## 2. System phases

Input, PrePhysics, Physics, PostPhysics, AI, Actuation, WorldCommit,
RenderPrepare, Sensor, NetworkPublish.

A system must declare read/write sets. The scheduler builds a dependency
DAG.

## 3. Consistency

Within one authoritative region, committed state is sequentially
consistent at commit boundaries. Parallel computation may use snapshots.

## 4. Transactions

A transaction contains: - base world version - reads - writes -
creates/destroys - events - capability context

Commit succeeds only if the declared conflict policy is satisfied.

## 5. Conflict policies

Reject, Merge, LastWriterByPriority, CommutativeMerge. Silent write
races are forbidden.

## 6. State version

Every commit increments a monotonic WorldVersion.

## 7. Snapshot

A snapshot identifies WorldVersion + TimePoint + SchemaSetHash.

## 8. Acceptance

Replay of the same deterministic inputs and schema set must reproduce
the same committed state hash.
