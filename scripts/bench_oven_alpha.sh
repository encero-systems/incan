#!/usr/bin/env bash

# Measure one compiler-shipped Oven Alpha workload without making Cargo part of the normal-command benchmark.
#
# A release archive supplies the supported Loafs. This harness starts with an empty Oven store, records the
# first normal command (Loaf materialization plus its caller-owned native bake), then records unchanged warm commands.
# A deterministic failing Cargo guard is mandatory: a successful normal stage proves that it did not launch Cargo.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  bash scripts/bench_oven_alpha.sh \
    --incan PATH --release-identity TEXT --checkout-revision TEXT --workload build|run|test --source PATH \
  --incan-home PATH --output PATH --cargo-guard-dir PATH [options]

The selected source must be in the documented compiler-shipped Oven Alpha envelope. The harness requires an empty
INCAN_HOME, records the first normal command separately from unchanged warm repeats, and never invokes Cargo.

Options:
  --repetitions N                 Unchanged warm repeats after first materialization (default: 2; minimum: 1)
  --release-identity TEXT         Release archive or CI artifact identity for the selected compiler (required)
  --checkout-revision TEXT        Revision of the checkout that supplied the benchmark fixture (required)
  --clean-worktree-source PATH    Identical fixture in a clean checkout for a final reuse run
  --cargo-guard-dir PATH          Directory containing a `cargo` executable that exits exactly 97 (required)
  --max-physical-bytes BYTES      Aggregate Oven physical-store policy (default: 3221225472)
  --max-domain-physical-bytes N   Per-domain Oven physical-store policy (default: 1073741824)
  --max-domain-logical-bytes N    Per-domain Oven logical-artifact policy (default: 805306368)
  -h, --help                      Show this help
EOF
}

incan=""
workload=""
source_path=""
incan_home=""
output_dir=""
release_identity=""
checkout_revision=""
clean_worktree_source=""
repetitions=2
cargo_guard_dir=""
max_physical_bytes=3221225472
max_domain_physical_bytes=1073741824
max_domain_logical_bytes=805306368

while [ "$#" -gt 0 ]; do
    case "$1" in
        --incan) incan=${2:?--incan requires a path}; shift 2 ;;
        --workload) workload=${2:?--workload requires build, run, or test}; shift 2 ;;
        --source) source_path=${2:?--source requires a path}; shift 2 ;;
        --incan-home) incan_home=${2:?--incan-home requires a path}; shift 2 ;;
        --output) output_dir=${2:?--output requires a path}; shift 2 ;;
        --release-identity) release_identity=${2:?--release-identity requires text}; shift 2 ;;
        --checkout-revision) checkout_revision=${2:?--checkout-revision requires text}; shift 2 ;;
        --clean-worktree-source) clean_worktree_source=${2:?--clean-worktree-source requires a path}; shift 2 ;;
        --repetitions) repetitions=${2:?--repetitions requires a number}; shift 2 ;;
        --cargo-guard-dir) cargo_guard_dir=${2:?--cargo-guard-dir requires a directory}; shift 2 ;;
        --max-physical-bytes) max_physical_bytes=${2:?--max-physical-bytes requires bytes}; shift 2 ;;
        --max-domain-physical-bytes) max_domain_physical_bytes=${2:?--max-domain-physical-bytes requires bytes}; shift 2 ;;
        --max-domain-logical-bytes) max_domain_logical_bytes=${2:?--max-domain-logical-bytes requires bytes}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

for required in incan workload source_path incan_home output_dir release_identity checkout_revision cargo_guard_dir; do
    if [ -z "${!required}" ]; then
        echo "missing required --${required//_/-}" >&2
        usage >&2
        exit 2
    fi
done

case "$workload" in
    build|run|test) ;;
    *) echo "--workload must be build, run, or test" >&2; exit 2 ;;
esac

case "$repetitions" in
    ''|*[!0-9]*) echo "--repetitions must be an integer of at least 1" >&2; exit 2 ;;
esac
if [ "$repetitions" -lt 1 ]; then
    echo "--repetitions must be at least 1 to prove unchanged reuse" >&2
    exit 2
