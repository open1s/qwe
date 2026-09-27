# Determinism

Determinism in PWE is not a slogan; it is a **compile-and-run gate**.

## The rule

```
Interpreter ≡ semantic baseline
JIT/AOT     ≡ optimized implementations
```

The **EIR interpreter is the semantic reference**. Compiler optimization MUST
NOT change semantics. Production execution must preserve

```
WIR → Domain IR → EIR → execution
```

— no undocumented bypasses.

## What is enforced

### Interpreter ≡ JIT, every step

`step_cross` runs both backends on identical state and requires
**byte-identical writes** *and* an **identical event / queue stream** on *every*
step — not a summary comparison at the end.

```rust
let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
rt.step_cross()?;     // interpreter == JIT, asserted
rt.step_cross_n(30)?;
```

### Analytic law conformance

Every simulation is asserted against its closed form: Newton cooling,
radioactive decay, logistic growth, Kepler orbits, harmonic energy,
action–reaction, reversible kinetics, wall reflection, and more.

```sh
cargo run -p pwe-conformance     # RFC-0029 report runner
```

### Snapshot / replay

State hashes reproduce **bit-for-bit** across runs and snapshots. A successful
commit increments `WorldVersion`.

### The frozen contract

The RFC set in [`rfc/`](https://github.com/open1s/qwe/tree/main/rfc) defines
canonical bytes, schemas, and protocols; conformance runs with **zero skips**.

```sh
cargo test --workspace
```

## Why it holds structurally

* Authoritative mutation goes through a `WorldTransaction` only — never
  opportunistic writes during Compute.
* Time is explicit and domain-based; the step grid is the clock events land on.
* Structural changes are deferred to commit.
* Randomness (`random()`, `noise()`, `emit()`) is seeded and replay-stable.
* Grid fields are ordinary deterministic world state — snapshot and replay
  included.
* Presentation fields are excluded from the state hash by design.
* A mutable entity has exactly one authoritative owner; epochs reject stale
  commands, so split-brain authority is never silently allowed.

## The frame pipeline

```
Observe → Compute → Prepare → Commit → Publish
```

## Turning it off

You cannot. Determinism is a first-class execution mode and is always
available; the differential gate runs in tests and cross-backend execution
paths. A runtime that cannot meet a MUST requirement fails **closed**: a
defined error and no authoritative mutation.

See also: [Architecture](architecture.md) ·
[The frozen v0.2 contract](../../rfc/README-v0.2.md) ·
[RFC-0013: Time & Determinism](../../rfc/RFC-0013-time-determinism.md)
