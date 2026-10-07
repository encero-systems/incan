//! Pinned native linking and the content identity of its remaining host inputs.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{OvenRustcError, digest_bytes, rustc_host_target};

/// Exact macOS linking closure; physical paths never participate in its identity.
pub(crate) struct PinnedAppleLink {
    linker: PathBuf,
    sdk: PathBuf,
    /// Hash of the linker, admitted stub bytes, and explicit platform policy.
    pub identity: String,
}

/// Resolve the selected compiler's LLD and materialize only the admitted macOS SDK stub closure.
///
/// SDK 26.5 is the default because LLVM 22 cannot parse SDK 27's `arm64e.x1` stubs. An explicit
/// `INCAN_OVEN_LINK_SDK` selects another retained SDK without consulting xcrun or ambient SDKROOT.
/// The three system libraries are self-contained TAPI files, including their embedded reexports.
pub(crate) fn pinned_apple_link(rustc: &Path, target: &str) -> Result<Option<PinnedAppleLink>, OvenRustcError> {
    if !target.ends_with("-apple-darwin") {
        return Ok(None);
    }
    let host = rustc_host_target(rustc)?;
    let sysroot = super::toolchain::normalized_std_sysroot(rustc, target)?;
    let host_root = sysroot.join("lib/rustlib").join(host);
    let linker = host_root.join("bin/rust-lld");
    let sdk = std::env::var_os("INCAN_OVEN_LINK_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk"));
    let mut members = BTreeMap::new();
    for relative in ["usr/lib/libSystem.tbd", "usr/lib/libc.tbd", "usr/lib/libm.tbd"] {
        let path = sdk.join(relative);
        let bytes = fs::read(&path).map_err(|source| OvenRustcError::Io { path, source })?;
        members.insert(relative, bytes);
    }
    let linker_bytes = fs::read(&linker).map_err(|source| OvenRustcError::Io {
        path: linker.clone(),
        source,
    })?;
    let mut identities = BTreeMap::new();
    identities.insert("rust-lld", digest_bytes(&linker_bytes));
    let llvm = host_root.join("lib/libLLVM.dylib");
    if llvm.is_file() {
        let bytes = fs::read(&llvm).map_err(|source| OvenRustcError::Io { path: llvm, source })?;
        identities.insert("lld-libLLVM", digest_bytes(&bytes));
    }
    for (relative, bytes) in &members {
        identities.insert(relative, digest_bytes(bytes));
    }
    // SDK version is an explicit link policy, never inferred from the machine's selected developer directory.
    identities.insert("platform-policy", "macos:min=11.0:sdk=26.5:direct-lld:v1".to_string());
    let encoded = serde_json::to_vec(&identities).map_err(|error| OvenRustcError::InvalidInput {
        field: "link closure",
        message: error.to_string(),
    })?;
    let identity = digest_bytes(&encoded);
    let sdk = std::env::temp_dir()
        .join("incan-oven-link-sdk")
        .join(identity.replace(':', "-"));
    fs::create_dir_all(sdk.join("usr/lib")).map_err(|source| OvenRustcError::Io {
        path: sdk.clone(),
        source,
    })?;
    for (relative, bytes) in members {
        let path = sdk.join(relative);
        // Recheck the staged bytes even on reuse; an ambient temp directory is not an authority.
        if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
            let mut temporary =
                tempfile::NamedTempFile::new_in(sdk.join("usr/lib")).map_err(|source| OvenRustcError::Io {
                    path: path.clone(),
                    source,
                })?;
            temporary.write_all(&bytes).map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            temporary.persist(&path).map_err(|error| OvenRustcError::Io {
                path,
                source: error.error,
            })?;
        }
    }
    Ok(Some(PinnedAppleLink { linker, sdk, identity }))
}

impl PinnedAppleLink {
    /// Select direct stable LLD, and make rustc and the linker agree on explicit platform versions.
    pub(crate) fn apply(&self, command: &mut Command) {
        command
            .env("SDKROOT", &self.sdk)
            .env("MACOSX_DEPLOYMENT_TARGET", "11.0");
        command.arg("-C").arg(format!("linker={}", self.linker.display()));
        command.args([
            "-C",
            "linker-flavor=ld64.lld",
            "-C",
            "link-arg=-platform_version",
            "-C",
            "link-arg=macos",
            "-C",
            "link-arg=11.0",
            "-C",
            "link-arg=26.5",
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::{fs, pinned_apple_link, rustc_host_target};

    /// The real stable linker produces identical proc macros across roots and ignores a hostile PATH linker.
    #[test]
    #[cfg(target_os = "macos")]
    fn pinned_proc_macro_bytes_ignore_root_and_path() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let rustc = super::super::resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let link = pinned_apple_link(&rustc, &target)?.ok_or("missing Apple link closure")?;
        let temp = tempfile::tempdir()?;
        let hostile = temp.path().join("hostile");
        fs::create_dir(&hostile)?;
        let marker = temp.path().join("ld-called");
        let wrapper = hostile.join("ld");
        fs::write(
            &wrapper,
            format!("#!/bin/sh\n/usr/bin/touch '{}'\nexit 99\n", marker.display()),
        )?;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
        let mut outputs = Vec::new();
        for root in ["a", "b"] {
            let directory = temp.path().join(root);
            fs::create_dir(&directory)?;
            let source = directory.join("probe.rs");
            fs::write(
                &source,
                "extern crate proc_macro;\n#[proc_macro] pub fn identity(x: proc_macro::TokenStream) -> proc_macro::TokenStream { x }\n",
            )?;
            let output = directory.join("libprobe.dylib");
            let mut command = super::super::toolchain::rustc_probe_command(&rustc);
            command
                .arg(&source)
                .args(["--crate-type", "proc-macro", "--crate-name", "probe"]);
            command.arg("-o").arg(&output);
            command.arg(format!("--remap-path-prefix={}=/probe", directory.display()));
            command.args([
                "-C",
                "link-arg=-install_name",
                "-C",
                "link-arg=@rpath/libprobe.dylib",
                "-C",
                "link-arg=-S",
            ]);
            if root == "b" {
                command.env("PATH", &hostile);
            }
            link.apply(&mut command);
            let result = command.output()?;
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            outputs.push(fs::read(&output)?);
        }
        assert_eq!(outputs[0], outputs[1]);
        assert!(!marker.exists());
        // Loading and expanding the freshly linked macro proves the result remains usable by rustc.
        let consumer = temp.path().join("consumer.rs");
        fs::write(&consumer, "probe::identity! { pub fn value() -> u8 { 42 } }")?;
        let result = super::super::toolchain::rustc_probe_command(&rustc)
            .arg(&consumer)
            .args(["--crate-type", "lib", "--extern"])
            .arg(format!("probe={}", temp.path().join("a/libprobe.dylib").display()))
            .arg("-o")
            .arg(temp.path().join("consumer.rlib"))
            .output()?;
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        Ok(())
    }
}
