# 0015 — `pwe run` reports the wrong step when a run fails (Low)

## Summary

When a run fails (invariant violation, trap, etc.), the CLI prints

```
pwe: step {done} failed: {e}
```

where `done` is the number of steps completed **before the current batch**,
not the step that actually failed. With `CROSS_BATCH = 16` the reported step
can be up to 15 earlier than the true failing step; for small `--steps` the
failure is always reported as `step 0`.

## Repro

```pwe
world { gravity = (0, 0, 0)
  entity m { state = (x = 0.0) }
}
systems {
  invariant { on = m; expr = x < 1000000.0 }
  update { on = m; dt = 1.0  x = 1.0e9 }
}
```

The invariant reads its input state at its program-order position, so step 0
passes (`x = 0` at check time) and **step 1** fails after the update wrote
`1e9`. Yet:

```
$ pwe run inv4_invariant_first_then_violate.pweb --steps 3
pwe: step 0 failed: PWE EirInvalid (69)
```

(Identical output for the batch1 `p5` probe; probe files in
`_probe_out/batch2/`.)

## Evidence

- `cli/src/main.rs:399-408` — `done` is incremented *after* the whole batch
  returns:

  ```rust
  while remaining > 0 {
      let k = remaining.min(CROSS_BATCH as u64) as u32;
      if let Err(e) = rt.step_cross_batched(k) {
          eprintln!("pwe: step {done} failed: {e}");   // ← batch start, not failing step
          return 1;
      }
      remaining -= k as u64;
      done += k as u64;
  }
  ```

- `reference/src/lang/runtime.rs:437-444` — `step_cross_batched` does not
  report which sub-step failed.

## Impact

Debugging aid is actively misleading: a user told "step 0 failed" inspects
initial state while the bug is at step 1 (or up to 15 with the default
batch). Also complicates triage of CI failures.

## Suggested fix

Have the stepping API return the failing step index (e.g. wrap
`step_interpreter`/`step_cross` inside `step_cross_batched` to report
`completed + i`), and print that.
