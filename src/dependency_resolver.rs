//! Rust dependency resolution for `rust::` imports and `incan.toml`.
//!
//! Implements RFC 013 resolution rules:
//! - `incan.toml` dependencies override inline annotations
//! - inline versions/features are merged across sites
//! - known-good defaults apply only when no explicit config exists
//! - dev-dependencies are restricted to test contexts

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use semver::VersionReq;

use crate::frontend::ast::Span;
use crate::frontend::diagnostics::CompileError;
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use incan_core::lang::stdlib::{self, StdlibExtraCrateSource};

/// Validate that a version requirement string uses Cargo SemVer syntax.
///
/// Returns `Ok(())` if valid, or an error message describing the problem.
/// This catches PEP 440 specifiers, typos, and other invalid strings early (RFC 013, Phase 1.2).
pub(crate) fn validate_cargo_version_req(version: &str) -> Result<(), String> {
    match VersionReq::parse(version) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "invalid Cargo SemVer requirement `{version}`: {e}. \
             Use Cargo syntax (e.g. \"1.0\", \"^1.2\", \"~0.5\", \">=1.0, <2.0\", \"=1.2.3\")"
        )),
    }
}

#[derive(Debug, Clone)]
pub struct InlineRustImport {
    pub crate_name: String,
    pub import_path: String,
    pub version: Option<String>,
    pub features: Vec<String>,
    pub span: Span,
    pub file_path: PathBuf,
    pub is_test_context: bool,
}

#[derive(Debug, Clone)]
pub struct DependencyError {
    pub file_path: PathBuf,
    pub error: CompileError,
}

#[derive(Debug, Clone)]
pub struct ResolvedDependencies {
    pub dependencies: Vec<DependencySpec>,
    pub dev_dependencies: Vec<DependencySpec>,
}

fn with_rust_import_context(error: CompileError, import: &InlineRustImport) -> CompileError {
    error
        .with_note(format!("import site: `{}`", import.import_path))
        .with_hint("Verify the Rust crate/module/item path in the import statement")
}

pub fn resolve_dependencies(
    manifest: Option<&ProjectManifest>,
    inline_imports: &[InlineRustImport],
    include_dev_dependencies: bool,
    cargo_features: &CargoFeatureSelection,
) -> Result<ResolvedDependencies, Vec<DependencyError>> {
    let mut errors = Vec::new();

    let (mut manifest_deps, mut manifest_dev_deps, library_dep_names) = match manifest {
        Some(manifest) => (
            manifest.rust_dependencies().clone(),
            manifest.rust_dev_dependencies().clone(),
            manifest
                .library_dependencies()
                .keys()
                .cloned()
                .collect::<HashSet<String>>(),
        ),
        None => (HashMap::new(), HashMap::new(), HashSet::new()),
    };

    normalize_specs(&mut manifest_deps);
    normalize_specs(&mut manifest_dev_deps);

    // Merge overlapping deps/dev-deps (treat as normal dependency, features unioned).
    if let Err(mut merge_errors) =
        merge_overlapping_dev_dependencies(&mut manifest_deps, &mut manifest_dev_deps, manifest.map(|m| m.path()))
    {
        errors.append(&mut merge_errors);
    }

    let inline_merge = merge_inline_imports(
        inline_imports,
        &manifest_deps,
        &manifest_dev_deps,
        &library_dep_names,
        &mut errors,
    );

    // Combine manifest deps with resolved inline specs.
    let mut resolved_deps: HashMap<String, DependencySpec> = manifest_deps.clone();
    let mut resolved_dev_deps: HashMap<String, DependencySpec> = if include_dev_dependencies {
        manifest_dev_deps.clone()
    } else {
        HashMap::new()
    };

    for (crate_name, inline) in inline_merge {
        if inline.is_test_only {
            if include_dev_dependencies {
                resolved_dev_deps.insert(crate_name, inline.spec);
            }
        } else {
            resolved_deps.insert(crate_name, inline.spec);
        }
    }

    if errors.is_empty() {
        validate_optional_imports(
            inline_imports,
            &resolved_deps,
            &resolved_dev_deps,
            cargo_features,
            &mut errors,
        );
    }

    if errors.is_empty() {
        Ok(ResolvedDependencies {
            dependencies: resolved_deps.into_values().collect(),
            dev_dependencies: resolved_dev_deps.into_values().collect(),
        })
    } else {
        Err(errors)
    }
}

