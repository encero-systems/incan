#!/bin/zsh
# Build and install Oven, the contract runner or the LSP with an installed dev.8 toolchain.
# Usage: DEV8_LANE=/path/to/lane SELFBUILD_HOME=/installed/home scripts/build-native-tools.sh oven|contracts|lsp [--rebuild]
set -e

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
name=${1:?select oven, contracts or lsp}
case $name in
    oven|contracts)
        project=$lane/incan/workspaces/$name
        output=$project/target/incan/release/bin/$name
        if [[ $name == oven ]]; then
            destination=$lane/oven-bin/incan
        else
            destination=$lane/contracts-bin/contracts
        fi
        ;;
    lsp)
        project=$lane/incan/loaves/toolchain/incan-lsp
        output=$project/target/incan/rust/release/bin/incan-lsp
        destination=$lane/lsp-bin/incan-lsp
        ;;
    *) print -u2 "unknown native tool: $name"; exit 2 ;;
esac
if [[ -n ${2:-} && $2 != --rebuild ]] || (( $# > 2 )); then
    print -u2 'the only optional argument is --rebuild'
    exit 2
fi

export INCAN_HOME=${SELFBUILD_HOME:-$lane/dev8-home}
export INCAN_STDLIB=${SELFBUILD_STDLIB:-$lane/incan/loaves/stdlib}
builder=${SELFBUILD_OVEN:-$lane/oven-bin/incan}
mkdir -p "$lane/out/native-builds"
log=$lane/out/native-builds/$name.log

# A generation proof must compile the source again, even when its executable stamp remains valid.
if [[ ${2:-} == --rebuild && -f $output.oven-inputs ]]; then
    rm "$output.oven-inputs"
fi
if ! OVEN_TRACE=- "$builder" build "$project" --profile release > "$log" 2>&1; then
    tail -20 "$log"
    exit 1
fi
[[ -f $output ]] || { print -u2 "native build did not publish $output"; exit 1; }

# Replace the directory entry after the copy, so an executing generation keeps its original file.
mkdir -p "${destination:h}"
cp "$output" "$destination.next"
mv "$destination.next" "$destination"
shasum -a 256 "$destination"
