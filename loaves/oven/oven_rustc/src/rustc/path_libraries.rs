//! Materializing declared path Rust libraries through direct rustc.

use super::{
    BTreeMap, BTreeSet, Command, DependencySource, DependencySpec, OvenCallerOwnedRustcLibrary,
    OvenRegistryLeafAuthority, OvenRustcArtifactPlan, OvenRustcError, Path, PathBuf, apply_oven_profile,
    canonical_directory, clear_inherited_cargo_environment, digest_bytes, digest_regular_file, digest_source_tree,
    direct_rustc_crate_name, fs, incan_owned_rustup_home, incan_owned_tool, parse_rustc_diagnostics,
    resolve_sealed_registry_leaf, resolve_sealed_registry_leaf_with_search_paths, rustc_identity,
    verified_regular_file,
};

/// Materialized child externs and their admitted dependency search closure.
type MaterializedPathDependencies = (Vec<(String, PathBuf)>, BTreeSet<PathBuf>);

/// Return the Cargo belonging to Incan's own provisioned toolchain, when one exists.
///
/// The compatibility baker's Cargo must match the compiler [`resolve_active_rustc`] selects; resolving one from the
/// isolated installation and the other from the user's ambient default would reintroduce the toolchain mismatch this
/// isolation exists to prevent.
pub fn incan_owned_cargo() -> Option<PathBuf> {
    incan_owned_tool(&incan_owned_rustup_home()?, "cargo")
}

/// Compile declared narrow Rust library closures with direct `rustc`, never with Cargo.
///
/// This bounded caller-dependency seam builds manifest-declared local path libraries and links registry leaves only
/// from a selected immutable Loaf catalog. It recursively follows only path-to-path edges and incorporates each child
/// output digest into its parent output identity. Git, optional/feature-driven roots, build scripts, and unsealed
/// registry closures remain explicit unsupported inputs.
///
/// Gate 6 of RFC 119 is the reader; until it lands only tests exercise this seam.
#[cfg(test)]
#[allow(dead_code, reason = "Gate 6 of RFC 119 is the reader")]
pub fn materialize_declared_rust_libraries(
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    dependencies: &[DependencySpec],
    registry_authority: Option<&OvenRegistryLeafAuthority>,
) -> Result<Vec<OvenCallerOwnedRustcLibrary>, OvenRustcError> {
    materialize_declared_rust_libraries_with_selected_path_authority(
        output_root,
        rustc,
        target,
        profile,
        dependencies,
        registry_authority,
        None,
    )
}

/// Materialize caller-owned libraries while allowing a scheduler-selected path dependency to remain plan-owned.
///
/// Ordinary callers pass no selected-path authority and therefore keep the conservative manifest-shaped behavior.
/// The compiler-suite scheduler alone supplies this additional authority after leasing the immutable data roots.
pub fn materialize_declared_rust_libraries_with_selected_path_authority(
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    dependencies: &[DependencySpec],
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
) -> Result<Vec<OvenCallerOwnedRustcLibrary>, OvenRustcError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(output_root).map_err(|source| OvenRustcError::Io {
        path: output_root.to_path_buf(),
        source,
    })?;
    let mut state = PathRustcMaterializationState::default();
    let mut names = BTreeSet::new();
    let mut dependencies = dependencies.to_vec();
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    for dependency in dependencies {
        let crate_name = direct_rustc_crate_name(&dependency.crate_name)?;
        if !names.insert(crate_name.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!("declares duplicate crate `{crate_name}`"),
            });
        }
        if dependency.optional {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` is optional; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        let selected_path = selected_path_authority.and_then(|authority| authority.resolve(&dependency));
        if !matches!(dependency.source, DependencySource::Registry)
            && !dependency.features.is_empty()
            && selected_path.is_none()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` explicitly enables path-package Cargo features; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        if matches!(dependency.source, DependencySource::Registry) && !dependency.default_features {
            return Err(OvenRustcError::InvalidInput {
                field: "Oven direct-rustc Rust dependency",
                message: format!(
                    "`{}` disables registry default features; prepare an explicit Oven-native closure",
                    dependency.crate_name
                ),
            });
        }
        let package_root = match &dependency.source {
            DependencySource::Path { path } => {
                if selected_path.is_some() {
                    // The final direct-Rustc plan already attaches this selected compiler-runtime extern. Keeping it
                    // out of caller-owned outputs avoids a second `--extern` with the same name.
                    continue;
                }
                path.clone()
            }
            DependencySource::Registry => {
                let sealed = resolve_sealed_registry_leaf(&dependency, registry_authority, profile)?;
                let output = selected_path_authority
                    .and_then(|authority| authority.matching_sealed_registry_artifact(&sealed))
                    .unwrap_or(sealed);
                let digest = digest_regular_file(&output, "sealed registry Rust dependency")?;
                state.record_extern(crate_name, output, digest)?;
                continue;
            }
            DependencySource::Git { .. } => {
                return Err(OvenRustcError::InvalidInput {
                    field: "Oven direct-rustc Rust dependency",
                    message: format!(
                        "`{}` is a Git package; prepare an explicit Oven-native closure",
                        dependency.crate_name
                    ),
                });
            }
        };
        let output = materialize_path_rust_library(
            &package_root,
            output_root,
            rustc,
            target,
            profile,
            registry_authority,
            selected_path_authority,
            &dependency,
            &mut state,
        )?;
        let digest = digest_regular_file(&output, "materialized path Rust library")?;
        state.record_extern(crate_name, output, digest)?;
    }
    Ok(state
        .externs
        .into_iter()
        .map(|(crate_name, (output, digest))| OvenCallerOwnedRustcLibrary {
            crate_name,
            output,
            digest,
            expose_extern: true,
        })
        .collect())
}

