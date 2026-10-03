//! Common Cargo compile-environment validation for reconstructed archives.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::super::OvenLegacyNativeInvocation;

/// Compile environment values that must agree across every object in one archive.
pub(super) struct CommonCompileEnvironment {
    /// Canonical crate source owner.
    pub(super) manifest_dir: PathBuf,
    /// Canonical generated-output owner.
    pub(super) out_dir: PathBuf,
    /// Target triple supplied to every compile.
    pub(super) target: String,
}

impl CommonCompileEnvironment {
    /// Read and validate the common Cargo compile environment for an archive.
    pub(super) fn from_invocations(invocations: &[&OvenLegacyNativeInvocation]) -> Result<Self, String> {
        Ok(Self {
            manifest_dir: common_environment_path(invocations, "CARGO_MANIFEST_DIR")?,
            out_dir: common_environment_path(invocations, "OUT_DIR")?,
            target: common_environment_value(invocations, "TARGET")?,
        })
    }
}

/// Resolve an observed relative path against the captured working directory without accepting a missing path.
pub(super) fn resolve_observed_path(working_directory: &str, value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(working_directory).join(path)
    };
    fs::canonicalize(&path).map_err(|error| format!("cannot resolve observed path: {error}"))
}

/// Require one common environment value across every compile invocation.
pub(super) fn common_environment_value(
    invocations: &[&OvenLegacyNativeInvocation],
    name: &str,
) -> Result<String, String> {
    let values = invocations
        .iter()
        .map(|invocation| {
            invocation
                .environment
                .get(name)
                .cloned()
                .ok_or_else(|| format!("native compile omitted {name}"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let values = values.into_iter().collect::<Vec<_>>();
    let [value] = values.as_slice() else {
        return Err(format!("native compiles disagree on {name}"));
    };
    Ok(value.clone())
}

/// Resolve one common environment path across every compile invocation.
pub(super) fn common_environment_path(
    invocations: &[&OvenLegacyNativeInvocation],
    name: &str,
) -> Result<PathBuf, String> {
    fs::canonicalize(common_environment_value(invocations, name)?)
        .map_err(|error| format!("cannot resolve native {name}: {error}"))
}
