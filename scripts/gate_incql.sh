#!/usr/bin/env bash

# Prove one Incan build against the real IncQL consumer before a release is trusted.
#
# A green compiler suite is not evidence that a release works: in August 2026 more than 2,600 tests passed while
# IncQL was unbuildable for weeks, because two independently resolved compilations of `tokio` linked into one
# binary and panicked with "no reactor running" at runtime. That failure is invisible to unit tests and visible
# here, so this gate runs the flagship consumer end to end and inspects the produced binary.
#
# This is deliberately a local gate. IncQL pulls DataFusion, Substrait, and Prost, which is far too heavy to put in
# front of every pull request; it belongs in front of a release instead.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  bash scripts/gate_incql.sh --incan PATH [options]

Builds the IncQL library and its quickstart consumer with the named Incan compiler, from a clean consumer state,
and fails unless the quickstart runs and links exactly one compiled `tokio`.

Options:
  --incan PATH        Incan command to test (required)
  --incql PATH        IncQL checkout (default: $INCQL_CHECKOUT, else ../tmp/incql-rc3-test beside this repo)
  --incan-source PATH Incan source checkout IncQL's vocab companion expects (default: this repository)
  --work PATH         Scratch directory holding the isolated Incan home (default: a fresh mktemp -d)
  --incan-home PATH   Existing Incan home to build against, rather than an empty isolated one. Required when
                      testing a released archive: its Loafs bind to the exact Rust the installer provisions, so
                      an empty home has no matching toolchain and every Loaf is rejected as incompatible.
  --keep-work         Leave the scratch directory in place for inspection
  --allow-dirty       Run even though the IncQL checkout has uncommitted changes
  -h, --help          Show this help
EOF
}

fail() {
    printf 'gate_incql: %s\n' "$*" >&2
    exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
incan=""
incql="${INCQL_CHECKOUT:-}"
incan_source="$repo_root"
work=""
incan_home_override=""
keep_work=0
allow_dirty=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --incan) incan="${2:-}"; shift 2 ;;
        --incql) incql="${2:-}"; shift 2 ;;
        --incan-source) incan_source="${2:-}"; shift 2 ;;
        --work) work="${2:-}"; shift 2 ;;
        --incan-home) incan_home_override="${2:-}"; shift 2 ;;
        --keep-work) keep_work=1; shift ;;
        --allow-dirty) allow_dirty=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $1" ;;
    esac
done

[ -n "$incan" ] || { usage >&2; fail "--incan is required"; }
[ -x "$incan" ] || fail "--incan does not name an executable: $incan"
incan="$(cd "$(dirname "$incan")" && pwd)/$(basename "$incan")"

[ -n "$incql" ] || incql="$(cd "$repo_root/.." && pwd)/tmp/incql-rc3-test"
[ -d "$incql" ] || fail "IncQL checkout not found: $incql (pass --incql or set INCQL_CHECKOUT)"
incql="$(cd "$incql" && pwd)"
quickstart="$incql/examples/quickstart"
[ -d "$quickstart" ] || fail "IncQL quickstart consumer not found: $quickstart"
[ -d "$incan_source" ] || fail "Incan source checkout not found: $incan_source"

# A consumer checkout carrying local edits is not a reference subject: a failure cannot be attributed to the
# compiler under test, and a pass proves nothing about the released consumer either. Refuse rather than produce a
# result that reads like evidence. `--allow-dirty` is for deliberately testing a work-in-progress consumer.
if [ "$allow_dirty" -eq 0 ] && git -C "$incql" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    # `oven.lock` is rewritten by the compiler itself, so a modified lockfile is evidence that the tool ran, not
    # that someone edited the consumer. Flagging it would make this guard fire on every ordinary checkout. The
    # pre-rename spelling stays filtered so a consumer checkout that still tracks one is not read as edited.
    dirty="$(git -C "$incql" status --porcelain --untracked-files=no | grep -Ev ' (oven|incan)\.lock$' || true)"
    if [ -n "$dirty" ]; then
        printf 'gate_incql: the IncQL checkout has uncommitted changes, so a gate result would not be attributable:\n' >&2
        printf '%s\n' "$dirty" >&2
        fail "commit or set aside those changes, or pass --allow-dirty to test this working tree deliberately"
    fi
fi

if [ -z "$work" ]; then
    work="$(mktemp -d)"
fi
mkdir -p "$work"
work="$(cd "$work" && pwd)"
# An empty home is right for a locally built compiler, which resolves its own stdlib source and seals Loafs against
# whatever Rust is ambient. A released archive is the opposite: its Loafs are sealed against the exact Rust the
# installer provisions into the toolchain's own home, so building against an empty home rejects every shipped Loaf
# with "no compatible release-cohort Loaf". Point this at an installed home to test a real release.
if [ -n "$incan_home_override" ]; then
    [ -d "$incan_home_override" ] || fail "--incan-home is not a directory: $incan_home_override"
    incan_home="$(cd "$incan_home_override" && pwd)"
else
    incan_home="$work/home"
fi
mkdir -p "$incan_home"

cleanup() {
    if [ "$keep_work" -eq 0 ]; then
        rm -rf "$work"
    else
        printf 'Scratch retained at %s\n' "$work"
    fi
}
trap cleanup EXIT