/// Compile one Cargo-manifest-shaped local Rust library using only explicitly supplied direct artifacts.
#[allow(clippy::too_many_arguments)]
fn materialize_path_rust_library(
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    requested_dependency: &DependencySpec,
    state: &mut PathRustcMaterializationState,
) -> Result<PathBuf, OvenRustcError> {
    let package_root = canonical_directory(package_root, "path Rust dependency")?;
    if let Some(output) = state.outputs.get(&package_root) {
        return Ok(output.clone());
    }
    if !state.active.insert(package_root.clone()) {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency",
            message: format!("contains a cyclic path dependency at {}", package_root.display()),
        });
    }
    let result = materialize_path_rust_library_inner(
        &package_root,
        output_root,
        rustc,
        target,
        profile,
        registry_authority,
        selected_path_authority,
        requested_dependency,
        state,
    );
    state.active.remove(&package_root);
    let output = result?;
    state.outputs.insert(package_root, output.clone());
    Ok(output)
}

/// Materialize one manifest-backed path library after its caller has established cycle and output ownership state.
#[allow(clippy::too_many_arguments)]
fn materialize_path_rust_library_inner(
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    requested_dependency: &DependencySpec,
    state: &mut PathRustcMaterializationState,
) -> Result<PathBuf, OvenRustcError> {
    let parsed = parse_path_rust_library(package_root, requested_dependency)?;
    let (child_dependencies, child_dependency_search_paths) = materialize_path_rust_dependencies(
        &parsed,
        package_root,
        output_root,
        rustc,
        target,
        profile,
        registry_authority,
        selected_path_authority,
        state,
    )?;
    compile_path_rust_library(
        parsed,
        package_root,
        output_root,
        rustc,
        target,
        profile,
        &child_dependencies,
        &child_dependency_search_paths,
    )
}

/// Parsed and validated compilation inputs from one path library manifest.
struct ParsedPathRustLibrary {
    manifest_path: PathBuf,
    manifest: toml::Value,
    crate_name: String,
    source: PathBuf,
    edition: String,
    is_proc_macro: bool,
}

