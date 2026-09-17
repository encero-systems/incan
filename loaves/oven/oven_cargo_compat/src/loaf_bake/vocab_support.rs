//! The compiler's vocab-support helpers, baked by Cargo into a Loaf: the bounded Cargo run, the artifact
//! selection from Cargo's own output, and the copy of the reported closure into the envelope.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::{fs, thread};

use oven_rustc::loaf::{LOAF_TEMP_SEQUENCE, OvenLoafError, OvenSourceCompilerVocabSupportRequest};
use oven_rustc::rustc::{
    OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcAuxiliaryTarget, OvenRustcSupportingArtifact,
    clear_inherited_cargo_environment,
};
use oven_store::digest_bytes;
use oven_store::process::{isolate_process_group, terminate_process_group};

use super::OvenLoafBakerContext;
use crate::publisher_capacity_probe_delay;

/// Run one baker-owned Cargo child while enforcing the aggregate transient physical allowance.
fn run_bounded_loaf_cargo(
    command: &mut Command,
    capacity_roots: &[&Path],
    transient_limit: u64,
    capture_root: &Path,
    label: &str,
) -> Result<(Vec<u8>, u64), OvenLoafError> {
    fs::create_dir_all(capture_root).map_err(|source| OvenLoafError::Io {
        path: capture_root.to_path_buf(),
        source,
    })?;
    let sequence = LOAF_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stdout_path = capture_root.join(format!(".oven-loaf-cargo-{sequence}.stdout"));
    let stderr_path = capture_root.join(format!(".oven-loaf-cargo-{sequence}.stderr"));
    let stdout = File::create(&stdout_path).map_err(|source| OvenLoafError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = File::create(&stderr_path).map_err(|source| OvenLoafError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    command.stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
    isolate_process_group(command);
    let mut child = command.spawn().map_err(|source| OvenLoafError::Io {
        path: PathBuf::from(label),
        source,
    })?;
    let mut peak = 0_u64;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|source| OvenLoafError::Io {
            path: PathBuf::from(label),
            source,
        })? {
            break status;
        }
        let scan_started = std::time::Instant::now();
        let observed = capacity_roots.iter().try_fold(0_u64, |total, root| {
            crate::conservative_directory_reservation(root).map(|bytes| total.saturating_add(bytes))
        })?;
        peak = peak.max(observed);
        if observed > transient_limit {
            terminate_process_group(&mut child).map_err(|source| OvenLoafError::Io {
                path: PathBuf::from(label),
                source,
            })?;
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "{label} exceeded the {transient_limit}-byte Loaf transient allowance at {observed} bytes"
                ),
            });
        }
        // This is the same physical-capacity supervisor used by the project publisher. A full scan walks the
        // complete private compiler-support target, so it yields for at least its own duration after a large scan
        // rather than immediately beginning another multi-gigabyte walk. The final exact admission below still
        // validates every persistent and transient root before this publisher returns.
        thread::sleep(publisher_capacity_probe_delay(scan_started.elapsed()));
    };
    let output = fs::read(&stdout_path).map_err(|source| OvenLoafError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let diagnostics = fs::read(&stderr_path).map_err(|source| OvenLoafError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    if !status.success() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "{label} publisher failed:\n{}",
                String::from_utf8_lossy(&diagnostics).trim()
            ),
        });
    }
    Ok((output, peak))
}

/// Build and seal the compiler-owned vocab registration closure into a native loaf.
///
/// A generated Incan program need not use JSON, while the compiler's vocab contract always serializes metadata.
/// Consequently, this compiler-owned closure cannot be inferred from a caller program's provider features. The
/// explicit `legacy_cargo` publisher builds only `incan_vocab` against the repository lockfile, copies its small
/// target-specific Rust closure into the immutable Loaf, and records the two helper roots as a host-target auxiliary
/// closure. Vocabulary extraction receives that closure; normal generated roots do not. This prevents a compiler
/// helper's separately compiled `serde_json` from becoming a second authority beside the full stdlib's `serde_json`.
/// No normal command can re-run this Cargo operation.
pub(super) fn bake_compiler_vocab_support(
    plan: &mut OvenRustcArtifactManifest,
    loaf_staging: &Path,
    context: &OvenLoafBakerContext<'_>,
) -> Result<u64, OvenLoafError> {
    bake_source_compiler_vocab_support(OvenSourceCompilerVocabSupportRequest {
        plan,
        loaf_staging,
        compiler_root: context.compiler_root,
        cargo: context.cargo,
        rustc: context.rustc,
        cargo_target: context.compiler_support_target,
        capacity_roots: &context.capacity_roots,
        transient_limit: context.transient_limit,
    })
}

