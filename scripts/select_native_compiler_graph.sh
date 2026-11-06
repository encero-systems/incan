#!/bin/sh
# Select the checked-in native graph for this host. Explicit caller graph overrides remain authoritative.
set -eu
if [ "$#" -ne 1 ]; then
    echo "usage: select_native_compiler_graph.sh CHECKOUT" >&2
    exit 2
fi
if [ -n "${INCAN_SDK_NATIVE_COMPILER_GRAPH:-}" ]; then
    graph=$INCAN_SDK_NATIVE_COMPILER_GRAPH
else
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) graph="$1/workspaces/oven/compiler-suite-native.json" ;;
        Linux/x86_64) graph="$1/workspaces/oven/compiler-suite-native-linux.json" ;;
        *) echo "No checked-in native compiler graph for this host; supply INCAN_SDK_NATIVE_COMPILER_GRAPH explicitly" >&2; exit 1 ;;
    esac
fi
if [ ! -f "$graph" ]; then
    echo "Selected native compiler graph does not exist: $graph" >&2
    exit 1
fi
printf '%s\n' "$graph"
