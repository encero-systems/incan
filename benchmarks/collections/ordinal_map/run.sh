#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
DEFAULT_KEYS=100000
DEFAULT_PROBES=1000000
PYTHON_BIN="${PYTHON:-python3}"

original_args=("$@")
keys="$DEFAULT_KEYS"
probes="$DEFAULT_PROBES"
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --keys)
      keys="$2"
      shift 2
      ;;
    --keys=*)
      keys="${1#*=}"
      shift
      ;;
    --probes)
      probes="$2"
      shift 2
      ;;
    --probes=*)
      probes="${1#*=}"
      shift
      ;;
    *)
      shift
      ;;
  esac
done

run_python_script() {
  local script="$1"
  if [[ "${#original_args[@]}" -eq 0 ]]; then
    "$PYTHON_BIN" "$script"
  else
    "$PYTHON_BIN" "$script" "${original_args[@]}"
  fi
}

echo "== Python builtin dict =="
run_python_script "$SCRIPT_DIR/builtin_dict.py"

echo ""
echo "== Python fastconstmap =="
status=0
run_python_script "$SCRIPT_DIR/fastconstmap_lookup.py" || status=$?
if [[ "$status" != "0" && "$status" != "77" ]]; then
  exit "$status"
fi

echo ""
echo "== Incan OrdinalMap =="
if [[ -z "${INCAN:-}" ]]; then
  cargo build --release --quiet --manifest-path "$PROJECT_ROOT/Cargo.toml"
  INCAN="$PROJECT_ROOT/target/release/incan"
elif [[ ! -x "$INCAN" ]]; then
  echo "INCAN points to a non-executable path: $INCAN" >&2
  exit 2
fi

incan_source="$SCRIPT_DIR/ordinal_map.incn"
if [[ "$keys" != "$DEFAULT_KEYS" || "$probes" != "$DEFAULT_PROBES" ]]; then
  generated_dir="$(mktemp -d "${TMPDIR:-/tmp}/incan-ordinal-map-bench-${keys}-${probes}.XXXXXX")"
  trap 'rm -rf "$generated_dir"' EXIT
  incan_source="$generated_dir/ordinal_map.incn"
  sed \
    -e "s/key_count = 100_000/key_count = ${keys}/" \
    -e "s/probe_count = 1_000_000/probe_count = ${probes}/" \
    "$SCRIPT_DIR/ordinal_map.incn" > "$incan_source"
fi

"$INCAN" build "$incan_source"
"$PROJECT_ROOT/target/incan/.cargo-target/release/ordinal_map"
