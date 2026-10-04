# RFC-0048: Discrete-event + hybrid core
**Status:** Proposed (step 3 of RFC-0047). No frozen-contract change is made by
this document; it specifies the shared discrete-event / hybrid surface and marks
the landed slices. Each slice below lands as its own change with a named
fixture + conformance case, per RFC-0047. Slices A and B are landed, along with
the C1 priority-queueing + statistics and C2 resource increments (recorded in
their sections); Slice D is Proposed.

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
alone. The queue previously relied on a *stable* insert under equal times
(`partition_point(|e| e.time <= time)`); the calendar slice (B) promotes `seq`
to an explicit field on each entry and inserts at
`partition_point(|e| (e.time, e.seq) <= (time, seq))`, so cross-region and
snapshot order is explicit rather than incidental. `step_cross` compares
`next_seq` too, so a backend that assigned sequence numbers differently is
caught even before the queue diverges.

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
produce identical wires; `step_cross` compares writes, `events`, `queue`, field
overlays, **and** the `hist`/`cross_time` maps, so a history-only divergence is
caught too.

**History-site scheme.** A function's history sites are a per-function
`HistRead` *count*, keyed into the runtime's `hist`/`cross_time` maps. Two
correctness fixes landed with this slice (both were filed while reviewing the
WIP):

- **#94 — function namespace (fixed):** the builder (`PhysicsProgram::
  build_with_guards`) rewrites each function's site constants to
  `(function_id << 32) | site`, so two *different systems* that use
  `deriv`/`cross` can no longer alias each other's previous value. Regression:
  `lang::tests::history_sites_are_namespaced_per_function`.
- **#95 — nesting order (fixed):** `lower_zero_crossing` now lowers its
  argument *before* taking its site, matching `deriv`, so
  `cross(deriv(x))` no longer shares the nested operator's site. Regression:
  `lang::tests::nested_history_operators_do_not_share_a_site`.

Sites remain distinct within a function and code-gen order is deterministic, so
cross-backend equivalence holds; `step_cross` additionally compares the
`hist`/`cross_time` maps (and, with Slice B, `next_seq`).

### Slice B — event calendar as a first-class value

An explicit calendar value (ordered `(time, seq, kind, payload)` entries) with
deterministic `next`/`pop`/`at_time` reads, so process-flow models can consume
events rather than only probe `last_event`.

**Landed (this slice):** `ScheduledEvent` and `EmittedEvent` carry an explicit
monotonic `seq` (assigned by `ExecEnv.next_seq`), so ordering is `(time, seq)`
everywhere — a tie in `time` is broken by insertion order, never by a float
comparison, and the order is stable across regions and snapshots. The language
surface is a read/pop view over the pending calendar, lowered to interpreter-
oracle opcodes:

| Builtin | Opcode | Meaning |
| --- | --- | --- |
| `event_count()` | `EventCount` | pending entries |
| `next_event_time()` | `NextEventTime` | earliest time, `f64::MAX` when empty |
| `next_event_kind()` | `NextEventKind` | earliest kind, `0` when empty |
| `next_event_payload()` | `NextEventPayload` | earliest payload, `0` when empty |
| `pop_event()` | `PopEvent` | remove + return the earliest payload |
| `events_seen(k)` | `EventSeenCount` | events delivered this step with kind `k` |

`pop_event` mutates the pending calendar; the rest are pure reads. All are
deterministic and cross-backend checked. (Time-advance — an explicit
`wait_until`/advance-to-next-event host contract — remains future work; this
slice deliberately adds only reads/pops so the host step grid is unchanged.)

### Slice C — queue / resource / statistics library

`queue`, `resource`, `seize`, `release`, `wait_time`, `utilization` primitives
lowered to a domain IR over the calendar, with their own deterministic
statistics accumulation. This is the DES-domain library; it depends on Slice B.

**Landed (C1 — priority queueing).** The calendar gained a queue discipline:
`ScheduledEvent` carries a `priority`, the queue is ordered by
`(time, priority, seq)` (earlier time, then **lower** priority, then insertion),
and the new `schedule_at(gate, delay, kind, payload, priority)` sets it while
`next_event_priority()` reads the head's priority. The plain `schedule(...)`
keeps priority 0, so its order is exactly Slice B's `(time, seq)` — this is a
strict generalisation. FIFO (priority 0), LIFO/EDF (priority −t) and
priority/SJF (priority = job length) are all expressible.

| Builtin | Opcode | Meaning |
| --- | --- | --- |
| `schedule_at(g, d, k, p, prio)` | `ScheduleEventAt = 246` | schedule with priority `prio` |
| `next_event_priority()` | `NextEventPriority = 247` | head priority, `0` when empty |

**Landed (C1 — statistics library).** `std/des.pwe` provides the deterministic
DES statistics a process-flow model needs — `utilization`, `availability`,
`throughput`, `wait_time`, `queue_length`, `mean`, `variance`, `ewma`,
`littles_law` — all as pure `funcs` over caller-held state (no hidden state, no
RNG), and all trap-free (`safe_ratio` clamps its denominator, since `if` is
eager). It is self-contained so a `funcs` module can use it.

**Landed (C2 — resources).** A `resource r { capacity = n }` world declaration
plus four builtins give capacity-gated `seize`/`release`, the DES "server"
primitive the statistics library consumes:

| Builtin | Opcode | Meaning |
| --- | --- | --- |
| `seize(r, cap)` | `SeizeResource = 248` | non-blocking grab; 1.0 on success, 0.0 when full |
| `release(r)` | `ReleaseResource = 249` | free one unit; returns the remaining busy count |
| `resource_busy(r)` | `ResourceBusy = 250` | current holders (`0` when unknown) |
| `resource_capacity(r)` | `ResourceCapacity = 251` | declared capacity (`0` when unknown) |

Resources live in the **execution context** (`ExecEnv.resources`, keyed by a
deterministic FNV-1a of the resource name), exactly like the event calendar:
`SeizeResource`/`ReleaseResource` mutate it, the reads are pure, `step_cross`
compares the whole table, and the JIT/AOT never compile these opcodes — so the
interpreter ≡ JIT guarantee holds. The **first** `seize` fixes the capacity
(the declaration value); a later call cannot widen it, which keeps a resource's
semantics independent of the order in which rules happen to run. Busy is
saturating at 0 on release. Because a resource is execution-context state, it is
deliberately *not* mirrored into a State component; the `capacity` in the
declaration is the only user-visible constant, and it is fixed at compile time.

### Slice D — hybrid coupling

Event-driven reinitialization for continuous states (`when cross(e) { … }`),
algebraic-loop detection, and the block-diagram IR that Simulink-class models
need. Depends on Slice A.

## New opcodes (Slice C2)

- `SeizeResource = 248` — operand 0 = capacity, resource id in `constant`,
  F64: 1.0 when acquired, 0.0 when full.
- `ReleaseResource = 249` — no operands, resource id in `constant`, F64:
  remaining busy count after freeing one unit (saturating at 0).
- `ResourceBusy = 250` — no operands, F64: current holders.
- `ResourceCapacity = 251` — no operands, F64: declared capacity.

All are interpreter-oracle; the codec round-trip test covers them.

## New opcodes (Slice C1)

- `ScheduleEventAt = 246` — operands `[gate, delay, kind, payload, priority]`,
  no result: enqueue with an explicit priority.
- `NextEventPriority = 247` — no operands, F64: the head's priority, `0` empty.

Both are interpreter-oracle; the codec round-trip test covers them.

## New opcodes (Slice B)

Added via the `declare_opcodes!` table (single source), pure, interpreter-oracle,
JIT/AOT must remain byte-identical. `PopEvent` is the only stateful one (it
removes the earliest pending entry); the rest read:

- `EventCount = 240` — no operands, F64: number of pending entries.
- `NextEventTime = 241` — no operands, F64: earliest time, `f64::MAX` when empty.
- `NextEventKind = 242` — no operands, F64: earliest kind, `0` when empty.
- `NextEventPayload = 243` — no operands, F64: earliest payload (bit pattern).
- `PopEvent = 244` — no operands, F64: remove + return the earliest payload.
- `EventSeenCount = 245` — operand 0 = kind, F64: delivered-this-step count.

The empty-calendar sentinels are finite (`f64::MAX` for time, `0` for kind and
payload) so a stored `next_event_time()` cannot trip the detail-88 non-finite
check.

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
history-site scheme above: the function id is folded into the key by the
builder, and the site is taken after the argument is lowered.

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
- `lang::tests::history_sites_are_namespaced_per_function` (#94) and
  `lang::tests::nested_history_operators_do_not_share_a_site` (#95).
- `lang::tests::builtin_arity_mismatch_is_a_diagnostic_not_a_panic` — a
  wrong-arity builtin call (including the new operators) reports detail 59
  rather than panicking in lowering.
- Slice B: `"RFC-0048 event calendar (event_count/next_event_*/pop_event/
  events_seen, cross-backend)"` — a producer schedules two future events and a
  consumer reads/pops the pending calendar; stepped cross-backend.
