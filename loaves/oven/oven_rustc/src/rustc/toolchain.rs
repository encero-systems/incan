//! Locating the Rust toolchain Incan was built against, without Cargo.
//!
//! The installer provisions Incan's own Rustup home; a development checkout resolves through the ambient Rustup.
//! `resolve_active_rustc`, the one-spawn `rustc -vV` probe, `rustdoc_for_rustc` and the dynamic-library environment a
//! direct compile runs under live here.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use super::{
    BTreeMap, BTreeSet, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError,
    clear_inherited_cargo_environment, normalized_relative_path, rustup_reported_tool, verified_regular_file,
};

/// Return the Rustup home Incan provisions for itself, when an installed toolchain has one.
///
/// Incan's Loafs are sealed against the exact compiler that baked them, so building with whatever compiler the user
/// happens to have made their global default fails closed with a Loaf-incompatibility error. The installer therefore
/// provisions the release's own required channel into `$INCAN_HOME/rust` (defaulting below the user home) and makes
/// it the default *within that home only*, leaving the user's own Rustup configuration untouched. This returns that
/// home when it exists, so ordinary builds resolve the compiler Incan was built against.
///
/// Returns `None` for development checkouts and for installations that predate this layout, both of which keep the
/// previous behavior of consulting the ambient Rustup default.
pub fn incan_owned_rustup_home() -> Option<PathBuf> {
    // The installer links commands into a bin directory, and `current_exe` is documented to be allowed to report
    // the symlink rather than its target. Resolving it first is what lets the ancestor walk see the installation
    // the command actually belongs to when that installation lives outside the user home.
    let executable = env::current_exe()
        .ok()
        .map(|executable| fs::canonicalize(&executable).unwrap_or(executable));
    incan_owned_rustup_home_in(
        env::var_os("INCAN_HOME"),
        executable,
        env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
    )
}

/// Resolve the Incan-owned Rustup home from explicit inputs, without reading process-global state.
///
/// Selection order: an explicitly named Incan home, then this executable's own installed layout, then the user
/// home default. The executable-relative step matters because the npm and pip shims install below their own
/// package directory rather than the user home and do not set `INCAN_HOME` when they spawn the compiler, so only
/// the executable's location identifies the provisioning that belongs to *this* installation.
///
/// Every candidate must actually contain `rust/toolchains`; a root without one yields `None` so development
/// checkouts and pre-isolation installations keep resolving through the ambient Rustup default.
pub fn incan_owned_rustup_home_in(
    incan_home: Option<OsString>,
    executable: Option<PathBuf>,
    user_home: Option<OsString>,
) -> Option<PathBuf> {
    /// Accept a candidate Incan home only when it actually carries a provisioned Rustup layout.
    fn provisioned(root: &Path) -> Option<PathBuf> {
        let rust_root = root.join("rust");
        rust_root.join("toolchains").is_dir().then_some(rust_root)
    }

    if let Some(root) = incan_home.filter(|path| !path.is_empty()).map(PathBuf::from)
        && let Some(rust_root) = provisioned(&root)
    {
        return Some(rust_root);
    }
    if let Some(executable) = executable {
        for ancestor in executable.ancestors().skip(1) {
            if let Some(rust_root) = provisioned(ancestor) {
                return Some(rust_root);
            }
        }
    }
    user_home
        .filter(|path| !path.is_empty())
        .and_then(|path| provisioned(&PathBuf::from(path).join(".incan")))
}

/// Name of the pointer file the installer writes to record which channel it provisioned.
pub const INCAN_OWNED_CHANNEL_POINTER: &str = "incan-channel.txt";

