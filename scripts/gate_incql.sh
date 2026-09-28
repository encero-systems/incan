#!/usr/bin/env bash

# Prove the pinned IncQL consumer closure against one released Incan toolchain and a Lane 7 attestation. This gate
# is intentionally local and heavy. Its PATH guards make Cargo and publisher-only native/tool work unavailable;
# the semantic locks and equivalence report then prove that every selected registry unit came from admitted assets.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  bash scripts/gate_incql.sh --incan PATH --incql PATH --equivalence-report PATH [options]

Builds and runs the pinned IncQL quickstart through Oven, with Cargo and publisher tools unavailable, and verifies
the complete admitted registry closure against the literal Cargo/Oven equivalence attestation.

Options:
  --incan PATH             Released Incan executable (required)
  --incql PATH             IncQL checkout pinned by scripts/incql_gate_pin.json (required)
  --equivalence-report PATH
                           Attestation emitted by make test-oven-artifact-equivalence (required)
  --pin PATH               Checkout/lock pin (default: scripts/incql_gate_pin.json)
  --expect PATH            Consumer-graph expectations (default: scripts/incql_gate_expectations.toml)
  --incan-source PATH      Incan source for IncQL's vocab companion (default: this repository)
  --work PATH              Scratch directory and isolated Incan home (default: fresh mktemp -d)
  --incan-home PATH        Existing installed toolchain home instead of the isolated home
  --keep-work              Retain scratch state
  -h, --help               Show this help
EOF
}

fail() {
    printf 'gate_incql: %s\n' "$*" >&2
    exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
incan=""
incql="${INCQL_CHECKOUT:-}"
equivalence_report="${INCQL_EQUIVALENCE_REPORT:-}"
pin="$repo_root/scripts/incql_gate_pin.json"
expect="$repo_root/scripts/incql_gate_expectations.toml"
incan_source="$repo_root"
work=""
incan_home_override=""
keep_work=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --incan) incan="${2:-}"; shift 2 ;;
        --incql) incql="${2:-}"; shift 2 ;;
        --equivalence-report) equivalence_report="${2:-}"; shift 2 ;;
        --pin) pin="${2:-}"; shift 2 ;;
        --expect) expect="${2:-}"; shift 2 ;;
        --incan-source) incan_source="${2:-}"; shift 2 ;;
        --work) work="${2:-}"; shift 2 ;;
        --incan-home) incan_home_override="${2:-}"; shift 2 ;;
        --keep-work) keep_work=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $1" ;;
    esac
done

[ -n "$incan" ] || { usage >&2; fail "--incan is required"; }
[ -x "$incan" ] || fail "--incan does not name an executable: $incan"
incan="$(cd "$(dirname "$incan")" && pwd)/$(basename "$incan")"
[ -n "$incql" ] || { usage >&2; fail "--incql or INCQL_CHECKOUT is required"; }
[ -d "$incql" ] || fail "IncQL checkout not found: $incql"
incql="$(cd "$incql" && pwd)"
quickstart="$incql/examples/quickstart"
[ -d "$quickstart" ] || fail "IncQL quickstart consumer not found: $quickstart"
[ -f "$pin" ] || fail "IncQL pin not found: $pin"
[ -f "$expect" ] || fail "IncQL gate expectations not found: $expect"
[ -n "$equivalence_report" ] || { usage >&2; fail "--equivalence-report or INCQL_EQUIVALENCE_REPORT is required"; }
[ -f "$equivalence_report" ] || fail "equivalence report not found: $equivalence_report"
[ -d "$incan_source" ] || fail "Incan source checkout not found: $incan_source"
incan_source="$(cd "$incan_source" && pwd)"

"$incan" oven gate registry-pin --pin "$pin" --checkout "$incql" \
    || fail "IncQL checkout or lock differs from its pin"

if [ -z "$work" ]; then
    work="$(mktemp -d)"
fi
mkdir -p "$work"
work="$(cd "$work" && pwd)"
if [ -n "$incan_home_override" ]; then
    [ -d "$incan_home_override" ] || fail "--incan-home is not a directory: $incan_home_override"
    incan_home="$(cd "$incan_home_override" && pwd)"
else
    incan_home="$work/home"
fi
mkdir -p "$incan_home"
created_incan_link=0

cleanup() {
    if [ "$created_incan_link" -eq 1 ] && [ -L "$incql/incan" ]; then
        rm "$incql/incan"
    fi
    if [ "$keep_work" -eq 0 ]; then
        rm -rf "$work"
    else
        printf 'Scratch retained at %s\n' "$work"
    fi
}
trap cleanup EXIT

# The pinned IncQL vocab companion retains a source-relative dependency on Incan's vocab crate. The gate creates
# only the expected link and removes it afterwards; an existing different path is refused rather than overwritten.
if [ -e "$incql/incan" ] || [ -L "$incql/incan" ]; then
    existing_incan_source="$(cd "$incql/incan" 2>/dev/null && pwd -P)" \
        || fail "existing IncQL incan companion path is not a readable directory: $incql/incan"
    [ "$existing_incan_source" = "$incan_source" ] \
        || fail "existing IncQL incan companion path resolves to $existing_incan_source, expected $incan_source"
