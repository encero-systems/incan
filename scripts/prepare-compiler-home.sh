#!/bin/zsh
# Bind a compiler build to one source-validated native support generation.
# Usage: prepare-compiler-home.sh INSTALLED_HOME COMPILER_HOME
set -eu

(( $# == 2 )) || { print -u2 'usage: prepare-compiler-home.sh INSTALLED_HOME COMPILER_HOME'; exit 2; }
installed=$(cd "$1" && pwd -P)
mkdir -p "$2"
compiler=$(cd "$2" && pwd -P)
[[ $installed != "$compiler" ]] || { print -u2 'compiler home must be distinct from the installation'; exit 2; }
selected=$installed/toolchains/0.6.0-dev.8
destination=$compiler/toolchains/0.6.0-dev.8
[[ -f $selected/toolchain.json ]] || { print -u2 "missing installed manifest: $selected/toolchain.json"; exit 2; }
mkdir -p "$destination"
if [[ -e $destination/loaves || -L $destination/loaves ]]; then
    [[ -L $destination/loaves && $(readlink "$destination/loaves") == "$selected/loaves" ]] \
        || { print -u2 "compiler home already selects another Loaf tree: $destination/loaves"; exit 2; }
else
    ln -s "$selected/loaves" "$destination/loaves"
fi

# Installed producers are validated against current source by Oven. Removing a shared root would rebake a second
# code generation variant while the runtime and standard Loaves still require the installed producer's exact bytes.
temporary=$(mktemp "$destination/toolchain.json.XXXXXX")
trap '[[ -z ${temporary:-} ]] || rm -- "$temporary"' EXIT
jq --arg base "$selected/" '
    def absolute: if startswith("/") then . else $base + . end;
    .driver |= absolute | .sysroot |= absolute | .rust_abi |= map(absolute)
    | .externs |= map(.path |= absolute)
    | .search |= map(absolute)
    | if (.registry_units // "") != "" then .registry_units |= absolute else . end
    | if (.registry_lock // "") != "" then .registry_lock |= absolute else . end
' "$selected/toolchain.json" > "$temporary"
if [[ -f $destination/toolchain.json ]] && cmp -s "$temporary" "$destination/toolchain.json"; then
    rm -- "$temporary"
else
    mv "$temporary" "$destination/toolchain.json"
fi
temporary=
