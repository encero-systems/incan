#!/usr/bin/env bash

# Compare how long Incan takes to build the same project across toolchain versions.
#
# This measures Incan, not Oven: whatever build strategy a release ships is what gets timed, so results stay
# comparable across versions that changed strategy entirely. `scripts/bench_oven_alpha.sh` answers the narrower
# question of how one Oven Alpha workload behaves; this answers the one a user asks, which is whether the new
# release builds their project faster or slower than the old one.
#
# Three timings are recorded per toolchain, because they fail differently:
#   cold        empty Incan home and a clean project -- what a new user waits for once
#   warm        immediate rebuild with nothing changed -- what caching is worth
#   incremental one source edit rebuilt -- what the edit/build loop actually costs
#
# Run this on an otherwise idle machine; a concurrent test suite will skew every number.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  bash scripts/bench_build_times.sh --toolchain LABEL=PATH [--toolchain LABEL=PATH ...] [options]

Options:
  --toolchain LABEL=PATH  Incan command to measure, repeatable (e.g. 0.4.0=/opt/incan-0.4.0/bin/incan)
  --project PATH          Existing project to build (default: a generated `incan new` starter)
  --repetitions N         Timed repeats per phase, best time wins (default: 3)
  --output PATH           Markdown results file (default: workspaces/benchmarks/results/build_times.md)
  --keep-work             Leave scratch directories for inspection
  -h, --help              Show this help
EOF
}

fail() {
    printf 'bench_build_times: %s\n' "$*" >&2
    exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
labels=()
paths=()
project=""
repetitions=3
output="$repo_root/workspaces/benchmarks/results/build_times.md"
keep_work=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --toolchain)
            [ "$#" -ge 2 ] || fail "--toolchain requires LABEL=PATH"
            case "$2" in
                *=*) labels+=("${2%%=*}"); paths+=("${2#*=}") ;;
                *) fail "--toolchain expects LABEL=PATH, got: $2" ;;
            esac
            shift 2 ;;
        --project) project="${2:-}"; shift 2 ;;
        --repetitions) repetitions="${2:-}"; shift 2 ;;
        --output) output="${2:-}"; shift 2 ;;
        --keep-work) keep_work=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $1" ;;
    esac
done

[ "${#labels[@]}" -gt 0 ] || { usage >&2; fail "at least one --toolchain is required"; }
[ "$repetitions" -ge 1 ] 2>/dev/null || fail "--repetitions must be a positive integer"

for path in "${paths[@]}"; do
    [ -x "$path" ] || fail "toolchain is not executable: $path"
done

work="$(mktemp -d)"
cleanup() {
    if [ "$keep_work" -eq 0 ]; then rm -rf "$work"; else printf 'Scratch retained at %s\n' "$work"; fi
}
trap cleanup EXIT

# Milliseconds since epoch, falling back to second precision where GNU date is unavailable.
now_ms() {
    if date +%s%3N 2>/dev/null | grep -qv N; then
        date +%s%3N
    else
        python3 -c 'import time; print(int(time.time()*1000))'
    fi
}

# Run one command silently and report how long it took in milliseconds.
time_command() {
    local started ended
    started="$(now_ms)"
    "$@" >/dev/null 2>&1 || return 1
    ended="$(now_ms)"
    printf '%s' "$(( ended - started ))"
}

# Best of N timed repeats, so one scheduling hiccup does not define the result.
best_of() {
    local best="" elapsed i
    for (( i = 0; i < repetitions; i++ )); do
        elapsed="$(time_command "$@")" || return 1
        if [ -z "$best" ] || [ "$elapsed" -lt "$best" ]; then best="$elapsed"; fi
    done
    printf '%s' "$best"
}

format_ms() {
    local ms="$1"
    if [ "$ms" -lt 1000 ]; then printf '%sms' "$ms"; else printf '%s.%02ss' "$(( ms / 1000 ))" "$(( (ms % 1000) / 10 ))"; fi
}

mkdir -p "$(dirname "$output")"
results=()

