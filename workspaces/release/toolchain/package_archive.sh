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

# Clear ambient Cargo/rustc-wrapper state before an internal Cargo invocation, mirroring
# `clear_inherited_cargo_environment` in loaves/oven/oven_rustc/src/rustc.rs. This script's own `cargo metadata`
# calls are the release support workspace's authority; they must not inherit CARGO_* state
# (target dir, build jobs, an rustc wrapper meant for a different build, etc.) from an
# already-running, possibly nested Cargo/toolchain-managed parent process such as `cargo test`.
# Unlike the Rust helper, this deliberately keeps `CARGO_HOME` -- callers (CI, local testing) rely
# on it to name the prewarmed registry cache, and this script has no explicit replacement value to
# re-inject the way Rust call sites do immediately after clearing.
clear_inherited_cargo_environment() {
  local name
  for name in CARGO "${!CARGO_@}" RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER; do
    [ "$name" = "CARGO_HOME" ] && continue
    unset "$name"
  done
}

# Re-run one failed Cargo invocation under `strace`, filtered to syscalls that could plausibly
# explain an unusual, non-Cargo-typical exit status (network/socket address-family errors,
# process/fork failures) with no diagnostic text on stderr. Best-effort only: installs a local
# strace copy on demand when the host doesn't already have one, and silently produces no output
# when that isn't possible (no network, no package manager, unsupported platform) rather than
# masking the original failure. Prints at most a bounded tail so the caller's error stays legible.
describe_failure_via_strace() {
  if ! command -v strace >/dev/null 2>&1; then
    if command -v apt-get >/dev/null 2>&1; then
      apt-get install -y --no-install-recommends strace >/tmp/package_archive_strace_install.log 2>&1 || true
    fi
  fi
  if ! command -v strace >/dev/null 2>&1; then
    printf '<strace unavailable for follow-up diagnosis>'
    return 0
  fi
  local trace_log
  trace_log="$(mktemp)"
  ( clear_inherited_cargo_environment; strace -f -yy -tt -e trace=network,process -o "$trace_log" "$@" >/dev/null 2>&1 ) || true
  tail -c 6000 "$trace_log" 2>/dev/null
  rm -f "$trace_log"
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

# The archive's support workspace inherits the same dependency table as the checkout, so a member manifest that says
# `serde = { workspace = true }` resolves in both. The nine support crates name each other through that table too;
# their entries are rewritten to the archive layout, where every support crate sits beside this manifest and the
# stdlib ring's version requirement still applies. Path entries for crates the archive does not ship are dropped.
workspace_dependencies() {
  awk '
    /^\[workspace.dependencies\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section && /^(incan_lang|incan_derive|incan_std_core|incan_std_data|incan_std_async|incan_std_web|incan_std_testing|incan_vocab|incan_web_macros) = / {
      entry = $0
      sub(/^[a-z_]+ = \{ path = "[^"]+"/, $1 " = { path = \"" $1 "\"", entry)
      print entry
      next
    }
    in_section && /path = "/ { next }
    in_section && /^[A-Za-z0-9_-]+ = / { print }
  ' Cargo.toml
}
workspace_dependencies_table="$(workspace_dependencies)"
[ -n "$workspace_dependencies_table" ] || fail "could not read [workspace.dependencies] from Cargo.toml"

if [ -n "${TOOLCHAIN_RELEASE:-}" ]; then
  release="$TOOLCHAIN_RELEASE"
elif [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
  release="${GITHUB_REF_NAME}"
else
  release="v${version}"
fi

incan_bin="${INCAN_BIN:-target/release/incan}"
incan_lsp_bin="${INCAN_LSP_BIN:-target/release/incan-lsp}"
stdlib_dir="${INCAN_STDLIB_SOURCE_DIR:-loaves/stdlib}"
distribution_profile="${INCAN_SDK_DISTRIBUTION_PROFILE:-full}"
[ -x "$incan_bin" ] || fail "incan binary is not executable: $incan_bin"
[ -x "$incan_lsp_bin" ] || fail "incan-lsp binary is not executable: $incan_lsp_bin"
[ -d "$stdlib_dir" ] || fail "stdlib root does not exist: $stdlib_dir"
[ -f "$stdlib_dir/sdk-components.toml" ] || fail "stdlib root is missing its component catalog: $stdlib_dir"
[ -f "$stdlib_dir/testing/src/testing.incn" ] || fail "stdlib root is missing the testing component's source: $stdlib_dir"
# The checkout keeps each support crate in its ring; the archive keeps them side by side under `crates/`, the layout
# every installed toolchain and staged runtime has. This table mirrors `development_support_crate_dir` in
# `oven_model::toolchain_layout`.
support_crate_source() {
  case "$1" in
    incan_lang) printf 'loaves/kernel/incan_lang' ;;
    incan_vocab) printf 'loaves/kernel/incan_vocab' ;;
    incan_derive) printf 'loaves/stdlib/derive/incan_derive' ;;
    incan_web_macros) printf 'loaves/stdlib/derive/incan_web_macros' ;;
    incan_std_core) printf 'loaves/stdlib/core/rust' ;;
    incan_std_data) printf 'loaves/stdlib/data/rust' ;;
    incan_std_async) printf 'loaves/stdlib/async/rust' ;;
    incan_std_web) printf 'loaves/stdlib/web/rust' ;;
    incan_std_testing) printf 'loaves/stdlib/testing/rust' ;;
    *) printf 'crates/%s' "$1" ;;
  esac
}

