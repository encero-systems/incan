#!/bin/sh
set -eu

[ "$#" -eq 0 ] || [ "$#" -eq 3 ] || {
  echo "usage: resolve_release_native_toolchain.sh [CC CXX C_SYSROOT]" >&2
  exit 2
}

if [ "$(uname -s)" = "Darwin" ]; then
  cc_bin="/Library/Developer/CommandLineTools/usr/bin/clang"
  cxx_bin="/Library/Developer/CommandLineTools/usr/bin/clang++"
  c_sysroot="$(realpath /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk)"
else
  cc_bin="/usr/bin/clang"
  cxx_bin="/usr/bin/clang++"
  c_sysroot="/usr"
fi
if [ "$#" -eq 3 ]; then
  cc_bin="${1:-$cc_bin}"
  cxx_bin="${2:-$cxx_bin}"
  c_sysroot="${3:-$c_sysroot}"
fi

if [ "$#" -eq 3 ]; then
  [ -x "$cc_bin" ] || {
    echo "C compiler is not executable: $cc_bin" >&2
    exit 1
  }
  [ -x "$cxx_bin" ] || {
    echo "C++ compiler is not executable: $cxx_bin" >&2
    exit 1
  }
  [ -d "$c_sysroot" ] || {
    echo "C sysroot is not a directory: $c_sysroot" >&2
    exit 1
  }
fi

printf '%s\t%s\t%s\n' "$cc_bin" "$cxx_bin" "$c_sysroot"
