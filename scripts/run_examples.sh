#!/usr/bin/env bash
set -euo pipefail

# Requires Bash. Do not run with `sh` (POSIX sh does not support process substitution `done < <(...)` used below).
#Use: `bash scripts/run_examples.sh` or `make examples`.

# Smoke-test examples:
# - Pre-build nested example library projects (`loaf.toml` + `src/lib.incn`)
# - Typecheck every example file under examples/ (recursively)
# - Run only entrypoints (files that define `def main(...)`)
# - Skip long-running examples (web examples) and anything that times out
#
# Configuration:
#   INCAN_BIN               path to the incan binary (default: $CARGO_TARGET_DIR/release/incan if present, else `incan`)
#   INCAN_EXAMPLES_TIMEOUT  per-example timeout in seconds for `incan run` (default: 30)
#   INCAN_EXAMPLES_ONLY     colon-separated repository-relative example paths to run (default: every example)
#   INCAN_EXAMPLES_TIMEOUT_MODE
#                           `skip` records timed-out runnable examples as skipped (default); `fail` makes one fail
#   INCAN_EXAMPLES_REQUIRE_CARGO_FREE
#                           `1` installs an exit-97 Cargo tripwire for this runner and fails on any invocation

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Honour CARGO_TARGET_DIR. A caller that redirects cargo's output — a worktree under a storage budget, a cache shared
# between worktrees — otherwise has cargo writing one binary while this script silently runs an older one from the
# default location, or falls through to whatever `incan` is on PATH.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
INCAN_BIN="${INCAN_BIN:-}"
if [[ -z "$INCAN_BIN" ]]; then
  if [[ -x "$TARGET_DIR/release/incan" ]]; then
    INCAN_BIN="$TARGET_DIR/release/incan"
  else
    INCAN_BIN="incan"
  fi
