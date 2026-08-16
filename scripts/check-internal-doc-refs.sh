#!/usr/bin/env bash
# Fails when tracked source cites an internal planning document by number.
# The design documents (RFC/ADR/PRD numbering) live outside this repository;
# a number that only resolves there is noise for every reader of the public
# tree, and worse when it ends up in generated output (tono init once wrote one
# into every user's tono.toml). Comments keep the "why"; they name the construct
# instead of the document.
set -euo pipefail

PATTERN='(RFC|ADR|PRD)-?[0-9]'
SCOPE=(backend frontend cli lsp runtimes ir-schema scripts)

hits=$(git grep -niE "$PATTERN" -- "${SCOPE[@]}" ':!**/package-lock.json' || true)

if [ -n "$hits" ]; then
  while IFS= read -r line; do
    f=${line%%:*}
    rest=${line#*:}
    n=${rest%%:*}
    echo "::error file=$f,line=$n::internal doc reference: $line"
  done <<< "$hits"
  echo "Describe the construct, not the document (see scripts/check-internal-doc-refs.sh)."
  exit 1
fi

echo "No internal doc references in ${SCOPE[*]}."
