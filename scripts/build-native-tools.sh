#!/bin/zsh
# Build and install Oven or the contract runner using only an installed dev.8 toolchain.
# Usage: DEV8_LANE=/path/to/lane SELFBUILD_HOME=/installed/home scripts/build-native-tools.sh oven|contracts [--rebuild]
set -e

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
name=${1:?select oven or contracts}
case $name in
    oven) destination=$lane/oven-bin/incan ;;
    contracts) destination=$lane/contracts-bin/contracts ;;
    *) print -u2 "unknown native tool: $name"; exit 2 ;;
esac
if [[ -n ${2:-} && $2 != --rebuild ]] || (( $# > 2 )); then
    print -u2 'the only optional argument is --rebuild'
    exit 2
fi

export INCAN_HOME=${SELFBUILD_HOME:-$lane/dev8-home}
export INCAN_STDLIB=${SELFBUILD_STDLIB:-$lane/bootstrap/stdlib-available}
builder=${SELFBUILD_OVEN:-$lane/oven-bin/incan}
project=$lane/incan/workspaces/$name
output=$project/target/incan/release/bin/$name
log=$lane/bootstrap/native-$name-build.log

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
