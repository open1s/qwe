#!/usr/bin/env bash
# Verifies that the README (en + zh) test and conformance counts match reality.
#
# The counts drifted three times (#74, the 415->438 drift, #89) because nothing
# ever checked them. This runs the suite and the conformance report, derives
# every documented number from the real results, and fails on any mismatch — so
# `tools/gate.sh` and the `gate` CI job catch the drift before it lands.
set -euo pipefail
cd "$(dirname "$0")/.."

# `--color=never` is load-bearing: GitHub Actions exports CARGO_TERM_COLOR=always,
# and the colorized header `\e[1m\e[92m     Running\e[0m …` does not start with
# `Running`, so the parser below matched nothing and every suite fell into
# "other" (the local gate passed while CI failed on exactly this).
echo "==> cargo test --workspace"
tests_out="$(cargo test --workspace --color=never 2>&1)" || {
    printf '%s\n' "$tests_out"
    echo "check-readme-counts: cargo test failed"
    exit 1
}
# Belt and braces: drop any ANSI escapes the environment still injects.
tests_out="$(printf '%s\n' "$tests_out" | sed $'s/\033\\[[0-9;]*[A-Za-z]//g')"
printf '%s\n' "$tests_out"

echo "==> cargo run -q -p pwe-conformance"
conf_out="$(cargo run --color=never -q -p pwe-conformance 2>&1)" || {
    printf '%s\n' "$conf_out"
    echo "check-readme-counts: conformance run failed"
    exit 1
}
conf_out="$(printf '%s\n' "$conf_out" | sed $'s/\033\\[[0-9;]*[A-Za-z]//g')"
printf '%s\n' "$conf_out"

# --- reality ----------------------------------------------------------------
# Sum every `test result: ok. N passed;` line.
total=$(printf '%s\n' "$tests_out" | awk '/^test result:/ { s += $4 + 0 } END { print s + 0 }')

# Per-binary count: cargo prints `Running <what> (<path>)` before each block, so
# match the following `test result:` line. Keyed on a substring of that line.
binary_total() {
    printf '%s\n' "$tests_out" | awk -v key="$1" '
        /^ *Running / { hit = (index($0, key) > 0); next }
        hit && /^test result:/ { print $4 + 0; exit }'
}
ref_lib=$(binary_total "pwe_reference-")
laws=$(binary_total "tests/laws.rs")
prop=$(binary_total "tests/prop.rs")
stdlib=$(binary_total "tests/stdlib.rs")
fuzz=$(binary_total "tests/fuzz.rs")
# Everything else (pwe-api, the CLI, the conformance bin, fmt, doc-tests) is the
# README's "Integration / other" row — derived so it can never silently drift.
other=$(( total - ref_lib - laws - prop - stdlib - fuzz ))
conf=$(printf '%s\n' "$conf_out" | sed -nE 's/.*total=([0-9]+) failed=.*/\1/p' | tail -1)

echo
echo "==> reality: tests=$total (lib=$ref_lib laws=$laws prop=$prop stdlib=$stdlib fuzz=$fuzz other=$other) conformance=$conf"
echo

# --- assertions -------------------------------------------------------------
fail=0
check() { # label got want
    if [ "$2" = "$3" ]; then
        printf '  ok    %-44s %s\n' "$1" "$2"
    else
        printf '  FAIL  %-44s got %s, want %s\n' "$1" "$2" "$3"
        fail=1
    fi
}
num() { # file sed-expr
    sed -nE "$2" "$1" | head -1
}

echo "==> README.md"
check 'badge  tests'          "$(num README.md 's/.*badge\/tests-([0-9]+)%20passing.*/\1/p')"          "$total"
check 'badge  conformance'    "$(num README.md 's/.*badge\/conformance-([0-9]+)%2F.*/\1/p')"          "$conf"
check 'quick-start comment'   "$(num README.md 's/.*# ([0-9]+) tests.*/\1/p')"                        "$total"
check 'prose  conformance'    "$(num README.md 's/.*plus ([0-9]+) conformance checks.*/\1/p')"        "$conf"
check 'total line conform.'   "$(num README.md 's/.*\*\*([0-9]+) \/ [0-9]+, zero skips\*\*.*/\1/p')" "$conf"
check 'table  runtime unit'   "$(num README.md 's/^\| Runtime \/ language unit tests \| ([0-9]+) \|.*/\1/p')" "$ref_lib"
check 'table  law-conform.'   "$(num README.md 's/^\| Analytic law-conformance \| ([0-9]+) \|.*/\1/p')"       "$laws"
check 'table  property'       "$(num README.md 's/^\| Property tests \| ([0-9]+) \|.*/\1/p')"                 "$prop"
check 'table  stdlib'         "$(num README.md 's/^\| Standard-library tests \| ([0-9]+) \|.*/\1/p')"         "$stdlib"
check 'table  fuzzing'        "$(num README.md 's/^\| Fuzzing \(deterministic\) \| ([0-9]+) \|.*/\1/p')"      "$fuzz"
check 'table  integration'    "$(num README.md 's/^\| Integration \/ other \| ([0-9]+) \|.*/\1/p')"           "$other"
check 'table  total'          "$(num README.md 's/^\| \*\*Total\*\* \| \*\*([0-9]+)\*\* \|.*/\1/p')"          "$total"

echo "==> README-ZH.md"
check 'badge  tests'          "$(num README-ZH.md 's/.*badge\/tests-([0-9]+)%20passing.*/\1/p')"          "$total"
check 'badge  conformance'    "$(num README-ZH.md 's/.*badge\/conformance-([0-9]+)%2F.*/\1/p')"          "$conf"
check 'quick-start comment'   "$(num README-ZH.md 's/.*# ([0-9]+) 个测试.*/\1/p')"                        "$total"
check 'prose  conformance'    "$(num README-ZH.md 's/.*测试与 ([0-9]+) 项符合性检查.*/\1/p')"             "$conf"
check 'total line conform.'   "$(num README-ZH.md 's/.*\*\*([0-9]+) \/ [0-9]+，零跳过\*\*.*/\1/p')"       "$conf"
check 'table  runtime unit'   "$(num README-ZH.md 's/^\| 运行时 \/ 语言单元测试 \| ([0-9]+) \|.*/\1/p')"  "$ref_lib"
check 'table  law-conform.'   "$(num README-ZH.md 's/^\| 解析解符合性 \| ([0-9]+) \|.*/\1/p')"           "$laws"
check 'table  property'       "$(num README-ZH.md 's/^\| 属性测试 \| ([0-9]+) \|.*/\1/p')"               "$prop"
check 'table  stdlib'         "$(num README-ZH.md 's/^\| 标准库测试 \| ([0-9]+) \|.*/\1/p')"             "$stdlib"
check 'table  fuzzing'        "$(num README-ZH.md 's/^\| 模糊测试（确定性） \| ([0-9]+) \|.*/\1/p')"      "$fuzz"
check 'table  integration'    "$(num README-ZH.md 's/^\| 集成 \/ 其它 \| ([0-9]+) \|.*/\1/p')"           "$other"
check 'table  total'          "$(num README-ZH.md 's/^\| \*\*合计\*\* \| \*\*([0-9]+)\*\* \|.*/\1/p')"    "$total"

echo
if [ "$fail" -ne 0 ]; then
    echo "check-readme-counts: README counts are stale — update README.md / README-ZH.md"
    exit 1
fi
echo "check-readme-counts: OK"
