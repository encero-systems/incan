#!/bin/sh

# Fetch the public GitHub facts consumed by render_roadmap.sh. The snapshot is
# replaced only after every page and blocked-by relationship has been fetched.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
api_root="https://api.github.com/repos/encero-systems/incan"
headers='Accept: application/vnd.github+json'
version_header='X-GitHub-Api-Version: 2022-11-28'
github_token=${GITHUB_TOKEN:-}
refresh_dir=$(mktemp -d "$script_dir/.refresh.XXXXXX")

cleanup() {
  rm -rf "$refresh_dir"
}

trap cleanup 0 1 2 15

github_get() {
  if [ -n "$github_token" ]; then
    curl --fail-with-body --silent --show-error \
      --header "$headers" \
      --header "$version_header" \
      --header "Authorization: Bearer $github_token" \
      "$1"
    return
  fi

  curl --fail-with-body --silent --show-error \
    --header "$headers" \
    --header "$version_header" \
    "$1"
}

issues_path="$refresh_dir/github_issues.json"
page_path="$refresh_dir/github_issues_page.json"
merged_path="$refresh_dir/github_issues_merged.json"
blocked_by_path="$refresh_dir/github_blocked_by.json"
blocked_by_merged_path="$refresh_dir/github_blocked_by_merged.json"

printf '[]\n' > "$issues_path"
page=1
while :; do
  github_get "$api_root/issues?milestone=6&state=all&per_page=100&page=$page" > "$page_path"
  jq -e 'type == "array"' "$page_path" >/dev/null

  page_count=$(jq 'length' "$page_path")
  jq -s 'add' "$issues_path" "$page_path" > "$merged_path"
  mv "$merged_path" "$issues_path"

  if [ "$page_count" -lt 100 ]; then
    break
  fi
  page=$((page + 1))
done

printf '{}\n' > "$blocked_by_path"
# `tr` strips the carriage return jq writes on Windows, where its stdout is opened in text mode. Without it
# the number carries a CR into the request path -- `issues/1284\r/dependencies/...` -- and curl rejects
# the URL with a message that names neither the issue nor the line ending responsible.
jq -r '.[] | select(.pull_request == null and .issue_dependencies_summary.blocked_by > 0) | .number' "$issues_path" |
tr -d '\015' |
while IFS= read -r issue_number; do
  response_path="$refresh_dir/blocked_by_$issue_number.json"
  github_get "$api_root/issues/$issue_number/dependencies/blocked_by?per_page=100" > "$response_path"
  jq -e 'type == "array"' "$response_path" >/dev/null
  jq --arg issue_number "$issue_number" --slurpfile response "$response_path" \
    '. + {($issue_number): $response[0]}' "$blocked_by_path" > "$blocked_by_merged_path"
  mv "$blocked_by_merged_path" "$blocked_by_path"
done

mv "$issues_path" "$script_dir/github_issues.json"
mv "$blocked_by_path" "$script_dir/github_blocked_by.json"
date +%Y-%m-%d > "$script_dir/github_snapshot_date.txt"
rm -f "$script_dir/github_issues_page.json" "$script_dir/github_issues_merged.json" "$script_dir"/github_blocked_by_*.json

printf '%s\n' "Refreshed the v0.6 roadmap GitHub snapshot."
