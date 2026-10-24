#!/usr/bin/env bash
# Run example entry points through installed Oven; dependencies are prepared by Oven itself.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
INCAN_BIN="${INCAN_BIN:-incan}"
INCAN_BIN="$(command -v "$INCAN_BIN")"
TIMEOUT_SECS="${INCAN_EXAMPLES_TIMEOUT:-120}"
OUT="${INCAN_EXAMPLES_OUT:-$ROOT_DIR/target/examples}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
if [[ -z "$TIMEOUT_BIN" ]]; then
  echo "Install GNU timeout (coreutils) to bound example commands." >&2
  exit 2
fi

selected() {
  [[ -z "${INCAN_EXAMPLES_ONLY:-}" || ":$INCAN_EXAMPLES_ONLY:" == *":$1:"* ]]
}

check_only() {
  grep -Fqx "$1" examples/check-only.txt
}

working_directory() {
  # Project data files are relative to the nearest manifest; standalone examples run at the repository root.
  local directory
  directory="$(dirname "$1")"
  while [[ "$directory" != . ]]; do
    if [[ -f "$directory/loaf.toml" ]]; then
      printf '%s\n' "$directory"
      return
    fi
    directory="$(dirname "$directory")"
  done
  printf '.\n'
}

invoke_example() {
  local mode="$1" example="$2" directory="$3" log="$4"
  local arguments=()
  if [[ "$example" == examples/pro/codegraph_importer/src/main.incn ]]; then
    # This example consumes a real compiler export; retain the generated input beside its evidence logs.
    "$TIMEOUT_BIN" --kill-after=5 "$TIMEOUT_SECS" "$INCAN_BIN" inspect codegraph \
      "$ROOT_DIR/examples/simple/hello.incn" --format jsonl > "$OUT/codegraph.jsonl" 2> "$log" || return $?
    arguments=(-- "$OUT/codegraph.jsonl")
  fi
  (cd "$directory" && "$TIMEOUT_BIN" --kill-after=5 "$TIMEOUT_SECS" \
    "$INCAN_BIN" "$mode" "$ROOT_DIR/$example" "${arguments[@]}") >> "$log" 2>&1
}

printf 'example\tmode\tstatus\texit\tlog\n' > "$OUT/examples.tsv"
passed=0
failed=0
while IFS= read -r example; do
  selected "$example" || continue
  mode=run
  check_only "$example" && mode=check
  directory="$(working_directory "$example")"
  log="$OUT/${example//\//_}.log"
  code=0
  : > "$log"
  invoke_example "$mode" "$example" "$directory" "$log" || code=$?
  status=PASS
  if [[ "$code" -eq 0 ]]; then
    passed=$((passed + 1))
  else
    status=FAIL
    failed=$((failed + 1))
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$example" "$mode" "$status" "$code" "$log" >> "$OUT/examples.tsv"
  printf '%s %s %s (exit %s)\n' "$status" "$mode" "$example" "$code"
done < <(
  {
    while IFS= read -r source; do
      if grep -Eq '^(pub )?(async )?def main\(' "$source"; then
        printf '%s\n' "$source"
      fi
    done < <(find examples -type d -name target -prune -o -type f -name '*.incn' -print)
    sed '/^#/d; /^$/d' examples/check-only.txt
  } | LC_ALL=C sort -u
)
printf '%s passed, %s failed\n' "$passed" "$failed"
[[ "$passed" -gt 0 && "$failed" -eq 0 ]]
