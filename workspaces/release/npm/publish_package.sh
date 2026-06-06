#!/usr/bin/env bash
set -euo pipefail

dist_dir="${1:-dist/sdk}"

if [ -z "${NPM_TOKEN:-}" ]; then
  echo "NPM_TOKEN is not configured; skipping npm publish."
  exit 0
fi

npmrc="${NPM_CONFIG_USERCONFIG:-$(mktemp)}"
if [ -z "${NPM_CONFIG_USERCONFIG:-}" ]; then
  trap 'rm -f "$npmrc"' EXIT
fi

printf '//registry.npmjs.org/:_authToken=%s\n' "${NPM_TOKEN}" > "$npmrc"
NPM_CONFIG_USERCONFIG="$npmrc" npm publish "${dist_dir}"/incan-sdk-*.tgz --access public
