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
  [ -n "$path_entry" ] || path_entry="."
  candidate="$path_entry/cargo"
  [ -x "$candidate" ] || continue
  case "$path_entry" in
    /*) scan_dir="$path_entry" ;;
    *) scan_dir="./$path_entry" ;;
  esac
  candidate_dir="$(CDPATH= cd -P "$scan_dir" 2>/dev/null && pwd -P)" || continue
  case "/$candidate_dir/" in
    */target/*) continue ;;
  esac
  resolved_cargo="$candidate_dir/cargo"
  break
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
