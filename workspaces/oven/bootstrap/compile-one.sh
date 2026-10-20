#!/bin/zsh
# Bootstrap: compile one Incan unit with the dev.8 incan-rustc-driver and the bootstrap runtime closure.
# usage: compile-one.sh PATH CRATE OUTPUT [--library]
#   PATH is what `incan describe-unit` takes (a source file or a project); OUTPUT is the binary or the rlib.
# The unit sees only the standard Loaves already compiled (in dependency order) for this toolchain.
set -e
LANE=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
B=$LANE/bootstrap
OVEN=$LANE/oven-bin/incan
TOOLCHAIN=$LANE/dev8-home/toolchains/0.6.0-dev.8/toolchain.json
DRIVER=$(jq -r .driver "$TOOLCHAIN")
SYSROOT=$(jq -r .sysroot "$TOOLCHAIN")
export INCAN_STDLIB=$LANE/incan/loaves/stdlib INCAN_HOME=$LANE/dev8-home
target=$1
crate=$2
output=$3
shift 3
mkdir -p "${output:h}"
unit=$output.unit.json
$OVEN describe-unit "$target" > "$unit.full"
compiled=()
for loaf in $(jq -r '.loaves[] | select(.standard) | .name + "=" + .dir' "$unit.full"); do
    name=${loaf%%=*}
    dir=${loaf#*=}
    [ -f "$dir/$name.incnlib" ] && compiled+=("$name")
done
jq --argjson keep "$(printf '%s\n' "${compiled[@]}" | jq -R . | jq -s 'map(select(. != ""))')" \
    '.loaves |= map(select((.standard | not) or (.name as $n | $keep | index($n))))' "$unit.full" > "$unit"
exec env -u RUSTC_BOOTSTRAP "$DRIVER" --unit "$unit" "$crate" "$output" "$SYSROOT" "$@"
