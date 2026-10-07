//! Cargo-compatible package inputs restored from pinned adoption data, never from Cargo files.

use super::{Error, PreparedUnit, SdkLockedUnit, index_file};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Construct the same package environment for compilation and inspection, with empty values for unrecorded metadata.
///
/// The current publisher omits authors and rust-version. Their empty fallback cannot establish Cargo equivalence
/// for packages that declared those fields; recording them is required before making that claim.
pub(super) fn package_environment(unit: &PreparedUnit) -> Result<BTreeMap<String, String>, Error> {
    let version = semver::Version::parse(&unit.binding.version)?;
    let root = unit.root.to_str().ok_or("package source root is not UTF-8")?;
    let mut environment = BTreeMap::from([
        ("CARGO_MANIFEST_DIR".into(), root.into()),
        (
            "CARGO_MANIFEST_PATH".into(),
            unit.root.join("Cargo.toml").to_string_lossy().into_owned(),
        ),
        (
            "CARGO_PKG_NAME".into(),
            unit.binding.loaf.trim_start_matches("crates-io/").into(),
        ),
        ("CARGO_PKG_VERSION".into(), unit.binding.version.clone()),
        ("CARGO_PKG_VERSION_MAJOR".into(), version.major.to_string()),
        ("CARGO_PKG_VERSION_MINOR".into(), version.minor.to_string()),
        ("CARGO_PKG_VERSION_PATCH".into(), version.patch.to_string()),
        ("CARGO_PKG_VERSION_PRE".into(), version.pre.to_string()),
        (
            "CARGO_CRATE_NAME".into(),
            unit.manifest
                .get("rust")
                .and_then(|facet| facet.get("name"))
                .and_then(toml::Value::as_str)
                .ok_or("missing Rust crate name")?
                .into(),
        ),
    ]);
    for key in [
        "description",
        "homepage",
        "repository",
        "license",
        "license-file",
        "rust-version",
        "readme",
    ] {
        environment.insert(
            format!("CARGO_PKG_{}", key.to_uppercase().replace('-', "_")),
            unit.about
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .into(),
        );
    }
    let authors = unit
        .about
        .get("authors")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_default();
    environment.insert("CARGO_PKG_AUTHORS".into(), authors);
    if unit.primary {
        environment.insert("CARGO_PRIMARY_PACKAGE".into(), "1".into());
    }
    Ok(environment)
}

/// Read adoption metadata from committed event objects and bind it to the archive named by the event.
///
/// Events are processed in sequence order, so a later record-about adoption supersedes its earlier metadata.
/// No mutable index worktree or upstream Cargo manifest supplies compilation inputs.
pub(super) fn adopted_about(
    index: &Path,
    commit: &str,
    bindings: &[SdkLockedUnit],
) -> Result<BTreeMap<String, serde_json::Value>, Error> {
    if bindings.is_empty() {
        return Ok(BTreeMap::new());
    }
    let listing = std::process::Command::new("git")
        .arg("-C")
        .arg(index)
        .args(["ls-tree", "-r", "--name-only", commit, "--", "events/"])
        .output()?;
    if !listing.status.success() {
        return Err("cannot enumerate pinned adoption events".into());
    }
    let suffixes: Vec<_> = bindings
        .iter()
        .map(|unit| format!("-adopt-{}-{}.json", unit.loaf.replace('/', "-"), unit.version))
        .collect();
    let mut records = BTreeMap::new();
    for path in std::str::from_utf8(&listing.stdout)?
        .lines()
        .filter(|path| suffixes.iter().any(|suffix| path.ends_with(suffix)))
    {
        let event: serde_json::Value = serde_json::from_slice(&index_file(index, commit, path)?)?;
        if event.get("kind").and_then(serde_json::Value::as_str) != Some("adopt") {
            return Err("adoption event has inconsistent kind".into());
        }
        if let (Some(archive), Some(about)) = (
            event.get("archive").and_then(serde_json::Value::as_str),
            event.get("about"),
        ) {
            records.insert(archive.to_string(), about.clone());
        }
    }
    Ok(records)
}

/// Root of the machine-independent source coordinates that `CARGO_MANIFEST_DIR` names during compilation.
///
/// A crate may embed its manifest directory in its output, so the coordinate must be the same absolute path on every
/// machine that reproduces a unit: a home, store, temporary or compiler-sysroot directory differs between a local
/// consumer and a publishing runner. Every coordinate below it is content-checked against the verified snapshot.
fn stable_source_base() -> PathBuf {
    PathBuf::from("/tmp/incan-oven-source")
}

/// Hold a content-checked source directory at a deterministic absolute path while macros expand.
///
/// The coordinate binds the archive, features, metadata, primary role and facts. It is independent of consumer home,
/// store, temporary and compiler roots. An exclusive file lock covers checking, generated-file writes and rustc
/// execution.
pub(super) fn stable_sources(
    snapshot: &Path,
    binding: &SdkLockedUnit,
    about: &serde_json::Value,
    primary: bool,
    fact: Option<&oven_model::manifest::RustFactRecord>,
) -> Result<(PathBuf, std::fs::File), Error> {
    let key = oven_store::digest_bytes(&serde_json::to_vec(&(
        binding.identity_binding(),
        about,
        primary,
        fact,
    ))?);
    let base = stable_source_base();
    std::fs::create_dir_all(&base)?;
    let key = key.replace(':', "-");
    let lease = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(base.join(format!("{key}.lock")))?;
    lease.lock()?;
    let root = base.join(&key);
    if !root.exists() {
        let staging = tempfile::Builder::new().prefix("source-").tempdir_in(&base)?;
        copy_sources(snapshot, staging.path())?;
        std::fs::rename(staging.path(), &root)?;
    } else {
        verify_sources(snapshot, &root)?;
    }
    lease.unlock()?;
    Ok((root, lease))
}

