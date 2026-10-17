#!/bin/sh
# Transport one frozen, receipt-bound compiler bundle without changing any executable bytes.
# The caller must exclusively own the destination and keep the source frozen. All five files are preflighted,
# copied and compared before publication. Renames are atomic per file, with the compiler published last; this is
# not an atomic multi-file transaction and no consumer may run concurrently with staging.
set -eu

fail() {
    echo "compiler bundle staging: $*" >&2
    exit 1
}

launcher=false
if [ "${1:-}" = "--launcher" ]; then launcher=true; shift; fi
[ "$#" -eq 2 ] || fail "usage: stage_compiler_bundle.sh [--launcher] SOURCE_COMPILER DESTINATION_COMPILER"
source_compiler=$1
destination_compiler=$2
# A local Cargo-target launcher may be the source of a later real-file release bundle. Resolve only that compiler
# launcher, then take all companions from its canonical owner rather than mixing adjacent launcher generations.
links=0
while :; do
    source_compiler="$(cd "$(dirname "$source_compiler")" && pwd -P)/$(basename "$source_compiler")"
    [ -L "$source_compiler" ] || break
    links=$((links + 1))
    [ "$links" -le 40 ] || fail "compiler launcher has too many symbolic links"
    target=$(readlink "$source_compiler")
    case "$target" in
        /*) source_compiler=$target ;;
        *) source_compiler="$(dirname "$source_compiler")/$target" ;;
    esac
done
source_directory=$(dirname "$source_compiler")
destination_directory=$(dirname "$destination_compiler")
destination_name=$(basename "$destination_compiler")
case "$destination_name" in
    .|..|native-runtime-inputs|oven-policy-engine) fail "invalid destination compiler name: $destination_name" ;;
esac

require_source() {
    [ -f "$1" ] && [ ! -L "$1" ] || fail "missing or non-regular bundle member: $1"
}
require_destination() {
    [ ! -L "$1" ] || fail "destination member is a symlink: $1"
    [ ! -e "$1" ] || [ -f "$1" ] || fail "destination member is not a regular file: $1"
}

require_source "$source_compiler"
require_source "$source_compiler.receipt.json"
require_source "$source_compiler.oven-output.json"
require_source "$source_directory/native-runtime-inputs"
require_source "$source_directory/oven-policy-engine"
[ -x "$source_compiler" ] || fail "compiler is not executable: $source_compiler"
[ -x "$source_directory/native-runtime-inputs" ] || fail "native runtime engine is not executable"
[ -x "$source_directory/oven-policy-engine" ] || fail "Oven policy engine is not executable"
if [ -L "$destination_compiler" ]; then
    [ ! -d "$destination_compiler" ] || fail "destination compiler points to a directory"
else
    require_destination "$destination_compiler"
fi
require_destination "$destination_compiler.receipt.json"
require_destination "$destination_compiler.oven-output.json"
require_destination "$destination_directory/native-runtime-inputs"
require_destination "$destination_directory/oven-policy-engine"

mkdir -p "$destination_directory"
staging=$(mktemp -d "$destination_directory/.compiler-bundle.XXXXXX")
trap 'rm -rf -- "$staging"' EXIT
trap 'exit 1' HUP INT TERM

stage_member() {
    cp -p "$1" "$staging/$2"
    cmp "$1" "$staging/$2" || fail "copied member differs: $1"
}
if [ "$launcher" = true ]; then
    destination_absolute="$(cd "$destination_directory" && pwd -P)/$destination_name"
    [ "$source_compiler" != "$destination_absolute" ] || fail "compiler launcher would point to itself"
    # Only the executable is linked. Native test hosts may observe launcher-relative engines, whose ordinary
    # freshness checks require regular files. Portable CI/release transport always uses the default copy mode.
    ln -s "$source_compiler" "$staging/$destination_name"
    cmp "$source_compiler" "$staging/$destination_name" || fail "compiler launcher differs from its owner"
else
    stage_member "$source_compiler" "$destination_name"
fi
stage_member "$source_compiler.receipt.json" "$destination_name.receipt.json"
stage_member "$source_compiler.oven-output.json" "$destination_name.oven-output.json"
stage_member "$source_directory/native-runtime-inputs" native-runtime-inputs
stage_member "$source_directory/oven-policy-engine" oven-policy-engine

# Preserve receipt-bound bytes, including any signature sealed by the producer; never codesign after copying.
mv -f "$staging/$destination_name.receipt.json" "$destination_compiler.receipt.json"
mv -f "$staging/$destination_name.oven-output.json" "$destination_compiler.oven-output.json"
mv -f "$staging/native-runtime-inputs" "$destination_directory/native-runtime-inputs"
mv -f "$staging/oven-policy-engine" "$destination_directory/oven-policy-engine"
mv -f "$staging/$destination_name" "$destination_compiler"
