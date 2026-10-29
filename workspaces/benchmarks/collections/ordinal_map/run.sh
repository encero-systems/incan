#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

[[ "$keys" =~ ^[1-9][0-9]*$ && "$probes" =~ ^[1-9][0-9]*$ ]] || {
  echo '--keys and --probes must be positive integers' >&2
  exit 2
}

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
INCAN="${INCAN:-$(command -v incan || true)}"
if [[ ! -x "$INCAN" ]]; then
  echo "INCAN points to a non-executable path: $INCAN" >&2
  exit 2
fi

"$INCAN" run "$SCRIPT_DIR/ordinal_map.incn" --profile release -- "$keys" "$probes"
