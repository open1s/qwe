# RFC-0048: Discrete-event + hybrid core
**Status:** Proposed (step 3 of RFC-0047). No frozen-contract change is made by
this document; it specifies the shared discrete-event / hybrid surface and marks
the first landed slice. Each slice below lands as its own change with a named
fixture + conformance case, per RFC-0047.

## Motivation

RFC-0047 sequences PWE toward a general simulation language. Steps 1–2 (typed
values RFC-0043, typed arrays RFC-0044) are Done. Step 3 is the **shared** need
of four product classes PWE cannot address today:

- **DES / agent-based** (AnyLogic, Simio, Arena): event calendar, queues,
  resources, entities, statistics.
- **Control / hybrid** (Simulink, PLECS): continuous states + discrete mode
  switches triggered by **zero crossings**, with event-driven reinitialization.
- **Power electronics / switching** (Simulink, PLECS): switch events whose
  timing must be detected, not stepped past.
- **Chemical kinetics** (COPASI): already has `gillespie` SSA, but needs the
  same event calendar/queue vocabulary for time-varying inputs.

PWE today has `emit`/`last_event` (ordered events), `at`/`periodic`/`schedule`
(a dynamic event queue on the step grid), and `watch` (a per-entity sign-change
flag that requires two **user** state slots as memory and flag). It has no
runtime-owned zero-crossing operator, no queue/resource library, and no
event-calendar surface. This RFC fixes the shape of all three and lands the
smallest one first.

## Design

### Deterministic event ordering (non-negotiable)

Events are ordered by the pair `(time, seq)`, where `seq` is a monotonically
increasing insertion counter owned by the execution context. Time ties are
therefore broken by insertion order, never by float comparison of `time`
alone. The existing queue is stable under equal times
(`ScheduledEvent` insertion via `partition_point(|e| e.time <= time)`), and the
zero-crossing / event surfaces below reuse that ordering; the calendar slice
promotes `seq` to an explicit field so cross-region and snapshot order is
explicit rather than incidental.

### Slice A — zero-crossing detection (this change)

Two runtime-owned operators detect an exact sign event on **any expression**,
using per-call-site history (the RFC-0043 `HistRead`/`HistWrite`/`HistHas`
mechanism already used by `deriv`), so no user state slot is consumed:

- `cross(e)`: fires 1.0 on the (sub)step where `e` changes sign (strictly, `prev
  ≠ 0 ∧ e ≠ 0 ∧ sign(prev) ≠ sign(e)`); 0.0 otherwise. Fire-once, transition
  edge.
- `rise(e)` / `fall(e)`: fire on a strict upward / downward transition.
- `last_cross(e)`: the simulation time of the most recent crossing of this
  call site (0.0 before any crossing).

Semantics are defined on the **substep grid**: under `rk4` the operator sees
every integration substage, so it cannot step over a crossing the way a
per-step `watch` can. The operators are pure and deterministic, so both backends
produce identical wires; `step_cross` today compares writes, `events`, `queue`,
and field overlays (it does **not** yet compare `hist`/`cross_time`, so a
history-only divergence would not be caught — see the history-site limitation
below).

**History-site limitation (known, tracked).** Each system × entity lowers to its
own EIR function, but a function's history sites are currently a bare
`HistRead` *count* into the runtime's **global** `hist`/`cross_time` maps, with
no function namespace (`lower.rs`, `eir.rs`). Two distinct concerns follow, both
filed as issues rather than solved here:

- **#94 — no function namespace:** the first `deriv`/zero-crossing operator in
  every function shares key `0`, so two *different systems* that use history can
  corrupt each other's previous value. Pre-existing on `main` for `deriv`;
  Slice A widens it to `cross_time`. (Sites are distinct *within* one function,
  and code-gen order is deterministic, so cross-backend equivalence still holds
  and `step_cross` is green — both backends make the same error.)
- **#95 — nesting order:** `lower_zero_crossing` computes its site *before*
  lowering its argument, so `cross(deriv(x))` gives the nested `deriv` the same
  site (the reverse nesting is safe).

The site scheme will be reworked (namespace the key by function id, compute the
site after the argument, and have `step_cross` compare the history maps). Until
then, keep history operators non-nested within one function and do not rely on
two systems that both use `deriv`/`cross` sharing a runtime.

