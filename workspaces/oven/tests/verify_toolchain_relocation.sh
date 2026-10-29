#!/bin/sh
# Usage: INCAN_HOME=SOURCE_HOME verify_toolchain_relocation.sh OVEN SYSROOT STDLIB NEW_SCRATCH_DIRECTORY
# STDLIB is the standard-library directory of the source checkout the toolchain was built from.
# Retain the archive, fresh home, copied sysroot and logs as inspectable relocation evidence.
set -eu
oven=$1
sysroot=$2
stdlib=$3
scratch=$4
mkdir "$scratch"
scratch=$(cd "$scratch" && pwd)
builder_home=$(cd "$INCAN_HOME" && pwd -P)
builder_source=$(cd "$stdlib/../.." && pwd -P)
cp -R "$sysroot" "$scratch/sysroot"
archive=$scratch/toolchain.tar.zst
INCAN_STDLIB=$stdlib "$oven" toolchain pack "$archive" > "$scratch/pack.log"
# The installation finds its standard sources itself; nothing names the checkout they came from.
unset INCAN_STDLIB
export INCAN_HOME=$scratch/home INCAN_SYSROOT=$scratch/sysroot
/usr/bin/time -p "$oven" toolchain install "$archive" > "$scratch/install.log" 2> "$scratch/install.time"
installed=$INCAN_HOME/toolchains/0.6.0-dev.8
test "$(readlink "$installed/sysroot")" = "$scratch/sysroot"
test "$(find "$installed" -type l | wc -l | tr -d ' ')" = 1

# No builder leftover travels, and no text record of the installation names the builder's home or source checkout.
# Compiled libraries are binary: the paths rustc embeds there are not read and stay outside this audit.
if find "$installed" -name '*.previous.*' | rg -q .; then
    echo 'the installation keeps builder leftovers' >&2
    exit 1
fi
if rg --hidden --no-ignore -l -F -e "$builder_home/" -e "$builder_source/" "$installed" > "$scratch/builder-paths.txt"; then
    echo "installed records name the builder: $scratch/builder-paths.txt" >&2
    exit 1
fi

# Every artifact-bearing document must resolve wholly inside the new installation.
audit_paths() {
    base=$1
    while IFS= read -r relative; do
        case "$relative" in
            ''|/*) echo "nonrelative artifact path: $relative" >&2; exit 1 ;;
        esac
        if test ! -e "$base/$relative"; then
            echo "missing installed artifact: $base/$relative" >&2
            exit 1
        fi
        resolved=$(realpath "$base/$relative")
        case "$resolved" in
            "$installed"/*) ;;
            *) echo "artifact escaped installation: $resolved" >&2; exit 1 ;;
        esac
    done
}
jq -r '.driver,.registry_units,.rust_abi[],.externs[].path,.search[]' "$installed/toolchain.json" | audit_paths "$installed"
jq -r '.externs[].path' "$installed/registry/closure.json" | audit_paths "$installed/registry"
find "$installed/loaves" -name '*.native.json' -print | while IFS= read -r document; do
    jq -r '.externs[][1],(.macro_externs // [])[][1],.search[]' "$document" | audit_paths "$(dirname "$document")"
done
# No document Oven reads may keep an absolute path of any kind, named by a field this audit knows or not.
if { echo "$installed/toolchain.json"; echo "$installed/registry/closure.json"; find "$installed/loaves" -name '*.native.json'; } \
    | xargs jq -r '.. | strings | select(startswith("/"))' | rg -q .; then
    echo 'an installed document keeps an absolute path' >&2
    exit 1
fi
jq -r '.units[].entry | gsub(":"; "-")' "$installed/registry/closure.json" | while IFS= read -r entry; do
    test -d "$installed/registry/entries/$entry/artifacts"
done

# On macOS prove the loaded rustc_driver comes from the copied sysroot, not the build-time runpath fallback.
driver=$(jq -r '.driver' "$installed/toolchain.json")
if test "$(uname -s)" = Darwin; then
    env -u RUSTC_BOOTSTRAP DYLD_PRINT_LIBRARIES=1 "$installed/$driver" --identity "$installed/sysroot" \
        > "$scratch/identity.stdout" 2> "$scratch/identity.stderr"
    rg -F "$scratch/sysroot/" "$scratch/identity.stderr" | rg -q 'librustc_driver'
fi

mkdir -p "$scratch/probe/src" "$scratch/probe/tests"
cat > "$scratch/probe/loaf.toml" <<'EOF'
[project]
name = "installed_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
EOF
cat > "$scratch/probe/src/main.incn" <<'EOF'
def main() -> None:
    """Run against the installed closure."""
    println("installed")
EOF
cat > "$scratch/probe/tests/test_installed.incn" <<'EOF'
from std.testing import assert_eq

def test_installed() -> None:
    """Link and execute the installed testing Loaf."""
    assert_eq(2 + 3, 5)
EOF
"$oven" check "$scratch/probe/src/main.incn" > "$scratch/check.log" 2>&1
"$oven" test "$scratch/probe/tests/test_installed.incn" > "$scratch/testing.log" 2>&1
rg -q 'test_installed.incn::test_installed PASSED' "$scratch/testing.log"
wc -c "$archive"
cat "$scratch/install.time"
echo 'relocation: PASS (copied sysroot, contained artifact paths, check, testing)'