else
    ln -s "$incan_source" "$incql/incan"
    created_incan_link=1
fi

# Refuse producer work in the consumer. C-family wrappers permit final executable linking but reject compilation
# (`-c` or a source operand); generator/bootstrap/archive commands are unavailable in every form selected via PATH
# or the conventional environment variables.
guard="$work/consumer-command-guard"
guard_log="$work/forbidden-commands.log"
mkdir -p "$guard"
: > "$guard_log"
real_cc="$(command -v cc || true)"
[ -n "$real_cc" ] || fail "no C linker driver is available for rustc's final executable link"
cat > "$guard/forbid" <<'EOF'
#!/bin/sh
command_name="$(basename "$0")"
printf 'gate_incql: forbidden consumer command: %s %s\n' "$command_name" "$*" | tee -a "$INCQL_OVEN_CONSUMER_GUARD_LOG" >&2
exit 97
EOF
chmod +x "$guard/forbid"
for command_name in cargo protoc cmake ar c++ g++ clang++; do
    ln -s forbid "$guard/$command_name"
done
cat > "$guard/cc-link-only" <<'EOF'
#!/bin/sh
command_name="$(basename "$0")"
for argument in "$@"; do
    case "$argument" in
        -c|*.c|*.cc|*.cpp|*.cxx|*.s|*.S)
            printf 'gate_incql: forbidden consumer command: %s %s\n' "$command_name" "$*" \
                | tee -a "$INCQL_OVEN_CONSUMER_GUARD_LOG" >&2
            exit 97
            ;;
    esac
done
exec "$INCQL_OVEN_REAL_CC" "$@"
EOF
chmod +x "$guard/cc-link-only"
for command_name in cc gcc clang; do
    ln -s cc-link-only "$guard/$command_name"
done

# Residual outputs can conceal a fallback or stale semantic graph. The pinned source and lock remain untouched.
printf '== Resetting consumer outputs ==\n'
rm -rf "$incql/.incan" "$incql/target" "$incql/oven.lock" \
    "$quickstart/.incan" "$quickstart/target" "$quickstart/oven.lock" "$quickstart/incan.lock"

run_stage() {
    local label="$1"
    local directory="$2"
    shift 2
    local started elapsed
    printf '== %s ==\n' "$label"
    started="$(date +%s)"
    (
        cd "$directory"
        PATH="$guard:$PATH" \
        CARGO="$guard/cargo" PROTOC="$guard/protoc" CMAKE="$guard/cmake" AR="$guard/ar" \
        CC="$guard/cc" CXX="$guard/c++" \
        INCAN_HOME="$incan_home" INCAN_OVEN_CARGO_GUARD_LOG="$guard_log" \
        INCQL_OVEN_CONSUMER_GUARD_LOG="$guard_log" INCQL_OVEN_REAL_CC="$real_cc" \
        "$@"
    ) || fail "$label failed"
    elapsed="$(( $(date +%s) - started ))"
    printf '   %s: %ss\n' "$label" "$elapsed"
}

run_stage "Bake IncQL library" "$incql" "$incan" oven bake --project .
run_stage "Bake quickstart consumer" "$quickstart" "$incan" oven bake --project .
[ ! -s "$guard_log" ] || fail "consumer attempted forbidden Cargo or publisher work; see $guard_log"

printf '== Checking complete admitted closure ==\n'
"$incan" oven gate consumer-graph --expect "$expect" \
    --equivalence-report "$equivalence_report" "$incql/oven.lock" "$quickstart/oven.lock" \
    || fail "IncQL selected units are not one complete attested registry closure"

# A Cargo-routed executable is a refusal, never secondary evidence. Only an Oven output can be the subject run.
fallback="$(find "$quickstart/target/debug" -type f -perm -111 2>/dev/null | head -1 || true)"
[ -z "$fallback" ] || fail "Cargo-layout fallback executable is present: $fallback"
binary="$(find "$quickstart/target/oven" -type f -perm -111 \
    -not -path '*/deps/*' -not -path '*/build/*' -not -name '*.d' 2>/dev/null | head -1 || true)"
[ -n "$binary" ] || fail "could not locate an Oven quickstart executable under $quickstart/target/oven"
printf '   executable: %s\n' "$binary"

printf '== Running quickstart ==\n'
run_output="$work/quickstart-run.txt"
if ! run_stage "Run quickstart consumer" "$quickstart" "$incan" run > "$run_output" 2>&1; then
    cat "$run_output" >&2
    fail "quickstart run failed"
fi
cat "$run_output"
grep -q "IncQL quickstart completed" "$run_output" \
    || fail "quickstart ran but did not report completion; see $run_output"
[ ! -s "$guard_log" ] || fail "quickstart attempted forbidden Cargo or publisher work; see $guard_log"

"$incan" oven gate registry-pin --pin "$pin" --checkout "$incql" \
    || fail "IncQL bake changed the pinned checkout or lock"

printf '\nIncQL gate passed (pinned closure + Cargo-free admitted assets + executable run).\n'