/// Exclusive compilation lease released on success, refusal or an early error.
pub(super) struct SourceLease<'a>(&'a std::fs::File);

impl<'a> SourceLease<'a> {
    /// Lock one unit immediately before execution; never hold multiple unit locks while ordering dependencies.
    pub(super) fn acquire(file: &'a std::fs::File) -> Result<Self, Error> {
        file.lock()?;
        Ok(Self(file))
    }
}

impl Drop for SourceLease<'_> {
    /// Release the unit coordinate when compilation exits, including an error from native facts or rustc.
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Copy only regular admitted source files, preserving the archive's directory geometry.
fn copy_sources(source: &Path, destination: &Path) -> Result<(), Error> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_sources(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), target)?;
        } else {
            return Err("stable source snapshot contains a link or special file".into());
        }
    }
    Ok(())
}

/// Refuse stale or modified source bytes before reusing a deterministic coordinate.
fn verify_sources(source: &Path, destination: &Path) -> Result<(), Error> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let target_kind = std::fs::symlink_metadata(&target)?.file_type();
        if target_kind.is_symlink() || !(target_kind.is_dir() || target_kind.is_file()) {
            return Err("stable source contains a link or special file".into());
        }
        if entry.file_type()?.is_dir() {
            verify_sources(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() && std::fs::read(entry.path())? == std::fs::read(&target)? {
            continue;
        } else {
            return Err("stable source bytes differ from the admitted archive".into());
        }
    }
    Ok(())
}

/// Bind every restored compiler environment value to the receipt that authorizes rustc execution.
pub(super) fn bind_environment(
    receipt: &oven_store::OvenReceipt,
    values: &BTreeMap<String, String>,
) -> Result<oven_store::OvenReceipt, Error> {
    Ok(oven_store::receipt_with_build_unit_input(
        receipt,
        "sdk-compile-environment",
        serde_json::to_string(values)?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::{Error, adopted_about, bind_environment, package_environment};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Cargo's optional metadata stays present and empty, while only explicitly requested roots are primary.
    #[test]
    fn package_values_preserve_cargo_formats_and_root_role() -> Result<(), Error> {
        let mut unit = super::super::tests::unit("crates-io/example", "target", "[rust]\nname='example_lib'\n", &[])?;
        unit.root = PathBuf::from("/deterministic/example");
        unit.binding.version = "2.3.4-alpha.1+build".into();
        unit.about = serde_json::json!({"authors": ["First", "Second"], "license": "MIT", "rust-version": "1.80"});
        let values = package_environment(&unit)?;
        assert_eq!(values["CARGO_PKG_AUTHORS"], "First:Second");
        assert_eq!(values["CARGO_PKG_VERSION_PRE"], "alpha.1");
        assert_eq!(values["CARGO_PKG_VERSION_MAJOR"], "2");
        assert_eq!(values["CARGO_PKG_RUST_VERSION"], "1.80");
        assert_eq!(values["CARGO_PKG_DESCRIPTION"], "");
        assert_eq!(values["CARGO_CRATE_NAME"], "example_lib");
        assert_eq!(values["CARGO_MANIFEST_PATH"], "/deterministic/example/Cargo.toml");
        assert!(!values.contains_key("CARGO_PRIMARY_PACKAGE"));
        unit.primary = true;
        assert_eq!(package_environment(&unit)?["CARGO_PRIMARY_PACKAGE"], "1");
        Ok(())
    }

    /// Pinned adoption data supplies metadata even when the mutable event file has changed.
    #[test]
    fn adoption_metadata_is_archive_bound_and_commit_pinned() -> Result<(), Error> {
        let root = tempfile::tempdir()?;
        let git = super::super::tests::fixture_git;
        git(root.path(), &["init", "--quiet"])?;
        std::fs::create_dir(root.path().join("events"))?;
        let unit = super::super::tests::unit("crates-io/example", "target", "[rust]\nname='example'\n", &[])?;
        let path = root.path().join("events/000001-adopt-crates-io-example-1.0.0.json");
        let event = serde_json::json!({"kind": "adopt", "archive": unit.binding.archive_digest, "about": {"description": "pinned"}});
        std::fs::write(&path, serde_json::to_vec(&event)?)?;
        git(root.path(), &["add", "events"])?;
        git(
            root.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "chore - 1698 pinned about fixture",
            ],
        )?;
        let revision = git(root.path(), &["rev-parse", "HEAD"])?;
        let commit = std::str::from_utf8(&revision)?.trim();
        std::fs::write(&path, b"invalid mutable data")?;
        let records = adopted_about(root.path(), commit, std::slice::from_ref(&unit.binding))?;
        assert_eq!(records[&unit.binding.archive_digest]["description"], "pinned");
        assert!(adopted_about(root.path(), commit, &[])?.is_empty());
        Ok(())
    }

    /// Changing a compile-time value changes the actual receipt, not merely an auxiliary metadata digest.
    #[test]
    fn environment_changes_receipt_identity() -> Result<(), Error> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("lib.rs");
        std::fs::write(&source, "pub fn value() {}")?;
        let receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                directory.path(),
                "example",
                "1.0.0",
                "target",
                "toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source("sdk-root", &source),
        )?;
        let mut values = BTreeMap::from([("CARGO_PKG_DESCRIPTION".to_string(), "first".to_string())]);
        let first = bind_environment(&receipt, &values)?;
        values.insert("CARGO_PKG_DESCRIPTION".into(), "second".into());
        let second = bind_environment(&receipt, &values)?;
        assert_ne!(first.identity, second.identity);
        Ok(())
    }
}
