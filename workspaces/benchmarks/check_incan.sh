#!/usr/bin/env bash
# Build every benchmark with the installed toolchain, without executing workloads.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/toolchain.sh"
benchmark_toolchain
failures=0
checked=0
while IFS= read -r -d '' file; do
    checked=$((checked + 1))
    echo "==> build: ${file#$SCRIPT_DIR/}"
    if ! "$INCAN" build "$file" --profile release; then
        failures=$((failures + 1))
    fi
done < <(find "$SCRIPT_DIR/compute" "$SCRIPT_DIR/sorting" "$SCRIPT_DIR/collections" -type d -name target -prune -o -type f -name '*.incn' -print0)
printf 'checked: %s; failed: %s\n' "$checked" "$failures"
[[ "$failures" -eq 0 ]]
