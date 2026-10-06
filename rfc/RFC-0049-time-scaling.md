# RFC-0049: Time scaling
**Status:** Landed. Specifies the programmatic real-time factor
(`set_time_scale` / `time_scale()` / `step_dt()`), the two world
pseudo-components that carry it, the rule that *every* baked `dt` must be
scaled through one helper, and the deliberately separate viewer **step rate**.
No frozen contract is broken: at the default scale `1.0` every lowered program
is bit-identical to the pre-RFC one.

## Motivation

A simulator needs two things its users phrase as "make it faster":

1. **Faster in simulated time** — run 60 simulated seconds per wall-clock
   second without changing a single line of the model. This is a *scaling of
   the model's own clock*; the integrator must genuinely take longer steps, or
   the reported `t` and the state disagree.
2. **Faster on screen** — the live viewer should not be pinned to one step per
   rendered frame. This is a *rate*, not a physical quantity: it changes how
   many steps a frame runs, never what a step means.

Conflating the two is the classic error. Scaling only the reported clock makes
the trajectory stand still while `t` runs away (and vice versa); scaling the
physics `dt` of a viewer by a slider re-derives every coefficient in the model
and hands a degenerate step to the solver.

RFC-0049 defines (1) as a runtime execution-context setting with one
normalisation point, and pins (2) as an explicitly *different* control that
lives in the presentation layer and never reaches `ExecEnv`.

## Design

### The scale is a runtime execution-context setting, not a model property

`ExecEnv` carries `time_scale: f64`, default `1.0`. It belongs to the
execution context — the model never declares it, and it survives recompiles.

```
set_time_scale(requested) -> applied : f64
    if requested.is_finite() && requested >= 0.0:
        time_scale = min(requested, TIME_SCALE_MAX)
    return time_scale
```

* `TIME_SCALE_MAX = 1.0e10` (`eir.rs`) — exact in `f64`, far past any useful
  real-time factor, and a runaway model can never turn the effective step into
  a non-finite number.
* A **negative**, `NaN` or infinite request is **rejected**: the current scale
  is left untouched and returned, so the caller observes the refusal. This is
  `ExecEnv::set_time_scale`, the `SetTimeScale` EIR opcode **and** the host API
  — one definition, no drift.
* `0` is valid: it pauses the clock while the step still runs. A step that
  asks for `0` is a model decision, not an error.

The effective step is

```
dt_eff = sim_dt * time_scale
```

and it applies **from the next step**. A `set_time_scale` issued *inside* a
step therefore never produces a half-scaled step: step `n` runs entirely at
scale `s`, step `n+1` entirely at the new scale. `scene.sim_time` advances by
`dt_eff` per step, so the clock and the state move together.

`time_scale()` reports the scale governing the step in progress;
`step_dt()` reports `dt_eff`, the length the step in progress is actually
integrating.

### Transport: two world pseudo-components, read through `ReadView`

The scale has to reach lowered EIR — including the threaded dispatcher and the
native JIT — without turning a pure function into an opaque host call. It is
carried as two world pseudo-components, canonical ids:

| id | name | read by |
|----|------|---------|
| `pwe.time.scale` (v1) | current time scale | `time_scale()`, the scaled `dt` |
| `pwe.time.step_dt` (v1) | current effective step | `step_dt()` |

Neither belongs to an entity. `SceneRuntime::set_time_context(time_scale,
step_dt)` stamps both at step start; `SceneRuntime::new` defaults them to
`1.0` / `0.0` (unscaled, no known step), and every production step path
stamps explicitly before interpreting. Reads go through `ReadView`, so the
instructions stay byte-identical between interpreter, JIT and threaded
backends — `step_cross` compares them, and a backend that ignored the scale
would be caught immediately.

### Scaling a step means scaling *every* coefficient derived from `dt`

This is the substance of the RFC and the part that is easy to get half-right.

`physics_eir::scaled_dt_reg(base, next_id, out)` is the **single** helper: it
emits `ReadView(pwe.time.scale)` then `Mul`, returning `base * time_scale`.
Because `x * 1.0 == x` exactly in `f64`, a scale of `1.0` is bit-identical to
baking the constant — every conformance and benchmark run is unaffected.

Every place that bakes a `dt` into an immediate must route through it:

| system family | what is scaled |
|---|---|
| `update` | the `dt` local the body reads |
| `rk4` | both substep shifts (half-step for stages 0–2, full step for the combine) and the `dt/6.0` combine weight — **not** the substep count |
| `gillespie` | the reaction window |
| `wave` | the Courant number `(v·dt/dx)`, scaled **before** it is squared, so the operand `cfl²` becomes `cfl²·scale²` |
| `nbody` | the Verlet kick, the Verlet drift, and the damping **decay** (`1 - damp·dt` — scale the decay, not the `keep` factor, or `keep` inflates past 1) |
| `pair` / `drift` | the pairwise kick, the drift, and the damping decay |
| `gravity` / `force` / `linear` / `integrate` | `gravity_y·dt`, `a·dt`, both parts of the `linear` delta, and the `integrate` displacement |

Two consequences are worth stating because both were real defects:

* Scaling only the clock while leaving `rk4`'s stage shifts baked produced a
  state identical at scale 1 and scale 2 — the trajectory frozen while `t`
  advanced. The fix is `scaled_dt_reg` on the shifts and the combine weight;
  the substep count stays fixed, because `substeps` is a resolution, not a
  length.