/// Resolve one tool from Incan's own provisioned toolchain by reading the Rustup layout directly.
///
/// This deliberately does not shell out to `rustup`. Rustup resolves a toolchain name from ambient state --
/// `RUSTUP_TOOLCHAIN` and any directory `rust-toolchain.toml` override both win over a home's default -- so asking
/// it would let the user's environment select a toolchain that does not exist inside Incan's home, reintroducing
/// the very coupling this isolation removes. The installed layout is deterministic, so reading it is both exact
/// and cheaper than a subprocess.
///
/// The installer records the channel it provisioned in [`INCAN_OWNED_CHANNEL_POINTER`]; that pointer disambiguates
/// the toolchain directory when an earlier channel is still present after an upgrade. Without a pointer the home
/// must hold exactly one toolchain, otherwise the choice would be arbitrary and this returns `None` so resolution
/// falls back to the ambient default rather than guessing.
pub fn incan_owned_tool(rust_root: &Path, tool: &str) -> Option<PathBuf> {
    let toolchains = rust_root.join("toolchains");
    let executable = |directory: &Path| -> Option<PathBuf> {
        let candidate = directory.join("bin").join(tool);
        candidate.is_file().then_some(candidate)
    };

    // ---- Preferred: the channel the installer recorded for this home ----
    if let Ok(channel) = fs::read_to_string(rust_root.join(INCAN_OWNED_CHANNEL_POINTER)) {
        let channel = channel.trim();
        if !channel.is_empty()
            && let Ok(entries) = fs::read_dir(&toolchains)
        {
            let mut matches: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name == channel || name.starts_with(&format!("{channel}-")))
                })
                .collect();
            matches.sort();
            if let Some(directory) = matches.first()
                && let Some(found) = executable(directory)
            {
                return Some(found);
            }
        }
    }

    // ---- Fallback: an unambiguous single-toolchain home ----
    let mut directories: Vec<PathBuf> = fs::read_dir(&toolchains)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    match directories.as_slice() {
        [only] => executable(only),
        _ => None,
    }
}

/// Report whether Incan's own provisioned toolchain carries a Rust target, when such a toolchain exists.
///
/// `None` means there is no Incan-owned toolchain to ask, so callers must fall back to the ambient Rustup. This
/// matters because the installer adds required targets to Incan's home and deliberately leaves the user's own
/// Rustup untouched: asking ambient Rustup about a target Incan provisioned for itself reports a false absence.
///
/// Membership is read from the toolchain layout (`lib/rustlib/<target>`) for the same reason [`incan_owned_tool`]
/// reads it: ambient toolchain selection cannot redirect a directory check.
pub fn incan_owned_target_installed(target: &str) -> Option<bool> {
    let rust_root = incan_owned_rustup_home()?;
    let rustc = incan_owned_tool(&rust_root, "rustc")?;
    let toolchain_root = rustc.parent()?.parent()?;
    Some(toolchain_root.join("lib").join("rustlib").join(target).is_dir())
}

/// Resolve the Rust compiler belonging to Incan's own provisioned toolchain.
///
/// Pairs with [`incan_owned_cargo`]. A toolchain-direct Cargo does not imply a matching compiler: Cargo resolves
/// `rustc` from `RUSTC` or `PATH`, and on a machine with Rustup installed `PATH` reaches the Rustup shim, which selects
/// the user's default toolchain. Selecting Incan's Cargo without also selecting its compiler therefore builds one
/// dependency graph with two rustc versions, which Cargo only reports much later as "found crate `x` compiled by an
/// incompatible version of rustc".
pub fn incan_owned_rustc() -> Option<PathBuf> {
    incan_owned_tool(&incan_owned_rustup_home()?, "rustc")
}

/// Resolve the active Rust compiler without involving Cargo or a Cargo target directory.
///
/// An explicit `RUSTC` must be a regular executable file, not a shell fragment. When it is absent, the Rustup
/// toolchain resolver supplies the compiler path; that remains separate from the explicit `legacy_cargo` publisher.
///
/// When `rustup` is not reachable the error names the most common cause: provisioning Rust from within an Incan
/// command wires Rustup into shell profiles for future shells, so the shell that triggered it keeps its original
/// `PATH` and needs to be refreshed before the compiler is visible.
pub fn resolve_active_rustc() -> Result<PathBuf, OvenRustcError> {
    if let Some(path) = env::var_os("RUSTC").filter(|path| !path.is_empty()) {
        return verified_regular_file(Path::new(&path), "RUSTC");
    }
    if let Some(isolated) = incan_owned_rustup_home().and_then(|home| incan_owned_tool(&home, "rustc")) {
        return verified_regular_file(&isolated, "rustc");
    }
    let Some(reported) = rustup_reported_tool("rustc") else {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "could not locate the active Rust compiler. If Rust was just installed, open a new shell (or \
                      run `. \"$HOME/.cargo/env\"`) so `rustup` is on PATH; otherwise install Rust through rustup, \
                      or set RUSTC to an explicit compiler path"
                .to_string(),
        });
    };
    verified_regular_file(Path::new(&reported), "rustc")
}

