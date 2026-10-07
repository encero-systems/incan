//! Pinned native linking and the content identity of its remaining host inputs.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{OvenRustcError, digest_bytes, rustc_host_target};

/// Exact native linking closure; physical staging paths never participate in its identity.
pub(crate) struct PinnedLink {
    linker: PathBuf,
    sdk: PathBuf,
    linux: Option<LinuxLinkPolicy>,
    /// Hash of the linker, admitted native inputs, and explicit platform policy.
    pub identity: String,
}

/// File name rustc gives a proc-macro library for `host`, from the crate stem (crate name plus any extra filename).
///
/// A proc macro is a host dynamic library, so its suffix follows the host, never the build target: `.dll` without a
/// `lib` prefix on Windows, `.dylib` on Apple, `.so` everywhere else. Every Oven path that names one uses this.
pub(crate) fn proc_macro_file_name(stem: &str, host: &str) -> String {
    if host.contains("windows") {
        format!("{stem}.dll")
    } else if host.contains("apple") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

/// Resolve the selected compiler's LLD and materialize the admitted native link closure.
///
/// GNU Linux admits the Ubuntu/Debian multiarch runtime under `INCAN_OVEN_LINK_SYSROOT` (default `/`).
/// Startup objects, GNU input scripts and their members, and the loader are content-bound before linking.
///
/// SDK 26.5 is the default because LLVM 22 cannot parse SDK 27's `arm64e.x1` stubs. An explicit
/// `INCAN_OVEN_LINK_SDK` selects another retained SDK without consulting xcrun or ambient SDKROOT.
/// System TAPI files contain their reexports; iconv additionally needs the admitted charset stub.
pub(crate) fn pinned_link(rustc: &Path, target: &str) -> Result<Option<PinnedLink>, OvenRustcError> {
    if matches!(target, "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu") {
        let host = rustc_host_target(rustc)?;
        let sysroot = super::toolchain::normalized_std_sysroot(rustc, target)?;
        let linker = sysroot.join("lib/rustlib").join(host).join("bin/rust-lld");
        let root = std::env::var_os("INCAN_OVEN_LINK_SYSROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        return linux_link(&linker, &root, target).map(Some);
    }
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
    // The normalized sysroot always places LLD's LLVM runtime (from the `rustc` component), so it is always bound.
    let llvm = host_root.join("lib/libLLVM.dylib");
    let bytes = fs::read(&llvm).map_err(|source| OvenRustcError::Io { path: llvm, source })?;
    identities.insert("lld-libLLVM", digest_bytes(&bytes));
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
    Ok(Some(PinnedLink {
        linker,
        sdk,
        linux: None,
        identity,
    }))
}

/// Return the content identity of the pinned native linker and admitted system inputs for receipt construction.
///
/// Apple and supported GNU Linux targets bind this value before Store selection; compilation
/// rechecks it so a changed closure cannot silently satisfy a previously selected linked-unit receipt.
pub fn pinned_link_closure_identity(rustc: &Path, target: &str) -> Result<Option<String>, OvenRustcError> {
    Ok(pinned_link(rustc, target)?.map(|link| link.identity))
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
    if let Some(link) = pinned_link(rustc, &receipt.intent.target)? {
        if let Some(bound) = receipt.sources.build_unit_inputs.get("link-closure")
            && bound != &link.identity
        {
            return Err(OvenRustcError::InvalidInput {
                field: "link closure",
                message: "Rustdoc receipt does not bind the active linker and native input bytes".to_string(),
            });
        }
        link.apply(command);
    }
    Ok(())
}

impl PinnedLink {
    /// Select direct stable LLD, and make rustc and the linker agree on explicit platform versions.
    pub(crate) fn apply(&self, command: &mut Command) {
        if let Some(policy) = &self.linux {
            policy.apply(command, &self.linker, &self.sdk);
            return;
        }
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

/// GNU Linux startup and dynamic-loader policy shared by all linked Rust units.
struct LinuxLinkPolicy {
    directories: Vec<PathBuf>,
    loader: String,
    crt_directory: PathBuf,
    gcc_directory: PathBuf,
}

impl LinuxLinkPolicy {
    /// Select stable direct LLD, with staged library search and explicit startup objects.
    ///
    /// Shared objects omit the executable entry point; PIE executables and harnesses use Scrt1.o.
    /// No compiler driver is invoked, and LLD's sysroot redirects absolute linker-script inputs.
    fn apply(&self, command: &mut Command, linker: &Path, root: &Path) {
        let args = command.get_args().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>();
        let shared = args.iter().any(|arg| {
            arg.as_ref() == "proc-macro"
                || arg.as_ref() == "dylib"
                || arg.as_ref() == "cdylib"
                || arg.starts_with("--crate-type=proc-macro")
                || arg.starts_with("--crate-type=dylib")
                || arg.starts_with("--crate-type=cdylib")
        });
        command.arg("-C").arg(format!("linker={}", linker.display()));
        command.args(["-C", "linker-flavor=ld.lld", "-C", "link-self-contained=no"]);
        command.arg("-C").arg(format!("link-arg=--sysroot={}", root.display()));
        command
            .arg("-C")
            .arg(format!("link-arg=--dynamic-linker={}", self.loader));
        for directory in &self.directories {
            command
                .arg("-L")
                .arg(format!("native={}", root.join(directory).display()));
        }
        if !shared {
            command.arg("-C").arg(format!(
                "link-arg={}",
                root.join(&self.crt_directory).join("Scrt1.o").display()
            ));
        }
        for object in [
            self.crt_directory.join("crti.o"),
            self.gcc_directory.join("crtbeginS.o"),
            self.gcc_directory.join("crtendS.o"),
            self.crt_directory.join("crtn.o"),
        ] {
            command
                .arg("-C")
                .arg(format!("link-arg={}", root.join(object).display()));
        }
    }
}

/// Refuse incomplete or unsupported native closures rather than falling back to host driver discovery.
fn linux_link_error(message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "Linux link closure",
        message: message.into(),
    }
}

/// Identify the Ubuntu/Debian GNU layout without executing cc, ld, or a package manager.
///
/// An explicit sysroot uses the same relative layout. A missing GCC runtime or multiarch library refuses linking.
fn linux_link_policy(root: &Path, target: &str) -> Result<LinuxLinkPolicy, OvenRustcError> {
    let (multiarch, loader) = match target {
        "x86_64-unknown-linux-gnu" => ("x86_64-linux-gnu", "/lib64/ld-linux-x86-64.so.2"),
        "aarch64-unknown-linux-gnu" => ("aarch64-linux-gnu", "/lib/ld-linux-aarch64.so.1"),
        _ => return Err(linux_link_error(format!("unsupported target {target}"))),
    };
    let gcc_root = PathBuf::from("usr/lib/gcc").join(multiarch);
    let path = root.join(&gcc_root);
    let mut versions = fs::read_dir(&path)
        .map_err(|source| OvenRustcError::Io { path, source })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| linux_link_error(error.to_string()))?
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.parse::<u32>().ok()?;
            entry
                .path()
                .join("crtbeginS.o")
                .is_file()
                .then_some((version, gcc_root.join(name)))
        })
        .collect::<Vec<_>>();
    versions.sort_by_key(|(version, _)| *version);
    let (_, gcc_directory) = versions
        .pop()
        .ok_or_else(|| linux_link_error("no admitted GCC startup runtime"))?;
    let crt_directory = PathBuf::from("usr/lib").join(multiarch);
    let directories = vec![
        gcc_directory.clone(),
        crt_directory.clone(),
        PathBuf::from("lib").join(multiarch),
    ];
    Ok(LinuxLinkPolicy {
        directories,
        loader: loader.to_string(),
        crt_directory,
        gcc_directory,
    })
}

