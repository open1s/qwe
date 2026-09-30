# Contributing to PWE

Thanks for helping. PWE is a reference implementation of a microkernel runtime
and language, so **correctness and documented contracts come first**.

## Before you start

Read, in order:

1. `AGENTS.md` — binding engineering rules (boundaries, IR rules, ABI, unsafe,
   dependencies, change policy).
2. `docs/architecture.md` — the system map.
3. The RFC for the area you touch (`rfc/`); `docs/rfc-alignment.md` maps RFCs to
   implementation/test status.

If implementation and an RFC disagree, the order is **RFC → conformance tests →
implementation**. If the RFC is wrong, update it deliberately.

## Toolchain

- Rust (see `rust-toolchain.toml`). MSRV is declared per crate (`rust-version`).
- Version control is **jj** (Jujutsu) on top of git; commit with a focused
  pathspec, e.g. `jj commit -m "…" reference/src`.
- **After a `jj` operation, force a rebuild.** A `jj` checkout can leave source
  files with an mtime older than a previously built binary, so Cargo's
  incremental check reports `Finished` and runs a **stale** binary (symptoms:
  a just-added feature "doesn't exist"/"unknown identifier"). After switching
  revisions, run `cargo clean -p pwe-cli && cargo build -p pwe-cli` (or
  `touch reference/src/lang/*.rs`) before trusting a run.

## Build & test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -q -p pwe-conformance          # RFC conformance gate
cargo run -q -p pwe-cli -- doctest       # compile runnable code blocks in docs
# Fuzzing (in-tree, deterministic): cargo test -p pwe-reference --test fuzz
# Deeper UB checks (needs nightly + miri): cargo +nightly miri test -p pwe-reference --test fuzz
# Editor integration: `pwe lsp` (stdio LSP: diagnostics + formatting)
cargo build --examples -p pwe-reference
cargo bench -p pwe-reference             # throughput record
cargo deny check                         # supply chain (licenses/advisories)
```

All of the above must pass before a change lands. CI runs them on Linux.

### Optional features

- **GPU offload** (macOS only): `cargo build -p pwe-cli --features gpu` then
  `pwe run prog.pweb --gpu`. Experimental (f32; currently slower than the CPU) —
  see `docs/adr/0005-*`.
- **GPU execution verification** (host-only, not in CI):
  `cargo run --release --manifest-path gpu-verify/Cargo.toml`.

## Rules that bite

- **No `unwrap()` outside tests** (clippy denies it). Use `Result`, or
  `.expect("proven: …")` for locally-proven invariants.
- **No `panic` in runtime paths**; return defined errors.
- **Differential backends.** Any new execution backend (JIT/AOT/GPU/threaded)
  must produce byte-identical results to the interpreter, or be explicitly
  documented as an approximate opt-in (with a differential or oracle test).
- **ABI/schema/protocol changes** require checking the corresponding RFC first
  (`AGENTS.md` §16).
- **Dependencies** are reviewed for license, maintenance, size, and compile cost
  (`AGENTS.md` §12); prefer small, mature crates, and gate heavy ones behind
  optional, target- or feature-specific deps.

## Adding a language feature (checklist)

1. Grammar (`reference/src/lang.pest`), AST (`lang/ast.rs`), parser/lowering.
2. Diagnostics: a new `detail` code + message in `lang/diagnostics.rs` if needed.
3. Tests: unit + a conformance case if RFC-relevant.
4. Docs: `docs/lang-usage.md` (and `.zh.md`) + the applicable RFC.
5. Gates (above).

## Architecture decisions

Record non-trivial decisions as ADRs in `docs/adr/` (one file per decision:
Context / Decision / Consequences / Alternatives). See `docs/adr/README.md`.