* The damping `keep` factor must not be scaled: `keep = 1 - damp·dt·scale` is
  correct, `keep = (1 - damp·dt)·scale` is not.

### The simulation clock's `sim_dt`

`sim_dt` — the multiplier for `time_scale` and the fallback step length — is
resolved in this order:

1. the first `update` system's `dt`,
2. else the first `gillespie` system's `dt`,
3. else the first system that declares a `dt` at all (`rk4`, `nbody`, `pair`,
   `drift`, `wave`, `gravity`, `integrate`, `force`, `linear`, …),
4. else `1/60`.

Clause 3 matters: without it an `rk4`-only program fell back to `1/60` while
its state integrated at the declared `dt`, so the clock and the model disagreed
by a factor of `dt · 60`.

### Host / CLI surface

```
pwe run model.pweb --time-scale 5     # 5× simulated speed
pwe run model.pweb --time-scale 0.25  # quarter speed
```

`--time-scale` requires a finite, non-negative number, reports when it clamps
above `TIME_SCALE_MAX`, and is orthogonal to `--max-steps-per-frame`.

### The viewer speed control is a *different* thing

`pwe present` exposes a slider (log10, `1e-8 … 1e10`) and an exact-value box
near the bottom-right buttons. Both send a **steps-per-frame rate** to
`/set-time-scale` (and, in the playground, `/api/set-time-scale`), which the
stepping loop reads as "how many steps this frame".

* It never writes `ExecEnv::time_scale`. The physics `dt` is untouched, so no
  coefficient in the model is re-derived and no step becomes degenerate —
  which is precisely what keeps the RFC-0021 `EirInvalid (18)` guard from
  firing on a slider drag.
* Unrun steps are **dropped, not queued**: `speed_frame` carries only the
  fraction, so a burst at `1e10` cannot leave a backlog that outlives the
  request. The burst is bounded by a per-frame wall-clock budget
  (`STEP_BUDGET_MS` / `PLAYGROUND_STEP_BUDGET_MS`), not by a numeric cap.
* `/state` echoes the applied value as `time_scale` and the simulated seconds
  actually advanced this frame as `effective_dt`, both formatted with
  `fmt_f64` so the JSON stays parseable.
* The request is validated once, in `apply_time_scale_query`, shared by the
  `pwe present` route, the playground handler and the token-carrying viewer
  rewrite.
* The browser keeps a `pendingScale` until an echo matches it (with a
  deadline), so a poll that left before the request landed cannot snap the
  control back to the previous value; a cancelled pointer drag clears
  `scaleDragging`, or the readout would stop following the runtime forever.

## Validation

* Conformance: `RFC-0049 time scale (applies from next step, clock advances
  1+2)` — after two steps with scales `1.0 → 2.0` the clock advanced
  `1 + 2 = 3` seconds.
* `reference/src/lang/tests.rs`:
  * `rfc_0049_time_scale_scales_the_clock_and_the_step_dt`
  * `rfc_0049_time_scale_change_applies_from_the_next_step`
  * `rfc_0049_invalid_time_scales_are_rejected_and_zero_pauses`
  * `rfc_0049_body_dt_is_the_effective_scaled_step`
  * `rfc_0049_host_api_shares_the_builtin_definition`
  * `rfc_0049_time_scale_moves_the_rk4_integrator` — harmonic oscillator
    `(cos t, -sin t)`; ten steps land on `t = 1` unscaled and `t = 2` at scale
    2, and the clock follows the declared `dt`.
  * `rfc_0049_time_scale_moves_the_gravity_and_integrate_systems` — free fall
    `-5.5` unscaled vs `-22.0` at scale 2, exactly the `t → 2t` signature.
  * `rfc_0049_time_scale_moves_the_nbody_integrator`
  * `rfc_0049_time_scale_moves_the_wave_courant_coefficient`
* `reference/src/present.rs`: `set_time_scale_query_validates_and_clamps`,
  `live_state_json_stays_parseable_for_extreme_values`,
  `speed_frame_is_a_rate_not_a_backlog`.

All of the above are stepped cross-backend, so interpreter ≡ JIT ≡ threaded
dispatcher must agree on every scaled read.

## Non-goals

* No change to what `dt` *means* for a system. The declared `dt` stays a
  physical model constant; the scale is a runtime multiplier of it.
* No wall-clock time dilation of the render loop, and no sleeping to hit a
  target rate. The viewer's rate is a step *count*, not a duration.
* No per-entity or per-system scale. The execution context has one.
* No weakening of the RFC-0021 `EirInvalid (18)` guard. `dt_eff` is finite and
  strictly positive for any accepted request; `time_scale = 0` is an explicit
  pause, not a divisor.

## Alternatives

* **Scale only `scene.sim_time`.** Rejected — the clock runs while the state
  stands still, which is worse than no feature.
* **Rescale `dt` in the host before lowering.** Rejected — the baked constant
  would differ between backends and between runs of the same source, breaking
  deterministic replay and `step_cross` byte-comparison.
* **A single `time_scale()` multiply inside each system body.** Rejected —
  every system would need its own copy of the read, and a system that forgot
  would diverge silently. One helper, called at each bake site, keeps the
  mistake loud in review.
* **Let the viewer slider write `ExecEnv::time_scale`.** Rejected — it would
  re-derive every model coefficient per drag and hand the solver a near-zero
  or enormous step; the step-rate control exists specifically to avoid this.