/// Resolve one named script or library input inside the admitted sysroot, never through PATH.
fn linux_library_path(root: &Path, policy: &LinuxLinkPolicy, name: &str) -> Result<PathBuf, OvenRustcError> {
    if name.starts_with('/') {
        let relative = PathBuf::from(name.trim_start_matches('/'));
        if root.join(&relative).is_file() {
            return Ok(relative);
        }
    } else {
        let names = if let Some(library) = name.strip_prefix("-l") {
            vec![format!("lib{library}.so"), format!("lib{library}.a")]
        } else {
            vec![name.to_string()]
        };
        for name in names {
            for directory in &policy.directories {
                let relative = directory.join(&name);
                if root.join(&relative).is_file() {
                    return Ok(relative);
                }
            }
        }
    }
    Err(linux_link_error(format!("missing admitted input {name}")))
}

/// Collect transitive GNU INPUT/GROUP script members as bytes, including libc_nonshared and libgcc_s targets.
///
/// Only ELF, archives, and simple GNU input scripts are accepted. SEARCH_DIR, INCLUDE, and unsupported commands
/// refuse rather than exposing ambient directories or silently leaving an unbound file in the link.
fn collect_linux_member(
    root: &Path,
    policy: &LinuxLinkPolicy,
    relative: PathBuf,
    members: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), OvenRustcError> {
    if members.contains_key(&relative) {
        return Ok(());
    }
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(linux_link_error("input escapes admitted sysroot"));
    }
    let path = root.join(&relative);
    let bytes = fs::read(&path).map_err(|source| OvenRustcError::Io { path, source })?;
    if bytes.starts_with(b"\x7fELF") || bytes.starts_with(b"!<arch>\n") {
        members.insert(relative, bytes);
        return Ok(());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| linux_link_error("unknown native input format"))?;
    let mut clean = String::new();
    let mut rest = text;
    while let Some((before, comment)) = rest.split_once("/*") {
        clean.push_str(before);
        rest = comment
            .split_once("*/")
            .ok_or_else(|| linux_link_error("unterminated script comment"))?
            .1;
    }
    clean.push_str(rest);
    let mut inputs = Vec::new();
    for token in clean
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | ';'))
        .filter(|token| !token.is_empty())
    {
        if matches!(token, "GROUP" | "INPUT" | "AS_NEEDED" | "OUTPUT_FORMAT") || token.starts_with("elf") {
            continue;
        }
        if !(token.starts_with('/') || token.starts_with("-l") || token.starts_with("lib")) {
            return Err(linux_link_error(format!("unsupported linker-script token {token}")));
        }
        inputs.push(linux_library_path(root, policy, token)?);
    }
    if inputs.is_empty() {
        return Err(linux_link_error("linker script contains no admitted inputs"));
    }
    members.insert(relative, bytes);
    for input in inputs {
        collect_linux_member(root, policy, input, members)?;
    }
    Ok(())
}