- `lang::tests::rfc_0048_calendar_reads_and_pop_the_pending_queue`,
  `lang::tests::rfc_0048_calendar_equal_time_ties_use_insertion_order`,
  `lang::tests::rfc_0048_events_seen_counts_delivered_events_by_kind`,
  `lang::tests::rfc_0048_empty_calendar_reads_are_finite_sentinels`;
  `eir::tests::eir_calendar_reads_and_pop_in_time_seq_order`,
  `eir::tests::eir_calendar_equal_time_ties_use_insertion_order`.
- Slice C1: `"RFC-0048 priority calendar (schedule_at / next_event_priority,
  cross-backend)"` and `"RFC-0048 DES statistics library (std/des utilization,
  cross-backend)"`;
  `lang::tests::rfc_0048_priority_calendar_orders_by_time_priority_seq`,
  `lang::tests::rfc_0048_priority_ties_use_insertion_order`,
  `eir::tests::eir_calendar_priority_orders_within_a_time`.
- Slice C2: `"RFC-0048 resources (seize/release/resource_busy, cross-backend)"`;
  `lang::tests::rfc_0048_resource_seize_release_respects_capacity`,
  `lang::tests::rfc_0048_resource_capacity_is_fixed_and_release_saturates`,
  `eir::tests::eir_resource_seize_release_respects_capacity`.

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
