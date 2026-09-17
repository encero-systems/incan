#!/bin/sh
set -eu

[ "$#" = 1 ] || {
  echo "usage: resolve_release_cargo.sh EXPLICIT_CARGO" >&2
  exit 2
}

explicit_cargo="$1"
if [ -n "$explicit_cargo" ]; then
  [ -x "$explicit_cargo" ] || {
    echo "explicit Cargo is not executable: $explicit_cargo" >&2
    exit 1
  }
  printf '%s\n' "$explicit_cargo"
  exit 0
fi

resolved_cargo=""
saved_ifs="$IFS"
IFS=':'
for path_entry in $PATH; do
  case "$path_entry" in
    */target/*) continue ;;
  esac
  if [ -x "$path_entry/cargo" ]; then
    resolved_cargo="$path_entry/cargo"
    break
  fi
done
IFS="$saved_ifs"
[ -n "$resolved_cargo" ] || {
  echo "could not resolve Cargo outside the repository target guard; set CARGO_BIN to an exact executable" >&2
  exit 1
}

rustup_bin="${resolved_cargo%/*}/rustup"
if [ ! -x "$rustup_bin" ]; then
  printf '%s\n' "$resolved_cargo"
  exit 0
fi

if [ -n "${RUSTUP_TOOLCHAIN:-}" ]; then
  selected_cargo="$("$rustup_bin" which --toolchain "$RUSTUP_TOOLCHAIN" cargo)" || {
    echo "could not resolve Cargo for pinned Rustup toolchain $RUSTUP_TOOLCHAIN" >&2
    exit 1
  }
else
  selected_cargo="$("$rustup_bin" which cargo)" || {
    echo "could not resolve Cargo for the active Rustup toolchain" >&2
    exit 1
  }
fi
[ -x "$selected_cargo" ] || {
  echo "Rustup selected Cargo is not executable: $selected_cargo" >&2
  exit 1
}
printf '%s\n' "$selected_cargo"