/// The two facts every command asks of the selected compiler, answered by one `rustc -vV`.
#[derive(Debug, Clone)]
pub struct RustcProbe {
    /// The first `-vV` line, identical to `rustc --version`.
    identity: String,
    /// The `host:` line, absent when the compiler (a test double, typically) printed none.
    host_target: Option<String>,
}

/// Process-wide memo of `rustc -vV` answers, keyed by the compiler file's canonical path, length and modification
/// time so a replaced compiler is probed afresh. Every normal command asked the compiler twice -- once for its
/// version, once for its host -- and each spawn cost about twenty milliseconds of a warm no-change build (#1111).
pub fn rustc_probe_memo() -> &'static Mutex<HashMap<RustcFileStamp, RustcProbe>> {
    static MEMO: OnceLock<Mutex<HashMap<RustcFileStamp, RustcProbe>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What identifies one compiler file for the probe memo: its path, length and modification time.
pub type RustcFileStamp = (PathBuf, u64, Option<std::time::SystemTime>);

/// Probe one regular Rust compiler with `-vV`, once per observed compiler file per process.
pub fn rustc_probe(rustc: &Path) -> Result<RustcProbe, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let metadata = fs::metadata(&rustc).map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    let key = (rustc.clone(), metadata.len(), metadata.modified().ok());
    if let Ok(memo) = rustc_probe_memo().lock()
        && let Some(probe) = memo.get(&key)
    {
        return Ok(probe.clone());
    }
    let mut command = Command::new(&rustc);
    command.arg("-vV");
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `-vV` identity and host target".to_string(),
        });
    }
    let output = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported non-UTF-8 `-vV` output: {error}"),
    })?;
    let identity = output
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "rustc",
            message: "reported an empty `-vV` identity".to_string(),
        })?
        .to_string();
    let host_target = output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(ToString::to_string);
    let probe = RustcProbe { identity, host_target };
    if let Ok(mut memo) = rustc_probe_memo().lock() {
        memo.insert(key, probe.clone());
    }
    Ok(probe)
}

/// Read one regular Rust compiler's stable `--version` identity without invoking Cargo.
pub fn rustc_identity(rustc: &Path) -> Result<String, OvenRustcError> {
    Ok(rustc_probe(rustc)?.identity)
}

/// Read the active compiler's host target from `rustc -vV` without consulting Cargo metadata.
pub fn rustc_host_target(rustc: &Path) -> Result<String, OvenRustcError> {
    rustc_probe(rustc)?
        .host_target
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "rustc",
            message: "did not report a host target in `-vV` output".to_string(),
        })
}

/// Resolve the selected compiler's sysroot without consulting Cargo.
pub fn rustc_sysroot(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.args(["--print", "sysroot"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `--print sysroot`".to_string(),
        });
    }
    let sysroot = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported a non-UTF-8 sysroot: {error}"),
    })?;
    let sysroot = PathBuf::from(sysroot.trim());
    if !sysroot.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("reported a missing or non-directory sysroot {}", sysroot.display()),
        });
    }
    Ok(sysroot)
}

/// Resolve the Rustdoc executable from the same verified sysroot as a receipt-selected compiler.
pub fn rustdoc_for_rustc(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustdoc = rustc_sysroot(rustc)?.join("bin/rustdoc");
    verified_regular_file(&rustdoc, "rustdoc")
}

