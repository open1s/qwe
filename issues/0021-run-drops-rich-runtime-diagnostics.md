# 0021 — `pwe run` prints only `PWE EirInvalid (NN) at 0`, dropping the diagnostic pushed for details 88 and 69 (Medium)

## Summary

The runtime pushes rich, targeted diagnostics for the physical-sanity
checks, but the run loop prints only the error's `Display`, which is just
status + detail + byte offset:

```rust
// reference/src/lang/runtime.rs:328-345 — pushed:
error_at(Status::EirInvalid, 88, 0,
         format!("entity `{name}` slot {i} is non-finite ({v})"))

// cli/src/main.rs:476 / :530 — printed:
eprintln!("pwe: step {} failed: {e}", done + j as u64);
```

The thread-local diagnostic queue is drained only on the **compile** path
(`cli/src/main.rs:345`); a runtime failure never touches it. Commit
`fcd7bc8e` ("failing with detail 88 **naming the entity/slot**") therefore
has no user-visible effect: the name is computed, queued, and lost.

Same story for invariants (detail 69):

```rust
// reference/src/lang/runtime.rs:425-433 — pushed:
error_at(Status::EirInvalid, 69, 0,
         format!("invariant '{desc}' violated for entity {}", w.entity))
```

## Repro

```pwe
world { gravity=(0,0,0) entity e { state=(x=2.0) } }
systems { update { on=e; dt=1.0  x = x*x } }
```

```
$ pwe run x88.pweb --steps 20 --check
pwe: step 9 failed: PWE EirInvalid (88) at 0
```

Expected: `entity `e` slot 0 is non-finite (inf)` (the pushed message).
`at 0` is actively misleading — the failure is at step 9, and 0 is not a
byte offset in any file.

```pwe
world { gravity=(0,0,0) entity e { state=(x=0.0) } }
systems { update { on=e; dt=1.0  x = x + 1.0 }  invariant { on=e; expr = x < 3.5 } }
```

```
$ pwe run inv1.pweb --steps 4
pwe: step 3 failed: PWE EirInvalid (69) at 0
```

Expected: `invariant 'x < 3.5' violated for entity 1`.

Side observation: detail 88 only runs under `--check`
(`cli/src/main.rs:457-459  if check { rt.set_finite_check(true); }`).
Without the flag the same program runs to completion and cheerfully prints
`state = [inf]` — arguably intentional (strict mode), but worth one line in
the docs either way.

## Evidence

- `reference/src/lang/runtime.rs:339-344` (88 message),
  `:425-433` (69 message) — both via `error_at`, which `push_diag`es.
- `cli/src/main.rs:476` and `:530` — step-failure prints `{e}` only.
- `cli/src/main.rs:345` — `take_diagnostics()` used solely after a
  successful compile (warnings), proving the queue mechanism exists.
- `cli/src/main.rs:457-459` — `set_finite_check(true)` gated on `--check`.
- Commit `fcd7bc8e`: "failing with detail 88 naming the entity/slot …
  Regression test + docs (detail 88)."

## Impact

The two checks most likely to fire on a runaway model are the ones whose
explanation the user never sees; they must guess between NaN, division by
zero, or an invariant — from a bare `(88) at 0`. Details 86/87/88 were
just locked into CI (`2d11c4af`), so this message is now the primary
humans-facing output of the new sanity suite, and it is empty of content.

## Suggested fix

On step failure, drain the queue before exiting, e.g.:

```rust
if let Err(e) = one {
    for d in lang::take_diagnostics() {
        eprintln!("pwe: step {} failed [{}]: {}", done + j as u64, d.detail, d.message);
    }
    eprintln!("pwe: step {} failed: {e}", done + j as u64);
    return 1;
}
```

(or fold the first pushed diagnostic into the `{e}` rendering). Apply to
both print sites (`:476`, `:530`) and add a test asserting the entity/slot
text appears in `pwe run --check` output.
