//! Oven-owned, receipt-bound permission for a declared toolchain driver unit.

use super::{Command, OvenRustcError, Path, fs, rustc_sysroot};
use oven_model::manifest::RustBinaryRole;
use oven_store::{OvenReceipt, digest_bytes};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identity input reserved for Oven's authorized driver compilation, never a fact environment entry.
pub const DRIVER_GRANT_INPUT: &str = "oven-driver-grant";

/// Exact unstable-feature permission and startup evidence for one declared Rust binary.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenDriverGrant {
    crate_name: String,
    capability: String,
    allow_features: Vec<String>,
    unstable_features: String,
    loader_policy: String,
    environment: BTreeMap<String, String>,
}

/// Authorize only the complete manifest capability under the pinned compiler with rustc-dev installed.
/// Ordinary units receive no grant; partial or unsupported declarations fail before compiler execution.
pub fn authorize_driver_grant(role: &RustBinaryRole, rustc: &Path) -> Result<Option<String>, OvenRustcError> {
    // ---- Declared unit capability ----
    if role.unstable_features.is_empty() && role.toolchain_components.is_empty() && role.sysroot_dependencies.is_empty()
    {
        return Ok(None);
    }
    let invalid = |message: String| OvenRustcError::InvalidInput {
        field: "driver capability",
        message,
    };
    if role.unstable_features != ["rustc_private"]
        || role.toolchain_components != ["rustc-dev"]
        || role.sysroot_dependencies != ["rustc_driver"]
    {
        return Err(invalid(
            "requires rustc_private, rustc-dev and rustc_driver on the same declared unit".into(),
        ));
    }

    // ---- Pinned compiler and rustc-dev evidence ----
    let root = rustc_sysroot(rustc)?
        .canonicalize()
        .map_err(|source| OvenRustcError::Io {
            path: rustc.into(),
            source,
        })?;
    let mut probe = Command::new(rustc);
    super::clear_inherited_cargo_environment(&mut probe);
    let output = probe.arg("-vV").output().map_err(|source| OvenRustcError::Io {
        path: rustc.into(),
        source,
    })?;
    if !output.status.success() {
        return Err(invalid("cannot query pinned compiler identity".into()));
    }
    let identity = String::from_utf8(output.stdout).map_err(|error| invalid(error.to_string()))?;
    let host = super::rustc_host_target(rustc)?;
    let metadata = root.join("lib/rustlib").join(host).join("lib");
    let entries = fs::read_dir(&metadata).map_err(|source| OvenRustcError::Io {
        path: metadata.clone(),
        source,
    })?;
    let mut has_middle = false;
    for entry in entries {
        let entry = entry.map_err(|source| OvenRustcError::Io {
            path: metadata.clone(),
            source,
        })?;
        has_middle |= entry.file_name().to_string_lossy().starts_with("librustc_middle-");
    }
    if !has_middle {
        return Err(invalid(format!(
            "missing rustc-dev metadata in {}; install rustc-dev for 1.98.0",
            metadata.display()
        )));
    }
    if !identity.lines().any(|line| line == "release: 1.98.0") {
        return Err(invalid("requires pinned rustc 1.98.0".into()));
    }
    let (library, bytes) = driver_library(&metadata)?;

    // ---- Receipt identity and startup inputs ----
    let grant = OvenDriverGrant {
        crate_name: role.name.replace('-', "_"),
        capability: "rustc_private".into(),
        allow_features: vec!["rustc_private".into()],
        unstable_features: "bootstrap-derived:Cheat".into(),
        loader_policy: "pinned-sysroot-first".into(),
        environment: BTreeMap::from([
            ("INCAN_DRIVER_SYSROOT".into(), root.to_string_lossy().into_owned()),
            ("INCAN_DRIVER_RUSTC_IDENTITY".into(), identity),
            (
                "INCAN_DRIVER_LIBRARY".into(),
                library
                    .strip_prefix(&root)
                    .map_err(|error| invalid(error.to_string()))?
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "INCAN_DRIVER_LIBRARY_DIGEST".into(),
                digest_bytes(&bytes).trim_start_matches("sha256:").into(),
            ),
        ]),
    };
    serde_json::to_string(&grant)
        .map(Some)
        .map_err(|error| invalid(error.to_string()))
}