fi
if [[ "$INCAN_BIN" == ./* ]]; then
  INCAN_BIN="$ROOT_DIR/${INCAN_BIN#./}"
fi

TIMEOUT_SECS="${INCAN_EXAMPLES_TIMEOUT:-30}"
ONLY_EXAMPLES="${INCAN_EXAMPLES_ONLY:-}"
TIMEOUT_MODE="${INCAN_EXAMPLES_TIMEOUT_MODE:-skip}"
REQUIRE_CARGO_FREE="${INCAN_EXAMPLES_REQUIRE_CARGO_FREE:-0}"
LOG_DIR="$(mktemp -d "${TMPDIR:-/tmp}/incan-example-logs.XXXXXX")"
trap 'rm -rf "$LOG_DIR"' EXIT

if [[ "$TIMEOUT_MODE" != "skip" && "$TIMEOUT_MODE" != "fail" ]]; then
  echo "INCAN_EXAMPLES_TIMEOUT_MODE must be 'skip' or 'fail', got: $TIMEOUT_MODE" >&2
  exit 2
fi

CARGO_GUARD_LOG=""
if [[ "$REQUIRE_CARGO_FREE" == "1" ]]; then
  guard_dir="$LOG_DIR/cargo-guard"
  mkdir -p "$guard_dir"
  CARGO_GUARD_LOG="$guard_dir/invocations.log"
  cat > "$guard_dir/cargo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "${INCAN_OVEN_CARGO_GUARD_LOG:?missing INCAN_OVEN_CARGO_GUARD_LOG}"
exit 97
EOF
  chmod +x "$guard_dir/cargo"
  : > "$CARGO_GUARD_LOG"
  export PATH="$guard_dir:$PATH"
  export INCAN_OVEN_CARGO_GUARD_LOG="$CARGO_GUARD_LOG"
fi

echo "Incan examples runner"
echo "  incan:    $INCAN_BIN"
echo "  timeout: ${TIMEOUT_SECS}s (only for runnable examples)"
echo ""

log_file_for() {
  local kind="$1"
  local path="$2"
  local safe="${path//\//_}"
  safe="${safe// /_}"
  printf '%s/%s-%s.log' "$LOG_DIR" "$kind" "$safe"
}

print_log() {
  local log_file="$1"
  if [[ -s "$log_file" ]]; then
    sed 's/^/  | /' "$log_file"
  fi
}

is_selected_example() {
  local file="$1"
  if [[ -z "$ONLY_EXAMPLES" ]]; then
    return 0
  fi
  local old_ifs="$IFS"
  local candidate
  IFS=':'
  for candidate in $ONLY_EXAMPLES; do
    if [[ "$file" == "$candidate" ]]; then
      IFS="$old_ifs"
      return 0
    fi
  done
  IFS="$old_ifs"
  return 1
}

selection_requires_project() {
  local project_dir="$1"
  if [[ -z "$ONLY_EXAMPLES" ]]; then
    return 0
  fi
  local old_ifs="$IFS"
  local candidate
  IFS=':'
  for candidate in $ONLY_EXAMPLES; do
    case "$candidate" in
      "$project_dir"/*)
        IFS="$old_ifs"
        return 0
        ;;
    esac
  done
  IFS="$old_ifs"
  return 1
}

python_run_with_timeout() {
  # Usage: python_run_with_timeout <cmd...>
  python3 -c 'import os, subprocess, sys
timeout = float(os.environ.get("INCAN_EXAMPLES_TIMEOUT", "30"))
try:
  p = subprocess.run(sys.argv[1:], timeout=timeout)
  sys.exit(p.returncode)
except subprocess.TimeoutExpired:
  sys.exit(124)
' "$@"
}

is_runnable_entrypoint() {
  # Runnable if it defines `def main(...)`
  local file="$1"
  # Use a regex compatible with both BSD grep (macOS) and GNU grep.
  # `[(]` matches a literal '(' without triggering ERE group parsing edge cases.
  grep -Eq '^[[:space:]]*def[[:space:]]+main[[:space:]]*[(]' "$file"
}

is_check_only_example() {
  # These RFC 081 conformance consumers deliberately have no runtime lowering hook (see their READMEs).
  case "$1" in
    examples/pro/vocab_markform/consumer/src/main.incn|\
    examples/pro/vocab_scriptkit/consumer/src/main.incn|\
    examples/pro/vocab_styleforge/consumer/src/main.incn)
      return 0
      ;;
  esac
  return 1
}

should_skip_run() {
  local file="$1"
  # Skip web examples (typically start a server)
  if [[ "$file" == examples/web/* ]]; then
    return 0
  fi
  return 1
}

bake_example_project() {
  local project_dir="$1"
  echo "==> oven-bake: $project_dir"
  local bake_log_file
  bake_log_file="$(log_file_for "oven-bake" "$project_dir")"
  if (cd "$project_dir" && INCAN_NO_BANNER=1 "$INCAN_BIN" oven bake --project . >"$bake_log_file" 2>&1); then
    :
  else
    echo "FAILED: oven bake --project $project_dir"
    print_log "$bake_log_file"
    failed_items+=("oven bake --project $project_dir")
    failed=$((failed + 1))
  fi
}

prebake_example_providers() {
  local manifest
  while IFS= read -r manifest; do
    [[ -z "$manifest" ]] && continue
    local project_dir
    project_dir="$(dirname "$manifest")"
    local consumer_dir
    consumer_dir="$(dirname "$project_dir")/consumer"
    if [[ "$(basename "$project_dir")" != "producer" || ! -d "$consumer_dir" ]]; then
      continue
    fi
    if selection_requires_project "$project_dir" || selection_requires_project "$consumer_dir"; then
      bake_example_project "$project_dir"
    fi
  done < <(
    find examples \
      \( -type d -name target -o -type d -name __pycache__ \) -prune -o \
      -type f -name 'loaf.toml' -print | sort
  )
}

prebuild_example_libraries() {
  # Consumers can import a package Loaf only after the sibling provider has published it. This must be a separate
  # pass because the stable manifest order visits `consumer` before `producer`.
  prebake_example_providers

  local manifest
  while IFS= read -r manifest; do
    [[ -z "$manifest" ]] && continue
    local project_dir
    project_dir="$(dirname "$manifest")"
    if ! selection_requires_project "$project_dir"; then
      continue
    fi

    # Projects with Rust inspection or an imported `pub::` library need an explicit bake before normal build or run.
    # Provider Loafs were published in the preceding pass, so a consumer bake now imports a complete closure.
    local needs_explicit_bake=false
    if grep -q -e '^\[rust-dependencies\]' -e '^\[dependencies\]' "$manifest"; then
      needs_explicit_bake=true
    fi
    # Their producer still needs publication, but frontend-only conformance consumers cannot be baked.
    if [[ "$needs_explicit_bake" == true ]] && ! is_check_only_example "$project_dir/src/main.incn"; then
      bake_example_project "$project_dir"
    fi

    if [[ ! -f "$project_dir/src/lib.incn" && ! -f "$project_dir/src/lib.incan" ]]; then
      continue
    fi

    echo "==> build-lib: $project_dir"
    local log_file
    log_file="$(log_file_for "build-lib" "$project_dir")"
    if (cd "$project_dir" && INCAN_NO_BANNER=1 "$INCAN_BIN" build --lib >"$log_file" 2>&1); then
      :
    else
      echo "FAILED: build --lib $project_dir"
      print_log "$log_file"
      failed_items+=("build --lib $project_dir")
      failed=$((failed + 1))
    fi

  done < <(
    find examples \
      \( -type d -name target -o -type d -name __pycache__ \) -prune -o \
      -type f -name 'loaf.toml' -print | sort
  )
}

checked=0
ran=0
skipped=0
failed=0
timed_out=0
failed_items=()

found_any=0

prebuild_example_libraries

# Note: macOS ships Bash 3.2 by default; avoid `mapfile` (Bash 4+).
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  if ! is_selected_example "$f"; then
    continue
  fi
  found_any=1
  if is_runnable_entrypoint "$f" && ! should_skip_run "$f" && ! is_check_only_example "$f"; then
    # For runnable entrypoints, `incan run` already performs compile-time validation,
    # so we avoid a redundant prior `--check`.
    echo "==> run:   $f"
    log_file="$(log_file_for "run" "$f")"
    set +e
    INCAN_EXAMPLES_TIMEOUT="$TIMEOUT_SECS" python_run_with_timeout "$INCAN_BIN" run "$f" >"$log_file" 2>&1
    rc=$?
    set -e

    if [[ "$rc" -eq 0 ]]; then
      checked=$((checked + 1))
      ran=$((ran + 1))
    elif [[ "$rc" -eq 124 ]]; then
      if [[ "$TIMEOUT_MODE" == "fail" ]]; then
        echo "FAILED: run $f (timeout after ${TIMEOUT_SECS}s)"
        print_log "$log_file"
        failed_items+=("run $f (timeout after ${TIMEOUT_SECS}s)")
        failed=$((failed + 1))
      else
        echo "==> skip:  $f (timeout after ${TIMEOUT_SECS}s)"
        print_log "$log_file"
        timed_out=$((timed_out + 1))
      fi
    else
      echo "FAILED: run $f (exit $rc)"
      print_log "$log_file"
      failed_items+=("run $f")
      failed=$((failed + 1))
    fi
    continue
  fi

  echo "==> check: $f"
  log_file="$(log_file_for "check" "$f")"
  if "$INCAN_BIN" --check "$f" >"$log_file" 2>&1; then
    checked=$((checked + 1))
    if is_check_only_example "$f"; then
      echo "==> skip:  $f (check-only: no runtime lowering hook)"
      skipped=$((skipped + 1))
    elif is_runnable_entrypoint "$f" && should_skip_run "$f"; then
      echo "==> skip:  $f (excluded: long-running)"
      skipped=$((skipped + 1))
    fi
  else
    echo "FAILED: check $f"
    print_log "$log_file"
    failed_items+=("check $f")
    failed=$((failed + 1))
  fi
done < <(
  find examples \
    \( -type d -name target -o -type d -name __pycache__ \) -prune -o \
    -type f \( -name '*.incn' -o -name '*.incan' \) -print | sort
)

if [[ "$found_any" -eq 0 ]]; then
  echo "No example files found under ./examples"
  exit 1
fi

echo ""
echo "Summary:"
echo "  checked:   $checked"
echo "  ran:       $ran"
echo "  skipped:   $skipped"
echo "  timed out: $timed_out"
echo "  failed:    $failed"

if [[ "$failed" -ne 0 ]]; then
  echo ""
  echo "Failed examples:"
  for item in "${failed_items[@]}"; do
    echo "  - $item"
  done
  exit 1
fi

if [[ "$REQUIRE_CARGO_FREE" == "1" && -s "$CARGO_GUARD_LOG" ]]; then
  echo ""
  echo "Cargo was invoked during a Cargo-free examples run:" >&2
  sed 's/^/  | /' "$CARGO_GUARD_LOG" >&2
  exit 1
fi