/// Parse and validate the Cargo-manifest subset supported by direct path-library compilation.
fn parse_path_rust_library(
    package_root: &Path,
    requested_dependency: &DependencySpec,
) -> Result<ParsedPathRustLibrary, OvenRustcError> {
    let manifest_path = package_root.join("Cargo.toml");
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| OvenRustcError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} is not UTF-8: {error}", manifest_path.display()),
    })?;
    let manifest = toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} is invalid TOML: {error}", manifest_path.display()),
    })?;
    let manifest = super::path_workspace::effective_path_manifest(&manifest_path, manifest)?;
    if manifest.get("target").is_some() {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} declares target-conditional dependencies; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    let package =
        manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!("{} has no [package] table", manifest_path.display()),
            })?;
    let package_name =
        package
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!("{} has no package name", manifest_path.display()),
            })?;
    let lib = manifest.get("lib").and_then(toml::Value::as_table);
    let is_proc_macro = lib
        .and_then(|lib| lib.get("proc-macro"))
        .map(|value| {
            value.as_bool().ok_or_else(|| OvenRustcError::InvalidInput {
                field: "path Rust dependency Cargo.toml",
                message: format!(
                    "{} has a non-boolean lib.proc-macro declaration",
                    manifest_path.display()
                ),
            })
        })
        .transpose()?
        .unwrap_or(false);
    if lib
        .and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
        .is_some_and(|types| {
            types.iter().any(|kind| {
                !(matches!(kind.as_str(), Some("lib" | "rlib"))
                    || (is_proc_macro && matches!(kind.as_str(), Some("proc-macro"))))
            })
        })
    {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} requests an unsupported crate type; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    let crate_name = direct_rustc_crate_name(
        lib.and_then(|lib| lib.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(package_name),
    )?;
    let source_relative = lib
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    let source = package_root.join(source_relative);
    let source = verified_regular_file(&source, "path Rust library source")?;
    let edition = package.get("edition").and_then(toml::Value::as_str).unwrap_or("2018");
    if !matches!(edition, "2018" | "2021" | "2024") {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!("{} has unsupported edition `{edition}`", manifest_path.display()),
        });
    }
    if manifest.get("build").is_some() || package.get("build").is_some_and(|build| build.as_bool() != Some(false)) {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} declares a build script; prepare an explicit Oven-native closure",
                manifest_path.display()
            ),
        });
    }
    validate_inactive_path_dependency_features(&manifest_path, &manifest, requested_dependency)?;
    let edition = edition.to_string();
    Ok(ParsedPathRustLibrary {
        manifest_path,
        manifest,
        crate_name,
        source,
        edition,
        is_proc_macro,
    })
}

/// Materialize the exact path and registry dependencies declared by one parsed library.
#[allow(clippy::too_many_arguments)]
fn materialize_path_rust_dependencies(
    parsed: &ParsedPathRustLibrary,
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    state: &mut PathRustcMaterializationState,
) -> Result<MaterializedPathDependencies, OvenRustcError> {
    let mut child_dependencies = Vec::new();
    let mut child_dependency_search_paths = BTreeSet::new();
    if let Some(dependencies) = parsed.manifest.get("dependencies").and_then(toml::Value::as_table) {
        for (name, specification) in dependencies {
            let Some(dependency) =
                path_rust_manifest_dependency(&parsed.manifest_path, package_root, name, specification)?
            else {
                continue;
            };
            let selected_path_artifact = selected_path_authority.and_then(|authority| authority.resolve(&dependency));
            let output = match &dependency.source {
                DependencySource::Path { path } => {
                    if let Some(output) = selected_path_artifact.clone() {
                        if let Some(authority) = selected_path_authority {
                            child_dependency_search_paths.extend(authority.dependency_search_paths().iter().cloned());
                        }
                        output
                    } else {
                        materialize_path_rust_library(
                            path,
                            output_root,
                            rustc,
                            target,
                            profile,
                            registry_authority,
                            selected_path_authority,
                            &dependency,
                            state,
                        )?
                    }
                }
                DependencySource::Registry => {
                    let resolved =
                        resolve_sealed_registry_leaf_with_search_paths(&dependency, registry_authority, profile)?;
                    if let Some(output) = selected_path_authority
                        .and_then(|authority| authority.matching_sealed_registry_artifact(&resolved.artifact))
                    {
                        if let Some(authority) = selected_path_authority {
                            child_dependency_search_paths.extend(authority.dependency_search_paths().iter().cloned());
                        }
                        output
                    } else {
                        child_dependency_search_paths.extend(resolved.dependency_search_paths);
                        resolved.artifact
                    }
                }
                DependencySource::Git { .. } => unreachable!("path manifest parser rejects Git dependencies"),
            };
            let child_name = direct_rustc_crate_name(name)?;
            if selected_path_artifact.is_none() {
                let digest = digest_regular_file(&output, "path Rust library dependency")?;
                state.record_extern(child_name.clone(), output.clone(), digest)?;
            }
            child_dependencies.push((child_name, output));
        }
    }
    child_dependencies.sort_by(|left, right| left.0.cmp(&right.0));
    Ok((child_dependencies, child_dependency_search_paths))
}