/// Seal the compiler-source vocabulary helper closure into an explicit publisher's immutable plan.
///
/// The caller supplies the compiler root that owns the checked `incan_vocab` manifest and workspace lock, plus only
/// publisher-private output and capacity roots. This is deliberately unavailable to normal build, run, and test
/// paths: after this function returns, those paths receive only the digest-verified direct-Rustc artifacts recorded
/// in `plan`.
pub fn bake_source_compiler_vocab_support(
    request: OvenSourceCompilerVocabSupportRequest<'_>,
) -> Result<u64, OvenLoafError> {
    let OvenSourceCompilerVocabSupportRequest {
        plan,
        loaf_staging,
        compiler_root,
        cargo,
        rustc,
        cargo_target,
        capacity_roots,
        transient_limit,
    } = request;
    const INCAN_VOCAB: &str = "incan_vocab";
    const VOCAB_DESUGARER_TARGET: &str = "wasm32-wasip1";
    if plan.externs.iter().any(|artifact| artifact.crate_name == INCAN_VOCAB) {
        return Err(OvenLoafError::Preparation {
            message: "native foundation unexpectedly declares the compiler-owned incan_vocab extern".to_string(),
        });
    }
    if !cargo.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!("native vocabulary publisher Cargo is not a file: {}", cargo.display()),
        });
    }
    if !rustc.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher Rust compiler is not a file: {}",
                rustc.display()
            ),
        });
    }
    let crate_root = oven_model::toolchain_layout::support_crate_dir_in(compiler_root, "incan_vocab");
    let manifest = crate_root.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher manifest is unavailable: {}",
                manifest.display()
            ),
        });
    }

    let support_root = loaf_staging.join("compiler-support");
    // Cargo treats an explicitly requested host target differently from its default target directory. Keep the
    // native and Wasm publisher outputs disjoint so a second target invocation cannot rewrite the first closure
    // before its compiler-artifact records are sealed.
    let native_cargo_target = cargo_target.join("native");
    let wasm_cargo_target = cargo_target.join("wasm");
    let mut command = Command::new(cargo);
    command
        .current_dir(&crate_root)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--message-format=json-render-diagnostics")
        .arg("--target")
        .arg(&plan.intent.target)
        .arg("--target-dir")
        .arg(&native_cargo_target)
        .arg("--locked")
        .arg("--offline");
    if plan.intent.profile == "release" {
        command.arg("--release");
    }
    clear_inherited_cargo_environment(&mut command);
    command.env("RUSTC", rustc).env("CARGO_NET_OFFLINE", "true");
    if plan.intent.profile == "debug" {
        command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    let (compile_stdout, native_peak) = run_bounded_loaf_cargo(
        &mut command,
        capacity_roots,
        transient_limit,
        &native_cargo_target,
        "native vocabulary support",
    )?;

    // Vocabulary companions may ship a Wasm desugarer. Build its compiler-owned dependency closure here at the
    // explicit publisher boundary; a normal `incan build --lib` later invokes only direct Rustc against the copied,
    // digest-verified files. Keep this as a separate target directory so host artifacts can never be selected for
    // a Wasm command by accident.
    let mut wasm_command = Command::new(cargo);
    wasm_command
        .current_dir(&crate_root)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--message-format=json-render-diagnostics")
        .arg("--target")
        .arg(VOCAB_DESUGARER_TARGET)
        .arg("--target-dir")
        .arg(&wasm_cargo_target)
        .arg("--locked")
        .arg("--offline");
    if plan.intent.profile == "release" {
        wasm_command.arg("--release");
    }
    clear_inherited_cargo_environment(&mut wasm_command);
    wasm_command.env("RUSTC", rustc).env("CARGO_NET_OFFLINE", "true");
    if plan.intent.profile == "debug" {
        wasm_command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    let (wasm_stdout, wasm_peak) = run_bounded_loaf_cargo(
        &mut wasm_command,
        capacity_roots,
        transient_limit,
        &wasm_cargo_target,
        "native vocabulary Wasm support",
    )?;

    let profile = if plan.intent.profile == "release" {
        "release"
    } else {
        "debug"
    };
    // Cargo versions place compiler-artifact files either in `deps/` or under target-specific build output. Admit
    // only the two target-profile roots; the structured compiler-artifact records below still name every retained
    // file, so this never turns an ambient target-tree scan into authority.
    let primary_artifact_directory = native_cargo_target.join(&plan.intent.target).join(profile);
    let host_artifact_directory = native_cargo_target.join(profile);
    let loaf_directory = support_root.join("deps");
    let host_artifacts = compiler_artifact_paths_from_cargo_output(
        &compile_stdout,
        &native_cargo_target,
        &[&primary_artifact_directory, &host_artifact_directory],
        INCAN_VOCAB,
        &primary_artifact_directory,
        "native vocabulary support",
    )?;
    copy_compiler_vocab_support_artifacts(
        &host_artifacts,
        &primary_artifact_directory,
        &primary_artifact_directory,
        &host_artifact_directory,
        &loaf_directory,
        plan,
    )?;
    let wasm_primary_artifact_directory = wasm_cargo_target.join(VOCAB_DESUGARER_TARGET).join(profile);
    let wasm_primary_artifact_directory_canonical =
        fs::canonicalize(&wasm_primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: wasm_primary_artifact_directory.clone(),
            source,
        })?;
    let wasm_host_artifact_directory = wasm_cargo_target.join(profile);
    let wasm_artifacts = compiler_artifact_paths_from_cargo_output(
        &wasm_stdout,
        &wasm_cargo_target,
        &[&wasm_primary_artifact_directory, &wasm_host_artifact_directory],
        INCAN_VOCAB,
        &wasm_primary_artifact_directory,
        "native vocabulary Wasm support",
    )?
    .into_iter()
    .filter(|artifact| artifact.starts_with(&wasm_primary_artifact_directory_canonical))
    .collect::<Vec<_>>();
    copy_compiler_vocab_auxiliary_target_artifacts(
        &wasm_artifacts,
        &support_root.join(VOCAB_DESUGARER_TARGET).join("deps"),
        VOCAB_DESUGARER_TARGET,
        plan,
    )?;
    plan.externs
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    plan.validate_shape(&plan.intent)?;
    Ok(native_peak.max(wasm_peak))
}