/// Derive the selected compiler's dynamic-library search environment without consulting Cargo.
///
/// Direct `--test` compilation of a proc-macro crate uses Cargo's `-C prefer-dynamic` convention. Its caller-owned
/// test binary must therefore receive the matching toolchain standard-library directory for both inventory and test
/// execution; ambient Cargo-provided dynamic-library state is not trusted.
pub fn rustc_dynamic_library_environment(rustc: &Path) -> Result<(String, String), OvenRustcError> {
    let sysroot = rustc_sysroot(rustc)?;
    let host_target = rustc_host_target(rustc)?;
    let target_libraries = sysroot.join("lib/rustlib").join(host_target).join("lib");
    let toolchain_libraries = sysroot.join("lib");
    if !target_libraries.is_dir() || !toolchain_libraries.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!(
                "sysroot {} does not contain direct-test dynamic library directories",
                sysroot.display()
            ),
        });
    }
    let value =
        env::join_paths([target_libraries, toolchain_libraries]).map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("cannot construct direct-test dynamic library search path: {error}"),
        })?;
    let value = value.into_string().map_err(|_| OvenRustcError::InvalidInput {
        field: "rustc",
        message: "direct-test dynamic library search path is not valid UTF-8".to_string(),
    })?;
    let key = if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else if cfg!(target_os = "windows") {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    Ok((key.to_string(), value))
}

/// Extend the receipt-selected dynamic toolchain closure with caller-owned direct-Rustc dynamic-library directories.
///
/// Rustdoc owns its generated runner binary and launches it before Oven can attach a separate environment. The
/// caller-owned paths are already validated by the direct-Rustc plan, so they are safe to transport alongside the
/// selected toolchain directories rather than relying on Cargo's ambient loader setup. Store artifact directories
/// are deliberately excluded: their opaque `sha256:` identities are not valid Unix path-list segments and Rustdoc
/// already receives them as individual direct compiler search paths.
pub fn rustc_dynamic_library_environment_with_caller_owned_paths(
    rustc: &Path,
    plan: &OvenRustcArtifactPlan,
) -> Result<(String, String), OvenRustcError> {
    let (name, toolchain_value) = rustc_dynamic_library_environment(rustc)?;
    let mut paths = BTreeSet::new();
    for (crate_name, artifact) in &plan.externs {
        if !plan.caller_owned_library_digests.contains_key(crate_name) {
            continue;
        }
        let extension = artifact.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("dylib" | "so" | "dll")) {
            continue;
        }
        let parent = artifact.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned dynamic library",
            message: format!("{} has no parent directory", artifact.display()),
        })?;
        paths.insert(parent.to_path_buf());
    }
    paths.extend(env::split_paths(&toolchain_value));
    let value = env::join_paths(paths)
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: format!("cannot construct direct-Rustc dynamic library search path: {error}"),
        })?
        .into_string()
        .map_err(|_| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: "direct-Rustc dynamic library search path is not valid UTF-8".to_string(),
        })?;
    Ok((name, value))
}

/// Collect every manifest-recorded artifact by safe relative path for directory completeness checks.
pub fn expected_artifacts(manifest: &OvenRustcArtifactManifest) -> Result<BTreeMap<String, String>, OvenRustcError> {
    let mut expected = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for (source, artifact) in manifest
        .externs
        .iter()
        .map(|artifact| ("externs", (artifact.relative_path.as_str(), artifact.digest.as_str())))
        .chain(manifest.supporting_artifacts.iter().map(|artifact| {
            (
                "supporting",
                (artifact.relative_path.as_str(), artifact.digest.as_str()),
            )
        }))
        .chain(manifest.vocab_auxiliary_targets.iter().flat_map(|auxiliary| {
            auxiliary.externs.iter().map(|artifact| {
                (
                    "vocab-auxiliary",
                    (artifact.relative_path.as_str(), artifact.digest.as_str()),
                )
            })
        }))
    {
        let normalized = normalized_relative_path(artifact.0, "artifact")?;
        if expected.insert(normalized.clone(), artifact.1.to_string()).is_some() {
            let previous = sources.get(&normalized).copied().unwrap_or("unknown");
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!(
                    "declares one relative artifact path more than once: `{}` ({previous} and {source})",
                    artifact.0
                ),
            });
        }
        sources.insert(normalized, source);
    }
    Ok(expected)
}

/// Return the exact commit hash reported by `rustc -vV`, used to remap installed `rust-src` checkouts onto the
/// virtual `/rustc/<commit>` prefix a source-less toolchain embeds in standard-library debug spans.
pub fn rustc_commit_hash(rustc: &Path) -> Option<String> {
    let output = Command::new(rustc).arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .map(|hash| hash.trim().to_string())
}