for support_crate in incan_lang incan_derive incan_std_core incan_std_data incan_std_async incan_std_web incan_std_testing incan_vocab incan_web_macros; do
  [ -f "$(support_crate_source "$support_crate")/Cargo.toml" ] || fail "support crate is missing: $(support_crate_source "$support_crate")"
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
  local staged_stdlib="$package_dir/stdlib"
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
release_policy_publisher_home=""

cleanup_release_staging() {
  if [ -n "$release_provider_store" ]; then
    rm -rf "$release_provider_store"
  fi
  if [ -n "$release_policy_publisher_home" ]; then
    rm -rf "$release_policy_publisher_home"
  fi
}
trap cleanup_release_staging EXIT

rm -rf "$package_dir"
mkdir -p "$package_dir/bin" "$package_dir/crates"
cp "$incan_bin" "$package_dir/bin/incan"
cp "$incan_lsp_bin" "$package_dir/bin/incan-lsp"
for support_crate in incan_lang incan_derive incan_std_core incan_std_data incan_std_async incan_std_web incan_std_testing incan_vocab incan_web_macros; do
  support_destination="$package_dir/crates/${support_crate}"
  stage_tracked_tree "$(support_crate_source "$support_crate")" "$support_destination"
done
# The standard library's sources live in the ring, one component directory each; the installed toolchain keeps them
# as `stdlib/` with the same component layout, one of the roots the compiler's stdlib-root policy looks for. Only
# what a compiler reads ships: the catalog, each component's manifest, Incan sources and vocab companion. The derive
# crates and each component's Rust facet ship as support crates under `crates/`, and tests and READMEs stay in the
# repository.
staged_stdlib_root="$package_dir/stdlib"
stage_tracked_tree "loaves/stdlib" "$staged_stdlib_root"
rm -rf "$staged_stdlib_root/derive" "$staged_stdlib_root/README.md"
for component_dir in "$staged_stdlib_root"/*/; do
  rm -rf "${component_dir}rust" "${component_dir}tests" "${component_dir}README.md"
done
# The interop component's vocab companion names `incan_vocab` by path. In the checkout that is the kernel ring; in
# the archive the crate sits under `crates/`, three levels up from the companion and one across.
staged_companion="$staged_stdlib_root/interop/vocab_companion/Cargo.toml"
[ -f "$staged_companion" ] || fail "release package is missing the interop vocab companion manifest"
sed -i.bak 's|incan_vocab = { path = "../../../kernel/incan_vocab" }|incan_vocab = { path = "../../../crates/incan_vocab" }|' "$staged_companion" \
  && rm -f "$staged_companion.bak"
grep -q 'path = "../../../crates/incan_vocab"' "$staged_companion" \
  || fail "release package's interop vocab companion does not name the archived incan_vocab crate"
if [ -z "${INCAN_SDK_PROVIDER_SEED_DIR:-}" ]; then
  release_provider_store="$package_dir/share/incan"
fi
cat > "$package_dir/crates/Cargo.toml" <<WORKSPACE
[workspace]
members = [
    "incan_lang",
    "incan_derive",
    "incan_std_async",
    "incan_std_core",
    "incan_std_data",
    "incan_std_testing",
    "incan_std_web",
    "incan_vocab",
    "incan_web_macros",
]
default-members = [
    "incan_lang",
    "incan_derive",
    "incan_std_async",
    "incan_std_core",
    "incan_std_data",
    "incan_std_testing",
    "incan_std_web",
    "incan_vocab",
    "incan_web_macros",
]
resolver = "2"

[workspace.package]
version = "${version}"
edition = "2024"
rust-version = "1.98"
license = "Apache-2.0"
authors = ["Danny Meijer <dannys.code.corner@gmail.com>"]
repository = "https://github.com/encero-systems/incan"
homepage = "https://github.com/encero-systems/incan"
keywords = ["programming-language", "compiler", "rust", "python"]
categories = ["compilers", "development-tools"]

[workspace.dependencies]
${workspace_dependencies_table}

# This non-default package keeps the locked registry-source authority used by the built-in release Loaf fixtures. It
# is metadata-only release infrastructure; normal support-workspace commands operate on the default members above.
[package]
name = "incan-release-inspection-authority"
version = "${version}"
edition = "2024"
publish = false

[lib]
path = "release-inspection-authority.rs"
WORKSPACE
printf '%s\n' '#![allow(dead_code)]' > "$package_dir/crates/release-inspection-authority.rs"
git show HEAD:loaves/oven/oven_rustc/src/fixtures/release_stdlib.toml \
  | awk '
      /^\[rust-dependencies\]$/ {
        print "[dependencies]"
        copy_dependencies = 1
        next
      }
      copy_dependencies { print }
    ' >> "$package_dir/crates/Cargo.toml" \
  || fail "could not stage the checked release Loaf inspection dependency authority"
git show HEAD:Cargo.lock > "$package_dir/crates/Cargo.lock" \
  || fail "could not stage the verified workspace Cargo.lock"
# Oven's compiler-suite test roots deliberately poison `cargo` on `PATH` with a guard binary that
# rejects any unexpected invocation, to catch tests that should never touch Cargo; this script is
# a legitimate exception to that guard (its caller, tests/toolchain_installer_tests.rs, is the one
# test that genuinely needs to package a real release archive). Confirmed by direct CI diagnosis:
# the guard sandbox clears `CARGO_HOME` and redirects `HOME` to an isolated, per-root scratch
# directory, so neither can locate the real Cargo; the real Cargo is still on `PATH` (rustup's
# normal install location), just shadowed because the guard's own directory -- always somewhere
# under this repository's own `target/` tree -- is prepended in front of it. Resolve Cargo in a
# way that specific trick cannot intercept, preferring the most explicit source available:
#   1. `CARGO_BIN`, when the caller names a verified real Cargo directly.
#   2. The first `cargo` on `PATH` whose directory is NOT inside a repository `target/` tree. A real,
#      system-installed Cargo is never legitimately located there; only a guard or build artifact would be.
explicit_cargo_bin="${CARGO_BIN:-}"
cargo_bin="$(workspaces/release/toolchain/resolve_release_cargo.sh "$explicit_cargo_bin")" \
  || fail "could not resolve authoritative Cargo for release packaging"
# Resolve the real Cargo home now, before the guarded `$HOME` can hide the offline registry cache.
# A selected Cargo can be either a user shim or a toolchain executable, so its parent directories
# alone are not authoritative for the registry location.
#
# This sibling derivation breaks for a package-manager rustup install: Homebrew's `rustup` formula
# keeps its `cargo` shim under `<prefix>/opt/rustup/bin/`, so deriving two directories up lands on
# the Homebrew prefix rather than a real Cargo home with a populated offline registry cache. `.cargo`
# and `.rustup` are always siblings under the true user home even when that install's shim lives
# elsewhere, and `$RUSTUP_HOME` reliably names the real `.rustup` even in the guarded/sandboxed case
# where `$HOME` itself is redirected to an isolated scratch directory but `$RUSTUP_HOME` still points
# at the real one (rustup exports it once initialized, independent of `$HOME`). Prefer deriving from
# `${RUSTUP_HOME:-$HOME/.rustup}`'s sibling `.cargo` first; fall back to `$HOME/.cargo` for hosts
# where neither `$RUSTUP_HOME` nor `.cargo`/`.rustup` are siblings of the resolved Cargo binary, and
# finally to the original sibling-of-Cargo derivation.
rustup_home_dir="${RUSTUP_HOME:-$HOME/.rustup}"
if [ -d "$(dirname "$rustup_home_dir")/.cargo/registry" ]; then
  cargo_home_dir="$(dirname "$rustup_home_dir")/.cargo"
elif [ -d "$HOME/.cargo/registry" ]; then
  cargo_home_dir="$HOME/.cargo"
else
  cargo_home_dir="$(dirname "$(dirname "$cargo_bin")")"
fi
# A Cargo resolved outside the repository guard is commonly Rustup's shim. Ask its sibling Rustup for the active
# toolchain's exact Cargo instead of selecting the first directory below `toolchains/`: filesystem order is not
# toolchain authority. A caller that sets `RUSTUP_TOOLCHAIN` selects that exact toolchain; otherwise Rustup's active
# override/default applies. The release workflow supplies the supported version explicitly. A caller-provided
# `CARGO_BIN` remains exact authority and a non-Rustup Cargo installation remains unchanged.
# `cargo metadata --offline` below resolves its registry cache from `$CARGO_HOME` (default
# `$HOME/.cargo`), which is equally a victim of the guard's `$HOME` redirect: the offline cache
# prewarmed into the real Cargo home would otherwise be invisible. `clear_inherited_cargo_environment`
# deliberately preserves `CARGO_HOME`, so exporting the real value here, once, is sufficient for
# every Cargo invocation this script makes afterward.
: "${CARGO_HOME:=$cargo_home_dir}"
export CARGO_HOME
printf 'package_archive: DEBUG cargo resolution: CARGO_BIN=%s CARGO_HOME=%s HOME=%s resolved=%s PATH=%s\n' \
  "${CARGO_BIN:-<unset>}" "${CARGO_HOME:-<unset>}" "${HOME:-<unset>}" "$cargo_bin" "$PATH" >&2
# The archive ships a deliberately reduced support workspace, so its lock must describe that workspace rather than the
# complete compiler repository. Seed resolution from the verified repository lock, reconcile only the removed workspace
# members without network access, then prove the shipped closure is stable under Cargo's locked mode.
#
# Each call runs in its own `( ... )` subshell so `clear_inherited_cargo_environment` only scopes that one Cargo
# invocation; the SDK component build later in this script still needs its own ambient Cargo/rustc-wrapper state.
set +e
metadata_error="$(
  clear_inherited_cargo_environment
  "$cargo_bin" metadata \
    --offline \
    --format-version 1 \
    --manifest-path "$package_dir/crates/Cargo.toml" 2>&1 >/dev/null
)"
metadata_exit=$?
set -e
if [ "$metadata_exit" -ne 0 ]; then
  strace_summary="$(describe_failure_via_strace "$cargo_bin" metadata --offline --format-version 1 --manifest-path "$package_dir/crates/Cargo.toml")"
  fail "could not derive the release support workspace lock from the verified repository lock (exit ${metadata_exit}): ${metadata_error}
strace (network/process syscalls, tail): ${strace_summary}"
fi
set +e
metadata_error="$(
  clear_inherited_cargo_environment
  "$cargo_bin" metadata \
    --locked \
    --offline \
    --format-version 1 \
    --manifest-path "$package_dir/crates/Cargo.toml" 2>&1 >/dev/null
)"
metadata_exit=$?
set -e
if [ "$metadata_exit" -ne 0 ]; then
  strace_summary="$(describe_failure_via_strace "$cargo_bin" metadata --locked --offline --format-version 1 --manifest-path "$package_dir/crates/Cargo.toml")"
  fail "release support workspace lock is not reproducible (exit ${metadata_exit}): ${metadata_error}
strace (network/process syscalls, tail): ${strace_summary}"
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
# still require these checked `.incn` declarations. The bundle ships as `stdlib/`, the ring's own layout minus the
# Rust facets, which are support crates.
[ -f "$package_dir/stdlib/sdk-components.toml" ] \
  || fail "release package is missing the built-in stdlib component catalog"
[ -f "$package_dir/stdlib/core/src/prelude.incn" ] \
  || fail "release package is missing the built-in stdlib prelude source"
[ -f "$package_dir/stdlib/testing/src/testing.incn" ] \
  || fail "release package is missing the built-in stdlib testing source"
[ ! -d "$package_dir/crates/incan_stdlib" ] || fail "the retired incan_stdlib crate unexpectedly entered the package"

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
  command -v jq >/dev/null 2>&1 \
    || fail "production release packaging requires jq to read the structured Oven bake report"
  rustc_bin="$(resolve_release_rustc)" || fail "could not resolve rustc for the release-only Loaf publisher"
  [ -f "workspaces/oven/loaf.toml" ] \
    || fail "release policy project is missing workspaces/oven/loaf.toml"
  [ -f "workspaces/oven/src/plan_json_main.incn" ] \
    || fail "release policy entrypoint is missing workspaces/oven/src/plan_json_main.incn"
  # Establish the ordinary package-local release family before compiling the policy engine. The staged `incan`
  # resolves compiler-owned Loafs relative to its own executable; publishing here keeps that lookup inside this
  # package and prevents an ambient development-checkout Loaf from becoming engine build authority.
  "$package_dir/bin/incan" oven legacy-cargo bake-loafs \
    --compiler-root "$package_dir" \
    --output "$loaf_root" \
    --envelope release \
    --sdk-inventory "$sdk_seed_root/sdk-inventory.json" \
    --cargo "$cargo_bin" \
    --rustc "$rustc_bin" \
    --format json >/dev/null \
    || fail "could not bake the package-local release Oven Loaf family"
  release_policy_publisher_home="$(mktemp -d "${TMPDIR:-/tmp}/incan-release-policy-${target}.XXXXXX")"
  policy_bake_report="$release_policy_publisher_home/core-engine-bake.json"
  INCAN_HOME="$release_policy_publisher_home" \
    INCAN_STDLIB="$staged_stdlib_root" \
    INCAN_SDK_INVENTORY="$sdk_seed_root/sdk-inventory.json" \
    INCAN_TOOLCHAIN_CRATES_DIR="$package_dir/crates" \
    CARGO="$cargo_bin" \
    RUSTC="$rustc_bin" \
    "$package_dir/bin/incan" oven bake \
      --project "workspaces/oven" \
      --target "$target" \
      --format json > "$policy_bake_report" \
    || fail "could not explicitly bake the release policy project"
  policy_output="$(workspaces/release/toolchain/select_release_policy_output.sh "$policy_bake_report" "$target")" \
    || fail "could not select the exact target-bound release core_engine ProjectOutput"
  policy_engine_store="${policy_output%%	*}"
  policy_engine_identity="${policy_output#*	}"
  [ -n "$policy_engine_store" ] && [ -n "$policy_engine_identity" ] && [ "$policy_engine_store" != "$policy_engine_identity" ] \
    || fail "release policy bake selection did not report store and artifact identity"
  # Re-enter the same explicit publisher with the exact ProjectOutput. Existing ordinary Loafs are reused; the
  # committed generation is replaced atomically by the envelope whose release member names this engine identity.
  "$package_dir/bin/incan" oven legacy-cargo bake-loafs \
    --compiler-root "$package_dir" \
    --output "$loaf_root" \
    --envelope release \
    --sdk-inventory "$sdk_seed_root/sdk-inventory.json" \
    --cargo "$cargo_bin" \
    --rustc "$rustc_bin" \
    --policy-engine-store "$policy_engine_store" \
    --policy-engine-identity "$policy_engine_identity" \
    --policy-engine-target "$target" \
    --format json >/dev/null \
    || fail "could not bake the release Oven Loaf envelope"
  packaged_policy_identity="$(jq -er '.release_store_member.artifact_identity | select(type == "string" and length > 0)' "$loaf_root/envelope.json")" \
    || fail "release Oven Loaf envelope did not retain its policy-engine member"
  [ "$packaged_policy_identity" = "$policy_engine_identity" ] \
    || fail "release Oven Loaf envelope retained a different policy-engine identity"
fi
[ -d "$loaf_root" ] || fail "release package is missing Oven Loafs"
[ "$(find "$loaf_root" -name loaf.json -type f | wc -l | tr -d ' ')" = "2" ] \
  || fail "release package must contain one release core and one debug Oven foundation Loaf"

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