/// Copy the sealed `incan_vocab` direct-Rustc support set produced by the compiler-owned package build.
///
/// The named publisher starts from an empty target directory, builds only `incan_vocab` against the checked lockfile,
/// and retains only the exact Rust-library paths in Cargo's `compiler-artifact` records for that invocation. The two
/// roots selected by vocabulary extraction (`incan_vocab` and `serde_json`) are host-target auxiliary externs; they
/// are deliberately not normal program externs. The remaining digested artifacts are their direct-Rustc support
/// closure, including host procedural macros. A stale or unrelated Cargo `deps` file is neither scanned nor admitted.
/// The normal guarded library-vocab regression exercises that sealed set and fails if a consumer attempts to launch
/// Cargo.
fn copy_compiler_vocab_support_artifacts(
    source_artifacts: &[PathBuf],
    target_artifact_directory: &Path,
    primary_artifact_directory: &Path,
    host_artifact_directory: &Path,
    loaf_directory: &Path,
    plan: &mut OvenRustcArtifactManifest,
) -> Result<(), OvenLoafError> {
    const INCAN_VOCAB: &str = "incan_vocab";
    const SERDE_JSON: &str = "serde_json";
    if !target_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no target artifact root: {}",
                target_artifact_directory.display()
            ),
        });
    }
    if !primary_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no primary artifact directory: {}",
                primary_artifact_directory.display()
            ),
        });
    }
    if !host_artifact_directory.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher produced no host artifact root: {}",
                host_artifact_directory.display()
            ),
        });
    }
    fs::create_dir_all(loaf_directory).map_err(|source| OvenLoafError::Io {
        path: loaf_directory.to_path_buf(),
        source,
    })?;
    let target_artifact_directory =
        fs::canonicalize(target_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: target_artifact_directory.to_path_buf(),
            source,
        })?;
    let primary_artifact_directory =
        fs::canonicalize(primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: primary_artifact_directory.to_path_buf(),
            source,
        })?;
    let host_artifact_directory = fs::canonicalize(host_artifact_directory).map_err(|source| OvenLoafError::Io {
        path: host_artifact_directory.to_path_buf(),
        source,
    })?;
    let mut copied = BTreeMap::new();
    let mut target_copied = BTreeMap::new();
    for source in source_artifacts {
        let target_artifacts = source.starts_with(&target_artifact_directory)
            || source.parent() == Some(primary_artifact_directory.as_path());
        if !target_artifacts && !source.starts_with(&host_artifact_directory) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary compiler-artifact escaped its declared target or host artifact root: {}",
                    source.display()
                ),
            });
        }
        let metadata = fs::symlink_metadata(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || !is_rust_library_artifact(source) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary compiler-artifact is not a regular direct-Rustc library: {}",
                    source.display()
                ),
            });
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!("native vocabulary artifact has no filename: {}", source.display()),
            })?
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        let digest = digest_bytes(&bytes);
        if let Some(existing) = copied.insert(file_name.clone(), digest.clone())
            && existing != digest
        {
            return Err(OvenLoafError::Preparation {
                message: format!("native vocabulary target and host closures conflict on artifact `{file_name}`"),
            });
        }
        if target_artifacts {
            target_copied.insert(file_name.clone(), digest.clone());
        }
        let destination = loaf_directory.join(&file_name);
        fs::write(&destination, bytes).map_err(|source_error| OvenLoafError::Io {
            path: destination,
            source: source_error,
        })?;
    }
    if copied.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "native vocabulary publisher retained no Rust artifacts from {}",
                target_artifact_directory.display()
            ),
        });
    }

    let relative_directory = "compiler-support/deps".to_string();
    let mut externs = Vec::new();
    for (file_name, digest) in &copied {
        let relative_path = format!("{relative_directory}/{file_name}");
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: relative_path.clone(),
            digest: digest.clone(),
        });
    }
    for crate_name in [INCAN_VOCAB, SERDE_JSON] {
        let matches = target_copied
            .iter()
            .filter(|(file_name, _)| is_named_rlib(file_name, crate_name))
            .collect::<Vec<_>>();
        let [(file_name, digest)] = matches.as_slice() else {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary publisher must retain exactly one `{crate_name}` rlib; found {}",
                    matches.len()
                ),
            });
        };
        let relative_path = format!("{relative_directory}/{file_name}");
        plan.supporting_artifacts
            .retain(|artifact| artifact.relative_path != relative_path);
        externs.push(OvenRustcArtifactExtern {
            crate_name: crate_name.to_string(),
            relative_path,
            digest: digest.to_string(),
        });
    }
    plan.vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
        target: plan.intent.target.clone(),
        dependency_search_paths: vec![relative_directory],
        externs,
    });
    plan.vocab_auxiliary_targets
        .sort_by(|left, right| left.target.cmp(&right.target));
    plan.supporting_artifacts
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(())
}

