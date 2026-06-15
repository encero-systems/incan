#!/usr/bin/env bash
set -euo pipefail

dist_dir="${1:-dist/sdk}"

if [ -z "${PYPI_API_TOKEN:-}" ]; then
  if [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
    echo "PYPI_API_TOKEN is required for tagged PyPI releases." >&2
    exit 1
  fi
  echo "PYPI_API_TOKEN is not configured; skipping PyPI publish."
  exit 0
fi

python3 -m pip install --upgrade twine
python3 -m twine upload --non-interactive --skip-existing -u __token__ -p "${PYPI_API_TOKEN}" "${dist_dir}"/incan_sdk-*.whl "${dist_dir}"/incan_sdk-*.tar.gz