fi

[ -x "$incan" ] || { echo "--incan is not executable: $incan" >&2; exit 2; }
[ -e "$source_path" ] || { echo "--source does not exist: $source_path" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "required executable is unavailable: python3" >&2; exit 2; }
command -v uname >/dev/null 2>&1 || { echo "required executable is unavailable: uname" >&2; exit 2; }
[ -d "$cargo_guard_dir" ] || { echo "--cargo-guard-dir is not a directory: $cargo_guard_dir" >&2; exit 2; }
[ -x "$cargo_guard_dir/cargo" ] || { echo "--cargo-guard-dir must contain an executable cargo guard" >&2; exit 2; }
if [ -n "$clean_worktree_source" ] && [ ! -e "$clean_worktree_source" ]; then
    echo "--clean-worktree-source does not exist: $clean_worktree_source" >&2
    exit 2
fi
set +e
"$cargo_guard_dir/cargo" --version >/dev/null 2>&1
cargo_guard_probe_status=$?
set -e
[ "$cargo_guard_probe_status" -eq 97 ] \
    || { echo "--cargo-guard-dir/cargo must exit exactly 97 when probed; got $cargo_guard_probe_status" >&2; exit 2; }

case "$max_physical_bytes:$max_domain_physical_bytes:$max_domain_logical_bytes" in
    *[!0-9:]*|::*|:*|*:) echo "storage limits must be whole-byte integers" >&2; exit 2 ;;
esac
if [ "$max_physical_bytes" -eq 0 ] || [ "$max_domain_physical_bytes" -eq 0 ] || [ "$max_domain_logical_bytes" -eq 0 ]; then
    echo "storage limits must be greater than zero" >&2
    exit 2
fi
if [ "$max_domain_physical_bytes" -gt "$max_physical_bytes" ]; then
    echo "per-domain physical policy cannot exceed aggregate policy" >&2
    exit 2
fi

