#!/usr/bin/env bash
set -euo pipefail

# Warm the shared Cargo registry cache for the release support workspace that
# package_archive.sh synthesizes at packaging time.
#
# package_archive.sh resolves that workspace with `cargo metadata --offline`, but nothing
# else in the repository's normal build (`cargo fetch --locked` against the compiler
# workspace, the vocab companion, or the Oven Loaf test fixtures) is guaranteed to have
# already fetched every crate named in `loaves/oven/oven_rustc/src/fixtures/release_stdlib.toml`. When one is
# missing from the offline registry cache, packaging fails with "could not derive the
# release support workspace lock from the verified repository lock" and gives no indication
# that a network-enabled prewarm step was skipped. Run this script -- with network access,
# before any `CARGO_NET_OFFLINE=true` step -- so the same workspace `cargo metadata`
# resolves offline later.

fail() {
  printf 'fetch_release_support_workspace_sources: %s\n' "$*" >&2
  exit 1
}

# Clear ambient Cargo/rustc-wrapper state before the internal Cargo invocation below, mirroring
# `clear_inherited_cargo_environment` in loaves/oven/oven_rustc/src/rustc.rs and package_archive.sh's own copy.
# Deliberately keeps `CARGO_HOME` -- callers rely on it to name the cache this script warms.
clear_inherited_cargo_environment() {
  local name
  for name in CARGO "${!CARGO_@}" RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER; do
    [ "$name" = "CARGO_HOME" ] && continue
    unset "$name"
  done
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$repo_root"

# The checkout keeps each support crate in its ring; the archive keeps them side by side under `crates/`, the layout
# every installed toolchain and staged runtime has. This table mirrors `development_support_crate_dir` in
# `oven_model::toolchain_layout`.
support_crate_source() {
  case "$1" in
    incan_core) printf 'loaves/kernel/incan_core' ;;
    incan_vocab) printf 'loaves/kernel/incan_vocab' ;;
    incan_derive) printf 'loaves/stdlib/derive/incan_derive' ;;
    incan_web_macros) printf 'loaves/stdlib/derive/incan_web_macros' ;;
    *) printf 'crates/%s' "$1" ;;
  esac
}

for support_crate in incan_core incan_derive incan_stdlib incan_vocab incan_web_macros; do
  [ -f "$(support_crate_source "$support_crate")/Cargo.toml" ] || fail "support crate is missing: $(support_crate_source "$support_crate")"
done
[ -f "loaves/oven/oven_rustc/src/fixtures/release_stdlib.toml" ] || fail "release stdlib dependency fixture is missing"

workdir="$(mktemp -d)"
trap 'rm -rf -- "$workdir"' EXIT
package_dir="$workdir/crates"
mkdir -p "$package_dir"

archive_counter=0
stage_tracked_tree() {
  local source_tree="${1#./}"
  local destination="$2"
  archive_counter=$((archive_counter + 1))
  local source_archive="$workdir/.tracked-source-${archive_counter}.tar"
  mkdir -p "$destination"
  git archive --format=tar --output="$source_archive" "HEAD:${source_tree}" \
    || fail "could not archive tracked source tree: ${source_tree}"
  tar -C "$destination" -xf "$source_archive" \
    || fail "could not extract tracked source tree into: ${destination}"
  rm "$source_archive"
}

for support_crate in incan_core incan_derive incan_stdlib incan_vocab incan_web_macros; do
  stage_tracked_tree "$(support_crate_source "$support_crate")" "$package_dir/${support_crate}"
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
# `serde = { workspace = true }` resolves in both. The five support crates name each other through that table too;
# their entries are rewritten to the archive layout, where every support crate sits beside this manifest and the
# stdlib ring's version requirement still applies. Path entries for crates the archive does not ship are dropped.
workspace_dependencies() {
  awk '
    /^\[workspace.dependencies\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section && /^(incan_core|incan_derive|incan_stdlib|incan_vocab|incan_web_macros) = / {
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

cat > "$package_dir/Cargo.toml" <<WORKSPACE
[workspace]
members = [
    "incan_core",
    "incan_derive",
    "incan_stdlib",
    "incan_vocab",
    "incan_web_macros",
]
default-members = [
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
rust-version = "1.98"
license = "Apache-2.0"
authors = ["Danny Meijer <dannys.code.corner@gmail.com>"]
repository = "https://github.com/encero-systems/incan"
homepage = "https://github.com/encero-systems/incan"
keywords = ["programming-language", "compiler", "rust", "python"]
categories = ["compilers", "development-tools"]

[workspace.dependencies]
${workspace_dependencies_table}

[package]
name = "incan-release-inspection-authority"
version = "${version}"
edition = "2024"
publish = false

[lib]
path = "release-inspection-authority.rs"
WORKSPACE
printf '%s\n' '#![allow(dead_code)]' > "$package_dir/release-inspection-authority.rs"
git show HEAD:loaves/oven/oven_rustc/src/fixtures/release_stdlib.toml \
  | awk '
      /^\[rust-dependencies\]$/ {
        print "[dependencies]"
        copy_dependencies = 1
        next
      }
      copy_dependencies { print }
    ' >> "$package_dir/Cargo.toml" \
  || fail "could not stage the checked release Loaf inspection dependency authority"
git show HEAD:Cargo.lock > "$package_dir/Cargo.lock" \
  || fail "could not stage the verified workspace Cargo.lock"

cargo_bin="$(command -v cargo)" || fail "could not resolve Cargo for the release support workspace"
(
  clear_inherited_cargo_environment
  "$cargo_bin" fetch --manifest-path "$package_dir/Cargo.toml"
) || fail "could not fetch the release support workspace's registry dependencies"
