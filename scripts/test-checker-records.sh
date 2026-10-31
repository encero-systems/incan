#!/bin/sh
# Run the existing scoped-reader contract with the canonical codec and its test-only native adapter dependency.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
oven=${1:?usage: test-checker-records.sh OVEN NEW_EVIDENCE_DIRECTORY}
evidence=${2:?usage: test-checker-records.sh OVEN NEW_EVIDENCE_DIRECTORY}
mkdir -p "$evidence"
evidence=$(CDPATH= cd -- "$evidence" && pwd)
project=$evidence/project
mkdir "$project"
mkdir "$project/src"
# These are generated build inputs copied from their sole authored home, not a second contract implementation.
for name in checker_records record_scope records record_service; do
    cp "$root/loaves/compiler/internal_models/src/$name.incn" "$project/src/$name.incn"
done
for name in canonical_record_controls test_record_scope; do
    cp "$root/loaves/compiler/internal_models/tests/$name.incn" "$project/src/$name.incn"
done
cat > "$project/loaf.toml" <<EOF
[project]
name = "checker_record_contract_test"
version = "0.1.0"
private = true
[project.scripts]
contract = "src/test_record_scope.incn"
[dependencies]
incan_frontend = { loaf = "incan_frontend", path = "$root/loaves/compiler/incan_frontend" }
EOF
# File-based Incan tests currently omit envs.test; this isolated manifest keeps the native adapter out of production.
"$oven" test "$project/src/test_record_scope.incn" --profile release
