#!/usr/bin/env bash
# Benchmark regression gate over `reference/benches/throughput.rs`.
#
#   tools/bench-check.sh            local: warn >5% and fail >15% vs the baseline
#   tools/bench-check.sh --ci       CI: fail when a label exceeds its ceiling
#   tools/bench-check.sh --full     local thresholds at full iteration counts
#   tools/bench-check.sh --update   re-record the baseline (and its ceilings)
#
# The baseline is `reference/benches/baseline.txt`: one tab-separated row per
# label — `label <TAB> baseline_us <TAB> ceiling_us`. Ceilings are 6x the
# baseline so a slower CI machine still passes, while an order-of-magnitude
# regression (a reintroduced O(n^2), a lost fast path) still fails.
#
# Iteration counts shrink via PWE_BENCH_SCALE (warm-up is outside the timed
# region, so the per-op number does not depend on it). The baseline is recorded
# at the local gate's default scale — pass the same `PWE_BENCH_SCALE` to
# `--update` as you intend to compare with.
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=reference/benches/baseline.txt
MODE=local
SCALE="${PWE_BENCH_SCALE:-}"
case "${1:-}" in
  "") ;;
  --ci) MODE=ci ;;
  --full) MODE=local; SCALE=1 ;;
  --update) MODE=update ;;
  *) echo "usage: $0 [--ci|--full|--update]" >&2; exit 2 ;;
esac

# The baseline is recorded at the local gate's scale, so a comparison is
# apples-to-apples (same iteration counts, same warm-up).
if [ -z "$SCALE" ]; then
  if [ "$MODE" = ci ]; then SCALE=2; else SCALE=4; fi
fi

RESULTS=$(mktemp)
trap 'rm -f "$RESULTS"' EXIT

echo "==> cargo bench -p pwe-reference (PWE_BENCH_SCALE=$SCALE)"
PWE_BENCH_SCALE="$SCALE" cargo bench -p pwe-reference | awk '/us\/op$/ {
  line = $0
  sub(/[[:space:]]*us\/op$/, "", line)
  n = split(line, tok, /[[:space:]]+/)
  value = tok[n]
  label = tok[1]
  for (i = 2; i < n; i++) label = label " " tok[i]
  printf "%s\t%s\n", label, value
}' >"$RESULTS"

if [ ! -s "$RESULTS" ]; then
  echo "bench-check: no benchmark lines parsed" >&2
  exit 1
fi

if [ "$MODE" = update ]; then
  {
    echo "# tools/bench-check.sh --update — label<TAB>baseline_us<TAB>ceiling_us"
    while IFS=$'\t' read -r label value; do
      ceiling=$(awk -v v="$value" 'BEGIN { printf "%.3f", v * 6 }')
      printf '%s\t%s\t%s\n' "$label" "$value" "$ceiling"
    done <"$RESULTS"
  } >"$BASELINE.tmp"
  mv "$BASELINE.tmp" "$BASELINE"
  echo "bench-check: wrote $(grep -cv '^#' "$BASELINE") rows to $BASELINE"
  cat "$BASELINE"
  exit 0
fi

if [ ! -f "$BASELINE" ]; then
  echo "bench-check: $BASELINE not found (run '$0 --update' once)" >&2
  exit 1
fi

awk -F'\t' -v mode="$MODE" '
  NR == FNR {
    if ($0 ~ /^#/ || NF < 2) next
    base[$1] = $2 + 0
    ceil[$1] = $3 + 0
    next
  }
  {
    label = $1
    value = $2 + 0
    seen[label] = 1
    if (!(label in base)) {
      printf "note: %-28s %10.3f us/op (no baseline entry)\n", label, value
      next
    }
    if (mode == "ci") {
      if (value > ceil[label]) {
        printf "FAIL: %-28s %10.3f > ceiling %10.3f\n", label, value, ceil[label]
        fails++
      } else {
        printf "ok:   %-28s %10.3f <= %10.3f\n", label, value, ceil[label]
      }
      next
    }
    ratio = value / base[label]
    if (ratio > 1.15) {
      printf "FAIL: %-28s %10.3f  %.0f%% over baseline %10.3f\n", label, value, (ratio - 1) * 100, base[label]
      fails++
    } else if (ratio > 1.05) {
      printf "WARN: %-28s %10.3f  %.0f%% over baseline %10.3f (if intended: tools/bench-check.sh --update)\n", label, value, (ratio - 1) * 100, base[label]
      warns++
    } else {
      printf "ok:   %-28s %10.3f  (baseline %10.3f)\n", label, value, base[label]
    }
  }
  END {
    for (label in base) {
      if (!(label in seen)) {
        printf "note: %-28s missing from this run (skipped by cfg/feature?)\n", label
      }
    }
    printf "bench-check: %d fail(s), %d warning(s)\n", fails, warns
    exit fails > 0
  }
' "$BASELINE" "$RESULTS"