/// Return only Rust-library files explicitly emitted by one named publisher Cargo invocation.
///
/// Cargo's target tree is staging state, not an Oven input contract. The publisher requests structured
/// `compiler-artifact` output and this helper admits only listed regular files beneath the exact target/host artifact
/// roots supplied by the caller. Cargo may report the primary package's rlib at the profile root while its metadata
/// remains under `deps`; that one exact, named rlib is admitted as an explicit Rustc extern. Every other Rust-library
/// file outside the declared artifact roots is refused. A path outside the one publisher target root is likewise
/// refused. This keeps an unrelated retained artifact from becoming a silent immutable Loaf dependency while still
/// retaining the publisher's real primary artifact.
fn compiler_artifact_paths_from_cargo_output(
    cargo_stdout: &[u8],
    publisher_target_root: &Path,
    allowed_directories: &[&Path],
    primary_crate_name: &str,
    primary_artifact_directory: &Path,
    publisher: &str,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    let publisher_target_root = fs::canonicalize(publisher_target_root).map_err(|source| OvenLoafError::Io {
        path: publisher_target_root.to_path_buf(),
        source,
    })?;
    let allowed_directories = allowed_directories
        .iter()
        .map(|directory| {
            fs::canonicalize(directory).map_err(|source| OvenLoafError::Io {
                path: (*directory).to_path_buf(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if allowed_directories.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("{publisher} publisher declared no Cargo artifact roots"),
        });
    }
    let primary_artifact_directory =
        fs::canonicalize(primary_artifact_directory).map_err(|source| OvenLoafError::Io {
            path: primary_artifact_directory.to_path_buf(),
            source,
        })?;
    let primary_artifact_filename = format!("lib{primary_crate_name}.rlib");
    let cargo_stdout = std::str::from_utf8(cargo_stdout).map_err(|error| OvenLoafError::Preparation {
        message: format!("{publisher} publisher emitted non-UTF-8 Cargo JSON: {error}"),
    })?;
    let mut artifacts = BTreeSet::new();
    for (line_number, line) in cargo_stdout.lines().enumerate() {
        let value = serde_json::from_str::<serde_json::Value>(line).map_err(|error| OvenLoafError::Preparation {
            message: format!(
                "{publisher} publisher emitted invalid Cargo JSON on line {}: {error}",
                line_number + 1
            ),
        })?;
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let filenames = value
            .get("filenames")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has no filenames",
                    line_number + 1
                ),
            })?;
        let artifact_target_name = value
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has no target name",
                    line_number + 1
                ),
            })?;
        for filename in filenames {
            let filename = filename.as_str().ok_or_else(|| OvenLoafError::Preparation {
                message: format!(
                    "{publisher} publisher compiler-artifact on line {} has a non-string filename",
                    line_number + 1
                ),
            })?;
            let source = PathBuf::from(filename);
            if !is_rust_library_artifact(&source) {
                continue;
            }
            let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLoafError::Io {
                path: source.clone(),
                source: source_error,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact is not a regular file: {}",
                        source.display()
                    ),
                });
            }
            let source = fs::canonicalize(&source).map_err(|source_error| OvenLoafError::Io {
                path: source.clone(),
                source: source_error,
            })?;
            let in_dependency_directory = allowed_directories
                .iter()
                .any(|directory| source.starts_with(directory));
            let is_primary_profile_rlib = artifact_target_name == primary_crate_name
                && source.parent() == Some(primary_artifact_directory.as_path())
                && source.file_name().and_then(|name| name.to_str()) == Some(primary_artifact_filename.as_str());
            if in_dependency_directory || is_primary_profile_rlib {
                artifacts.insert(source);
            } else if !source.starts_with(&publisher_target_root) {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact escaped its target root: {}",
                        source.display()
                    ),
                });
            } else {
                return Err(OvenLoafError::Preparation {
                    message: format!(
                        "{publisher} publisher compiler-artifact escaped its declared artifact roots: {}",
                        source.display()
                    ),
                });
            }
        }
    }
    if artifacts.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("{publisher} publisher emitted no Rust compiler-artifact files"),
        });
    }
    Ok(artifacts.into_iter().collect())
}

