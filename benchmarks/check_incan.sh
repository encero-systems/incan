#!/usr/bin/env bash
set -euo pipefail

# Quick smoke-check for benchmark Incan sources without running Python/Rust comparisons.
#
# This is meant for CI/dev sanity: ensure all benchmark `.incn` files can be built
# (typecheck + codegen + Rust build) by the Incan compiler.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Incan benchmark smoke-check (build only)"
echo "  root:  $PROJECT_ROOT"
echo ""

cd "$PROJECT_ROOT"
INCAN="$PROJECT_ROOT/target/release/incan"
if [[ ! -x "$INCAN" ]]; then
  cargo build --release --quiet
fi

failures=0
checked=0

while IFS= read -r -d '' file; do
  checked=$((checked + 1))
  echo "==> build: ${file#$PROJECT_ROOT/}"
  if ! "$INCAN" build "$file"; then
    echo "FAILED: $file"
    failures=$((failures + 1))
  fi
done < <(find "$SCRIPT_DIR" -type f -name "*.incn" -print0)

echo ""
echo "Summary:"
echo "  checked: $checked"
echo "  failed:  $failures"

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi
