#!/usr/bin/env bash
set -euo pipefail

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

EXAMPLE_DIR="workspaces/docs-site/docs/_snippets/language/examples"
SOURCE_VERIFIED_PROJECTS=(
  "examples/advanced/async_worker_pipeline.incn"
  "examples/advanced/library_package/producer/src/lib.incn"
  "examples/advanced/library_package/consumer/src/main.incn"
  "examples/advanced/typed_data_processor/src/main.incn"
  "examples/advanced/typed_data_processor/tests/test_transform.incn"
)
FORMAT_ONLY_EXAMPLES=(
  "examples/advanced/library_package/producer/src/pricing.incn"
  "examples/advanced/typed_data_processor/src/domain.incn"
  "examples/advanced/typed_data_processor/src/transform.incn"
  "examples/rust_interop_regex/main.incn"
  "examples/web/hello_web.incn"
)

format_targets=()

verified_count=0
while IFS= read -r example; do
  echo "==> check docs example: $example"
  INCAN_NO_BANNER=1 "$INCAN_BIN" --check "$example"
  format_targets+=("$example")
  verified_count=$((verified_count + 1))
done < <(find "$EXAMPLE_DIR" -maxdepth 1 -type f -name 'verified_*.incn' -print | sort)
if [[ "$verified_count" -eq 0 ]]; then
  echo "No verified documentation examples found in $EXAMPLE_DIR" >&2
  exit 1
fi

for example in "${SOURCE_VERIFIED_PROJECTS[@]}"; do
  echo "==> check source-verified tutorial project: $example"
  INCAN_NO_BANNER=1 "$INCAN_BIN" --check "$example"
  format_targets+=("$example")
done

format_targets+=("${FORMAT_ONLY_EXAMPLES[@]}")

for example in "${format_targets[@]}"; do
  echo "==> check canonical formatting: $example"
  INCAN_NO_BANNER=1 "$INCAN_BIN" fmt --check "$example"
done

if [[ "${INCAN_DOCS_BUILD_WEB:-0}" == "1" ]]; then
  web_count=0
  while IFS= read -r example; do
    echo "==> build docs web example: $example"
    if [[ "${INCAN_DOCS_OFFLINE:-0}" == "1" ]]; then
      INCAN_NO_BANNER=1 "$INCAN_BIN" build "$example" --offline
    else
      INCAN_NO_BANNER=1 "$INCAN_BIN" build "$example"
    fi
    web_count=$((web_count + 1))
  done < <(find "$EXAMPLE_DIR" -maxdepth 1 -type f -name 'verified_web_*.incn' -print | sort)
  if [[ "$web_count" -eq 0 ]]; then
    echo "No verified web documentation examples found in $EXAMPLE_DIR" >&2
    exit 1
  fi
else
  echo "Web snippets typechecked; set INCAN_DOCS_BUILD_WEB=1 with a receipt-compatible Oven Loaf to run backend builds."
fi
