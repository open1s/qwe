#!/usr/bin/env bash
# Lints the subjects of the commits a push would carry — the `subject` CI job
# (`.github/workflows/commit-lint.yml`), run locally *before* the push.
#
# The regex is the one that workflow uses. Running it here means a bad subject
# (`lang: …` instead of `feat(lang): …`) is caught while the commit is still a
# one-word reword, instead of after it has landed on main and turned the
# required check red (#84/#90).
#
# Range: `main@origin..main` with jj, or `origin/main..HEAD` with git when the
# repo is not a jj workspace. Nothing to push is not a failure.
set -euo pipefail
cd "$(dirname "$0")/.."

jj_bin="$(command -v jj 2>/dev/null || true)"
if [ -z "$jj_bin" ] && [ -x /opt/homebrew/bin/jj ]; then
    jj_bin=/opt/homebrew/bin/jj
fi

subjects=""
if [ -n "$jj_bin" ] && [ -d .jj ]; then
    subjects="$("$jj_bin" log --no-graph -r 'main@origin..main' \
        -T 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' 2>/dev/null || true)"
elif command -v git >/dev/null 2>&1; then
    subjects="$(git log --format='%h %s' origin/main..HEAD 2>/dev/null || true)"
fi

if [ -z "${subjects//[[:space:]]/}" ]; then
    echo "check-commit-subjects: nothing to push (main is level with main@origin)."
    exit 0
fi

# The subjects arrive through the environment, not a pipe: a heredoc onto
# `python3 -` already owns stdin, so `printf … | python3 - <<'PY'` would feed the
# program and leave `sys.stdin` at EOF (silently linting nothing).
SUBJECTS="$subjects" python3 <<'PY'
import os
import re
import sys

TYPE = re.compile(
    r"^(feat|fix|docs|test|chore|refactor|perf|build|ci|revert|style)"
    r"(\([a-z0-9][a-z0-9/-]*\))?: \S.*$"
)
DEFAULT = re.compile(r"^Update \d+ files?$")

bad = []
count = 0
for line in os.environ.get("SUBJECTS", "").splitlines():
    if not line.strip():
        continue
    sha, _, subject = line.partition(" ")
    subject = subject.strip()
    count += 1
    if not subject:
        bad.append(f"{sha}: empty subject")
    elif DEFAULT.match(subject):
        bad.append(f"{sha}: GitHub default subject {subject!r} — describe the change")
    elif len(subject) > 100:
        bad.append(f"{sha}: subject is {len(subject)} chars (max 100): {subject!r}")
    elif not TYPE.match(subject):
        bad.append(
            f"{sha}: {subject!r} — use `<type>(<scope>): <what>`; "
            f"see CONTRIBUTING.md § Commit messages"
        )

if bad:
    print("commit-subject lint failed:")
    for m in bad:
        print(f"  - {m}")
    sys.exit(1)
print(f"check-commit-subjects: OK ({count} subject(s) to push)")
PY
