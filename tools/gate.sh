#!/usr/bin/env bash
# The pre-commit gate: exactly what `.github/workflows/ci.yml` runs.
#
# Run it before every commit — CI is this list, so a red gate locally means a
# red main. Struct/field additions are covered: `clippy --all-targets` compiles
# `examples/` and `tests/` too, which is what catches a missed initializer.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets --all-features -- -D warnings"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "==> cargo run -q -p pwe-conformance"
cargo run -q -p pwe-conformance

echo "==> cargo build --examples -p pwe-reference"
cargo build --examples -p pwe-reference

echo "gate: OK"
