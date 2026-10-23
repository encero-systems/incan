#!/bin/sh
# Exercise the installed driver's identity boundary without changing the supplied toolchain or sysroot.
# Usage: verify_identity.sh DRIVER SYSROOT UNIT NEW_SCRATCH_DIRECTORY
# UNIT must be an independently compilable program description produced by Oven.
set -eu

driver=$1
sysroot=$2
unit=$3
scratch=$4
mkdir "$scratch"
home=$scratch/toolchain
mkdir -p "$home/bin" "$scratch/logs"
cp "$driver" "$home/bin/incan-rustc-driver"
printf '{}\n' > "$home/toolchain.json"
driver=$home/bin/incan-rustc-driver
proof=$home/driver-library.proof
trap 'chmod u+w "$home"' EXIT HUP INT TERM

# Each successful compilation must prove identity exactly once and expose the cache decision in its trace.
compile() {
    name=$1
    mkdir "$scratch/logs/$name"
    env -u RUSTC_BOOTSTRAP INCAN_TRACE_DIR="$scratch/logs/$name" \
        "$driver" --unit "$unit" identity_probe "$scratch/$name" "$sysroot" \
        > "$scratch/$name.stdout" 2> "$scratch/$name.stderr"
    jq -s -e '[.[] | select(.kind == "begin" and .function == "incan_rustc_driver::verify_identity")] | length == 1' \
        "$scratch/logs/$name/"*.jsonl > /dev/null
}

# A complete trace must report exactly one loaded-library proof decision with the expected value.
cached() {
    jq -s -e --argjson expected "$2" \
        '[.[] | select(.kind == "event" and .fields.message == "loaded compiler library proof") | .fields.cached] == [$expected]' \
        "$scratch/logs/$1/"*.jsonl > /dev/null
}

# Refusals still apply to a warm proof and must occur before native output publication.
refused() {
    name=$1
    message=$2
    shift 2
    if "$@" > "$scratch/$name.stdout" 2> "$scratch/$name.stderr"; then
        printf 'unexpected acceptance: %s\n' "$name" >&2
        exit 1
    fi
    grep -F "$message" "$scratch/$name.stderr" > /dev/null
    test ! -e "$scratch/$name"
}

compile cold
cached cold false
cp "$proof" "$scratch/expected.proof"
compile warm
cached warm true
cmp "$proof" "$scratch/expected.proof"

for key in path device inode size mtime mtime_ns ctime ctime_ns digest schema; do
    jq -e --arg key "$key" 'has($key)' "$scratch/expected.proof" > /dev/null
    jq -c --arg key "$key" '.[$key] = "invalid"' "$scratch/expected.proof" > "$proof"
    compile "invalid_$key"
    cached "invalid_$key" false
    cmp "$proof" "$scratch/expected.proof"
done

printf 'incomplete' > "$proof"
compile malformed
cached malformed false
cmp "$proof" "$scratch/expected.proof"

mv "$proof" "$scratch/saved.proof"
mkdir "$proof"
compile unreadable
cached unreadable false
test -d "$proof"
rmdir "$proof"

ln -s "$scratch/saved.proof" "$proof"
compile symlink
cached symlink false
test ! -L "$proof"
cmp "$proof" "$scratch/expected.proof"
mv "$proof" "$scratch/before_unwritable.proof"

chmod u-w "$home"
compile unwritable
cached unwritable false
test ! -e "$proof"
chmod u+w "$home"

# Simultaneous misses must leave a complete proof and independently compile their programs.
compile concurrent_a &
first=$!
compile concurrent_b &
second=$!
wait "$first"
wait "$second"
cmp "$proof" "$scratch/expected.proof"
compile after_concurrent
cached after_concurrent true

refused bootstrap 'Bootstrap' env RUSTC_BOOTSTRAP= \
    "$driver" --unit "$unit" identity_probe "$scratch/bootstrap" "$sysroot"
refused other_sysroot 'sysroot path differs from the build-bound sysroot' env -u RUSTC_BOOTSTRAP \
    "$driver" --unit "$unit" identity_probe "$scratch/other_sysroot" "$scratch"

# Identical bytes loaded from another path must still be refused, even with an otherwise valid warm proof.
library=$(jq -r .path "$scratch/expected.proof" | xxd -r -p)
mkdir "$scratch/other-library"
cp "$library" "$scratch/other-library/"
case $(uname -s) in
    Darwin) loader_path=DYLD_LIBRARY_PATH ;;
    Linux) loader_path=LD_LIBRARY_PATH ;;
    *) printf 'unsupported loader test platform\n' >&2; exit 1 ;;
esac
refused other_library 'loaded a different rustc_driver shared library' \
    env -u RUSTC_BOOTSTRAP "$loader_path=$scratch/other-library" \
    "$driver" --unit "$unit" identity_probe "$scratch/other_library" "$sysroot"

mv "$home/toolchain.json" "$home/toolchain.json.saved"
compile uninstalled
cached uninstalled false

printf 'PASS identity: cold/warm, all proof keys, malformed/unreadable/unwritable store, concurrency, bootstrap, sysroot, loaded library, uninstalled driver\n'