# IncQL's vocab companion resolves a sibling `incan` source checkout from inside the IncQL directory.
ln -sfn "$incan_source" "$incql/incan"

# Residual consumer state can mask a broken selection path: the packaged-provider bug reproduced only from clean.
printf '== Resetting consumer state ==\n'
rm -rf "$quickstart/.incan" "$quickstart/target" "$quickstart/oven.lock" "$quickstart/incan.lock"

run_stage() {
    local label="$1"
    local directory="$2"
    shift 2
    local started elapsed
    printf '== %s ==\n' "$label"
    started="$(date +%s)"
    ( cd "$directory" && INCAN_HOME="$incan_home" "$@" ) || fail "$label failed"
    elapsed="$(( $(date +%s) - started ))"
    printf '   %s: %ss\n' "$label" "$elapsed"
}

run_stage "Bake IncQL library" "$incql" "$incan" oven bake --project .
run_stage "Bake quickstart consumer" "$quickstart" "$incan" oven bake --project .

printf '== Running quickstart ==\n'
run_output="$work/quickstart-run.txt"
run_started="$(date +%s)"
( cd "$quickstart" && INCAN_HOME="$incan_home" "$incan" run ) > "$run_output" 2>&1 \
    || { cat "$run_output" >&2; fail "quickstart run failed"; }
run_elapsed="$(( $(date +%s) - run_started ))"
cat "$run_output"
printf '   run: %ss\n' "$run_elapsed"

grep -q "IncQL quickstart completed" "$run_output" \
    || fail "quickstart ran but did not report completion; see $run_output"

# One compiled `tokio` per binary. Two distinct v0 mangling hashes mean two independently compiled instances, which
# is the exact shape of the "no reactor running" panic: the runtime a task registers with is not the runtime that
# polls it. Checking the produced binary is what makes this real; a successful build proves nothing about linkage.
printf '== Checking for duplicate compiled tokio instances ==\n'
# The linked executable lands under the Oven output directory; the Cargo-routed layout keeps one under
# `target/debug` instead. Build scripts and dependency rlibs share those trees, so exclude them explicitly rather
# than matching on the package name, which differs from the directory name.
find_executable() {
    find "$quickstart/target" -type f -perm -111 -path "$1" \
        -not -path '*/deps/*' -not -path '*/build/*' -not -name 'build_script_build' -not -name '*.d' \
        2>/dev/null | head -1
}
binary="$(find_executable '*/oven/debug/*')"
[ -n "$binary" ] || binary="$(find_executable '*/oven/release/*')"
[ -n "$binary" ] || binary="$(find_executable '*/target/debug/*')"
[ -n "$binary" ] || fail "could not locate the quickstart binary to inspect under $quickstart/target"
printf '   binary: %s\n' "$binary"

# Two manglings have to be handled, and neither may be allowed to "pass" by finding nothing.
#
# v0 encodes the crate instance directly (`Cs<hash>_5tokio`), so distinct hashes count instances. A binary built
# through the unified-Cargo fallback uses legacy mangling instead, where symbols carry no per-instance hash -- a v0
# pattern matches zero symbols there and would report a reassuring `0` while verifying nothing. For that case count
# the compiled rlibs Cargo produced for the binary's own profile, which is the same evidence stated directly.
tokio_symbols="$(nm "$binary" 2>/dev/null | grep -c 'tokio' || true)"
[ "${tokio_symbols:-0}" -gt 0 ] || fail "binary links no tokio symbols at all; the duplicate-instance check cannot be trusted here"

tokio_hashes="$(nm "$binary" 2>/dev/null | grep -oE 'Cs[a-zA-Z0-9]+_5tokio' | sort -u || true)"
tokio_count="$(printf '%s' "$tokio_hashes" | grep -c . || true)"
evidence="v0 symbol hashes"

if [ "${tokio_count:-0}" -eq 0 ]; then
    # Legacy mangling: count rlibs within the profile this binary was built at, so a debug/release pair is not
    # mistaken for two instances linked into one artifact.
    case "$binary" in
        */release/*) binary_profile="release" ;;
        *) binary_profile="debug" ;;
    esac
    tokio_count="$(find "$quickstart/target" -path "*/${binary_profile}/deps/libtokio-*.rlib" 2>/dev/null \
        | sed 's|.*/||' | sort -u | wc -l | tr -d ' ')"
    evidence="compiled rlibs at the ${binary_profile} profile"
    [ "${tokio_count:-0}" -gt 0 ] \
        || fail "binary links tokio but neither v0 symbols nor compiled rlibs could be counted; cannot verify single-instance linkage"
fi

if [ "$tokio_count" -gt 1 ]; then
    printf '%s\n' "$tokio_hashes" >&2
    find "$quickstart/target" -name "libtokio-*.rlib" 2>/dev/null | sed 's/^/  /' >&2
    fail "binary links ${tokio_count} compiled tokio instances; exactly one is required"
fi
printf '   compiled tokio instances: %s (by %s)\n' "$tokio_count" "$evidence"

printf '\nIncQL gate passed (bake + run + single-tokio linkage).\n'