// ============================================================================
// Inline merge + validation
// ============================================================================

struct InlineMergedSpec {
    spec: DependencySpec,
    is_test_only: bool,
    first_site: InlineRustImport,
}

fn matching_dep_spec<'a>(
    deps: &'a HashMap<String, DependencySpec>,
    crate_name: &str,
) -> Option<(&'a String, &'a DependencySpec)> {
    deps.get_key_value(crate_name)
        .or_else(|| deps.get_key_value(&crate_name.replace('_', "-")))
        .or_else(|| deps.get_key_value(&crate_name.replace('-', "_")))
}

fn merge_inline_imports(
    inline_imports: &[InlineRustImport],
    manifest_deps: &HashMap<String, DependencySpec>,
    manifest_dev_deps: &HashMap<String, DependencySpec>,
    library_dep_names: &HashSet<String>,
    errors: &mut Vec<DependencyError>,
) -> HashMap<String, InlineMergedSpec> {
    let mut merged: HashMap<String, InlineMergedSpec> = HashMap::new();

    for import in inline_imports {
        if import.crate_name == stdlib::STDLIB_ROOT {
            continue;
        }

        let has_inline_spec = import.version.is_some() || !import.features.is_empty();
        if let Some(version) = &import.version {
            if version.trim().is_empty() {
                errors.push(DependencyError {
                    file_path: import.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(
                            format!(
                                "Rust import for `{}` has an empty version requirement",
                                import.crate_name
                            ),
                            import.span,
                        )
                        .with_hint("Use a non-empty Cargo SemVer requirement string."),
                        import,
                    ),
                });
                continue;
            }

            if let Err(msg) = validate_cargo_version_req(version) {
                errors.push(DependencyError {
                    file_path: import.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(format!("Rust import for `{}`: {msg}", import.crate_name), import.span),
                        import,
                    ),
                });
                continue;
            }
        }

        let manifest_dep_match = matching_dep_spec(manifest_deps, &import.crate_name);
        let manifest_dev_dep_match = matching_dep_spec(manifest_dev_deps, &import.crate_name);

        if library_dep_names.contains(&import.crate_name)
            && manifest_dep_match.is_none()
            && manifest_dev_dep_match.is_none()
        {
            errors.push(DependencyError {
                file_path: import.file_path.clone(),
                error: with_rust_import_context(
                    CompileError::new(
                        format!(
                            "Rust crate `{}` is declared under `[dependencies]`, which is reserved for Incan library dependencies",
                            import.crate_name
                        ),
                        import.span,
                    )
                    .with_hint(format!(
                        "Move `{}` to `[rust-dependencies]` in incan.toml for `rust::` imports.",
                        import.crate_name
                    )),
                    import,
                ),
            });
            continue;
        }

        if manifest_dep_match.is_some() || manifest_dev_dep_match.is_some() {
            if has_inline_spec {
                errors.push(DependencyError {
                    file_path: import.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(
                            format!(
                                "inline Rust dependency annotation for `{}` is not allowed because it is configured in incan.toml",
                                import.crate_name
                            ),
                            import.span,
                        )
                        .with_hint("Remove the inline annotation or update incan.toml."),
                        import,
                    ),
                });
            }

            if manifest_dev_dep_match.is_some() && manifest_dep_match.is_none() && !import.is_test_context {
                errors.push(DependencyError {
                    file_path: import.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(
                            format!(
                                "Rust crate `{}` is dev-only and cannot be imported from production code",
                                import.crate_name
                            ),
                            import.span,
                        )
                        .with_hint("Move the dependency to [rust-dependencies], or import it only from tests."),
                        import,
                    ),
                });
            }

            // Manifest is authoritative; no inline merge needed.
            continue;
        }

        // `incan_stdlib` is always injected as a workspace path dependency for generated projects.
        // Version-less stdlib-internal `from rust::incan_stdlib::... import ...` leaves should not be forced to add
        // inline annotations or duplicate manifest entries.
        if import.crate_name == "incan_stdlib" {
            continue;
        }

        let entry = merged
            .entry(import.crate_name.clone())
            .or_insert_with(|| InlineMergedSpec {
                spec: inline_spec_from_import(import),
                is_test_only: import.is_test_context,
                first_site: import.clone(),
            });

        if let Err(conflict) = merge_inline_spec(entry, import) {
            errors.push(DependencyError {
                file_path: import.file_path.clone(),
                error: CompileError::new(conflict, import.span)
                    .with_note(format!(
                        "first declaration was in {}",
                        entry.first_site.file_path.display()
                    ))
                    .with_note(format!("first import site: `{}`", entry.first_site.import_path))
                    .with_note(format!("conflicting import site: `{}`", import.import_path)),
            });
            continue;
        }

        if !import.is_test_context {
            entry.is_test_only = false;
        }
    }

    // Fill in known-good defaults for version-less specs.
    let mut resolved = HashMap::new();
    for (crate_name, mut merged_spec) in merged {
        if merged_spec.spec.version.is_none() {
            let Some(default) = known_good_spec(&crate_name) else {
                errors.push(DependencyError {
                    file_path: merged_spec.first_site.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(
                            format!("unknown Rust crate `{}`: no version specified", crate_name),
                            merged_spec.first_site.span,
                        )
                        .with_hint(format!(
                            "Add a version annotation: `import rust::{crate_name} @ \"1.0\"` or add it to incan.toml."
                        )),
                        &merged_spec.first_site,
                    ),
                });
                continue;
            };
            let requested_features = std::mem::take(&mut merged_spec.spec.features);
            merged_spec.spec = default;
            for feature in requested_features {
                if !merged_spec.spec.features.contains(&feature) {
                    merged_spec.spec.features.push(feature);
                }
            }
            merged_spec.spec = merged_spec.spec.normalized();
        }

        resolved.insert(crate_name, merged_spec);
    }

    resolved
}

