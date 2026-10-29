#!/usr/bin/env bash
# Publish the real release bundles only after the release proves self-hosting.
# Usage: publish-build-toolchain.sh COMMIT MAC_ARCHIVE MAC_OVEN MAC_LSP LINUX_ARCHIVE LINUX_OVEN LINUX_LSP OUTPUT_JSON
# Both hosts' bundles are built from COMMIT; CI installs each host's bundle from the one pin.
# Authenticate oras and gh first. OUTPUT_JSON retains the BUILD_TOOLCHAIN repository variable value.
# A packed compiler omits the tools; ship matching Oven and LSP executables in the same pinned OCI artifact.
set -euo pipefail

fail() {
    echo "publish-build-toolchain: $*" >&2
    exit 1
}

# Refuse a host's bundle before anything is pushed.
check_host() {
    local archive=$1 oven=$2 lsp=$3 input
    for input in "$archive" "$oven" "$lsp"; do
        [[ -f "$input" ]] || fail "missing release input: $input"
    done
    [[ "$archive" == *.tar.zst ]] || fail "compiler archives must be .tar.zst: $archive"
    for input in "$oven" "$lsp"; do
        [[ -x "$input" ]] || fail "release tool is not executable: $input"
    done
}

# Publish one bundle, deriving the pin from the manifest returned by this push, never a second tag lookup.
publish_host() {
    local target=$1 archive=$2 oven=$3 lsp=$4 directory reference digest
    directory=$stage/$target
    mkdir "$directory"
    cp "$archive" "$directory/toolchain.tar.zst"
    cp "$oven" "$directory/incan"
    cp "$lsp" "$directory/incan-lsp"
    reference=ghcr.io/encero-systems/incan.pub/toolchain/incan:$commit-$target
    (
        cd "$directory"
        oras push "$reference" \
            --artifact-type application/vnd.incan.toolchain.v1 \
            --annotation 'org.opencontainers.image.source=https://github.com/encero-systems/incan' \
            --annotation "org.opencontainers.image.version=$version" \
            --annotation "org.opencontainers.image.revision=$commit" \
            --annotation "dev.incan.toolchain.commit=$commit" \
            --annotation "dev.incan.toolchain.target=$target" \
            --format json \
            toolchain.tar.zst:application/vnd.incan.toolchain.v1.tar+zstd \
            incan:application/octet-stream \
            incan-lsp:application/octet-stream > descriptor.json
    )
    digest=$(jq -er '.digest | select(test("^sha256:[0-9a-f]{64}$"))' "$directory/descriptor.json")
    jq -n --arg target "$target" --arg reference "$reference@$digest" '{($target): $reference}' \
        > "$directory/pin.json"
}

[[ $# == 8 ]] || fail 'usage: publish-build-toolchain.sh COMMIT MAC_ARCHIVE MAC_OVEN MAC_LSP LINUX_ARCHIVE LINUX_OVEN LINUX_LSP OUTPUT_JSON'
commit=$1
version=0.6.0-dev.8
output=$8
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail 'COMMIT must be the full source commit every bundle is built from'
command -v oras >/dev/null || fail 'oras 1.3 is required'
command -v jq >/dev/null || fail 'jq is required'
command -v gh >/dev/null || fail 'gh is required to pin the repository variable'
check_host "$2" "$3" "$4"
check_host "$5" "$6" "$7"
[[ ! -e "$output" ]] || fail "refusing to replace existing pins: $output"
stage=$(mktemp -d "${TMPDIR:-/tmp}/publish-build-toolchain.XXXXXX")
trap 'rm -rf "$stage"' EXIT
publish_host aarch64-apple-darwin "$2" "$3" "$4"
publish_host x86_64-unknown-linux-gnu "$5" "$6" "$7"
# Replace the single repository variable only after every immutable pin exists; retain the receipt if that update fails.
jq -s --arg version "$version" '{version: $version} + add' "$stage"/*/pin.json > "$output"
gh variable set BUILD_TOOLCHAIN --repo encero-systems/incan < "$output"
echo "Published both bundles and pinned BUILD_TOOLCHAIN; receipt: $output"
