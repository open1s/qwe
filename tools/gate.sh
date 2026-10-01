#!/usr/bin/env bash
# The pre-commit gate: the `gate` CI job plus the benchmark gate, so a red
# gate locally means a red main. Run it before every commit.
#
# Struct/field additions are covered: `clippy --all-targets` compiles
# `examples/` and `tests/` too, which is what catches a missed initializer.
# The other CI jobs are separate: `supply-chain` (`cargo deny check`),
# `subject` (commit-message lint) and `bench` (`tools/bench-check.sh --ci`,
# cross-machine ceilings — this script uses the tighter local thresholds).
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

echo "==> tools/bench-check.sh (benchmark regression, local thresholds)"
./tools/bench-check.sh

echo "gate: OK"