/// Bind reuse identity, invoke rustc, and atomically publish one parsed path library.
#[allow(clippy::too_many_arguments)]
fn compile_path_rust_library(
    parsed: ParsedPathRustLibrary,
    package_root: &Path,
    output_root: &Path,
    rustc: &Path,
    target: &str,
    profile: &str,
    child_dependencies: &[(String, PathBuf)],
    child_dependency_search_paths: &BTreeSet<PathBuf>,
) -> Result<PathBuf, OvenRustcError> {
    let ParsedPathRustLibrary {
        crate_name,
        source,
        edition,
        is_proc_macro,
        ..
    } = parsed;
    let source_digest = digest_source_tree(package_root).map_err(|error| OvenRustcError::InvalidInput {
        field: "path Rust dependency",
        message: error.to_string(),
    })?;
    let manifest_digest = digest_bytes(parsed.manifest.to_string().as_bytes());
    let child_digest_records = child_dependencies
        .iter()
        .map(|(name, output)| {
            let bytes = fs::read(output).map_err(|source| OvenRustcError::Io {
                path: output.clone(),
                source,
            })?;
            Ok(format!("{name}|{}", digest_bytes(&bytes)))
        })
        .collect::<Result<Vec<_>, OvenRustcError>>()?;
    let toolchain = rustc_identity(rustc)?;
    let identity = digest_bytes(
        format!(
            "{source_digest}\n{manifest_digest}\n{target}\n{profile}\n{toolchain}\n{}",
            child_digest_records.join("\n")
        )
        .as_bytes(),
    );
    let output_directory = output_root.join(identity.strip_prefix("sha256:").unwrap_or(identity.as_str()));
    let extension = if is_proc_macro {
        std::env::consts::DLL_SUFFIX
    } else {
        ".rlib"
    };
    let output = output_directory.join(format!("lib{crate_name}{extension}"));
    if output.is_file() {
        return verified_regular_file(&output, "materialized path Rust library");
    }
    fs::create_dir_all(&output_directory).map_err(|source| OvenRustcError::Io {
        path: output_directory.clone(),
        source,
    })?;
    let temporary = output_directory.join(format!(".lib{crate_name}.{}.tmp", std::process::id()));
    let mut command = Command::new(rustc);
    command
        .arg("--target")
        .arg(target)
        .arg(format!("--edition={edition}"))
        .arg("--crate-name")
        .arg(&crate_name)
        .arg("--error-format=json")
        .arg(&source)
        .arg("-o")
        .arg(&temporary);
    if is_proc_macro {
        command.args(["--crate-type", "proc-macro", "--extern", "proc_macro"]);
    } else {
        command.args(["--crate-type", "lib"]);
    }
    apply_oven_profile(&mut command, profile);
    clear_inherited_cargo_environment(&mut command);
    for dependency_search_path in child_dependency_search_paths {
        command
            .arg("-L")
            .arg(format!("dependency={}", dependency_search_path.display()));
    }
    for (child_name, child_output) in child_dependencies {
        command
            .arg("--extern")
            .arg(format!("{child_name}={}", child_output.display()));
    }
    let output_result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source: source_error,
    })?;
    if !output_result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&output_result.stdout, &output_result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(&temporary, "materialized path Rust library")?;
    fs::rename(&temporary, &output).map_err(|source| OvenRustcError::Io {
        path: output.clone(),
        source,
    })?;
    Ok(output)
}

