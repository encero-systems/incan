//! Fail closed on a detached compiler or shared library before invoking embedded rustc.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Startup refusals distinguish missing build evidence from a mismatched runtime.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// The process inherited an ambient unstable-feature permission.
    #[error("RUSTC_BOOTSTRAP is forbidden, including inherited values")]
    Bootstrap,
    /// Oven did not supply the driver's build-bound identity evidence.
    #[error("driver build is missing `{0}` identity evidence")]
    Missing(&'static str),
    /// The requested runtime differs from the exact compiler used to build this executable.
    #[error("driver identity mismatch: {0}")]
    Mismatch(String),
    /// Reading or executing the selected compiler failed before compilation began.
    #[error("cannot verify driver identity: {0}")]
    Io(#[from] std::io::Error),
}

/// Exact sysroot and compiler identity, admitted only after shared-library byte equivalence is proved.
pub struct DriverIdentity {
    /// Canonical sysroot whose compiler and loaded driver library match the build evidence.
    pub sysroot: PathBuf,
}

/// Dynamic loader metadata for the library owning the embedded rustc entry point (Unix ABI).
#[cfg(unix)]
#[repr(C)]
struct DlInfo {
    name: *const std::ffi::c_char,
    base: *mut std::ffi::c_void,
    symbol: *const std::ffi::c_char,
    address: *mut std::ffi::c_void,
}

#[cfg(unix)]
#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    /// Query the loaded object owning an address; the returned strings remain owned by the loader.
    fn dladdr(address: *const std::ffi::c_void, info: *mut DlInfo) -> std::ffi::c_int;
}

/// Verify the object actually loaded by this process, rather than a similarly named file on disk.
#[cfg(unix)]
fn loaded_library() -> Result<PathBuf, IdentityError> {
    let mut info = DlInfo {
        name: std::ptr::null(),
        base: std::ptr::null_mut(),
        symbol: std::ptr::null(),
        address: std::ptr::null_mut(),
    };
    let address = rustc_driver::run_compiler as *const () as *const std::ffi::c_void;
    // The function address belongs to a loaded object and the output points to an initialized ABI-sized record.
    let found = unsafe { dladdr(address, &mut info) };
    if found == 0 || info.name.is_null() {
        return Err(IdentityError::Mismatch(
            "cannot identify the loaded rustc_driver library".into(),
        ));
    }
    // A successful dladdr owns this nonnull, NUL-terminated name for the lifetime of the loaded object.
    let name = unsafe { std::ffi::CStr::from_ptr(info.name) };
    use std::os::unix::ffi::OsStrExt;
    Ok(fs::canonicalize(Path::new(std::ffi::OsStr::from_bytes(
        name.to_bytes(),
    )))?)
}

/// Unsupported loader APIs are a refusal, never an unverified assumption about library identity.
#[cfg(not(unix))]
fn loaded_library() -> Result<PathBuf, IdentityError> {
    Err(IdentityError::Mismatch(
        "this platform's loaded-library identity API is not implemented".into(),
    ))
}

/// Verify build-time evidence against the runtime selected by the caller, before creating output files.
///
/// These values must be recorded in the Oven unit identity and supplied as declared compile-time environment;
/// an arbitrary ambient path or a matching version label alone cannot authorize the driver.
pub fn verify(sysroot: &Path) -> Result<DriverIdentity, IdentityError> {
    if std::env::var_os("RUSTC_BOOTSTRAP").is_some() {
        return Err(IdentityError::Bootstrap);
    }
    let built_root = option_env!("INCAN_DRIVER_SYSROOT").ok_or(IdentityError::Missing("INCAN_DRIVER_SYSROOT"))?;
    let built_compiler =
        option_env!("INCAN_DRIVER_RUSTC_IDENTITY").ok_or(IdentityError::Missing("INCAN_DRIVER_RUSTC_IDENTITY"))?;
    let built_library = option_env!("INCAN_DRIVER_LIBRARY").ok_or(IdentityError::Missing("INCAN_DRIVER_LIBRARY"))?;
    let built_digest =
        option_env!("INCAN_DRIVER_LIBRARY_DIGEST").ok_or(IdentityError::Missing("INCAN_DRIVER_LIBRARY_DIGEST"))?;
    let root = fs::canonicalize(sysroot)?;
    if root != fs::canonicalize(built_root)? {
        return Err(IdentityError::Mismatch(
            "sysroot path differs from the build-bound sysroot".into(),
        ));
    }
    let output = Command::new(root.join("bin/rustc"))
        .arg("-vV")
        .env_remove("RUSTC_BOOTSTRAP")
        .output()?;
    let compiler = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || compiler.as_ref() != built_compiler
        || !compiler.lines().any(|line| line == "release: 1.98.0")
    {
        return Err(IdentityError::Mismatch(
            "rustc must match the complete pinned 1.98.0 build identity".into(),
        ));
    }
    let library = fs::canonicalize(root.join(built_library))?;
    if !library.starts_with(&root) {
        return Err(IdentityError::Mismatch(
            "rustc_driver library escapes the selected sysroot".into(),
        ));
    }
    if loaded_library()? != library {
        return Err(IdentityError::Mismatch(
            "the process loaded a different rustc_driver shared library".into(),
        ));
    }
    let digest = incan_driver::build::native_runtime_inputs::digest_native_compiler_input(&library)?;
    if digest.strip_prefix("sha256:") != Some(built_digest) {
        return Err(IdentityError::Mismatch(
            "rustc_driver shared-library bytes differ from the build identity".into(),
        ));
    }
    Ok(DriverIdentity { sysroot: root })
}
