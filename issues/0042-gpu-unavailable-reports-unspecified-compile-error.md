# 0042 — `pwe run --gpu` on a build without the backend reports "unspecified compile error" (Low)

**Reported:** pass 16 (commit `55457ff1`)
**Severity:** Low
**Component:** `cli/src/main.rs` (`--gpu` handling), `reference/src/lang/runtime.rs` (`enable_gpu` stub)
**Status:** open

## Summary

`pwe run <model> --gpu` on a build that lacks the `gpu` feature (or on a
non-macOS host) exits 1 with:

```
pwe: --gpu unavailable: error 5: unspecified compile error
```

The commit message promises "clear error when the build lacks the feature",
but the message is a generic detail-5 fallback from `lang::diagnose`
(`diagnostics.rs` `_ => "unspecified compile error"`) — nothing about GPUs,
features, or platforms. The stub `enable_gpu` (runtime.rs) returns bare
`Invalid d5` for both "feature not built" and "not macOS", and
`Gpu::new()` uses the same `d5` for "no Metal device / kernel failed to
compile", so three distinct situations collapse into one misleading string.

## Reproduction

```
cargo build --release -p pwe-cli            # no --features gpu
./target/release/pwe run model.pweb --gpu   # rc=1
# pwe: --gpu unavailable: error 5: unspecified compile error
```

## Fix directions

- Give each failure a distinct diagnostic detail (or a dedicated message
  string) covering at least: feature not built ("rebuild with
  `--features gpu`"), non-macOS host, no Metal device, kernel compile
  failure.
- Or handle it in the CLI: `cfg(feature = "gpu")` is known to `pwe-cli`
  (it forwards `pwe-reference/gpu`), so the non-feature build can print
  its own message before calling `enable_gpu`, and the macos-target case
  separately ("Metal backend requires macOS").
- Keep the banner line (`GPU backend enabled (Metal field sweeps, f32
  approximate)`) — that part reads well.