/// Copy the target-only vocabulary support closure used to produce Wasm desugarers without Cargo.
///
/// The host closure remains an auxiliary search path because Rustc may need host procedural macros while compiling
/// target code. The target rlibs are retained separately and named explicitly, which prevents host and Wasm copies
/// of the same crate from occupying one ambiguous direct-Rustc search directory.
fn copy_compiler_vocab_auxiliary_target_artifacts(
    source_artifacts: &[PathBuf],
    loaf_directory: &Path,
    target: &str,
    plan: &mut OvenRustcArtifactManifest,
) -> Result<(), OvenLoafError> {
    const INCAN_VOCAB: &str = "incan_vocab";
    const SERDE_JSON: &str = "serde_json";
    fs::create_dir_all(loaf_directory).map_err(|source| OvenLoafError::Io {
        path: loaf_directory.to_path_buf(),
        source,
    })?;
    let mut artifacts = BTreeMap::new();
    for source in source_artifacts {
        let metadata = fs::symlink_metadata(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || !is_rust_library_artifact(source) {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary {target} compiler-artifact is not a regular direct-Rustc library: {}",
                    source.display()
                ),
            });
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| OvenLoafError::Preparation {
                message: format!("native vocabulary artifact has no filename: {}", source.display()),
            })?
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(source).map_err(|source_error| OvenLoafError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        let digest = digest_bytes(&bytes);
        if artifacts.insert(file_name.clone(), digest.clone()).is_some() {
            return Err(OvenLoafError::Preparation {
                message: format!("native vocabulary {target} closure duplicates artifact `{file_name}`"),
            });
        }
        let destination = loaf_directory.join(&file_name);
        fs::write(&destination, bytes).map_err(|source_error| OvenLoafError::Io {
            path: destination,
            source: source_error,
        })?;
    }
    if artifacts.is_empty() {
        return Err(OvenLoafError::Preparation {
            message: format!("native vocabulary publisher retained no declared {target} Rust artifacts"),
        });
    }
    let relative_directory = format!("compiler-support/{target}/deps");
    let mut externs = Vec::new();
    for crate_name in [INCAN_VOCAB, SERDE_JSON] {
        let matches = artifacts
            .iter()
            .filter(|(file_name, _)| is_named_rlib(file_name, crate_name))
            .collect::<Vec<_>>();
        let [(file_name, digest)] = matches.as_slice() else {
            return Err(OvenLoafError::Preparation {
                message: format!(
                    "native vocabulary {target} publisher must retain exactly one `{crate_name}` rlib; found {}",
                    matches.len()
                ),
            });
        };
        externs.push(OvenRustcArtifactExtern {
            crate_name: crate_name.to_string(),
            relative_path: format!("{relative_directory}/{file_name}"),
            digest: digest.to_string(),
        });
    }
    for (file_name, digest) in artifacts {
        let relative_path = format!("{relative_directory}/{file_name}");
        if externs.iter().any(|artifact| artifact.relative_path == relative_path) {
            continue;
        }
        plan.supporting_artifacts
            .push(OvenRustcSupportingArtifact { relative_path, digest });
    }
    // `compiler-support/deps` holds host proc macros that Rustc may load while expanding the target closure.
    let mut dependency_search_paths = vec![relative_directory, "compiler-support/deps".to_string()];
    dependency_search_paths.sort();
    plan.vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
        target: target.to_string(),
        dependency_search_paths,
        externs,
    });
    plan.vocab_auxiliary_targets
        .sort_by(|left, right| left.target.cmp(&right.target));
    Ok(())
}

