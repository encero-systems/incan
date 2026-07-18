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
  INCAN_SDK_PROVIDER_BUILDER_BIN
                 Host incan binary used to prepare the platform-neutral SDK provider seed (default: INCAN_BIN)
  INCAN_SDK_PROVIDER_SEED_DIR
                 Prebuilt SDK provider seed override used by packaging tests and controlled release staging
  INCAN_SDK_DISTRIBUTION_PROFILE
                 SDK profile whose component payloads are packaged (default: full)
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
distribution_profile="${INCAN_SDK_DISTRIBUTION_PROFILE:-full}"
[ -x "$incan_bin" ] || fail "incan binary is not executable: $incan_bin"
[ -x "$incan_lsp_bin" ] || fail "incan-lsp binary is not executable: $incan_lsp_bin"
[ -d "$stdlib_dir" ] || fail "stdlib source directory does not exist: $stdlib_dir"
[ -f "$stdlib_dir/testing.incn" ] || fail "stdlib source directory is missing testing.incn: $stdlib_dir"
for support_crate in incan_core incan_derive incan_stdlib incan_vocab incan_web_macros; do
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

validate_sdk_provider_seed() {
  local seed_dir="$1"
  [ -d "$seed_dir" ] || fail "SDK provider seed directory does not exist: $seed_dir"
  [ -f "$seed_dir/sdk-inventory.json" ] || fail "SDK provider seed is missing sdk-inventory.json: $seed_dir"
  [ -f "$seed_dir/Cargo.lock" ] || fail "SDK provider seed is missing its shared Cargo.lock: $seed_dir"
  [ ! -d "$seed_dir/.cargo-target" ] || fail "SDK provider seed contains a Cargo build target: $seed_dir"
  local required_components excluded_components
  case "$distribution_profile" in
    minimal)
      required_components="stdlib-core"
      excluded_components="stdlib-system stdlib-codecs stdlib-data stdlib-async stdlib-observability stdlib-web stdlib-testing"
      ;;
    default|full)
      required_components="stdlib-core stdlib-system stdlib-codecs stdlib-data stdlib-async stdlib-observability stdlib-web stdlib-testing"
      excluded_components=""
      ;;
    *)
      fail "unsupported SDK distribution profile: $distribution_profile"
      ;;
  esac
  local component component_dir manifest_count
  for component in $required_components
  do
    component_dir="$seed_dir/components/$component"
    [ -d "$component_dir" ] || fail "SDK provider seed is missing component $component"
    [ -f "$component_dir/Cargo.toml" ] || fail "SDK component $component is missing Cargo.toml"
    [ ! -f "$component_dir/Cargo.lock" ] || fail "SDK component $component duplicates the shared Cargo.lock"
    [ -f "$component_dir/src/lib.rs" ] || fail "SDK component $component is missing src/lib.rs"
    manifest_count="$(find "$component_dir" -maxdepth 1 -type f -name '*.incnlib' | wc -l | tr -d ' ')"
    [ "$manifest_count" = "1" ] || fail "SDK component $component must contain exactly one .incnlib manifest"
  done
  for component in $excluded_components; do
    [ ! -e "$seed_dir/components/$component" ] \
      || fail "SDK distribution profile $distribution_profile unexpectedly contains component $component"
  done
  if grep -R -E '(/Users/|/home/|/private/tmp/|/tmp/)' \
    "$seed_dir/sdk-inventory.json" "$seed_dir/components"/*/Cargo.toml >/dev/null 2>&1
  then
    fail "SDK provider seed contains a producer-specific absolute path"
  fi
}

prepare_sdk_provider_seed() {
  if [ -n "${INCAN_SDK_PROVIDER_SEED_DIR:-}" ]; then
    printf '%s\n' "$INCAN_SDK_PROVIDER_SEED_DIR"
    return
  fi

  local provider_builder="${INCAN_SDK_PROVIDER_BUILDER_BIN:-$incan_bin}"
  [ -x "$provider_builder" ] || fail "SDK provider builder is not executable: $provider_builder"
  provider_builder="$(cd "$(dirname "$provider_builder")" && pwd -P)/$(basename "$provider_builder")"
  local staged_stdlib="$package_dir/crates/incan_stdlib/stdlib"
  local probe="$package_dir/.incan-sdk-provider-seed-${target}-$$.incn"
  local path_file="$package_dir/.incan-sdk-provider-seed-${target}-$$.path"
  printf 'from std.result import map\n\ndef main() -> None:\n    pass\n' > "$probe"
  rm -f "$path_file"
  if (
    cd "$package_dir"
    INCAN_STDLIB="$staged_stdlib" \
      INCAN_TOOLCHAIN_CRATES_DIR="$package_dir/crates" \
      INCAN_INTERNAL_SDK_PROVIDER_STORE="$release_provider_store" \
      INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE="$path_file" \
      INCAN_INTERNAL_SDK_DISTRIBUTION_PROFILE="$distribution_profile" \
      "$provider_builder" check "$probe" --sdk-profile "$distribution_profile" >/dev/null
  ); then
    :
  else
    rm -f "$probe" "$path_file"
    fail "could not prepare the release-compatible SDK provider seed"
  fi
  [ -s "$path_file" ] || fail "SDK provider builder did not report its seed path"
  local prepared_seed
  prepared_seed="$(sed -n '1p' "$path_file")"
  rm -f "$probe" "$path_file"
  printf '%s\n' "$prepared_seed"
}

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd -P)"
package_dir="$out_dir/dist/incan-${release}-${target}"
archive="$out_dir/incan-${release}-${target}.tar.gz"
release_provider_store=""

cleanup_release_provider_store() {
  if [ -n "$release_provider_store" ]; then
    rm -rf "$release_provider_store"
  fi
}
trap cleanup_release_provider_store EXIT

rm -rf "$package_dir"
mkdir -p "$package_dir/bin" "$package_dir/crates"
cp "$incan_bin" "$package_dir/bin/incan"
cp "$incan_lsp_bin" "$package_dir/bin/incan-lsp"
for support_crate in incan_core incan_derive incan_stdlib incan_vocab incan_web_macros; do
  support_destination="$package_dir/crates/${support_crate}"
  stage_tracked_tree "crates/${support_crate}" "$support_destination"
done
if [ -z "${INCAN_SDK_PROVIDER_SEED_DIR:-}" ]; then
  release_provider_store="$package_dir/share/incan"
fi
cat > "$package_dir/crates/Cargo.toml" <<WORKSPACE
[workspace]
members = [
    "incan_core",
    "incan_derive",
    "incan_stdlib",
    "incan_vocab",
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
keywords = ["programming-language", "compiler", "rust", "python"]
categories = ["compilers", "development-tools"]
WORKSPACE
git show HEAD:Cargo.lock > "$package_dir/crates/Cargo.lock" \
  || fail "could not stage the verified workspace Cargo.lock"

# Ship one immutable component-aware SDK seed. The fixed `share/incan/sdk` location is relocation-stable and contains
# only checked manifests, generated Rust crates, and resolved locks; mutable cache identities and Cargo targets stay out.
sdk_provider_seed="$(prepare_sdk_provider_seed)"
validate_sdk_provider_seed "$sdk_provider_seed"
sdk_seed_root="$package_dir/share/incan/sdk"
if [ -n "${INCAN_SDK_PROVIDER_SEED_DIR:-}" ]; then
  rm -rf "$sdk_seed_root"
  mkdir -p "$(dirname "$sdk_seed_root")"
  cp -R "$sdk_provider_seed" "$sdk_seed_root"
elif [ "$sdk_provider_seed" != "$sdk_seed_root" ]; then
  rm -rf "$sdk_seed_root"
  mv "$sdk_provider_seed" "$sdk_seed_root"
fi
release_provider_store=""
rm -f "$package_dir/share/incan/.incan.lock"
# Provider source is a packaging input, not an installed SDK component. Remove it only after every checked provider
# artifact has been published; consumers receive the Rust support crate plus the relocatable provider payloads.
rm -rf "$package_dir/crates/incan_stdlib/stdlib"
[ ! -d "$package_dir/stdlib" ] || fail "legacy top-level stdlib source unexpectedly entered the package"
[ ! -d "$package_dir/crates/incan_stdlib/stdlib" ] \
  || fail "provider-owned stdlib source unexpectedly entered the package"

tar -C "$package_dir" -czf "$archive" .
shasum -a 256 "$archive" | awk '{print $1}' > "${archive}.sha256"
sdk_component_count="$(find "$sdk_seed_root/components" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
sdk_payload_bytes="$(find "$sdk_seed_root" -type f -exec wc -c {} + | awk '{ total += $1 } END { print total + 0 }')"
archive_bytes="$(wc -c < "$archive" | tr -d ' ')"
cat > "${archive}.profile.json" <<PROFILE_EVIDENCE
{
  "schema_version": 1,
  "release": "${release}",
  "target": "${target}",
  "sdk_profile": "${distribution_profile}",
  "sdk_component_count": ${sdk_component_count},
  "sdk_payload_bytes": ${sdk_payload_bytes},
  "archive_bytes": ${archive_bytes}
}
PROFILE_EVIDENCE
printf '%s\n' "$version" > "$out_dir/toolchain-version.txt"
printf '%s\n' "$release" > "$out_dir/toolchain-release.txt"

printf 'Packaged %s\n' "$archive"
