#!/bin/sh
set -eu

[ "$#" = 2 ] || {
  echo "usage: select_release_policy_output.sh REPORT TARGET" >&2
  exit 2
}

report="$1"
target="$2"
[ -n "$target" ] || {
  echo "release policy target must not be empty" >&2
  exit 2
}

count="$(
  jq -er --arg target "$target" \
    '[.outputs[] | select(.project_target == "executable:src/plan_json_main.incn" and .profile == "release" and .target == $target)] | length' \
    "$report"
)"
[ "$count" = 1 ] || {
  echo "release policy bake must report exactly one release core_engine ProjectOutput for $target" >&2
  exit 1
}

store="$(jq -er '.store | select(type == "string" and length > 0)' "$report")"
identity="$(
  jq -er --arg target "$target" \
    '.outputs[] | select(.project_target == "executable:src/plan_json_main.incn" and .profile == "release" and .target == $target) | .artifact_identity | select(type == "string" and length > 0)' \
    "$report"
)"
printf '%s\t%s\n' "$store" "$identity"
