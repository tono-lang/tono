#!/usr/bin/env bash
# Fails when anything headed for the public repository cites internal
# bookkeeping: a planning document by number (RFC/ADR/PRD-NNNN), a task id, or
# the private docs tree. Those documents live outside this repository, so a
# number that only resolves there is noise for every reader, and worse when it
# ends up in generated output (tono init once wrote one into every user's
# tono.toml). Comments keep the "why"; they name the construct instead.
#
# Two layers, both run by CI on every pull request:
#   1. the tracked tree (git grep over the source directories);
#   2. the PR's own metadata: branch name, title, body and every commit message
#      not yet on the base branch. Merges are squashed, so gating the title is
#      what keeps the base branch's history clean.
# Locally, `scripts/check-internal-doc-refs.sh` checks the tree and the
# commits since origin/main; the PR title/body layer runs only in CI.
set -euo pipefail

PATTERN='(RFC|ADR|PRD|TASK)-?[0-9]|jaum|personal-docs'
SCOPE=(backend frontend cli lsp runtimes ir-schema scripts)
fail=0

report() { # <where> <text>
  echo "::error::internal reference in $1: $2"
  fail=1
}

# 1. tracked tree (this script carries the pattern by definition)
hits=$(git grep -niE "$PATTERN" -- "${SCOPE[@]}" \
  ':!**/package-lock.json' ':!scripts/check-internal-doc-refs.sh' || true)
if [ -n "$hits" ]; then
  while IFS= read -r line; do
    f=${line%%:*}; rest=${line#*:}; n=${rest%%:*}
    echo "::error file=$f,line=$n::internal reference: $line"
  done <<< "$hits"
  fail=1
fi

# 2. commit messages headed for the base branch
BASE="${BASE_REF:-origin/main}"
if git rev-parse --verify -q "$BASE" >/dev/null; then
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    if git log -1 --format='%B' "$c" | grep -qiE "$PATTERN"; then
      report "commit $(git log -1 --format='%h %s' "$c")" \
        "$(git log -1 --format='%B' "$c" | grep -iE "$PATTERN" | head -3 | tr '\n' ' ')"
    fi
  done < <(git rev-list "$BASE..HEAD" 2>/dev/null || true)
fi

# 3. branch name, PR title and body (CI passes these in; empty locally)
for var in PR_BRANCH PR_TITLE PR_BODY; do
  val="${!var:-}"
  if printf '%s' "$val" | grep -qiE "$PATTERN"; then
    report "$var" "$(printf '%s' "$val" | grep -iE "$PATTERN" | head -3 | tr '\n' ' ')"
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "Describe the feature, not the internal bookkeeping (see scripts/check-internal-doc-refs.sh)."
  exit 1
fi
echo "No internal references in tree, commits, branch, PR title or body."
