#!/bin/zsh
# Compile the lane's standard Loaves from source, using the runtime's freshly selected native closure.
# Usage: DEV8_LANE=/path/to/lane scripts/compile-stdlib.sh [--cold] [PACKAGE...]
# --cold clears all provider outputs first. With no packages, build all ten in dependency order.
set -eu
set -o pipefail
setopt extendedglob

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
repo=$lane/incan
bootstrap=$lane/bootstrap
oven=$lane/oven-bin/incan
toolchain=$lane/dev8-home/toolchains/0.6.0-dev.8
cold=false
if [[ ${1:-} == --cold ]]; then
    cold=true
    shift
fi
packages=("$@")
(( ${#packages} > 0 )) || packages=(core testing interop async system codecs compression web data observability)

# Rebuild this profile without withdrawing another profile's compiled provider closure.
remove_provider() {
    local provider=$1
    [[ $provider == incan_stdlib_[a-z]## ]] || {
        print -u2 "invalid standard package name: $provider"
        return 2
    }
    [[ ! -e $toolchain/loaves/$provider/debug ]] || rm -r -- "$toolchain/loaves/$provider/debug"
}

# Reject invalid package names before changing an installed provider.
for package in "${packages[@]}"; do
    [[ $package == [a-z]## && -f $repo/loaves/stdlib/$package/loaf.toml ]] || {
        print -u2 "unknown standard package: $package"
        exit 2
    }
done
if ! zsh "$repo/scripts/prepare-bootstrap-runtime.sh" > "$bootstrap/runtime-selection.log" 2>&1; then
    tail -12 "$bootstrap/runtime-selection.log" >&2
    exit 1
fi
export INCAN_HOME=$lane/dev8-home INCAN_STDLIB=$repo/loaves/stdlib
mkdir -p "$bootstrap/std-logs" "$bootstrap/stdlib-available"

# A cold sweep must not let a later provider's old ABI or Body IR influence an earlier provider's checking pass.
if $cold; then
    for package in core testing interop async system codecs compression web data observability; do
        name=$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$repo/loaves/stdlib/$package/loaf.toml" | head -1)
        remove_provider "$name"
    done
fi

driver=$(jq -r .driver "$toolchain/toolchain.json")
sysroot=$(jq -r .sysroot "$toolchain/toolchain.json")
[[ $driver == /* ]] || driver="$toolchain/$driver"
[[ $sysroot == /* ]] || sysroot="$toolchain/$sysroot"
failed=false
for package in "${packages[@]}"; do
    name=$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$repo/loaves/stdlib/$package/loaf.toml" | head -1)
    remove_provider "$name"
    directory=$toolchain/loaves/$name/debug
    mkdir -p "$directory"
    unit=$directory/lib$name.rlib.unit.json
    if (
        "$oven" describe-unit "$repo/loaves/stdlib/$package/src/lib.incn" > "$unit.full" || exit $?
        # Only successfully published providers participate. Keep the complete description as build evidence.
        compiled=()
        for loaf in ${(f)"$(jq -r '.loaves[] | select(.standard) | .name + "=" + .dir' "$unit.full")"}; do
            dependency=${loaf%%=*}
            location=${loaf#*=}
            [[ ! -f $location/$dependency.incnlib ]] || compiled+=("$dependency")
        done
        jq --argjson keep "$(print -rl -- "${compiled[@]}" | jq -R . | jq -s 'map(select(. != ""))')" '
            .loaves |= map(select((.standard | not) or (.name as $name | $keep | index($name))))
        ' "$unit.full" > "$unit" || exit $?
        env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" "$name" "$directory/lib$name.rlib" "$sysroot" --library
    ) > "$bootstrap/std-logs/$package.log" 2>&1; then
        ln -sfn "$repo/loaves/stdlib/$package" "$bootstrap/stdlib-available/$package"
        print "$package: ok"
    else
        print -u2 "$package: failed; see $bootstrap/std-logs/$package.log"
        failed=true
    fi
done
! $failed
