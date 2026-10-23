#!/bin/sh
# Usage: verify_toolchain_pack.sh OVEN NEW_SCRATCH_DIRECTORY
# Exercise portable archive documents without rebuilding a compiler or adding contract cases.
set -eu
oven=$1
scratch=$2
mkdir "$scratch"
scratch=$(cd "$scratch" && pwd)
source=$scratch/source
toolchain=$source/toolchains/0.6.0-dev.8
native=$scratch/builder/native/runtime-hash
entries=$scratch/builder/entries/sha256-example/artifacts
loaf=$toolchain/loaves/example/debug
mkdir -p "$toolchain/bin" "$toolchain/rust-abi" "$loaf" "$native" "$entries"
other=$toolchain/loaves/other/debug
foreign=$scratch/foreign/loaves/example/debug
macros=$scratch/builder/native/macro-hash
mkdir -p "$other" "$foreign" "$macros"
printf 'other loaf\n' > "$other/libother.rlib"
printf 'foreign snapshot\n' > "$foreign/libexample.rlib"
printf 'macro crate\n' > "$macros/libmacro.dylib"
cat > "$toolchain/bin/driver" <<'EOF'
#!/bin/sh
set -eu
test -z "${RUSTC_BOOTSTRAP:-}"
test "$1" = --identity
test -L "$2"
EOF
chmod +x "$toolchain/bin/driver"
printf 'stale\n' > "$toolchain/bin/unused-driver"
printf 'abi\n' > "$toolchain/rust-abi/std.rustabi"
printf 'runtime\n' > "$native/libruntime.rlib"
printf 'registry\n' > "$entries/libexample.rlib"
printf 'loaf\n' > "$loaf/libexample.rlib"
printf 'private unit\n' > "$loaf/libexample.rlib.unit.json"
cat > "$scratch/closure.json" <<EOF
{"schema":"incan.oven.closure/1","externs":{"example":{"path":"$entries/libexample.rlib"}},"units":[{"loaf":"crates-io/example","version":"1.0.0","domain":"target","features":[],"entry":"sha256:example"}]}
EOF
cat > "$toolchain/toolchain.json" <<EOF
{"schema":"incan.toolchain/1","driver":"bin/driver","sysroot":"/unused/sysroot","rust_abi":["rust-abi/std.rustabi"],"registry_units":"$scratch/closure.json","externs":[{"name":"runtime","path":"$native/libruntime.rlib"}],"search":["$native","$entries","$scratch/missing"]}
EOF
cat > "$loaf/example.native.json" <<EOF
{"externs":[["runtime","$native/libruntime.rlib"],["example","$loaf/libexample.rlib"],["other","$other/libother.rlib"],["foreign","$foreign/libexample.rlib"]],"macro_externs":[["macro","$macros/libmacro.dylib"]],"search":["$native","$entries","$scratch/missing","$loaf","$other","$foreign"]}
EOF
for suffix in tar tar.gz tar.zst; do
    INCAN_HOME="$source" "$oven" toolchain pack "$scratch/toolchain.$suffix"
    target=$scratch/unpacked-$suffix
    mkdir "$target"
    tar -xf "$scratch/toolchain.$suffix" -C "$target"
    test -z "$(find "$target" -type l -print)"
    test ! -e "$target/sysroot"
    test ! -e "$target/bin/unused-driver"
    test ! -e "$target/loaves/example/debug/libexample.rlib.unit.json"
    test -f "$target/native/runtime-hash/libruntime.rlib"
    test -f "$target/registry/entries/sha256-example/artifacts/libexample.rlib"
    jq -e '.name == "0.6.0-dev.8" and .sysroot == "sysroot" and (.search | length) == 2' "$target/toolchain.json" > /dev/null
    jq -e '.externs[0][1] == "../../../native/runtime-hash/libruntime.rlib" and (.externs[1][1] | startswith("../../../native/example-")) and (.externs[2][1] | startswith("../../../native/other-")) and .externs[1][1] != .externs[3][1] and (.search | length) == 5' "$target/loaves/example/debug/example.native.json" > /dev/null
    jq -e '.macro_externs[0][1] == "../../../native/macro-hash/libmacro.dylib"' "$target/loaves/example/debug/example.native.json" > /dev/null
    jq -r '.externs[][1],.macro_externs[][1]' "$target/loaves/example/debug/example.native.json" | while IFS= read -r path; do
        test -f "$target/loaves/example/debug/$path"
    done
    foreign_path=$(jq -r '.externs[3][1]' "$target/loaves/example/debug/example.native.json")
    cmp "$foreign/libexample.rlib" "$target/loaves/example/debug/$foreign_path"
    test ! -e "$target/native/debug"
    jq -e '.externs.example.path == "entries/sha256-example/artifacts/libexample.rlib"' "$target/registry/closure.json" > /dev/null
    if rg -l -F "$scratch" "$target" -g '*.json'; then
        echo 'builder path escaped into packed documents' >&2
        exit 1
    fi
