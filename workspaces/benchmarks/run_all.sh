#!/usr/bin/env bash
# Measure installed Incan, its pinned Rust compiler, and Python on identical workloads.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/toolchain.sh"
benchmark_toolchain
for tool in hyperfine jq python3; do
    command -v "$tool" >/dev/null || { echo "Required tool missing: $tool" >&2; exit 2; }
done
RUNS="${RUNS:-10}"
[[ "$RUNS" =~ ^[0-9]+$ && "$RUNS" -ge 10 ]] || { echo 'RUNS must be at least 10' >&2; exit 2; }
RESULTS_DIR="${RESULTS_DIR:-$SCRIPT_DIR/results}"
mkdir -p "$RESULTS_DIR"
ARTIFACTS="$(mktemp -d "${BENCH_SCRATCH:-$SCRIPT_DIR/target}/run.XXXXXX")"
RESULTS_FILE="$RESULTS_DIR/results.md"
BUILD_FILE="$RESULTS_DIR/build_times.md"
failures=0

write_identity() {
    printf 'Measured: %s\n\nCommit: `%s`\n\n' "$(date -u +%FT%TZ)" "$(git -C "$SCRIPT_DIR" rev-parse HEAD)"
    printf 'Toolchain: `%s`; driver SHA-256: `%s`.\n\n' "$TOOLCHAIN_NAME" "$(shasum -a 256 "$DRIVER" | cut -d ' ' -f1)"
    printf 'Rust: `%s`; Python: `%s`; host: `%s`.\n\n' "$("$PINNED_RUSTC" --version)" "$(python3 --version)" "$(uname -sm)"
    printf 'Raw evidence directory: `%s` under `BENCH_SCRATCH` (default `workspaces/benchmarks/target`).\n\n' "${ARTIFACTS##*/}"
}

{
    echo '# Benchmark results'
    echo
    write_identity
    echo 'Hyperfine: two warmups, at least ten measured runs, shell disabled. Times are means in milliseconds.'
    echo 'August values are the 0.5 measurements from 2026-08-21; they are historical comparisons on a different toolchain.'
    echo
    echo '| Benchmark | dev.8 ms | August Incan ms | Rust ms | Python ms | Output | Performance flags |'
    echo '| --- | ---: | ---: | ---: | ---: | --- | --- |'
} > "$RESULTS_FILE"
{
    echo '# Incan build times'
    echo
    write_identity
    echo 'Cold means one release build in a fresh project with installed dependencies already available. Warm is an unchanged second build. Both include CLI startup; neither measures installing a toolchain or flushing OS caches.'
    echo
    echo '| Benchmark | Cold ms | Warm ms | Status |'
    echo '| --- | ---: | ---: | --- |'
} > "$BUILD_FILE"

mean_ms() {
    jq -r --argjson index "$2" '.results[$index].mean * 1000 | . * 100 | round / 100' "$1"
}

run_benchmark() {
    local area=$1 name=$2 august=$3
    local source="$SCRIPT_DIR/$area/$name" project="$ARTIFACTS/$name"
    local cold warm status=verified incan_time=refused flags=none
    mkdir -p "$project/src"
    cp "$source/$name.incn" "$project/src/main.incn"
    printf '[project]\nname = "%s"\nversion = "0.1.0"\n\n[project.scripts]\n%s = "src/main.incn"\n' "$name" "$name" > "$project/loaf.toml"
    uptime > "$project/build-load.txt"
    echo "Building $name"
    if /usr/bin/time -p "$INCAN" build "$project/src/main.incn" --profile release > "$project/cold.log" 2> "$project/cold.time"; then
        /usr/bin/time -p "$INCAN" build "$project/src/main.incn" --profile release > "$project/warm.log" 2> "$project/warm.time"
        cold=$(awk '$1 == "real" {print $2 * 1000}' "$project/cold.time")
        warm=$(awk '$1 == "real" {print $2 * 1000}' "$project/warm.time")
    else
        status='build refused'
        cold=refused
        warm=unmeasured
        failures=$((failures + 1))
        tail -8 "$project/cold.time"
    fi
    printf '| %s | %s | %s | %s |\n' "$name" "$cold" "$warm" "$status" >> "$BUILD_FILE"
    "$PINNED_RUSTC" --edition 2024 -C opt-level=3 "$source/$name.rs" -o "$project/rust"
    "$project/rust" > "$project/rust.stdout"
    python3 "$source/$name.py" > "$project/python.stdout"
    if ! cmp -s "$project/rust.stdout" "$project/python.stdout"; then
        status='Rust/Python mismatch'
        failures=$((failures + 1))
    fi
    local binary="$project/target/incan/release/bin/$name"
    local commands=("\"$project/rust\"" "python3 \"$source/$name.py\"")
    if [[ -x "$binary" ]]; then
        if ! "$binary" > "$project/incan.stdout" 2> "$project/incan.stderr"; then
            status='Incan execution failed'
            failures=$((failures + 1))
        elif ! cmp -s "$project/incan.stdout" "$project/rust.stdout"; then
            status='Incan output mismatch'
            failures=$((failures + 1))
        fi
        if [[ "$status" == verified ]]; then
            commands=("\"$binary\"" "${commands[@]}")
        fi
    elif [[ "$status" == verified ]]; then
        status='missing Incan executable'
        failures=$((failures + 1))
    fi
    uptime > "$project/measurement-load.txt"
    hyperfine --shell=none --warmup 2 --runs "$RUNS" --export-json "$project/times.json" "${commands[@]}" > "$project/hyperfine.log" 2>&1
    local rust_index=0
    if [[ "$status" == verified ]]; then
        incan_time=$(mean_ms "$project/times.json" 0)
        rust_index=1
    fi
    local rust_time python_time
    rust_time=$(mean_ms "$project/times.json" "$rust_index")
    python_time=$(mean_ms "$project/times.json" "$((rust_index + 1))")
    if [[ "$status" == verified ]]; then
        flags=$(jq -nr --argjson incan "$incan_time" --argjson rust "$rust_time" --argjson old "$august" \
            '[if $incan > $old * 1.1 then ">10% August" else empty end, if $incan > $rust * 1.25 then ">25% Rust" else empty end] | if length == 0 then "none" else join(", ") end')
    fi
    printf '| %s | %s | %s | %s | %s | %s | %s |\n' "$name" "$incan_time" "$august" "$rust_time" "$python_time" "$status" "$flags" >> "$RESULTS_FILE"
}

# Optional names select a smaller measurement loop without changing the workload.
for row in 'compute fib 3' 'compute collatz 94' 'compute gcd 92' 'compute mandelbrot 113' 'compute nbody 20' 'compute primes 141' 'sorting quicksort 56' 'sorting mergesort 83'; do
    read -r area name august <<< "$row"
    if [[ $# -eq 0 || " $* " == *" $name "* ]]; then
        run_benchmark "$area" "$name" "$august"
    fi
done
cat "$RESULTS_FILE"
[[ "$failures" -eq 0 ]]
