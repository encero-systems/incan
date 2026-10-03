//! Canonical compiler authority and bounded owner-closure resolution.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use oven_model::manifest::RustFactExecutable;

use super::super::super::{OvenLegacyCargoError, OvenLegacyNativeInvocation, digest_bytes};

/// Ask one explicit native compiler for the resource directory that belongs to its owner closure.
pub(super) fn native_resource_directory(compiler: &Path, label: &str) -> Result<PathBuf, OvenLegacyCargoError> {
    let output = std::process::Command::new(compiler)
        .arg("-print-resource-dir")
        .output()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: compiler.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "explicit {label} did not report its resource directory"
        )));
    }
    let resource = PathBuf::from(
        String::from_utf8(output.stdout)
            .map_err(|error| OvenLegacyCargoError::Plan(format!("{label} resource directory is not UTF-8: {error}")))?
            .trim(),
    );
    fs::canonicalize(&resource).map_err(|source| OvenLegacyCargoError::Io { path: resource, source })
}

/// Native compiler authority used to validate and materialize one adopted link record.
pub(in crate::harvest) struct NativeCompiler<'a> {
    pub(in crate::harvest) executable: &'a Path,
    pub(in crate::harvest) resource_dir: &'a Path,
    pub(in crate::harvest) sysroot: &'a Path,
}

/// Canonical compiler executable and the bounded owner closure needed to replay it.
pub(super) struct CompilerAuthority {
    compiler: PathBuf,
    /// Common physical owner of the compiler, resource directory, and sysroot.
    pub(super) owner_root: PathBuf,
    /// Portable compiler executable path below the owner.
    pub(super) executable_path: String,
    /// Portable resource directory path below the owner.
    pub(super) resource_path: String,
    /// Portable sysroot path below the owner.
    pub(super) sysroot_path: String,
}

impl CompilerAuthority {
    /// Resolve the one declared compiler used by every object in an archive.
    pub(super) fn from_invocations(
        invocations: &[&OvenLegacyNativeInvocation],
        compilers: &[NativeCompiler<'_>],
    ) -> Result<Self, String> {
        let observed = invocations
            .iter()
            .map(|invocation| {
                fs::canonicalize(&invocation.executable)
                    .map_err(|error| format!("cannot resolve observed compiler: {error}"))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let observed = observed.iter().collect::<Vec<_>>();
        let [compiler] = observed.as_slice() else {
            return Err("one archive combines objects from different compiler executables".to_string());
        };
        let declared = compilers
            .iter()
            .find(|candidate| fs::canonicalize(candidate.executable).ok().as_ref() == Some(*compiler))
            .ok_or_else(|| "native compile used a compiler other than explicit --cc or --cxx".to_string())?;
        Self::from_declared(declared, "compiler", "C")
    }

    /// Resolve a declared compiler and its resource/sysroot owner closure.
    fn from_declared(declared: &NativeCompiler<'_>, compiler_label: &str, sysroot_label: &str) -> Result<Self, String> {
        let compiler = fs::canonicalize(declared.executable)
            .map_err(|error| format!("cannot resolve explicit native {compiler_label}: {error}"))?;
        let resource_dir = fs::canonicalize(declared.resource_dir)
            .map_err(|error| format!("cannot resolve compiler resource directory: {error}"))?;
        let sysroot = fs::canonicalize(declared.sysroot)
            .map_err(|error| format!("cannot resolve {sysroot_label} sysroot: {error}"))?;
        let owner_root = common_ancestor(&[compiler.as_path(), resource_dir.as_path(), sysroot.as_path()])
            .ok_or_else(|| "compiler, resource directory, and sysroot have no common owner root".to_string())?;
        Ok(Self {
            executable_path: portable_beneath(&owner_root, &compiler, "compiler executable")?,
            resource_path: portable_beneath(&owner_root, &resource_dir, "compiler resource directory")?,
            sysroot_path: portable_beneath(&owner_root, &sysroot, "C sysroot")?,
            compiler,
            owner_root,
        })
    }

    /// Materialize the typed executable after the adopted source walk names the owner closure.
    pub(super) fn executable(&self, owner_paths: &BTreeSet<String>) -> Result<RustFactExecutable, String> {
        let owner = oven_store::publisher_owner::publisher_owner_identity(
            &self.owner_root,
            owner_paths.iter().map(String::as_str),
        )
        .map_err(|error| format!("cannot identify compiler owner closure: {error}"))?;
        let name = self
            .compiler
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| "compiler executable name is not portable UTF-8".to_string())?;
        Ok(RustFactExecutable {
            name: name.to_string(),
            owner,
            path: self.executable_path.clone(),
            digest: digest_bytes(&fs::read(&self.compiler).map_err(|error| format!("cannot read compiler: {error}"))?),
        })
    }
}

/// Compute the longest common ancestor directory of absolute selected paths.
pub(super) fn common_ancestor(paths: &[&Path]) -> Option<PathBuf> {
    let first = paths.first()?;
    first
        .ancestors()
        .find(|ancestor| paths.iter().all(|path| path.starts_with(ancestor)))
        .map(Path::to_path_buf)
}

/// Render one physical path below an owner as a non-empty portable relative path.
pub(super) fn portable_beneath(owner: &Path, path: &Path, field: &str) -> Result<String, String> {
    let relative = path
        .strip_prefix(owner)
        .map_err(|_| format!("{field} is outside its owner"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("{field} does not have a portable relative path"));
    }
    relative
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| format!("{field} is not UTF-8"))
}
