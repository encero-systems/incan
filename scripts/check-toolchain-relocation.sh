#!/usr/bin/env bash
# Refuse builder paths and stale generations even when verification shares the builder's filesystem.
set -euo pipefail
[[ $# == 1 && -d $1 ]] || { echo 'usage: check-toolchain-relocation.sh INSTALLED_HOME' >&2; exit 2; }
installation=$(cd "$1" && pwd -P)
toolchain=$installation/toolchains/0.6.0-dev.8
[[ -d $toolchain ]] || { echo "missing installed toolchain: $toolchain" >&2; exit 1; }
previous=$(find "$toolchain" -name '*.previous.*' -print -quit)
[[ -z $previous ]] || { echo "stale toolchain generation: $previous" >&2; exit 1; }
while IFS= read -r -d '' document; do
    outside=$(jq -r --arg installation "$installation" '
        def strings_and_keys:
            if type == "object" then to_entries[] | .key, (.value | strings_and_keys)
            elif type == "array" then .[] | strings_and_keys
            elif type == "string" then . else empty end;
        [strings_and_keys | select(startswith("/") and . != $installation
            and (startswith($installation + "/") | not))] | .[0] // empty
    ' "$document")
    [[ -z $outside ]] || { echo "nonportable toolchain JSON $document: $outside" >&2; exit 1; }
done < <(find "$toolchain" -type f -name '*.json' -print0)
echo 'toolchain relocation audit passed'