/// Resolve the unique pinned driver shared library and its bytes; absent rustc-dev is an explicit refusal.
fn driver_library(library_dir: &Path) -> Result<(std::path::PathBuf, Vec<u8>), OvenRustcError> {
    let invalid = |message: String| OvenRustcError::InvalidInput {
        field: "rustc-dev",
        message,
    };
    let mut libraries = fs::read_dir(library_dir)
        .map_err(|source| OvenRustcError::Io {
            path: library_dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: library_dir.to_path_buf(),
            source,
        })?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("librustc_driver-") && (name.ends_with(".dylib") || name.ends_with(".so"))
            })
        })
        .collect::<Vec<_>>();
    libraries.sort();
    let [library] = libraries.as_slice() else {
        return Err(invalid(
            "missing unambiguous rustc-dev driver library; install rustc-dev for 1.98.0".into(),
        ));
    };
    let bytes = fs::read(library).map_err(|source| OvenRustcError::Io {
        path: library.clone(),
        source,
    })?;
    Ok((library.clone(), bytes))
}

/// Apply a verified unit grant after ambient stripping, with matching crate name and a feature allowlist.
/// A receipt for another unit or an unsupported grant is refused rather than broadening permission.
pub(super) fn apply_driver_grant(
    command: &mut Command,
    receipt: &OvenReceipt,
    crate_name: &str,
) -> Result<(), OvenRustcError> {
    let Some(encoded) = receipt.sources.build_unit_inputs.get(DRIVER_GRANT_INPUT) else {
        return Ok(());
    };
    let invalid = |message: String| OvenRustcError::InvalidInput {
        field: "driver grant",
        message,
    };
    let grant: OvenDriverGrant = serde_json::from_str(encoded).map_err(|error| invalid(error.to_string()))?;
    if grant.crate_name != crate_name
        || grant.capability != "rustc_private"
        || grant.allow_features != ["rustc_private"]
        || grant.unstable_features != "bootstrap-derived:Cheat"
        || grant.loader_policy != "pinned-sysroot-first"
    {
        return Err(invalid(
            "grant does not authorize this unit and its exact rustc_private feature".into(),
        ));
    }

    // ---- Admitted startup identity environment ----
    if grant.environment.len() != 4
        || grant.environment.keys().any(|name| {
            !matches!(
                name.as_str(),
                "INCAN_DRIVER_SYSROOT"
                    | "INCAN_DRIVER_RUSTC_IDENTITY"
                    | "INCAN_DRIVER_LIBRARY"
                    | "INCAN_DRIVER_LIBRARY_DIGEST"
            )
        })
    {
        return Err(invalid(
            "grant must contain exactly the four driver startup identity inputs".into(),
        ));
    }

    // ---- One crate-scoped compiler invocation ----
    command
        .env("RUSTC_BOOTSTRAP", crate_name)
        .arg("-Zallow-features=rustc_private");
    if let (Some(root), Some(library)) = (
        grant.environment.get("INCAN_DRIVER_SYSROOT"),
        grant.environment.get("INCAN_DRIVER_LIBRARY"),
    ) {
        let path = Path::new(root).join(library);
        let directory = path
            .parent()
            .ok_or_else(|| invalid("driver library has no parent".into()))?;
        command
            .arg("-C")
            .arg(format!("link-arg=-Wl,-rpath,{}", directory.display()));
    }
    for (name, value) in grant.environment {
        command.env(name, value);
    }
    command.args(["-C", "rpath"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project, receipt_with_build_unit_input};

    /// The declared grant changes both receipt identities and applies only to its named compilation.
    #[test]
    fn grant_is_unit_scoped_and_identity_bound() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("loaf.toml"), "[project]\nname = \"grant-test\"\n")?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let request = OvenGeneratedProjectRequest::new(
            root.path(),
            "grant-test",
            "0.1.0",
            "host",
            "compiler",
            "debug",
            Vec::new(),
        )
        .with_generated_source("rust-unit", &source);
        let ordinary = receipt_generated_project(&request)?;
        let mut grant = OvenDriverGrant {
            crate_name: "declared_driver".into(),
            capability: "rustc_private".into(),
            allow_features: vec!["rustc_private".into()],
            unstable_features: "bootstrap-derived:Cheat".into(),
            loader_policy: "pinned-sysroot-first".into(),
            environment: BTreeMap::from([
                ("INCAN_DRIVER_SYSROOT".into(), "/fixture".into()),
                ("INCAN_DRIVER_LIBRARY".into(), "lib/libdriver.so".into()),
                ("INCAN_DRIVER_RUSTC_IDENTITY".into(), "pinned compiler".into()),
                ("INCAN_DRIVER_LIBRARY_DIGEST".into(), "digest".into()),
            ]),
        };
        let authorized = receipt_with_build_unit_input(&ordinary, DRIVER_GRANT_INPUT, serde_json::to_string(&grant)?)?;
        assert_ne!(ordinary.identity, authorized.identity);
        assert_ne!(ordinary.build_unit_identity, authorized.build_unit_identity);
        let mut command = Command::new("rustc");
        command.env("RUSTC_BOOTSTRAP", "ambient");
        command.env_remove("RUSTC_BOOTSTRAP");
        apply_driver_grant(&mut command, &authorized, "declared_driver")?;
        assert!(
            command.get_envs().any(
                |(name, value)| name == "RUSTC_BOOTSTRAP" && value == Some(std::ffi::OsStr::new("declared_driver"))
            )
        );
        assert!(command.get_args().any(|arg| arg == "-Zallow-features=rustc_private"));
        assert!(apply_driver_grant(&mut Command::new("rustc"), &authorized, "undeclared_unit").is_err());
        let mut plain = Command::new("rustc");
        apply_driver_grant(&mut plain, &ordinary, "undeclared_unit")?;
        assert!(
            !plain
                .get_envs()
                .any(|(name, value)| name == "RUSTC_BOOTSTRAP" && value.is_some())
        );
        let changed = receipt_with_build_unit_input(&ordinary, DRIVER_GRANT_INPUT, "changed")?;
        assert_ne!(authorized.identity, changed.identity);
        assert!(apply_driver_grant(&mut plain, &changed, "declared_driver").is_err());
        grant.environment.insert("RUSTC_BOOTSTRAP".into(), "1".into());
        let forged = receipt_with_build_unit_input(&ordinary, DRIVER_GRANT_INPUT, serde_json::to_string(&grant)?)?;
        assert!(apply_driver_grant(&mut plain, &forged, "declared_driver").is_err());
        Ok(())
    }

    /// Permission cannot be inferred from a component declaration or ambient compiler controls.
    #[test]
    fn undeclared_and_incomplete_units_cannot_receive_a_grant() -> Result<(), Box<dyn std::error::Error>> {
        let mut role = RustBinaryRole {
            name: "ordinary".into(),
            path: "src/main.rs".into(),
            unstable_features: Vec::new(),
            toolchain_components: Vec::new(),
            sysroot_dependencies: Vec::new(),
        };
        assert!(authorize_driver_grant(&role, Path::new("unused-rustc"))?.is_none());
        role.toolchain_components.push("rustc-dev".into());
        assert!(authorize_driver_grant(&role, Path::new("unused-rustc")).is_err());
        assert!(super::super::environment::direct_rustc_excludes_inherited_environment(
            std::ffi::OsStr::new("RUSTC_BOOTSTRAP")
        ));
        Ok(())
    }
}