for index in "${!labels[@]}"; do
    label="${labels[$index]}"
    incan="${paths[$index]}"
    printf '\n======== %s ========\n' "$label"
    "$incan" --version 2>/dev/null | head -1 || true

    home="$work/home-$index"
    build_root="$work/project-$index"
    mkdir -p "$home"
    rm -rf "$build_root"

    if [ -n "$project" ]; then
        cp -R "$project" "$build_root"
    else
        mkdir -p "$build_root"
        ( cd "$build_root" && INCAN_HOME="$home" "$incan" new bench --yes >/dev/null 2>&1 ) \
            || fail "$label could not create a starter project"
        build_root="$build_root/bench"
    fi
    rm -rf "$build_root/.incan" "$build_root/target" "$build_root/oven.lock" "$build_root/incan.lock"

    # ---- Cold: empty home, clean project. Timed once; repeating it would measure a warm build. ----
    printf 'cold... '
    if ! cold="$(cd "$build_root" && time_command env INCAN_HOME="$home" "$incan" build)"; then
        printf 'FAILED\n'
        results+=("$label|failed|-|-")
        continue
    fi
    printf '%s\n' "$(format_ms "$cold")"

    # ---- Warm: nothing changed. ----
    printf 'warm... '
    warm="$(cd "$build_root" && best_of env INCAN_HOME="$home" "$incan" build)" \
        || { printf 'FAILED\n'; results+=("$label|$(format_ms "$cold")|failed|-"); continue; }
    printf '%s\n' "$(format_ms "$warm")"

    # ---- Incremental: one source edit, which is the loop a developer actually lives in. ----
    #
    # Every edit changes observable behavior and is verified by running the binary afterwards. A toolchain that
    # reports a successful build without actually rebuilding produces a very fast and completely meaningless
    # number; Incan 0.4.0 does exactly that, so an unverified incremental timing is not evidence of anything.
    printf 'incremental... '
    entry="$build_root/src/main.incn"
    [ -f "$entry" ] || entry="$(find "$build_root/src" -name '*.incn' 2>/dev/null | head -1)"
    if [ -z "$entry" ] || [ ! -f "$entry" ]; then
        printf 'skipped (no source file found)\n'
        results+=("$label|$(format_ms "$cold")|$(format_ms "$warm")|-")
        continue
    fi

    incremental_best=""
    stale=0
    for (( r = 0; r < repetitions; r++ )); do
        token="bench-edit-${index}-${r}"
        python3 - "$entry" "$token" <<'PY'
import pathlib, re, sys
path, token = pathlib.Path(sys.argv[1]), sys.argv[2]
text = path.read_text()
# Rewrite whatever the entry point returns so each iteration is a real, observable behavior change.
updated, count = re.subn(r'return "[^"]*"', f'return "{token}"', text, count=1)
path.write_text(updated if count else text + f'\n# {token}\n')
PY
        if ! elapsed="$(cd "$build_root" && time_command env INCAN_HOME="$home" "$incan" build)"; then
            incremental_best=""
            break
        fi
        observed="$(cd "$build_root" && INCAN_HOME="$home" "$incan" run 2>/dev/null || true)"
        if ! printf '%s' "$observed" | grep -q "$token"; then
            stale=1
            break
        fi
        if [ -z "$incremental_best" ] || [ "$elapsed" -lt "$incremental_best" ]; then incremental_best="$elapsed"; fi
    done

    if [ "$stale" -eq 1 ]; then
        printf 'STALE - reported success without rebuilding\n'
        results+=("$label|$(format_ms "$cold")|$(format_ms "$warm")|stale")
    elif [ -n "$incremental_best" ]; then
        printf '%s\n' "$(format_ms "$incremental_best")"
        results+=("$label|$(format_ms "$cold")|$(format_ms "$warm")|$(format_ms "$incremental_best")")
    else
        printf 'FAILED\n'
        results+=("$label|$(format_ms "$cold")|$(format_ms "$warm")|failed")
    fi
done

{
    printf '# Incan build times\n\n'
    printf 'Generated: %s\n\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'Host: %s %s\n\n' "$(uname -s)" "$(uname -m)"
    printf 'Rust: %s\n\n' "$(rustc --version 2>/dev/null || echo 'unknown')"
    printf 'Best of %s timed repeats per phase, except cold, which is measured once per toolchain because a\n' "$repetitions"
    printf 'repeated cold build is no longer cold.\n\n'
    printf 'Incremental edits are verified by running the binary afterwards. `stale` means the toolchain reported a\n'
    printf 'successful build but the change never reached the binary, so no timing for it would be meaningful --\n'
    printf 'and its warm column is measuring a no-op rather than a rebuild.\n\n'
    printf '| Toolchain | Cold (new user) | Warm (no changes) | Incremental (one edit) |\n'
    printf '|---|---:|---:|---:|\n'
    for row in "${results[@]}"; do
        IFS='|' read -r label cold warm incremental <<<"$row"
        printf '| %s | %s | %s | %s |\n' "$label" "$cold" "$warm" "$incremental"
    done
    printf '\n'
} > "$output"

printf '\nWrote %s\n\n' "$output"
cat "$output"