sha256_file() {
    python3 - "$1" <<'PY'
import hashlib
import pathlib
import sys

print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

source_sha256="$(sha256_file "$source_path")"
clean_worktree_source_sha256=""
if [ -n "$clean_worktree_source" ]; then
    clean_worktree_source_sha256="$(sha256_file "$clean_worktree_source")"
    if [ "$clean_worktree_source_sha256" != "$source_sha256" ]; then
        echo "--clean-worktree-source must have identical bytes to --source for a reuse measurement" >&2
        exit 2
    fi
fi

store_root="$incan_home/oven/store/v1"
if [ -d "$store_root/entries" ] && find "$store_root/entries" -mindepth 1 -maxdepth 1 -type d | grep -q .; then
    echo "--incan-home must start with an empty Oven store so first materialization is attributable: $store_root" >&2
    exit 2
fi

mkdir -p "$output_dir" "$store_root"
phase_tsv="$output_dir/phases.tsv"
junction_tsv="$output_dir/storage-junctions.tsv"
storage_junctions_dir="$output_dir/storage-junctions"
: > "$phase_tsv"
: > "$junction_tsv"
mkdir -p "$storage_junctions_dir"

now_ms() {
    python3 -c 'import time; print(time.monotonic_ns() // 1_000_000)'
}

# Keep one monotonic envelope around the complete benchmark, including each storage inspection. Individual command
# phases remain the actionable cold/warm timings; this envelope makes the report auditable without asking readers to
# reconstruct wall-clock cost from a TSV that may also contain inspection retries.
benchmark_started_ms="$(now_ms)"

run_stage() {
    local stage=$1
    shift
    local started finished status
    started=$(now_ms)
    set +e
    # The measured consumer must use the selected compiler's shipped runtime and Loafs. Developer-shell
    # source overrides intentionally remain supported elsewhere, but inheriting one from a different checkout would
    # turn this packaged-toolchain benchmark into an ambient-state measurement and make its receipt incompatible
    # with the compiler-owned Loaf.
    env -u INCAN_SOURCE_ROOT -u INCAN_STDLIB -u INCAN_STDLIB_DIR -u INCAN_STDLIB_PATH \
        -u INCAN_TOOLCHAIN_CRATES_DIR -u INCAN_SDK_INVENTORY \
        -u INCAN_INTERNAL_SDK_PROVIDER_STORE -u INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE \
        -u INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT -u INCAN_INTERNAL_OVEN_LOAF_EXECUTION \
        -u INCAN_INTERNAL_OVEN_RUNTIME_ROOT \
        INCAN_HOME="$incan_home" \
        INCAN_OVEN_MAX_PHYSICAL_BYTES="$max_physical_bytes" \
        INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES="$max_domain_physical_bytes" \
        INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES="$max_domain_logical_bytes" \
        PATH="${cargo_guard_dir:+$cargo_guard_dir:}$PATH" \
        "$@" >"$output_dir/$stage.log" 2>&1
    status=$?
    set -e
    finished=$(now_ms)
    printf '%s\t%s\t%s\n' "$stage" "$((finished - started))" "$status" >> "$phase_tsv"
    return "$status"
}

# Store accounting is a first-class benchmark result, not a final afterthought.  Capture it after every normal
# command so a retained report distinguishes initial state, first materialization, and each unchanged reuse without
# treating caller-owned logs as immutable Oven artifacts.
capture_storage_junction() {
    local junction=$1
    local junction_dir="$storage_junctions_dir/$junction"
    mkdir -p "$junction_dir"
    if ! run_stage "store_snapshot_$junction" "$incan" oven store inspect --store "$store_root" --format json; then
        echo "Oven store inspection failed at benchmark junction $junction" >&2
        return 1
    fi
    cp "$output_dir/store_snapshot_$junction.log" "$junction_dir/store-inspection.json"
    # A nested normal command may remove a temporary file while recursive `du` walks the caller-owned output tree.
    # Retry the physical snapshot so a transient vanished-file race cannot turn a successful benchmark into a
    # missing-evidence failure.
    local disk_status=1
    for _ in 1 2 3 4 5; do
        if du -sk "$store_root" "$output_dir" > "$junction_dir/disk-usage-kib.tsv" 2>/dev/null; then
            disk_status=0
            break
        fi
        sleep 1
    done
    if [ "$disk_status" -ne 0 ]; then
        echo "failed to capture stable physical disk usage at benchmark junction $junction" >&2
        return 1
    fi
    printf '%s\n' "$junction" >> "$junction_tsv"
}

case "$workload" in
    build) normal_args=(build "$source_path" --report json) ;;
    run) normal_args=(run "$source_path") ;;
    test) normal_args=(test --verbose "$source_path") ;;
esac

capture_storage_junction initial

if ! run_stage first_materialization "$incan" "${normal_args[@]}"; then
    echo "the first normal command failed; the source is unsupported or the release archive is incomplete" >&2
    sed -n '1,80p' "$output_dir/first_materialization.log" >&2
    exit 1
fi
capture_storage_junction after_first_materialization

for run_index in $(seq 1 "$repetitions"); do
    if ! run_stage "warm_repeat_$run_index" "$incan" "${normal_args[@]}"; then
        echo "unchanged warm command $run_index failed" >&2
        sed -n '1,80p' "$output_dir/warm_repeat_$run_index.log" >&2
        exit 1
    fi
    capture_storage_junction "after_warm_repeat_$run_index"
done

if [ -n "$clean_worktree_source" ]; then
    case "$workload" in
        build) clean_worktree_args=(build "$clean_worktree_source" --report json) ;;
        run) clean_worktree_args=(run "$clean_worktree_source") ;;
        test) clean_worktree_args=(test --verbose "$clean_worktree_source") ;;
    esac
    if ! run_stage clean_worktree_reuse "$incan" "${clean_worktree_args[@]}"; then
        echo "clean-worktree reuse command failed" >&2
        sed -n '1,80p' "$output_dir/clean_worktree_reuse.log" >&2
        exit 1
    fi
    capture_storage_junction after_clean_worktree_reuse
fi