/// Bind the exact linker, GNU runtime files, loader path and startup policy, then stage only those inputs.
///
/// Physical root paths are excluded; logical file names and bytes are included. Script absolute paths remain
/// intact and are redirected by LLD's explicit sysroot. System symlinks are materialized as their referent bytes.
fn linux_link(linker: &Path, root: &Path, target: &str) -> Result<PinnedLink, OvenRustcError> {
    let policy = linux_link_policy(root, target)?;
    let mut members = BTreeMap::new();
    for name in ["crt1.o", "Scrt1.o", "crti.o", "crtn.o"] {
        collect_linux_member(root, &policy, policy.crt_directory.join(name), &mut members)?;
    }
    for name in ["crtbeginS.o", "crtendS.o", "libgcc.a", "libgcc_eh.a", "libgcc_s.so"] {
        collect_linux_member(root, &policy, policy.gcc_directory.join(name), &mut members)?;
    }
    for name in [
        "libc.so",
        "libm.so",
        "libdl.a",
        "libpthread.a",
        "librt.a",
        "libutil.a",
        "libresolv.so",
    ] {
        let relative = linux_library_path(root, &policy, name)?;
        collect_linux_member(root, &policy, relative, &mut members)?;
    }
    collect_linux_member(
        root,
        &policy,
        PathBuf::from(policy.loader.trim_start_matches('/')),
        &mut members,
    )?;
    let mut identities = BTreeMap::new();
    let bytes = fs::read(linker).map_err(|source| OvenRustcError::Io {
        path: linker.to_path_buf(),
        source,
    })?;
    identities.insert("rust-lld".to_string(), digest_bytes(&bytes));
    identities.insert(
        "platform-policy".to_string(),
        format!("{target}:ld.lld:dynamic-pie:crt-v1:loader={}", policy.loader),
    );
    for (relative, bytes) in &members {
        identities.insert(relative.to_string_lossy().into_owned(), digest_bytes(bytes));
    }
    let encoded = serde_json::to_vec(&identities).map_err(|error| linux_link_error(error.to_string()))?;
    let identity = digest_bytes(&encoded);
    let sdk = std::env::temp_dir()
        .join("incan-oven-link-linux")
        .join(identity.replace(':', "-"));
    for (relative, bytes) in members {
        let path = sdk.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| linux_link_error("native member has no parent"))?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| OvenRustcError::Io {
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
    Ok(PinnedLink {
        linker: linker.to_path_buf(),
        sdk,
        linux: Some(policy),
        identity,
    })
}

#[cfg(test)]
mod tests {
    use super::{fs, pinned_link, rustc_host_target};

    /// Synthetic Ubuntu closures prove both architectures without executing Linux code on the macOS host.
    #[test]
    fn linux_closure_binds_transitive_inputs_and_explicit_arguments() -> Result<(), Box<dyn std::error::Error>> {
        for (target, multiarch, loader) in [
            (
                "x86_64-unknown-linux-gnu",
                "x86_64-linux-gnu",
                "lib64/ld-linux-x86-64.so.2",
            ),
            (
                "aarch64-unknown-linux-gnu",
                "aarch64-linux-gnu",
                "lib/ld-linux-aarch64.so.1",
            ),
        ] {
            let temp = tempfile::tempdir()?;
            let linker = temp.path().join("rust-lld");
            fs::write(&linker, b"pinned linker")?;
            let mut closures = Vec::new();
            for name in ["a", "b"] {
                let root = temp.path().join(name);
                let gcc = format!("usr/lib/gcc/{multiarch}/14");
                let libraries = format!("usr/lib/{multiarch}");
                for directory in [&gcc, &libraries, "lib64", "lib"] {
                    fs::create_dir_all(root.join(directory))?;
                }
                for name in [
                    "crt1.o",
                    "Scrt1.o",
                    "crti.o",
                    "crtn.o",
                    "libc.so.6",
                    "libc_nonshared.a",
                    "libm.so",
                    "libdl.a",
                    "libpthread.a",
                    "librt.a",
                    "libutil.a",
                    "libresolv.so",
                ] {
                    fs::write(root.join(&libraries).join(name), b"\x7fELFfixture")?;
                }
                for name in ["crtbeginS.o", "crtendS.o", "libgcc.a", "libgcc_eh.a", "libgcc_s.so.1"] {
                    fs::write(root.join(&gcc).join(name), b"\x7fELFfixture")?;
                }
                fs::write(root.join(&gcc).join("libgcc_s.so"), "GROUP ( libgcc_s.so.1 -lgcc )")?;
                fs::write(
                    root.join(&libraries).join("libc.so"),
                    format!(
                        "/* GNU ld script */ OUTPUT_FORMAT(elf64-test) GROUP ( /{libraries}/libc.so.6 /{libraries}/libc_nonshared.a AS_NEEDED ( /{loader} ) )"
                    ),
                )?;
                fs::write(root.join(loader), b"\x7fELFloader")?;
                let link = super::linux_link(&linker, &root, target)?;
                for shared in [false, true] {
                    let mut command = std::process::Command::new("rustc");
                    if shared {
                        command.args(["--crate-type", "proc-macro"]);
                    } else {
                        command.arg("--test");
                    }
                    link.apply(&mut command);
                    let args = command
                        .get_args()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    assert!(args.contains(&"linker-flavor=ld.lld".to_string()));
                    assert!(args.contains(&"link-self-contained=no".to_string()));
                    assert!(args.contains(&format!("linker={}", linker.display())));
                    assert!(args.contains(&format!("link-arg=--dynamic-linker=/{loader}")));
                    assert_eq!(args.iter().any(|arg| arg.ends_with("/Scrt1.o")), !shared);
                    assert!(args.iter().any(|arg| arg.ends_with("/crti.o")));
                    assert!(args.iter().any(|arg| arg.ends_with("/crtn.o")));
                    assert!(!args.iter().any(|arg| arg.contains("gnu-lld")));
                }
                fs::write(&linker, b"different linker")?;
                assert_ne!(link.identity, super::linux_link(&linker, &root, target)?.identity);
                fs::write(&linker, b"pinned linker")?;
                closures.push(link.identity);
                fs::write(root.join(&libraries).join("libc_nonshared.a"), b"\x7fELFchanged")?;
                assert_ne!(
                    closures.last(),
                    Some(&super::linux_link(&linker, &root, target)?.identity)
                );
                fs::write(
                    root.join(&libraries).join("libc.so"),
                    "SEARCH_DIR(/ambient) INPUT(libc.so.6)",
                )?;
                assert!(super::linux_link(&linker, &root, target).is_err());
            }
            assert_eq!(closures[0], closures[1]);
        }
        Ok(())
    }

    /// Stable 1.98.0 accepts the direct GNU LLD spelling for both Linux targets without -Z flags.
    #[test]
    fn linux_stable_linker_flags_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = super::super::resolve_active_rustc()?;
        for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
            let output = super::super::toolchain::rustc_probe_command(&rustc)
                .args([
                    "-C",
                    "linker-flavor=ld.lld",
                    "-C",
                    "link-self-contained=no",
                    "--print",
                    "cfg",
                    "--target",
                    target,
                ])
                .output()?;
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(())
    }

    /// Executables and test harnesses find iconv in the admitted SDK and record the explicit platform policy.
    #[test]
    #[cfg(target_os = "macos")]
    fn pinned_executable_and_test_link_use_sdk_closure() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = super::super::resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let link = pinned_link(&rustc, &target)?.ok_or("missing Apple link closure")?;
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
        let link = pinned_link(&rustc, &target)?.ok_or("missing Apple link closure")?;
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