/// Parse one unconditional local-manifest dependency into the same receipt-bound direct-Rustc representation used by
/// top-level caller dependencies. Optional dependencies are inactive because activated path features remain rejected;
/// registry dependencies must be satisfied by the supplied sealed authority, never Cargo.
pub(super) fn path_rust_manifest_dependency(
    manifest_path: &Path,
    package_root: &Path,
    name: &str,
    specification: &toml::Value,
) -> Result<Option<DependencySpec>, OvenRustcError> {
    let dependency = match specification {
        toml::Value::String(version) => DependencySpec {
            crate_name: name.to_string(),
            version: Some(version.clone()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        },
        toml::Value::Table(table) => {
            let Some(dependency) = path_rust_table_dependency(manifest_path, package_root, name, table)? else {
                return Ok(None);
            };
            dependency
        }
        _ => {
            return Err(path_rust_dependency_error(format!(
                "{} dependency `{name}` has an unsupported Cargo manifest shape",
                manifest_path.display()
            )));
        }
    }
    .normalized();
    Ok(Some(dependency))
}

/// Decode one table-shaped Cargo dependency after rejecting workspace, Git, and optional forms.
fn path_rust_table_dependency(
    manifest_path: &Path,
    package_root: &Path,
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
) -> Result<Option<DependencySpec>, OvenRustcError> {
    if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        return Err(path_rust_dependency_error(format!(
            "{} dependency `{name}` inherits a Cargo workspace declaration; prepare an explicit Oven-native closure",
            manifest_path.display()
        )));
    }
    if table.get("git").is_some() {
        return Err(path_rust_dependency_error(format!(
            "{} dependency `{name}` is Git-sourced; prepare an explicit Oven-native closure",
            manifest_path.display()
        )));
    }
    let optional = table
        .get("optional")
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                path_rust_dependency_error(format!(
                    "{} dependency `{name}` has a non-boolean optional declaration",
                    manifest_path.display()
                ))
            })
        })
        .transpose()?
        .unwrap_or(false);
    if optional {
        return Ok(None);
    }
    let features = table
        .get("features")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| {
                    path_rust_dependency_error(format!(
                        "{} dependency `{name}` has a non-array feature declaration",
                        manifest_path.display()
                    ))
                })?
                .iter()
                .map(|feature| {
                    feature.as_str().map(str::to_string).ok_or_else(|| {
                        path_rust_dependency_error(format!(
                            "{} dependency `{name}` has a non-string feature",
                            manifest_path.display()
                        ))
                    })
                })
                .collect::<Result<Vec<_>, OvenRustcError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let default_features = table
        .get("default-features")
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                path_rust_dependency_error(format!(
                    "{} dependency `{name}` has a non-boolean default-features declaration",
                    manifest_path.display()
                ))
            })
        })
        .transpose()?
        .unwrap_or(true);
    let package = table
        .get("package")
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                path_rust_dependency_error(format!(
                    "{} dependency `{name}` has a non-string package alias",
                    manifest_path.display()
                ))
            })
        })
        .transpose()?;
    let version = table
        .get("version")
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                path_rust_dependency_error(format!(
                    "{} dependency `{name}` has a non-string version",
                    manifest_path.display()
                ))
            })
        })
        .transpose()?;
    let source = path_rust_dependency_source(
        manifest_path,
        package_root,
        name,
        table,
        &features,
        default_features,
        version.as_ref(),
    )?;
    let dependency = DependencySpec {
        crate_name: name.to_string(),
        version,
        features,
        default_features,
        source,
        optional: false,
        package,
    };
    Ok(Some(dependency))
}

/// Resolve the admitted path or registry source for one parsed dependency table.
fn path_rust_dependency_source(
    manifest_path: &Path,
    package_root: &Path,
    name: &str,
    table: &toml::map::Map<String, toml::Value>,
    features: &[String],
    default_features: bool,
    version: Option<&String>,
) -> Result<DependencySource, OvenRustcError> {
    let source = match table.get("path") {
        Some(path) => {
            if !features.is_empty() {
                return Err(path_rust_dependency_error(format!(
                    "{} path dependency `{name}` explicitly enables Cargo features; prepare an explicit Oven-native closure",
                    manifest_path.display()
                )));
            }
            let path = path.as_str().ok_or_else(|| {
                path_rust_dependency_error(format!(
                    "{} dependency `{name}` has a non-string path",
                    manifest_path.display()
                ))
            })?;
            DependencySource::Path {
                path: package_root.join(path),
            }
        }
        None => {
            if !default_features {
                return Err(path_rust_dependency_error(format!(
                    "{} registry dependency `{name}` disables default features; prepare an explicit Oven-native closure",
                    manifest_path.display()
                )));
            }
            if version.is_none() {
                return Err(path_rust_dependency_error(format!(
                    "{} registry dependency `{name}` has no version requirement",
                    manifest_path.display()
                )));
            }
            DependencySource::Registry
        }
    };
    Ok(source)
}

