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
/// System TAPI files contain their reexports; iconv additionally needs the admitted charset stub.
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
    for relative in [
        "usr/lib/libSystem.tbd",
        "usr/lib/libc.tbd",
        "usr/lib/libm.tbd",
        "usr/lib/libiconv.tbd",
        "usr/lib/libcharset.1.tbd",
    ] {
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
    identities.insert("platform-policy", "macos:min=11.0:sdk=26.5:direct-lld:v2".to_string());
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

/// Return the content identity of the pinned macOS linker and admitted SDK inputs for receipt construction.
///
/// Non-Apple targets retain their existing policy. Callers bind this value before Store selection; compilation
/// rechecks it so a changed closure cannot silently satisfy a previously selected linked-unit receipt.
pub fn pinned_link_closure_identity(rustc: &Path, target: &str) -> Result<Option<String>, OvenRustcError> {
    Ok(pinned_apple_link(rustc, target)?.map(|link| link.identity))
}

/// Apply pinned linking to a Rustdoc consumer only after checking any selected receipt binding.
///
/// Rustdoc owns its transient harness outputs; the enclosing receipt remains their source and dependency authority.
/// An explicitly bound link closure must match before Rustdoc can launch any child compiler.
pub(super) fn apply_receipt_link(
    command: &mut Command,
    rustc: &Path,
    receipt: &oven_store::OvenReceipt,
) -> Result<(), OvenRustcError> {
    if let Some(link) = pinned_apple_link(rustc, &receipt.intent.target)? {
        if let Some(bound) = receipt.sources.build_unit_inputs.get("link-closure")
            && bound != &link.identity
        {
            return Err(OvenRustcError::InvalidInput {
                field: "link closure",
                message: "Rustdoc receipt does not bind the active linker and SDK bytes".to_string(),
            });
        }
        link.apply(command);
    }
    Ok(())
}

impl PinnedAppleLink {
    /// Select direct stable LLD, and make rustc and the linker agree on explicit platform versions.
    pub(crate) fn apply(&self, command: &mut Command) {
        command
            .env("SDKROOT", &self.sdk)
            .env("MACOSX_DEPLOYMENT_TARGET", "11.0");
        command.arg("-C").arg(format!("linker={}", self.linker.display()));
        command.args(["-C", "link-arg=-syslibroot"]);
        command.arg("-C").arg(format!("link-arg={}", self.sdk.display()));
        command
            .arg("-L")
            .arg(format!("native={}", self.sdk.join("usr/lib").display()));
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

    /// Executables and test harnesses find iconv in the admitted SDK and record the explicit platform policy.
    #[test]
    #[cfg(target_os = "macos")]
    fn pinned_executable_and_test_link_use_sdk_closure() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = super::super::resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let link = pinned_apple_link(&rustc, &target)?.ok_or("missing Apple link closure")?;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("probe.rs");
        fs::write(
            &source,
            "#[link(name=\"iconv\")] unsafe extern \"C\" { fn iconv_close(handle: *mut core::ffi::c_void) -> i32; }\nfn main() { let _function = iconv_close as unsafe extern \"C\" fn(*mut core::ffi::c_void) -> i32; }\n#[test] fn runs() { main(); }\n",
        )?;
        let native_source = temp.path().join("native.c");
        let native_object = temp.path().join("native.o");
        fs::write(&native_source, "int native_deployment_probe(void) { return 42; }")?;
        let compiled = std::process::Command::new("/usr/bin/clang")
            .args(["-c", "-mmacosx-version-min=11.0"])
            .arg(&native_source)
            .arg("-o")
            .arg(&native_object)
            .output()?;
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(
            compiled.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for harness in [false, true] {
            let output = temp
                .path()
                .join(if harness { "test-probe" } else { "executable-probe" });
            let mut command = super::super::toolchain::rustc_probe_command(&rustc);
            command.arg(&source).arg("-o").arg(&output).arg("--edition=2024");
            if harness {
                command.arg("--test");
            }
            link.apply(&mut command);
            command.arg("-C").arg(format!("link-arg={}", native_object.display()));
            let result = command.output()?;
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{stderr}");
            assert!(!stderr.contains("rust-lld: warning"), "{stderr}");
            assert!(std::process::Command::new(&output).status()?.success());
            let load = std::process::Command::new("/usr/bin/otool")
                .arg("-l")
                .arg(&output)
                .output()?;
            assert!(load.status.success());
            let commands = String::from_utf8(load.stdout)?;
            assert!(commands.contains("minos 11.0"), "{commands}");
            assert!(commands.contains("sdk 26.5"), "{commands}");
        }
        Ok(())
    }

    /// Rustdoc's generated harness accepts the shared stable linker flags and resolves the admitted iconv closure.
    #[test]
    #[cfg(target_os = "macos")]
    fn pinned_rustdoc_harness_uses_sdk_closure() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = super::super::resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("docs.rs");
        fs::write(
            &source,
            "/// ```\n/// #[link(name=\"iconv\")] unsafe extern \"C\" {}\n/// assert_eq!(2 + 2, 4);\n/// ```\npub fn documented() {}\n",
        )?;
        let receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                temp.path(),
                "docs",
                "1.0.0",
                &target,
                super::super::rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("root", &source),
        )?;
        let rustdoc = super::super::rustdoc_for_rustc(&rustc)?;
        let mut command = std::process::Command::new(rustdoc);
        command
            .arg("--test")
            .arg(&source)
            .args(["--edition=2024", "--crate-name", "docs"]);
        super::apply_receipt_link(&mut command, &rustc, &receipt)?;
        let result = command.output()?;
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            result.status.success(),
            "{stderr}\n{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(!stderr.contains("rust-lld: warning"), "{stderr}");
        Ok(())
    }

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