# Preserve the former final-inspection path for callers that consume the compact store summary while keeping the
# complete per-junction sequence under `storage-junctions/`.
final_junction="after_warm_repeat_$repetitions"
if [ -n "$clean_worktree_source" ]; then
    final_junction="after_clean_worktree_reuse"
fi
cp "$storage_junctions_dir/$final_junction/store-inspection.json" "$output_dir/store_inspect.log"

"$incan" --version >"$output_dir/incan-version.txt"
uname -a >"$output_dir/uname.txt"
benchmark_finished_ms="$(now_ms)"
benchmark_wall_clock_ms=$((benchmark_finished_ms - benchmark_started_ms))

python3 - "$output_dir" "$workload" "$source_path" "$store_root" "$cargo_guard_dir" "$cargo_guard_probe_status" \
    "$max_physical_bytes" "$max_domain_physical_bytes" "$max_domain_logical_bytes" "$release_identity" \
    "$checkout_revision" "$source_sha256" "$clean_worktree_source" "$clean_worktree_source_sha256" \
    "$benchmark_wall_clock_ms" <<'PY'
import json
import pathlib
import sys

output = pathlib.Path(sys.argv[1])
phases = []
for line in (output / "phases.tsv").read_text().splitlines():
    name, duration_ms, exit_code = line.split("\t")
    phases.append({"name": name, "duration_ms": int(duration_ms), "exit_code": int(exit_code)})

storage_junctions = []
for name in (output / "storage-junctions.tsv").read_text().splitlines():
    junction = output / "storage-junctions" / name
    raw_disk_usage = []
    for line in (junction / "disk-usage-kib.tsv").read_text().splitlines():
        kib, path = line.split(maxsplit=1)
        raw_disk_usage.append({"path": path, "kib": int(kib), "bytes": int(kib) * 1024})
    storage_junctions.append({
        "name": name,
        "inspection": json.loads((junction / "store-inspection.json").read_text()),
        "raw_disk_usage_kib": raw_disk_usage,
        "reports": {
            "inspection": f"storage-junctions/{name}/store-inspection.json",
            "disk_usage": f"storage-junctions/{name}/disk-usage-kib.tsv",
        },
    })

final_inspection = storage_junctions[-1]["inspection"]

report = {
    "schema_version": 4,
    "purpose": "Oven Alpha packaged-unit first-materialization and warm normal-command evidence",
    "machine": {"uname": (output / "uname.txt").read_text().strip()},
    "toolchain": {
        "incan": (output / "incan-version.txt").read_text().strip(),
        "release_identity": sys.argv[10],
    },
    "provenance": {"checkout_revision": sys.argv[11]},
    "workload": {
        "kind": sys.argv[2],
        "source": sys.argv[3],
        "source_sha256": sys.argv[12],
        "clean_worktree_source": sys.argv[13] or None,
        "clean_worktree_source_sha256": sys.argv[14] or None,
    },
    "cargo_guard": {
        "required": True,
        "directory": sys.argv[5],
        "probe_exit_code": int(sys.argv[6]),
        "verdict": "successful normal stages imply that Cargo was not launched",
    },
    "store": {
        "root": sys.argv[4],
        "max_physical_bytes": int(sys.argv[7]),
        "max_domain_physical_bytes": int(sys.argv[8]),
        "max_domain_logical_bytes": int(sys.argv[9]),
        "inspection": final_inspection,
    },
    "timing": {
        "wall_clock_ms": int(sys.argv[15]),
        "first_materialization_ms": next(
            phase["duration_ms"] for phase in phases if phase["name"] == "first_materialization"
        ),
        "warm_repeat_total_ms": sum(
            phase["duration_ms"] for phase in phases if phase["name"].startswith("warm_repeat_")
        ),
        "clean_worktree_reuse_ms": next(
            (phase["duration_ms"] for phase in phases if phase["name"] == "clean_worktree_reuse"),
            None,
        ),
        "phase_source": "phases.tsv",
    },
    "phases": phases,
    "storage_junctions": storage_junctions,
    "logs": {phase["name"]: f"{phase['name']}.log" for phase in phases},
}
(output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
PY

echo "Oven Alpha benchmark evidence: $output_dir/report.json"
