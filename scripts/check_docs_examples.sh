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

verified_count=0
while IFS= read -r example; do
  echo "==> check docs example: $example"
  INCAN_NO_BANNER=1 "$INCAN_BIN" --check "$example"
  verified_count=$((verified_count + 1))
done < <(find "$EXAMPLE_DIR" -maxdepth 1 -type f -name 'verified_*.incn' -print | sort)
if [[ "$verified_count" -eq 0 ]]; then
  echo "No verified documentation examples found in $EXAMPLE_DIR" >&2
  exit 1
fi

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
