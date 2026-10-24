#!/usr/bin/env bash
# Exercise example mode selection, project working directories, and negative status propagation without compilation.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/incan-examples-test.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
cat > "$scratch/incan" <<'EOF'
#!/usr/bin/env bash
printf '%s\t%s\t%s\n' "$1" "$2" "$PWD" >> "$EXAMPLES_CALLS"
if [[ "$1" == "${EXAMPLES_FAIL_MODE:-}" ]]; then
  exit 7
fi
EOF
chmod +x "$scratch/incan"
export INCAN_BIN="$scratch/incan" EXAMPLES_CALLS="$scratch/calls.tsv"
export INCAN_EXAMPLES_ONLY=examples/simple/hello.incn:examples/pro/vocab_markform/consumer/src/main.incn:examples/pro/vocab_scriptkit/consumer/src/main.incn:examples/pro/vocab_styleforge/consumer/src/main.incn:examples/advanced/package_features/consumer/src/main.incn:examples/advanced/async_await.incn
export INCAN_EXAMPLES_OUT="$scratch/results"
bash "$ROOT_DIR/scripts/run_examples.sh" > "$scratch/runner.log"
grep -Fqx "$(printf 'run\t%s/examples/simple/hello.incn\t%s' "$ROOT_DIR" "$ROOT_DIR")" "$EXAMPLES_CALLS"
for project in vocab_markform vocab_scriptkit vocab_styleforge; do
  grep -Fqx "$(printf 'check\t%s/examples/pro/%s/consumer/src/main.incn\t%s/examples/pro/%s/consumer' \
    "$ROOT_DIR" "$project" "$ROOT_DIR" "$project")" "$EXAMPLES_CALLS"
done
grep -Fqx "$(printf 'run\t%s/examples/advanced/package_features/consumer/src/main.incn\t%s/examples/advanced/package_features/consumer' \
  "$ROOT_DIR" "$ROOT_DIR")" "$EXAMPLES_CALLS"
grep -Fqx "$(printf 'run\t%s/examples/advanced/async_await.incn\t%s' "$ROOT_DIR" "$ROOT_DIR")" "$EXAMPLES_CALLS"
[[ "$(wc -l < "$EXAMPLES_CALLS" | tr -d ' ')" == 6 ]]

for mode in run check; do
  export EXAMPLES_FAIL_MODE="$mode"
  if bash "$ROOT_DIR/scripts/run_examples.sh" > "$scratch/runner.log"; then
    echo "runner suppressed a failing $mode command" >&2
    exit 1
  fi
  grep -q "$(printf '\t%s\tFAIL\t7\t' "$mode")" "$INCAN_EXAMPLES_OUT/examples.tsv"
done

export EXAMPLES_FAIL_MODE=inspect INCAN_EXAMPLES_ONLY=examples/pro/codegraph_importer/src/main.incn
: > "$EXAMPLES_CALLS"
if bash "$ROOT_DIR/scripts/run_examples.sh" > "$scratch/runner.log"; then
  echo "runner suppressed a failing snapshot export" >&2
  exit 1
fi
[[ "$(wc -l < "$EXAMPLES_CALLS" | tr -d ' ')" == 1 ]]
grep -q "$(printf '\trun\tFAIL\t7\t')" "$INCAN_EXAMPLES_OUT/examples.tsv"
echo "example runner orchestration passed"
