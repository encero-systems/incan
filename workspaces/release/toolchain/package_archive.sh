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
  INCAN_OVEN_LOAF_DIR
                 Prebuilt compiler-owned Oven Loafs used by packaging tests and controlled staging
  INCAN_SDK_DISTRIBUTION_PROFILE
                 SDK profile whose component payloads are packaged (default: full)
  TOOLCHAIN_RELEASE    Release name override (default: tag name or v<workspace version>)
USAGE
}

fail() {
  printf 'package_archive: %s\n' "$*" >&2
  exit 1
}

# Resolve the exact rustc that seals this archive's Oven Loafs.
resolve_release_rustc() {
  local resolved
  if [ -n "${RUSTC:-}" ]; then
    resolved="$RUSTC"
  elif command -v rustup >/dev/null 2>&1; then
    resolved="$(rustup which rustc)"
  else
    resolved="$(command -v rustc)"
  fi
  [ -x "$resolved" ] || return 1
  printf '%s\n' "$resolved"
}

# Report the plain version number ("1.98.0") of a Rust compiler.
#
# Loafs are sealed against the compiler's full `rustc --version` identity and `verify_rustc_identity` demands an
# exact match, so a release must tell installers precisely which compiler to provision. A Rustup channel naming a
# concrete version resolves to exactly one build, which is what makes the shipped Loafs usable on a user's machine;
# a floating "stable" channel does not, and drifts out from under the release the moment upstream publishes.
rustc_channel_version() {
  local rustc_bin="$1"
  local reported
  reported="$("$rustc_bin" --version)" || return 1
  printf '%s\n' "$reported" | awk '{ print $2 }'
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
      excluded_components="stdlib-system stdlib-codecs stdlib-compression stdlib-data stdlib-async stdlib-observability stdlib-web stdlib-testing"
      ;;
    default|full)
      required_components="stdlib-core stdlib-system stdlib-codecs stdlib-compression stdlib-data stdlib-async stdlib-observability stdlib-web stdlib-testing"
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
# The staged source tree is the installed toolchain's authoritative Incan-language stdlib surface. SDK providers and
# Oven Loafs contain generated Rust/runtime artifacts, but source imports, test discovery, and metadata inspection
# still require these checked `.incn` declarations. Keep the versioned bundle beside its support crate rather than
# restoring the obsolete top-level `stdlib/` layout.
[ -f "$package_dir/crates/incan_stdlib/stdlib/prelude.incn" ] \
  || fail "release package is missing the built-in stdlib prelude source"
[ -f "$package_dir/crates/incan_stdlib/stdlib/testing.incn" ] \
  || fail "release package is missing the built-in stdlib testing source"
[ ! -d "$package_dir/stdlib" ] || fail "legacy top-level stdlib source unexpectedly entered the package"

# Ship the typed release envelope through the same explicit baker used by local and CI preparation. The baker owns
# fixture source, identity, admission, accounting, and atomic publication; this packaging script only stages its output.
loaf_root="$package_dir/share/incan/oven/loafs"
if [ -n "${INCAN_OVEN_LOAF_DIR:-}" ]; then
  [ "${INCAN_OVEN_LOAF_OVERRIDE_TEST_ONLY:-0}" = "1" ] \
    || fail "INCAN_OVEN_LOAF_DIR is reserved for controlled packaging tests; production archives must invoke the baker"
  [ -d "$INCAN_OVEN_LOAF_DIR" ] \
    || fail "Oven Loaf override does not exist: $INCAN_OVEN_LOAF_DIR"
  mkdir -p "$(dirname "$loaf_root")"
  cp -R "$INCAN_OVEN_LOAF_DIR" "$loaf_root"
else
  rustc_bin="$(resolve_release_rustc)" || fail "could not resolve rustc for the release-only Loaf publisher"
  "$package_dir/bin/incan" oven legacy-cargo bake-loafs \
    --compiler-root "$package_dir" \
    --output "$loaf_root" \
    --envelope release \
    --sdk-inventory "$sdk_seed_root/sdk-inventory.json" \
    --cargo "$cargo_bin" \
    --rustc "$rustc_bin" \
    --format json >/dev/null \
    || fail "could not bake the release Oven Loaf envelope"
fi
[ -d "$loaf_root" ] || fail "release package is missing Oven Loafs"
[ "$(find "$loaf_root" -name loaf.json -type f | wc -l | tr -d ' ')" = "2" ] \
  || fail "release package must contain one release core and one debug Oven foundation Loaf"

# Install only the reviewed toolchain-owned selector. The explicit publisher retains original Engine/output owners;
# a normal compiler command never builds a missing module or takes an executable override from a project.
core_engine_source="$package_dir/share/incan/oven/core-source"
stage_tracked_tree "workspaces/oven" "$core_engine_source"
"$package_dir/bin/incan" oven install-core-engine --toolchain-root "$package_dir" \
  || fail "could not publish the toolchain core Engine"

sdk_component_count="$(find "$sdk_seed_root/components" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
sdk_payload_bytes="$(find "$sdk_seed_root" -type f -exec wc -c {} + | awk '$2 != "total" { total += $1 } END { print total + 0 }')"
loaf_count="$(find "$loaf_root" -name loaf.json -type f | wc -l | tr -d ' ')"
loaf_payload_bytes="$(find "$loaf_root" -type f -exec wc -c {} + | awk '$2 != "total" { total += $1 } END { print total + 0 }')"
loaf_physical_bytes="$(du -sk "$loaf_root" | awk '{ print $1 * 1024 }')"

# The baker has already admitted the complete immutable closure under the central Oven policy. Packaging records both
# logical and host-physical measurements without redefining that policy.
tar -C "$package_dir" -czf "$archive" .
shasum -a 256 "$archive" | awk '{print $1}' > "${archive}.sha256"
archive_bytes="$(wc -c < "$archive" | tr -d ' ')"
cat > "${archive}.profile.json" <<PROFILE_EVIDENCE
{
  "schema_version": 1,
  "release": "${release}",
  "target": "${target}",
  "sdk_profile": "${distribution_profile}",
  "sdk_component_count": ${sdk_component_count},
  "sdk_payload_bytes": ${sdk_payload_bytes},
  "oven_loaf_count": ${loaf_count},
  "oven_loaf_logical_bytes": ${loaf_payload_bytes},
  "oven_loaf_physical_bytes": ${loaf_physical_bytes},
  "archive_bytes": ${archive_bytes}
}
PROFILE_EVIDENCE
printf '%s\n' "$version" > "$out_dir/toolchain-version.txt"
printf '%s\n' "$release" > "$out_dir/toolchain-release.txt"

# Record the exact Rust compiler that sealed this host's Loafs, under a per-host name because the publish job
# merges every host's artifacts into one directory. Manifest preparation requires all hosts to agree and refuses
# to publish otherwise, so a Rust release landing mid-workflow fails loudly instead of shipping a manifest whose
# advertised channel matches only some of the archives.
release_rustc_bin="$(resolve_release_rustc)" || fail "could not resolve rustc to record the release Rust channel"
release_rust_channel="$(rustc_channel_version "$release_rustc_bin")" \
  || fail "could not read the release Rust channel from $release_rustc_bin"
[ -n "$release_rust_channel" ] || fail "resolved an empty release Rust channel from $release_rustc_bin"
printf '%s\n' "$release_rust_channel" > "$out_dir/rust-channel-${target}.txt"

printf 'Packaged %s\n' "$archive"