/// Return whether a file can participate in a direct Rustc dependency closure.
fn is_rust_library_artifact(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rlib" | "dylib" | "so" | "dll")
    )
}

/// Return whether a retained artifact is the exact rlib for one compiler-owned crate.
pub(super) fn is_named_rlib(relative_path: &str, crate_name: &str) -> bool {
    let Some(name) = Path::new(relative_path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == format!("lib{crate_name}.rlib")
        || (name.starts_with(&format!("lib{crate_name}-")) && name.ends_with(".rlib"))
}

/// Return whether a manifest path is the dynamic compiler-owned `incan_derive` procedural macro.
pub(super) fn is_incan_derive_artifact(relative_path: &str) -> bool {
    let Some(name) = Path::new(relative_path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("libincan_derive-")
        && matches!(
            Path::new(name).extension().and_then(|extension| extension.to_str()),
            Some("dylib" | "so" | "dll")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::{fs, thread};

    use oven_rustc::loaf::OvenLoafError;
    use oven_rustc::rustc::{OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    fn runtime_receipt(
        source: &Path,
        providers: &str,
        rust_dependencies: &str,
        stdlib_facets: &str,
    ) -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
        let provider_plan = digest_bytes(providers.as_bytes());
        let mut request = OvenGeneratedProjectRequest::new(
            source.parent().ok_or("source has no parent")?,
            "runtime_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc seeded-test",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", source)
        .with_build_unit_input("runtime-lock", "runtime-lock")
        .with_build_unit_input("rust-dependencies", rust_dependencies)
        .with_build_unit_input("stdlib-facets", stdlib_facets)
        .with_build_unit_input("provider-plan", provider_plan);
        if !providers.is_empty() {
            request = request.with_build_unit_input("providers", providers);
        }
        Ok(receipt_generated_project(&request)?)
    }

    fn runtime_receipt_for_plan() -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        // The receipt owns no filesystem path, so retaining only its value is valid after this helper drops the
        // temporary source tree.
        runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-facets")
    }

    fn empty_manifest(receipt: &oven_store::OvenReceipt) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn loaf_capacity_abort_terminates_fake_cargo_descendants() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        use std::time::{Duration, Instant};

        let fixture = tempfile::tempdir()?;
        let capacity_root = fixture.path().join("capacity");
        let capture_root = fixture.path().join("capture");
        let overflow = capacity_root.join("overflow");
        let descendant_pid = fixture.path().join("descendant-pid");
        let cargo = fixture.path().join("cargo");
        fs::create_dir_all(&capacity_root)?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\ndd if=/dev/zero of=\"{}\" bs=8192 count=1 2>/dev/null\nwait\n",
                descendant_pid.display(),
                overflow.display(),
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let mut command = Command::new(&cargo);
        let started = Instant::now();
        let result = run_bounded_loaf_cargo(&mut command, &[&capacity_root], 4 * 1024, &capture_root, "fake Cargo");
        assert!(matches!(result, Err(OvenLoafError::Preparation { .. })));
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "capacity abort waited for the fake Cargo descendant instead of terminating its process group"
        );
        fs::remove_dir_all(&capacity_root)?;
        fs::remove_dir_all(&capture_root)?;
        assert!(
            !capacity_root.exists(),
            "capacity-aborted Loaf staging was not removable"
        );
        assert!(
            !capture_root.exists(),
            "capacity-aborted Loaf capture output was not removable"
        );
        let pid = fs::read_to_string(descendant_pid)?.trim().parse::<u32>()?;
        for _ in 0..100 {
            if !oven_store::process::process_is_running(pid)? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Err("Loaf capacity abort left the fake-Cargo descendant running".into())
    }

    #[test]
    fn native_vocab_loaf_copies_only_cargo_reported_artifact_closure() -> Result<(), Box<dyn std::error::Error>> {
        let publisher = tempfile::tempdir()?;
        let target_deps = publisher.path().join("target/deps");
        let host_deps = publisher.path().join("host/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        let reported = [
            ("serde_json", "libserde_json-reported.rlib", b"json".as_slice()),
            (
                "required_transitive",
                "librequired_transitive-reported.rlib",
                b"transitive".as_slice(),
            ),
        ];
        for (_, name, contents) in reported {
            fs::write(target_deps.join(name), contents)?;
        }
        let unreported = target_deps.join("libunrelated_cargo_residue.rlib");
        fs::write(&unreported, b"unreported")?;
        // Cargo reports both the hashed `deps` input and an unhashed convenience copy at the profile root. The latter
        // is publisher output, not a direct-rustc input, and must not expand the sealed loaf closure.
        let profile_copy = publisher.path().join("target/libincan_vocab.rlib");
        fs::write(&profile_copy, b"profile copy")?;
        let profile_copy_canonical = fs::canonicalize(&profile_copy)?;
        let mut cargo_output = reported
            .iter()
            .map(|(crate_name, name, _)| {
                serde_json::json!({
                    "reason": "compiler-artifact",
                    "target": { "name": crate_name },
                    "filenames": [target_deps.join(name).display().to_string()],
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        cargo_output.push('\n');
        cargo_output.push_str(
            &serde_json::json!({
                "reason": "compiler-artifact",
                "target": { "name": "incan_vocab" },
                "filenames": [profile_copy.display().to_string()],
            })
            .to_string(),
        );
        let artifacts = super::compiler_artifact_paths_from_cargo_output(
            cargo_output.as_bytes(),
            publisher.path(),
            &[target_deps.as_path(), host_deps.as_path()],
            "incan_vocab",
            publisher.path().join("target").as_path(),
            "native vocabulary fixture",
        )?;
        let loaf = tempfile::tempdir()?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.dependency_search_paths = vec!["target/deps".to_string(), "host/deps".to_string()];
        for (relative_path, bytes) in [
            ("target/deps/libnormal.rlib", b"normal".as_slice()),
            ("host/deps/libmacro.rlib", b"macro".as_slice()),
        ] {
            let file = loaf.path().join(relative_path);
            fs::create_dir_all(file.parent().ok_or("normal artifact parent missing")?)?;
            fs::write(file, bytes)?;
            plan.supporting_artifacts
                .push(oven_rustc::rustc::OvenRustcSupportingArtifact {
                    relative_path: relative_path.to_string(),
                    digest: digest_bytes(bytes),
                });
        }
        super::super::record_generated_root_externs(&mut plan)?;
        let original_role = plan.entrypoint_dependency_search_paths["generated-root"].clone();
        super::copy_compiler_vocab_support_artifacts(
            &artifacts,
            &target_deps,
            &publisher.path().join("target"),
            &host_deps,
            &loaf.path().join("compiler-support/deps"),
            &mut plan,
        )?;

        assert_eq!(plan.entrypoint_dependency_search_paths["generated-root"], original_role);
        let materialized = plan.materialize(loaf.path(), &receipt.intent)?;
        let selected =
            oven_rustc::rustc::trusted_artifact_plan_for_source_evidence(&materialized, &plan, "generated-root")?;
        let physical_root = fs::canonicalize(loaf.path())?;
        assert_eq!(
            selected
                .dependency_search_paths
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([physical_root.join("target/deps"), physical_root.join("host/deps")])
        );
        assert!(
            !selected
                .dependency_search_paths
                .contains(&physical_root.join("compiler-support/deps"))
        );
        assert!(
            !loaf
                .path()
                .join("compiler-support/deps/libunrelated_cargo_residue.rlib")
                .exists()
        );
        assert!(
            artifacts.iter().any(|artifact| artifact == &profile_copy_canonical),
            "the named publisher's reported profile-root rlib must enter the direct-rustc closure"
        );
        assert!(
            plan.externs.is_empty(),
            "compiler-only vocab roots must not become program externs"
        );
        let host_support = plan
            .vocab_auxiliary_targets
            .iter()
            .find(|target| target.target == receipt.intent.target)
            .ok_or("missing host compiler vocabulary auxiliary closure")?;
        assert_eq!(
            host_support
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["incan_vocab", "serde_json"]),
            "the host vocabulary helper must retain its exact direct-Rustc roots outside the normal program plan"
        );
        assert!(
            plan.supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("librequired_transitive-reported.rlib"))
        );
        assert!(
            !plan
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("libunrelated_cargo_residue.rlib"))
        );
        Ok(())
    }

    #[test]
    fn native_vocab_loaf_rejects_unexpected_profile_root_compiler_artifact() -> Result<(), Box<dyn std::error::Error>> {
        let publisher = tempfile::tempdir()?;
        let target_deps = publisher.path().join("target/deps");
        let host_deps = publisher.path().join("host/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        let unexpected = publisher.path().join("target/libunrelated.rlib");
        fs::write(&unexpected, b"must not become a sealed input")?;
        let cargo_output = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "unrelated" },
            "filenames": [unexpected.display().to_string()],
        })
        .to_string();

        let error = match super::compiler_artifact_paths_from_cargo_output(
            cargo_output.as_bytes(),
            publisher.path(),
            &[target_deps.as_path(), host_deps.as_path()],
            "incan_vocab",
            publisher.path().join("target").as_path(),
            "native vocabulary fixture",
        ) {
            Ok(_) => return Err("unexpected profile-root artifact must fail closed".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("escaped its declared artifact roots"));
        Ok(())
    }
}