/// Construct the stable error category used for unsupported path-manifest dependency shapes.
fn path_rust_dependency_error(message: String) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message,
    }
}
/// Permit a path package only when the requested direct-Rustc configuration activates no Cargo feature.
///
/// `default-features = false` is not itself a feature activation: Cargo would compile the dependency without any
/// `feature=...` cfg values. That is exactly the direct-Rustc configuration Oven emits, so accepting it avoids
/// rejecting generated SDK components whose projection disables an empty/default feature set. Explicit features and
/// non-empty default feature groups still need an Oven-native feature closure and remain fail-closed.
pub(super) fn validate_inactive_path_dependency_features(
    manifest_path: &Path,
    manifest: &toml::Value,
    dependency: &DependencySpec,
) -> Result<(), OvenRustcError> {
    if !dependency.features.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "path Rust dependency Cargo.toml",
            message: format!(
                "{} path dependency `{}` explicitly enables Cargo features; prepare an explicit Oven-native closure",
                manifest_path.display(),
                dependency.crate_name
            ),
        });
    }
    if !dependency.default_features {
        return Ok(());
    }
    let Some(features) = manifest.get("features") else {
        return Ok(());
    };
    let features = features.as_table().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!("{} has a non-table [features] declaration", manifest_path.display()),
    })?;
    let Some(default) = features.get("default") else {
        return Ok(());
    };
    let default = default.as_array().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!(
            "{} has a non-array default feature declaration",
            manifest_path.display()
        ),
    })?;
    if default.is_empty() {
        return Ok(());
    }
    Err(OvenRustcError::InvalidInput {
        field: "path Rust dependency Cargo.toml",
        message: format!(
            "{} path dependency `{}` activates default Cargo features; prepare an explicit Oven-native closure",
            manifest_path.display(),
            dependency.crate_name
        ),
    })
}

/// Exact scheduler-selected path artifacts that a caller-owned direct-Rustc closure may reuse.
///
/// This authority is deliberately narrower than a path allowlist. A dependency must both reside below a
/// scheduler-owned immutable root and use a crate name that the selected plan already exposes. The artifact and
/// metadata search paths then come from that plan, rather than from the dependency's Cargo manifest or an ambient
/// Cargo target directory. This permits a caller-owned library to link a compiler-runtime dependency such as
/// `incan_std_core` without re-materializing that runtime crate or interpreting its Cargo feature table.
#[derive(Debug, Clone)]
pub struct OvenSelectedPathRustcAuthority {
    owned_roots: Vec<PathBuf>,
    externs: BTreeMap<String, PathBuf>,
    dependency_search_paths: Vec<PathBuf>,
    /// Exact declared selectors for compiler workspace units; absent for the existing runtime authority.
    declared_dependencies: Option<Vec<DependencySpec>>,
}

#[derive(Default)]
struct PathRustcMaterializationState {
    outputs: BTreeMap<PathBuf, PathBuf>,
    externs: BTreeMap<String, (PathBuf, String)>,
    active: BTreeSet<PathBuf>,
}
impl OvenSelectedPathRustcAuthority {
    /// Construct the authority from a scheduler-selected, already verified direct-Rustc plan.
    #[must_use]
    pub fn new(owned_roots: &[PathBuf], artifact_plan: &OvenRustcArtifactPlan) -> Self {
        let mut owned_roots = owned_roots.to_vec();
        owned_roots.sort();
        owned_roots.dedup();
        let mut dependency_search_paths = artifact_plan.dependency_search_paths.clone();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        Self {
            owned_roots,
            externs: artifact_plan.externs.iter().cloned().collect(),
            dependency_search_paths,
            declared_dependencies: None,
        }
    }

    /// Bind workspace artifacts to exact declared dependency selectors, including disabled defaults.
    ///
    /// The scheduler must compile the feature-unified variant before constructing this authority. Unlike the
    /// runtime-only constructor, this form admits explicit features only when the caller repeats the exact request.
    /// Source paths are canonicalized during resolution, so a relative spelling cannot bypass selector checks.
    #[must_use]
    pub fn with_declared_dependencies(mut self, dependencies: &[DependencySpec]) -> Self {
        self.declared_dependencies = Some(dependencies.to_vec());
        self
    }