done
test -f "$toolchain/bin/unused-driver"
test -f "$loaf/libexample.rlib.unit.json"
mkdir "$scratch/sysroot"
destination=$scratch/installed
INCAN_HOME="$destination" INCAN_SYSROOT="$scratch/sysroot" RUSTC_BOOTSTRAP=1 \
    "$oven" toolchain install "$scratch/toolchain.tar.zst"
installed=$destination/toolchains/0.6.0-dev.8
test -L "$installed/sysroot"
test "$(readlink "$installed/sysroot")" = "$scratch/sysroot"
printf 'previous installation\n' > "$installed/preserve"

# Wrong-version and incompatible-driver archives must leave the current installation intact.
fixture=$scratch/unpacked-tar
jq '.name = "wrong-version"' "$fixture/toolchain.json" > "$scratch/wrong.json"
cp "$scratch/wrong.json" "$fixture/toolchain.json"
tar -cf "$scratch/wrong.tar" -C "$fixture" .
if INCAN_HOME="$destination" INCAN_SYSROOT="$scratch/sysroot" "$oven" toolchain install "$scratch/wrong.tar" > "$scratch/wrong.log" 2>&1; then
    echo 'accepted a wrong-version archive' >&2
    exit 1
fi
rg -q 'does not match' "$scratch/wrong.log"
test -f "$installed/preserve"
cp "$scratch/unpacked-tar.gz/toolchain.json" "$fixture/toolchain.json"
printf '#!/bin/sh\nexit 1\n' > "$fixture/bin/driver"
tar -cf "$scratch/incompatible.tar" -C "$fixture" .
if INCAN_HOME="$destination" INCAN_SYSROOT="$scratch/sysroot" "$oven" toolchain install "$scratch/incompatible.tar" > "$scratch/incompatible.log" 2>&1; then
    echo 'accepted an incompatible driver' >&2
    exit 1
fi
test -f "$installed/preserve"
test ! -d "$destination/toolchains/.install-0.6.0-dev.8"

# A verified reinstall replaces the old closure, rather than merging stale artifacts into it.
INCAN_HOME="$destination" INCAN_SYSROOT="$scratch/sysroot" "$oven" toolchain install "$scratch/toolchain.tar.gz"
test ! -e "$installed/preserve"

# A lost builder artifact must refuse packing and preserve an already published archive.
shasum -a 256 "$scratch/toolchain.tar.zst" > "$scratch/archive-before.sha256"
mv "$other/libother.rlib" "$other/libother.saved"
if INCAN_HOME="$source" "$oven" toolchain pack "$scratch/toolchain.tar.zst" > "$scratch/missing.log" 2>&1; then
    echo 'packed a missing named artifact' >&2
    exit 1
fi
rg -q 'missing toolchain artifact' "$scratch/missing.log"
shasum -a 256 -c "$scratch/archive-before.sha256" > /dev/null
echo 'portable pack: PASS (tar, gzip, zstd; relative manifests; named artifacts only)'
echo 'portable install: PASS (identity invocation, version refusal, identity refusal, replacement)'