/// Convert one inline `rust::` import annotation into the dependency spec emitted to generated Cargo manifests.
fn inline_spec_from_import(import: &InlineRustImport) -> DependencySpec {
    DependencySpec {
        crate_name: import.crate_name.clone(),
        version: import.version.clone(),
        features: import.features.clone(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: stdlib::extra_crate_package_alias(&import.crate_name).map(str::to_string),
    }
    .normalized()
}

fn merge_inline_spec(existing: &mut InlineMergedSpec, next: &InlineRustImport) -> Result<(), String> {
    let next_version = next.version.clone();
    if existing.spec.version != next_version {
        return Err(format!(
            "conflicting inline dependency specifications for `{}`",
            existing.spec.crate_name
        ));
    }

    for feature in &next.features {
        if !existing.spec.features.contains(feature) {
            existing.spec.features.push(feature.clone());
        }
    }
    existing.spec = existing.spec.clone().normalized();

    Ok(())
}

fn merge_overlapping_dev_dependencies(
    deps: &mut HashMap<String, DependencySpec>,
    dev_deps: &mut HashMap<String, DependencySpec>,
    manifest_path: Option<&Path>,
) -> Result<(), Vec<DependencyError>> {
    let mut errors = Vec::new();
    let overlap: Vec<String> = deps
        .keys()
        .filter(|name| dev_deps.contains_key(*name))
        .cloned()
        .collect();

    for name in overlap {
        let dep = deps.get(&name).cloned();
        let dev = dev_deps.get(&name).cloned();
        let (Some(dep), Some(dev)) = (dep, dev) else {
            continue;
        };

        if dep.version != dev.version
            || dep.source != dev.source
            || dep.default_features != dev.default_features
            || dep.optional != dev.optional
            || dep.package != dev.package
        {
            let file_path = manifest_path
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("incan.toml"));
            errors.push(DependencyError {
                file_path,
                error: CompileError::new(
                    format!(
                        "dependency `{}` is declared in both [rust-dependencies] and [rust-dev-dependencies] with incompatible specs",
                        name
                    ),
                    Span::default(),
                )
                .with_hint("Make the entries compatible or keep only one."),
            });
            continue;
        }

        let mut merged = dep.clone();
        merged.features.extend(dev.features);
        merged = merged.normalized();
        deps.insert(name.clone(), merged);
        dev_deps.remove(&name);
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn normalize_specs(specs: &mut HashMap<String, DependencySpec>) {
    for spec in specs.values_mut() {
        let normalized = spec.clone().normalized();
        *spec = normalized;
    }
}

fn validate_optional_imports(
    inline_imports: &[InlineRustImport],
    deps: &HashMap<String, DependencySpec>,
    dev_deps: &HashMap<String, DependencySpec>,
    cargo_features: &CargoFeatureSelection,
    errors: &mut Vec<DependencyError>,
) {
    let mut first_sites: HashMap<String, &InlineRustImport> = HashMap::new();
    for import in inline_imports {
        first_sites.entry(import.crate_name.clone()).or_insert(import);
    }

    let enabled_features: HashSet<&str> = cargo_features.cargo_features.iter().map(|f| f.as_str()).collect();
    let all_features = cargo_features.cargo_all_features;

    for (crate_name, import) in first_sites {
        let spec = matching_dep_spec(deps, &crate_name)
            .map(|(_, spec)| spec)
            .or_else(|| matching_dep_spec(dev_deps, &crate_name).map(|(_, spec)| spec));
        let Some(spec) = spec else {
            continue;
        };
        if !spec.optional {
            continue;
        }
        let enabled = all_features || enabled_features.contains(crate_name.as_str());
        if enabled {
            continue;
        }

        errors.push(DependencyError {
            file_path: import.file_path.clone(),
            error: CompileError::new(
                format!("Rust crate `{}` is optional but not enabled for this build", crate_name),
                import.span,
            )
            .with_note(format!(
                "The dependency `{}` is declared optional in incan.toml.",
                crate_name
            ))
            .with_hint(format!(
                "Enable it via `--cargo-features {}` when building/testing.",
                crate_name
            )),
        });
    }
}

// ============================================================================
// Known-good defaults (RFC 013)
// ============================================================================

fn known_good_spec(crate_name: &str) -> Option<DependencySpec> {
    if let Some(spec) = known_good_spec_from_stdlib(crate_name) {
        return Some(spec);
    }

    let (version, features): (&str, Vec<&str>) = match crate_name {
        "serde" => ("1.0", vec!["derive"]),
        "serde_json" => ("1.0", vec![]),
        "tokio" => ("1", vec!["rt-multi-thread", "macros", "time", "sync"]),
        "time" => ("0.3", vec!["formatting", "macros"]),
        "chrono" => ("0.4", vec!["serde"]),
        "reqwest" => ("0.11", vec!["json"]),
        "uuid" => ("1.0", vec!["v4", "serde"]),
        "anyhow" => ("1.0", vec![]),
        "thiserror" => ("1.0", vec![]),
        "tracing" => ("0.1", vec![]),
        "clap" => ("4.0", vec!["derive"]),
        "log" => ("0.4", vec![]),
        "env_logger" => ("0.10", vec![]),
        "sqlx" => ("0.7", vec!["runtime-tokio-native-tls", "postgres"]),
        "futures" => ("0.3", vec![]),
        "bytes" => ("1.0", vec![]),
        "itertools" => ("0.12", vec![]),
        _ => return None,
    };

    Some(
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some(version.to_string()),
            features: features.iter().map(|f| f.to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
        .normalized(),
    )
}

/// Look up a known-good spec for crates declared as `extra_crate_deps` in any stdlib namespace.
///
/// This makes the stdlib registry the single source of truth for stdlib-managed crate versions. When a stdlib `.incn`
/// file writes `from rust::axum import ...` without an inline version annotation, the resolver finds the version here
/// rather than requiring a duplicate hardcoded entry in `known_good_spec`.
fn known_good_spec_from_stdlib(crate_name: &str) -> Option<DependencySpec> {
    let dep = stdlib::extra_crate_deps()
        .find(|dep| dep.crate_name == crate_name && matches!(dep.source, StdlibExtraCrateSource::Version(_)))?;
    let StdlibExtraCrateSource::Version(version) = dep.source else {
        return None;
    };
    Some(
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some(version.to_string()),
            features: dep.features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: stdlib::extra_crate_package_alias(crate_name).map(str::to_string),
        }
        .normalized(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::Span;
    use crate::lockfile::CargoFeatureSelection;
    use std::error::Error;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn dummy_span() -> Span {
        Span::default()
    }

    fn inline(crate_name: &str, version: Option<&str>, features: &[&str], test: bool) -> InlineRustImport {
        InlineRustImport {
            crate_name: crate_name.to_string(),
            import_path: format!("rust::{}", crate_name),
            version: version.map(|v| v.to_string()),
            features: features.iter().map(|f| f.to_string()).collect(),
            span: dummy_span(),
            file_path: PathBuf::from("test.incn"),
            is_test_context: test,
        }
    }

    fn default_cargo_features() -> CargoFeatureSelection {
        CargoFeatureSelection::default()
    }

    fn parse_manifest(toml_str: &str) -> TestResult<ProjectManifest> {
        ProjectManifest::from_str(toml_str, Path::new(".")).map_err(|err| Box::new(err) as Box<dyn Error>)
    }

    fn resolve_ok(
        manifest: Option<&ProjectManifest>,
        inline_imports: &[InlineRustImport],
        include_dev_dependencies: bool,
        cargo_features: &CargoFeatureSelection,
    ) -> TestResult<ResolvedDependencies> {
        resolve_dependencies(manifest, inline_imports, include_dev_dependencies, cargo_features)
            .map_err(|errors| std::io::Error::other(format!("{errors:?}")).into())
    }

    fn dependency<'a>(deps: &'a [DependencySpec], crate_name: &str) -> TestResult<&'a DependencySpec> {
        deps.iter()
            .find(|dep| dep.crate_name == crate_name)
            .ok_or_else(|| std::io::Error::other(format!("expected dependency `{crate_name}` to be present")).into())
    }

    fn first_error(errors: &[DependencyError]) -> TestResult<&DependencyError> {
        errors
            .first()
            .ok_or_else(|| std::io::Error::other("expected at least one dependency error").into())
    }

    // ---- Phase 2: Feature union across multiple import sites ----

    #[test]
    fn features_from_multiple_sites_are_unioned() -> TestResult {
        let imports = vec![
            inline("tokio", Some("1.0"), &["rt"], false),
            inline("tokio", Some("1.0"), &["macros"], false),
        ];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        let tokio = dependency(&resolved.dependencies, "tokio")?;
        assert!(
            tokio.features.contains(&"macros".to_string()),
            "expected 'macros' feature"
        );
        assert!(tokio.features.contains(&"rt".to_string()), "expected 'rt' feature");
        Ok(())
    }

    #[test]
    fn inline_rust_import_can_resolve_known_package_renames() -> TestResult {
        let imports = vec![
            inline("md5", Some("0.10"), &[], false),
            inline("xxhash_rust", Some("0.8"), &["xxh3"], false),
        ];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        let md5 = dependency(&resolved.dependencies, "md5")?;
        let xxhash = dependency(&resolved.dependencies, "xxhash_rust")?;

        assert_eq!(md5.package.as_deref(), Some("md-5"));
        assert_eq!(xxhash.package.as_deref(), Some("xxhash-rust"));
        Ok(())
    }

    // ---- Phase 2: Version conflict across inline sites ----

    #[test]
    fn version_conflict_across_sites_is_error() -> TestResult {
        let imports = vec![
            inline("tokio", Some("1.0"), &[], false),
            inline("tokio", Some("2.0"), &[], false),
        ];

        let err = match resolve_dependencies(None, &imports, false, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected version conflict, got successful resolution: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty(), "expected at least one error");
        let err = first_error(&err)?;
        let msg = &err.error.message;
        assert!(msg.contains("conflicting"), "expected conflict error, got: {msg}");
        Ok(())
    }

    // ---- Phase 3: Manifest overrides inline (error if both present) ----

    #[test]
    fn manifest_forbids_inline_annotation() -> TestResult {
        let toml_str = r#"
[rust-dependencies]
serde = "1.0"
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports = vec![inline("serde", Some("2.0"), &[], false)];

        let err = match resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected inline annotation to be rejected, got: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty());
        let err = first_error(&err)?;
        assert!(
            err.error.message.contains("not allowed"),
            "expected 'not allowed' error, got: {}",
            err.error.message
        );
        Ok(())
    }

    // ---- Phase 3: Manifest crate without inline annotation is accepted ----

    #[test]
    fn manifest_crate_without_inline_is_ok() -> TestResult {
        let toml_str = r#"
[rust-dependencies]
serde = "1.0"
"#;
        let manifest = parse_manifest(toml_str)?;
        // Import the crate but no version/features annotation
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_ok(Some(&manifest), &imports, false, &default_cargo_features())?;
        let serde = dependency(&resolved.dependencies, "serde")?;
        assert_eq!(serde.version.as_deref(), Some("1.0"));
        Ok(())
    }

    // ---- Phase 3: Dev-dep gating (test context only) ----

    #[test]
    fn dev_dep_in_production_code_is_error() -> TestResult {
        let toml_str = r#"
[rust-dev-dependencies]
test_lib = "0.5"
"#;
        let manifest = parse_manifest(toml_str)?;
        // Import from production code (is_test_context = false)
        let imports = vec![inline("test_lib", None, &[], false)];

        let err = match resolve_dependencies(Some(&manifest), &imports, true, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected dev-only dependency to be rejected, got: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty());
        let err = first_error(&err)?;
        assert!(
            err.error.message.contains("dev-only"),
            "expected dev-only error, got: {}",
            err.error.message
        );
        Ok(())
    }

    #[test]
    fn dev_dep_in_test_context_is_ok() -> TestResult {
        let toml_str = r#"
[rust-dev-dependencies]
test_lib = "0.5"
"#;
        let manifest = parse_manifest(toml_str)?;
        // Import from test code (is_test_context = true)
        let imports = vec![inline("test_lib", None, &[], true)];

        let resolved = resolve_ok(Some(&manifest), &imports, true, &default_cargo_features())?;
        let test_lib = dependency(&resolved.dev_dependencies, "test_lib")?;
        assert_eq!(test_lib.version.as_deref(), Some("0.5"));
        Ok(())
    }

    // ---- Phase 3: Known-good defaults ----

    #[test]
    fn known_good_default_applied_when_no_version() -> TestResult {
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        let serde = dependency(&resolved.dependencies, "serde")?;
        assert_eq!(serde.version.as_deref(), Some("1.0"));
        assert!(serde.features.contains(&"derive".to_string()));
        Ok(())
    }

    #[test]
    fn known_good_default_allows_features_without_inline_version() -> TestResult {
        let imports = vec![inline("tokio", None, &["full"], false)];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        let tokio = dependency(&resolved.dependencies, "tokio")?;
        assert_eq!(tokio.version.as_deref(), Some("1"));
        assert!(tokio.features.contains(&"rt-multi-thread".to_string()));
        assert!(tokio.features.contains(&"full".to_string()));
        Ok(())
    }

    #[test]
    fn stdlib_registry_version_dependencies_drive_known_good_defaults() -> TestResult {
        for ns in stdlib::STDLIB_NAMESPACES {
            for dep in ns.extra_crate_deps {
                let StdlibExtraCrateSource::Version(version) = dep.source else {
                    continue;
                };
                let spec = known_good_spec(dep.crate_name).ok_or_else(|| {
                    std::io::Error::other(format!(
                        "expected registry dependency `{}` to resolve as a known-good default",
                        dep.crate_name
                    ))
                })?;
                assert_eq!(
                    spec.version.as_deref(),
                    Some(version),
                    "dependency resolver drifted from stdlib registry metadata for `{}`",
                    dep.crate_name
                );
                assert_eq!(
                    spec.features,
                    dep.features
                        .iter()
                        .map(|feature| (*feature).to_string())
                        .collect::<Vec<_>>(),
                    "dependency resolver drifted from stdlib registry feature metadata for `{}`",
                    dep.crate_name
                );
            }
        }
        Ok(())
    }

    #[test]
    fn unknown_crate_without_version_is_error() -> TestResult {
        let imports = vec![inline("unknown_crate_xyz", None, &[], false)];

        let err = match resolve_dependencies(None, &imports, false, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected unknown crate error, got successful resolution: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty());
        let err = first_error(&err)?;
        assert!(
            err.error.message.contains("unknown Rust crate"),
            "expected unknown crate error, got: {}",
            err.error.message
        );
        assert!(
            err.error.notes.iter().any(|n| n.contains("import site:")),
            "expected import-site note, got: {:?}",
            err.error.notes
        );
        assert!(
            err.error
                .hints
                .iter()
                .any(|h| h.contains("Verify the Rust crate/module/item path")),
            "expected path/item verification hint, got: {:?}",
            err.error.hints
        );
        Ok(())
    }

    #[test]
    fn rust_std_import_does_not_create_dependency() -> TestResult {
        let imports = vec![inline("std", None, &[], false)];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        assert!(
            !resolved.dependencies.iter().any(|d| d.crate_name == "std"),
            "rust::std must not be emitted as Cargo dependency"
        );
        assert!(
            !resolved.dev_dependencies.iter().any(|d| d.crate_name == "std"),
            "rust::std must not be emitted as Cargo dev-dependency"
        );
        Ok(())
    }

    #[test]
    fn incan_stdlib_import_does_not_require_inline_version() -> TestResult {
        let imports = vec![inline("incan_stdlib", None, &[], false)];

        let resolved = resolve_ok(None, &imports, false, &default_cargo_features())?;
        assert!(
            !resolved.dependencies.iter().any(|d| d.crate_name == "incan_stdlib"),
            "incan_stdlib should already be provided by generated projects"
        );
        assert!(
            !resolved.dev_dependencies.iter().any(|d| d.crate_name == "incan_stdlib"),
            "incan_stdlib should not be duplicated in dev-dependencies"
        );
        Ok(())
    }

    // ---- Phase 3: Resolution precedence (incan.toml > inline > known-good) ----

    #[test]
    fn manifest_takes_precedence_over_known_good() -> TestResult {
        let toml_str = r#"
[rust-dependencies]
serde = { version = "2.0", features = ["custom"] }
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_ok(Some(&manifest), &imports, false, &default_cargo_features())?;
        let serde = dependency(&resolved.dependencies, "serde")?;
        // Should be manifest version, not known-good "1.0"
        assert_eq!(serde.version.as_deref(), Some("2.0"));
        assert!(serde.features.contains(&"custom".to_string()));
        Ok(())
    }

    // ---- Phase 4: Strict mode rejects git branch deps ----
    // (This is tested via CLI layer, but we test the resolver's output is correct
    //  for the strict_git_source_error function to catch)

    #[test]
    fn git_branch_dep_appears_in_resolved_for_strict_check() -> TestResult {
        let toml_str = r#"
[rust-dependencies]
my_lib = { git = "https://github.com/example/my_lib.git", branch = "main" }
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports: Vec<InlineRustImport> = vec![];

        let resolved = resolve_ok(Some(&manifest), &imports, false, &default_cargo_features())?;
        let my_lib = dependency(&resolved.dependencies, "my_lib")?;
        assert!(
            matches!(my_lib.source, DependencySource::Git { .. }),
            "expected git source"
        );
        Ok(())
    }

    // ---- Phase 5: Optional dependency validation ----

    #[test]
    fn optional_dep_without_feature_flag_is_error() -> TestResult {
        let toml_str = r#"
[rust-dependencies.optional]
extra_lib = "1.0"
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports = vec![inline("extra_lib", None, &[], false)];

        let err = match resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected optional dependency to be rejected, got: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty());
        let err = first_error(&err)?;
        assert!(
            err.error.message.contains("optional"),
            "expected optional dep error, got: {}",
            err.error.message
        );
        Ok(())
    }

    #[test]
    fn optional_dep_with_feature_flag_is_ok() -> TestResult {
        let toml_str = r#"
[rust-dependencies.optional]
extra_lib = "1.0"
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports = vec![inline("extra_lib", None, &[], false)];

        let features = CargoFeatureSelection {
            cargo_features: vec!["extra_lib".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        };

        let resolved = resolve_ok(Some(&manifest), &imports, false, &features)?;
        assert!(
            resolved.dependencies.iter().any(|d| d.crate_name == "extra_lib"),
            "expected extra_lib in resolved deps"
        );
        Ok(())
    }

    #[test]
    fn rust_import_declared_in_library_dependencies_emits_migration_error() -> TestResult {
        let toml_str = r#"
[dependencies]
legacy_rust = { path = "../legacy_rust" }
"#;
        let manifest = parse_manifest(toml_str)?;
        let imports = vec![inline("legacy_rust", None, &[], false)];

        let err = match resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()) {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected migration diagnostic, got successful resolution: {resolved:?}"
                ))
                .into());
            }
            Err(err) => err,
        };
        assert!(!err.is_empty());
        let err = first_error(&err)?;
        assert!(
            err.error.message.contains("reserved for Incan library dependencies"),
            "expected migration diagnostic, got: {}",
            err.error.message
        );
        assert!(
            err.error.hints.iter().any(|h| h.contains("[rust-dependencies]")),
            "expected rust-dependencies migration hint, got: {:?}",
            err.error.hints
        );
        Ok(())
    }

    // ---- SemVer validation (RFC 013, Phase 1.2) ----

    #[test]
    fn validate_cargo_version_req_accepts_valid_specs() {
        assert!(validate_cargo_version_req("1.0").is_ok());
        assert!(validate_cargo_version_req("^1.2").is_ok());
        assert!(validate_cargo_version_req("~0.5").is_ok());
        assert!(validate_cargo_version_req(">=1.0, <2.0").is_ok());
        assert!(validate_cargo_version_req("=1.2.3").is_ok());
        assert!(validate_cargo_version_req("1.0.195").is_ok());
    }

    #[test]
    fn validate_cargo_version_req_rejects_invalid_specs() {
        assert!(validate_cargo_version_req("banana").is_err());
        assert!(validate_cargo_version_req("~=1.2").is_err()); // PEP 440
        assert!(validate_cargo_version_req("==1.2.*").is_err()); // PEP 440
        assert!(validate_cargo_version_req("!=1.3").is_err()); // PEP 440
    }

    #[test]
    fn invalid_inline_version_produces_error() -> TestResult {
        let imports = vec![inline("my_crate", Some("banana"), &[], false)];
        let result = resolve_dependencies(None, &imports, false, &default_cargo_features());
        let errors = match result {
            Ok(resolved) => {
                return Err(std::io::Error::other(format!(
                    "expected error for invalid version, got successful resolution: {resolved:?}"
                ))
                .into());
            }
            Err(errors) => errors,
        };
        let errors = first_error(&errors)?;
        assert!(
            errors.error.message.contains("invalid Cargo SemVer requirement"),
            "expected SemVer error, got: {}",
            errors.error.message
        );
        Ok(())
    }
}
