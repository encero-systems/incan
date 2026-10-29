#!/usr/bin/env bash
# Run tests for every source Loaf with a Rust facet, retaining each suite's complete output.
# Usage: scripts/test-rust-facets.sh [OVEN] [EVIDENCE_DIRECTORY]
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
oven=$(command -v "${1:-incan}")
out=${2:-$ROOT/target/rust-tests}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$ROOT"

# Fixtures deliberately describe absent and refused crates; only production Loaves own suites here.
git ls-files 'loaves/**/loaf.toml' > "$out/manifests.txt"
status=0
suites=0
while IFS= read -r manifest; do
    if ! awk '/^[[:space:]]*\[rust\][[:space:]]*(#.*)?$/ || /^[[:space:]]*\[\[rust[.]bin\]\][[:space:]]*(#.*)?$/ {found=1} END {exit !found}' "$manifest"; then
        continue
    fi
    project=${manifest%/loaf.toml}
    label=${project//\//-}
    suites=$((suites + 1))
    # Release matches the installed standard runtime's support closure, including its Rust facet profile.
    if "$oven" test "$ROOT/$project" --profile release > "$out/$label.log" 2>&1; then
        printf 'PASS %s\n' "$project"
    else
        printf 'FAIL %s (see %s)\n' "$project" "$out/$label.log"
        status=1
    fi
done < "$out/manifests.txt"
if [[ $suites == 0 ]]; then
    echo 'no Rust facets found' >&2
    exit 1
fi
echo "Rust facet suites: $suites; status: $status"
exit "$status"
