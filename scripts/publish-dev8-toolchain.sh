#!/usr/bin/env bash
# Publish the two real release bundles only after the maintainer proves self-hosting.
# Usage: publish-dev8-toolchain.sh COMMIT MAC_ARCHIVE MAC_OVEN LINUX_ARCHIVE LINUX_OVEN OUTPUT_JSON
# Authenticate oras and gh first. OUTPUT_JSON retains the DEV8_TOOLCHAINS repository variable value.
# A packed compiler omits Oven; ship its matching executable beside it in the same pinned OCI artifact.
set -euo pipefail

fail() {
    echo "publish-dev8: $*" >&2
    exit 1
}

# Publish one bundle, deriving the pin from the manifest returned by this push, never a second tag lookup.
publish_host() {
    local target=$1 archive=$2 oven=$3 directory reference digest
    directory=$stage/$target
    mkdir "$directory"
    cp "$archive" "$directory/toolchain.tar.zst"
    cp "$oven" "$directory/incan"
    reference=ghcr.io/encero-systems/incan.pub/toolchain/incan:$commit-$target
    (
        cd "$directory"
        oras push "$reference" \
            --artifact-type application/vnd.incan.toolchain.v1 \
            --annotation 'org.opencontainers.image.source=https://github.com/encero-systems/incan' \
            --annotation 'org.opencontainers.image.version=0.6.0-dev.8' \
            --annotation "org.opencontainers.image.revision=$commit" \
            --annotation "dev.incan.toolchain.commit=$commit" \
            --annotation "dev.incan.toolchain.target=$target" \
            --format json \
            toolchain.tar.zst:application/vnd.incan.toolchain.v1.tar+zstd \
            incan:application/octet-stream > descriptor.json
    )
    digest=$(jq -er '.digest | select(test("^sha256:[0-9a-f]{64}$"))' "$directory/descriptor.json")
    jq -n --arg target "$target" --arg reference "$reference@$digest" '{($target): $reference}' \
        > "$directory/pin.json"
}

[[ $# == 6 ]] || fail 'usage: publish-dev8-toolchain.sh COMMIT MAC_ARCHIVE MAC_OVEN LINUX_ARCHIVE LINUX_OVEN OUTPUT_JSON'
commit=$1
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail 'COMMIT must be a full source commit shared by both bundles'
command -v oras >/dev/null || fail 'oras 1.3 is required'
command -v jq >/dev/null || fail 'jq is required'
command -v gh >/dev/null || fail 'gh is required to pin the repository variable'
for input in "$2" "$3" "$4" "$5"; do
    [[ -f "$input" ]] || fail "missing release input: $input"
done
[[ "$2" == *.tar.zst && "$4" == *.tar.zst ]] || fail 'both compiler archives must be .tar.zst'
output=$6
[[ ! -e "$output" ]] || fail "refusing to replace existing pins: $output"
stage=$(mktemp -d "${TMPDIR:-/tmp}/publish-dev8.XXXXXX")
trap 'rm -rf "$stage"' EXIT
publish_host aarch64-apple-darwin "$2" "$3"
publish_host x86_64-unknown-linux-gnu "$4" "$5"
# Replace the single repository variable only after both immutable pins exist; retain the receipt if that update fails.
jq -s '.[0] + .[1]' "$stage/aarch64-apple-darwin/pin.json" "$stage/x86_64-unknown-linux-gnu/pin.json" > "$output"
gh variable set DEV8_TOOLCHAINS --repo encero-systems/incan < "$output"
echo "Published both bundles and pinned DEV8_TOOLCHAINS; receipt: $output"