    /// Resolve an explicitly selected compiler dependency, refusing a missing path or feature selector by name.
    ///
    /// This strict consumer never falls back to manifest materialization. The parent scheduler owns compilation;
    /// nested explicit bakes may only use the exact artifact it selected.
    pub fn resolve_declared_dependency(&self, dependency: &DependencySpec) -> Result<PathBuf, OvenRustcError> {
        let output = self.resolve(dependency).ok_or_else(|| OvenRustcError::InvalidInput {
            field: "selected workspace Rust dependency",
            message: format!(
                "`{}` has no selected path artifact with its declared features and default-feature policy",
                dependency.crate_name
            ),
        })?;
        verified_regular_file(&output, "selected workspace Rust dependency")
    }

    /// Return the selected artifact only for an exact compiler-runtime dependency under a leased scheduler root.
    pub(super) fn resolve(&self, dependency: &DependencySpec) -> Option<PathBuf> {
        let DependencySource::Path { path } = &dependency.source else {
            return None;
        };
        let path = fs::canonicalize(path).ok()?;
        if !self.owned_roots.iter().any(|root| path.starts_with(root)) {
            return None;
        }
        if let Some(declared) = &self.declared_dependencies {
            let mut features = dependency.features.clone();
            features.sort();
            features.dedup();
            if !declared.iter().any(|selected| {
                let DependencySource::Path { path: selected_path } = &selected.source else {
                    return false;
                };
                let mut selected_features = selected.features.clone();
                selected_features.sort();
                selected_features.dedup();
                selected.crate_name == dependency.crate_name
                    && selected.package == dependency.package
                    && selected.default_features == dependency.default_features
                    && selected.optional == dependency.optional
                    && selected_features == features
                    && fs::canonicalize(selected_path).ok().as_ref() == Some(&path)
            }) {
                return None;
            }
        } else if !dependency.features.is_empty() {
            // A legacy runtime grant carries no feature evidence and cannot authorize an explicit variant.
            return None;
        }
        self.externs.get(&dependency.crate_name.replace('-', "_")).cloned()
    }

    /// Return only the verified dependency directories paired with the selected externs.
    pub(super) fn dependency_search_paths(&self) -> &[PathBuf] {
        &self.dependency_search_paths
    }

    /// Prefer an equivalent sealed registry artifact already present in this selected plan.
    ///
    /// A compatible Loaf catalog may live in the read-only toolchain envelope while the normal command has copied
    /// the same direct-Rustc closure into its actively leased Oven store. Linking the catalog copy as an additional
    /// `--extern` would expose Rustc to two physical copies of one StableCrateId. The caller has already validated
    /// the package, version, features, and digest against the sealed catalog; this method merely reuses the same
    /// metadata-bearing artifact name in one of the selected plan's verified dependency directories. Cargo can emit
    /// byte-distinct rlibs for one portable unit when separate publishers retain staging-sensitive payload details;
    /// the sealed leaf resolver first proves equivalence from the RFC 124 selected-unit identity.
    pub(super) fn matching_sealed_registry_artifact(&self, sealed_artifact: &Path) -> Option<PathBuf> {
        let filename = sealed_artifact.file_name()?;
        let mut matches = self
            .dependency_search_paths
            .iter()
            .filter_map(|directory| {
                let candidate = verified_regular_file(&directory.join(filename), "selected registry artifact").ok()?;
                Some(candidate)
            })
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        matches.into_iter().next()
    }
}

impl PathRustcMaterializationState {
    /// Retain every direct dependency under the name its Rust source uses.
    ///
    /// Rust metadata for an outer rlib names its local path dependencies too, so the final consumer must receive
    /// their explicit `--extern` bindings as well as the top-level import. A name may repeat only when it resolves
    /// to the exact same already-materialized artifact.
    pub(super) fn record_extern(
        &mut self,
        crate_name: String,
        output: PathBuf,
        digest: String,
    ) -> Result<(), OvenRustcError> {
        match self.externs.get(&crate_name) {
            Some((existing_output, existing_digest)) if existing_output == &output && existing_digest == &digest => {
                Ok(())
            }
            Some((existing_output, _)) => Err(OvenRustcError::InvalidInput {
                field: "path Rust dependency",
                message: format!(
                    "resolves Rust crate `{crate_name}` to both {} and {}",
                    existing_output.display(),
                    output.display()
                ),
            }),
            None => {
                self.externs.insert(crate_name, (output, digest));
                Ok(())
            }
        }
    }
}
