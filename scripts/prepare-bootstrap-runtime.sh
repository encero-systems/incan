#!/bin/zsh
# Rebuild the lane's emitted runtime and install the native roots Oven selects from its current source graph.
# Usage: DEV8_LANE=/path/to/lane scripts/prepare-bootstrap-runtime.sh
set -eu
set -o pipefail

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
bootstrap=$lane/bootstrap
runtime=$bootstrap/loaves/incan_native_runtime
installed_home=$lane/dev8-home
toolchain=$installed_home/toolchains/0.6.0-dev.8
oven=$lane/oven-bin/incan
# As for driver relinking, keep the parent compiler closure fixed while replacing runtime outputs. Otherwise a
# runtime build invalidates its own native inputs and every unchanged invocation rebuilds them again.
build_home=$bootstrap/driver-build-home
[[ -f $build_home/toolchains/0.6.0-dev.8/toolchain.json ]] || {
    print -u2 'prepare the pinned parent toolchain with scripts/relink-driver.sh first'
    exit 1
}
export INCAN_HOME=$build_home INCAN_STDLIB=$bootstrap/empty-stdlib
mkdir -p "$bootstrap/empty-stdlib"

# This describes the generated Rust project's native facet; no Incan program is compiled from this marker.
entry=$runtime/src/bootstrap_closure.incn
if [[ ! -f $entry ]]; then
    print '"""Describe the emitted runtime native closure for bootstrap installation."""' > "$entry"
fi
# The marker is an explicit source entry for description. It stays outside the conventional lib/main paths so
# building this directory still selects its Rust facet, rather than compiling an Incan entry.
registration='bootstrap_closure = "src/bootstrap_closure.incn"'
if ! grep -Fqx "$registration" "$runtime/loaf.toml"; then
    if grep -Eq '^\[project\.scripts\]' "$runtime/loaf.toml"; then
        print -u2 'the generated runtime already declares scripts without its closure marker'
        exit 1
    fi
    cat >> "$runtime/loaf.toml" <<'EOF'

[project.scripts]
bootstrap_closure = "src/bootstrap_closure.incn"
EOF
fi

# Both commands use Oven's selected local/registry edges. A copied runtime descriptor can name a different build
# of a support crate even when that crate's source has not changed: the native identity includes its project path.
"$oven" build "$runtime" > "$bootstrap/runtime-build.log" 2>&1
descriptor=$(mktemp "$bootstrap/runtime-native.unit.XXXXXX")
manifest=$(mktemp "$toolchain/toolchain.json.XXXXXX")
trap 'rm -f "$descriptor" "$manifest"' EXIT
"$oven" describe-unit "$entry" > "$descriptor"

# Generated standard components are dependencies of the bootstrap runtime, not direct inputs to dev.8 programs.
# Their directories remain searchable so rustc can load the runtime's transitive dependencies.
jq --slurpfile runtime "$descriptor" '
    .externs = [$runtime[0].externs[] | select(.name | startswith("incan_stdlib_") | not)] |
    .search = ((.search + $runtime[0].search) | unique)
' "$toolchain/toolchain.json" > "$manifest"
jq -e '.externs | group_by(.name) | all(length == 1)' "$manifest" > /dev/null
for artifact in ${(f)"$(jq -r '.externs[].path' "$manifest")"}; do
    [[ -f $artifact ]] || { print -u2 "missing runtime artifact: $artifact"; exit 1; }
done
mv "$descriptor" "$bootstrap/runtime-native.unit.json"
mv "$manifest" "$toolchain/toolchain.json"
print 'runtime native closure selected from source'
