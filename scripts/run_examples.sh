#!/usr/bin/env bash
set -euo pipefail

# Requires Bash. Do not run with `sh` (POSIX sh does not support process substitution `done < <(...)` used below).
#Use: `bash scripts/run_examples.sh` or `make examples`.

# Smoke-test examples:
# - Pre-build nested example library projects (`incan.toml` + `src/lib.incn`)
# - Typecheck every example file under examples/ (recursively)
# - Run only entrypoints (files that define `def main(...)`)
# - Skip long-running examples (web examples) and anything that times out
#
# Configuration:
#   INCAN_BIN               path to the incan binary (default: ./target/release/incan if present, else `incan`)
#   INCAN_EXAMPLES_TIMEOUT  per-example timeout in seconds for `incan run` (default: 5)

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

INCAN_BIN="${INCAN_BIN:-}"
if [[ -z "$INCAN_BIN" ]]; then
  if [[ -x "./target/release/incan" ]]; then
    INCAN_BIN="./target/release/incan"
  else
    INCAN_BIN="incan"
  fi
fi
if [[ "$INCAN_BIN" == ./* ]]; then
  INCAN_BIN="$ROOT_DIR/${INCAN_BIN#./}"
fi

TIMEOUT_SECS="${INCAN_EXAMPLES_TIMEOUT:-5}"

echo "Incan examples runner"
echo "  incan:    $INCAN_BIN"
echo "  timeout: ${TIMEOUT_SECS}s (only for runnable examples)"
echo ""

python_run_with_timeout() {
  # Usage: python_run_with_timeout <cmd...>
  python3 -c 'import os, subprocess, sys
timeout = float(os.environ.get("INCAN_EXAMPLES_TIMEOUT", "5"))
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

should_skip_run() {
  local file="$1"
  # Skip web examples (typically start a server)
  if [[ "$file" == examples/web/* ]]; then
    return 0
  fi
  return 1
}

prebuild_example_libraries() {
  local manifest
  while IFS= read -r manifest; do
    [[ -z "$manifest" ]] && continue
    local project_dir
    project_dir="$(dirname "$manifest")"
    if [[ ! -f "$project_dir/src/lib.incn" && ! -f "$project_dir/src/lib.incan" ]]; then
      continue
    fi

    echo "==> build-lib: $project_dir"
    if (cd "$project_dir" && INCAN_NO_BANNER=1 "$INCAN_BIN" build --lib >/dev/null); then
      :
    else
      echo "FAILED: build --lib $project_dir"
      failed=$((failed + 1))
    fi
  done < <(
    find examples \
      \( -type d -name target -o -type d -name __pycache__ \) -prune -o \
      -type f -name 'incan.toml' -print | sort
  )
}

checked=0
ran=0
skipped=0
failed=0
timed_out=0

found_any=0

prebuild_example_libraries

# Note: macOS ships Bash 3.2 by default; avoid `mapfile` (Bash 4+).
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  found_any=1
  if is_runnable_entrypoint "$f" && ! should_skip_run "$f"; then
    # For runnable entrypoints, `incan run` already performs compile-time validation,
    # so we avoid a redundant prior `--check`.
    echo "==> run:   $f"
    set +e
    INCAN_EXAMPLES_TIMEOUT="$TIMEOUT_SECS" python_run_with_timeout "$INCAN_BIN" run "$f" >/dev/null
    rc=$?
    set -e

    if [[ "$rc" -eq 0 ]]; then
      checked=$((checked + 1))
      ran=$((ran + 1))
    elif [[ "$rc" -eq 124 ]]; then
      echo "==> skip:  $f (timeout after ${TIMEOUT_SECS}s)"
      timed_out=$((timed_out + 1))
    else
      echo "FAILED: run $f (exit $rc)"
      failed=$((failed + 1))
    fi
    continue
  fi

  echo "==> check: $f"
  if "$INCAN_BIN" --check "$f" >/dev/null; then
    checked=$((checked + 1))
    if is_runnable_entrypoint "$f" && should_skip_run "$f"; then
      echo "==> skip:  $f (excluded: long-running)"
      skipped=$((skipped + 1))
    fi
  else
    echo "FAILED: check $f"
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
  exit 1
fi