### Slice B — event calendar as a first-class value

An explicit calendar value (ordered `(time, seq, kind, payload)` entries) with
deterministic `next`/`pop`/`at_time` reads and a `wait_until` surface, so
process-flow models can consume events rather than only probe `last_event`.

### Slice C — queue / resource / statistics library

`queue`, `resource`, `seize`, `release`, `wait_time`, `utilization` primitives
lowered to a domain IR over the calendar, with their own deterministic
statistics accumulation. This is the DES-domain library; it depends on Slice B.

### Slice D — hybrid coupling

Event-driven reinitialization for continuous states (`when cross(e) { … }`),
algebraic-loop detection, and the block-diagram IR that Simulink-class models
need. Depends on Slice A.

## New opcodes (Slice A)

Added via the `declare_opcodes!` table (single source), pure (no effect bits),
interpreter-oracle, JIT/AOT must remain byte-identical:

- `CrossDown = 236` — operands `[prev, cur, has]`, result F64: 1.0 on a strict
  sign change `has ≠ 0 ∧ prev ≠ 0 ∧ cur ≠ 0 ∧ sign(prev) ≠ sign(cur)`, else 0.0.
- `RiseEdge = 237` — operands `[prev, cur, has]`, F64: 1.0 on `has ≠ 0 ∧
  prev ≤ 0 ∧ cur > 0`.
- `FallEdge = 238` — operands `[prev, cur, has]`, F64: 1.0 on `has ≠ 0 ∧
  prev ≥ 0 ∧ cur < 0`.
- `LastCross = 239` — no operands, F64: the simulation time of the most recent
  crossing at this site (0.0 before any).

`has` is the history-present flag, so the first (sub)step never fires a
crossing. The site id is the count of `HistRead`s emitted so far in the
function: `deriv` uses the same counter and each of these operators emits
exactly one `HistRead` plus one `HistWrite`, so ids are unique *within a single
function* and do not collide with `deriv` *there*. The previous value reuses
`ExecEnv.hist`; the crossing timestamp uses a parallel `ExecEnv.cross_time` map.
`last_cross(e)` lowers to the same edge detection (which timestamps the site)
followed by `LastCross`, so it reports *when* the transition happened. See the
history-site limitation above (#94/#95): the bare count is not namespaced by
function, and it is computed before the argument is lowered.

## Validation

- Conformance: `"RFC-0048 zero-crossing detection (cross/rise/fall/last_cross,
  cross-backend)"` — a `<src>` entity whose signal crosses while a `<w>` entity
  detects `cross`/`rise`/`fall` and timestamps it via `last_cross`; stepped
  cross-backend (interpreter ≡ JIT) for the new opcodes.
- `lang::tests::rfc_0048_zero_crossing_operators` — `cross`/`rise`/`fall` fire on
  the transition step, `last_cross` records the time, and both backends agree.
- `lang::tests::rfc_0048_zero_crossing_is_strict_and_fires_once` — strictness
  (an exact zero touch is not a crossing) and fire-once.
- `lang::tests::rfc_0048_zero_crossing_gates_when_for_reinit` — a zero-crossing
  expression composes with `when = rise(e)` for event-driven reinitialization.
- `eir::tests::eir_zero_crossing_operators_fire_and_timestamp` — opcode-level
  edge semantics and timestamps; the codec round-trip test covers every new
  opcode (the RFC-0021 drift guard).
- `lang::tests::builtin_arity_mismatch_is_a_diagnostic_not_a_panic` — a
  wrong-arity builtin call (including the new operators) reports detail 59
  rather than panicking in lowering.

## Non-goals

- No visual block editor, no GUI.
- No new world state for Slice A: zero crossings are derived signals.
- No weakening of determinism; no float-only time ordering (ties use `seq`).

## Alternatives

- **Extend `watch`**: rejected — it consumes user state slots, sees only the
  step grid (misses substep crossings), and cannot report *when* the crossing
  happened.
- **Queue/resource first**: rejected as Slice A — it needs new runtime
  components, scene layout, and a serialization surface, so it is the largest
  and riskiest slice, while Slice A reuses the existing `deriv` history
  precedent end to end.
