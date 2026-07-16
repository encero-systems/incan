#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Package the Incan toolchain commands for one host target.

Usage:
  package_archive.sh <target> [--out-dir <dir>]

Environment:
  INCAN_BIN      Path to the built incan binary (default: target/release/incan)
  INCAN_LSP_BIN  Path to the built incan-lsp binary (default: target/release/incan-lsp)
  INCAN_STDLIB_ARTIFACT_BUILDER_BIN
                 Host incan binary used to prepare the platform-neutral stdlib seed (default: INCAN_BIN)
  INCAN_BUILTIN_STDLIB_SEED_DIR
                 Prebuilt stdlib seed override used by packaging tests and controlled release staging
  TOOLCHAIN_RELEASE    Release name override (default: tag name or v<workspace version>)
USAGE
}

fail() {
  printf 'package_archive: %s\n' "$*" >&2
  exit 1
}

if [ "$#" -lt 1 ]; then
  usage >&2
  exit 2
fi

target="$1"
shift
out_dir="."

[ -n "$target" ] || fail "target must not be empty"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --out-dir)
      [ "$#" -ge 2 ] || fail "--out-dir requires a value"
      out_dir="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

workspace_version() {
  awk '
    /^\[workspace.package\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section && /^version = / {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
}

version="$(workspace_version)"
[ -n "$version" ] || fail "could not read workspace package version from Cargo.toml"

if [ -n "${TOOLCHAIN_RELEASE:-}" ]; then
  release="$TOOLCHAIN_RELEASE"
elif [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
  release="${GITHUB_REF_NAME}"
else
  release="v${version}"
fi

incan_bin="${INCAN_BIN:-target/release/incan}"
incan_lsp_bin="${INCAN_LSP_BIN:-target/release/incan-lsp}"
stdlib_dir="${INCAN_STDLIB_SOURCE_DIR:-crates/incan_stdlib/stdlib}"
[ -x "$incan_bin" ] || fail "incan binary is not executable: $incan_bin"
[ -x "$incan_lsp_bin" ] || fail "incan-lsp binary is not executable: $incan_lsp_bin"
[ -d "$stdlib_dir" ] || fail "stdlib source directory does not exist: $stdlib_dir"
[ -f "$stdlib_dir/testing.incn" ] || fail "stdlib source directory is missing testing.incn: $stdlib_dir"
for support_crate in incan_core incan_derive incan_stdlib incan_web_macros; do
  [ -f "crates/${support_crate}/Cargo.toml" ] || fail "support crate is missing: crates/${support_crate}"
done

archive_counter=0
stage_tracked_tree() {
  local source_tree="${1#./}"
  local destination="$2"
  archive_counter=$((archive_counter + 1))
  local source_archive="$package_dir/.tracked-source-${archive_counter}.tar"
  mkdir -p "$destination"
  git archive --format=tar --output="$source_archive" "HEAD:${source_tree}" \
    || fail "could not archive tracked source tree: ${source_tree}"
  tar -C "$destination" -xf "$source_archive" \
    || fail "could not extract tracked source tree into: ${destination}"
  rm "$source_archive"
}

validate_builtin_stdlib_seed() {
  local seed_dir="$1"
  [ -d "$seed_dir" ] || fail "compiled stdlib seed directory does not exist: $seed_dir"
  [ -f "$seed_dir/.incan-artifact-identity" ] \
    || fail "compiled stdlib seed is missing .incan-artifact-identity: $seed_dir"
  [ -f "$seed_dir/incan_builtin_stdlib.incnlib" ] \
    || fail "compiled stdlib seed is missing incan_builtin_stdlib.incnlib: $seed_dir"
  [ -f "$seed_dir/Cargo.toml" ] || fail "compiled stdlib seed is missing Cargo.toml: $seed_dir"
  [ -f "$seed_dir/Cargo.lock" ] || fail "compiled stdlib seed is missing Cargo.lock: $seed_dir"
  [ -f "$seed_dir/src/lib.rs" ] || fail "compiled stdlib seed is missing src/lib.rs: $seed_dir"
  [ ! -d "$seed_dir/.cargo-target" ] || fail "compiled stdlib seed contains a Cargo build target: $seed_dir"
  local seed_identity
  seed_identity="$(basename "$seed_dir")"
  local recorded_identity
  recorded_identity="$(tr -d '\r\n' < "$seed_dir/.incan-artifact-identity")"
  [ "$recorded_identity" = "$seed_identity" ] \
    || fail "compiled stdlib seed identity marker does not match directory ${seed_identity}"
}

prepare_builtin_stdlib_seed() {
  if [ -n "${INCAN_BUILTIN_STDLIB_SEED_DIR:-}" ]; then
    printf '%s\n' "$INCAN_BUILTIN_STDLIB_SEED_DIR"
    return
  fi

  local artifact_builder="${INCAN_STDLIB_ARTIFACT_BUILDER_BIN:-$incan_bin}"
  [ -x "$artifact_builder" ] || fail "compiled stdlib artifact builder is not executable: $artifact_builder"
  artifact_builder="$(cd "$(dirname "$artifact_builder")" && pwd -P)/$(basename "$artifact_builder")"
  local staged_stdlib="$package_dir/crates/incan_stdlib/stdlib"
  local probe="$package_dir/.incan-builtin-stdlib-seed-${target}-$$.incn"
  local path_file="$package_dir/.incan-builtin-stdlib-seed-${target}-$$.path"
  printf 'from std.fs.path import Path\n\ndef main() -> None:\n    _ = Path("seed")\n' > "$probe"
  rm -f "$path_file"
  if (
    cd "$package_dir"
    INCAN_STDLIB="$staged_stdlib" \
      INCAN_TOOLCHAIN_CRATES_DIR="$package_dir/crates" \
      INCAN_INTERNAL_BUILTIN_STDLIB_RELEASE_SEED=1 \
      INCAN_INTERNAL_BUILTIN_STDLIB_ARTIFACT_STORE="$release_seed_store" \
      INCAN_INTERNAL_BUILTIN_STDLIB_ARTIFACT_PATH_FILE="$path_file" \
      "$artifact_builder" check "$probe" >/dev/null
  ); then
    :
  else
    rm -f "$probe" "$path_file"
    fail "could not prepare the release-compatible compiled stdlib seed"
  fi
  [ -s "$path_file" ] || fail "compiled stdlib artifact builder did not report its seed path"
  local prepared_seed
  prepared_seed="$(sed -n '1p' "$path_file")"
  rm -f "$probe" "$path_file"
  printf '%s\n' "$prepared_seed"
}

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd -P)"
package_dir="$out_dir/dist/incan-${release}-${target}"
archive="$out_dir/incan-${release}-${target}.tar.gz"
release_seed_store=""

cleanup_release_seed_store() {
  if [ -n "$release_seed_store" ]; then
    rm -rf "$release_seed_store"
  fi
}
trap cleanup_release_seed_store EXIT

rm -rf "$package_dir"
mkdir -p "$package_dir/bin" "$package_dir/crates" "$package_dir/stdlib"
cp "$incan_bin" "$package_dir/bin/incan"
cp "$incan_lsp_bin" "$package_dir/bin/incan-lsp"
stage_tracked_tree "$stdlib_dir" "$package_dir/stdlib"
for support_crate in incan_core incan_derive incan_stdlib incan_web_macros; do
  support_destination="$package_dir/crates/${support_crate}"
  stage_tracked_tree "crates/${support_crate}" "$support_destination"
done
if [ -z "${INCAN_BUILTIN_STDLIB_SEED_DIR:-}" ]; then
  release_seed_store="$package_dir/crates/incan_stdlib/stdlib/target/.incan_builtin_stdlib_release_${target}_$$"
fi
cat > "$package_dir/crates/Cargo.toml" <<WORKSPACE
[workspace]
members = [
    "incan_core",
    "incan_derive",
    "incan_stdlib",
    "incan_web_macros",
]
resolver = "2"

[workspace.package]
version = "${version}"
edition = "2024"
rust-version = "1.93"
license = "Apache-2.0"
authors = ["Danny Meijer <dannys.code.corner@gmail.com>"]
repository = "https://github.com/encero-systems/incan"
homepage = "https://github.com/encero-systems/incan"
WORKSPACE
git show HEAD:Cargo.lock > "$package_dir/crates/Cargo.lock" \
  || fail "could not stage the verified workspace Cargo.lock"

# Ship exactly one immutable, release-compatible stdlib seed. The seed contains the semantic `.incnlib`, generated
# Rust crate, and resolved lock closure. Cargo's build target and historical local identities remain excluded.
builtin_stdlib_seed="$(prepare_builtin_stdlib_seed)"
validate_builtin_stdlib_seed "$builtin_stdlib_seed"
seed_identity="$(basename "$builtin_stdlib_seed")"
seed_store="$package_dir/crates/incan_stdlib/stdlib/target/incan_builtin_stdlib"
mkdir -p "$seed_store"
cp -R "$builtin_stdlib_seed" "$seed_store/$seed_identity"
printf '%s\n' "$seed_identity" > "$seed_store/.incan-release-seed-identity"
cleanup_release_seed_store
release_seed_store=""
# Type discovery used while generating the seed can leave a mutable `incan_lock/rust_inspect` workspace beside it.
# The archive contract contains the immutable seed only, never producer-side inspection or Cargo scratch state.
rm -rf "$package_dir/crates/incan_stdlib/stdlib/target/incan_lock"

tar -C "$package_dir" -czf "$archive" .
shasum -a 256 "$archive" | awk '{print $1}' > "${archive}.sha256"
printf '%s\n' "$version" > "$out_dir/toolchain-version.txt"
printf '%s\n' "$release" > "$out_dir/toolchain-release.txt"

printf 'Packaged %s\n' "$archive"
