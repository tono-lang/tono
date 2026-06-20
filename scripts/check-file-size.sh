#!/usr/bin/env bash
# Fails when a tracked source file exceeds the size ceiling.
# Guideline: target ~300 lines per file; hard ceiling 800 before splitting.
# Generated files, lockfiles and vendored trees are excluded.
set -euo pipefail

LIMIT="${FILE_SIZE_LIMIT:-800}"
fail=0

while IFS= read -r f; do
  [ -f "$f" ] || continue
  lines=$(wc -l < "$f")
  if [ "$lines" -gt "$LIMIT" ]; then
    echo "::error file=$f::$f has $lines lines (limit $LIMIT)"
    fail=1
  fi
done < <(git ls-files \
  '*.ml' '*.mli' '*.rs' '*.ts' '*.js' '*.go' '*.py' '*.java' \
  | grep -vE '(^|/)(target|_build|node_modules|dist)/' || true)

if [ "$fail" -eq 0 ]; then
  echo "All source files within the $LIMIT-line ceiling."
fi
exit "$fail"
