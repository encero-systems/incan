#!/usr/bin/env bash
# Fetch the pinned incan.pub Clang for one Linux target by digest, without signing in.
# Usage: fetch-c-toolchain.sh TARGET OUT
# OUT receives `clang/`, with `bin/clang` and its resource headers. C ABI checks read the machine's own headers.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
pins=$ROOT/scripts/c-toolchain.json

fail() {
    echo "fetch-c-toolchain: $*" >&2
    exit 1
}

[[ $# == 2 ]] || fail 'usage: fetch-c-toolchain.sh TARGET OUT'
target=$1
out=$2
[[ ! -e $out ]] || fail "output already exists: $out"
registry=$(jq -er '.registry' "$pins")
namespace=${registry#ghcr.io/}

# Pull every layer of one artifact by its manifest digest, verifying the manifest and each layer before use.
pull() {
    local repository=$1 digest=$2 directory=$3 token layer title
    token=$(curl -fsSL "https://ghcr.io/token?scope=repository:$namespace/$repository:pull" | jq -er '.token')
    mkdir -p "$directory"
    curl -fsSL -H "Authorization: Bearer $token" -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
        "https://ghcr.io/v2/$namespace/$repository/manifests/$digest" -o "$directory/manifest.json"
    [[ sha256:$(shasum -a 256 "$directory/manifest.json" | cut -d ' ' -f1) == "$digest" ]] \
        || fail "manifest digest differs: $repository@$digest"
    while IFS=$'\t' read -r layer title; do
        [[ $title =~ ^[A-Za-z0-9._-]+$ ]] || fail "unsafe layer title in $repository: $title"
        curl -fsSL -H "Authorization: Bearer $token" "https://ghcr.io/v2/$namespace/$repository/blobs/$layer" \
            -o "$directory/$title"
        [[ sha256:$(shasum -a 256 "$directory/$title" | cut -d ' ' -f1) == "$layer" ]] \
            || fail "layer digest differs: $repository $title"
    done < <(jq -r '.layers[] | [.digest, .annotations["org.opencontainers.image.title"]] | @tsv' \
        "$directory/manifest.json")
}

clang=$(jq -er --arg target "$target" '.clang[$target].digest' "$pins") || fail "no pinned Clang for $target"
mkdir -p "$out/clang"
pull toolchain/clang "$clang" "$out/downloads/clang"
tar -xzf "$out"/downloads/clang/clang-*.tar.gz -C "$out/clang" --strip-components=1
rm -rf -- "$out/downloads"
[[ -x $out/clang/bin/clang ]] || fail 'pinned Clang is incomplete'
"$out/clang/bin/clang" --version
