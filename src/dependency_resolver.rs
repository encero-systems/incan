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
use incan_core::lang::stdlib::{self, STDLIB_NAMESPACES, StdlibExtraCrateSource};

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

    let (mut manifest_deps, mut manifest_dev_deps) = match manifest {
        Some(manifest) => (manifest.dependencies().clone(), manifest.dev_dependencies().clone()),
        None => (HashMap::new(), HashMap::new()),
    };

    normalize_specs(&mut manifest_deps);
    normalize_specs(&mut manifest_dev_deps);

    // Merge overlapping deps/dev-deps (treat as normal dependency, features unioned).
    if let Err(mut merge_errors) =
        merge_overlapping_dev_dependencies(&mut manifest_deps, &mut manifest_dev_deps, manifest.map(|m| m.path()))
    {
        errors.append(&mut merge_errors);
    }

    let inline_merge = merge_inline_imports(inline_imports, &manifest_deps, &manifest_dev_deps, &mut errors);

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

fn merge_inline_imports(
    inline_imports: &[InlineRustImport],
    manifest_deps: &HashMap<String, DependencySpec>,
    manifest_dev_deps: &HashMap<String, DependencySpec>,
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

        if manifest_deps.contains_key(&import.crate_name) || manifest_dev_deps.contains_key(&import.crate_name) {
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

            if manifest_dev_deps.contains_key(&import.crate_name)
                && !manifest_deps.contains_key(&import.crate_name)
                && !import.is_test_context
            {
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
                        .with_hint("Move the dependency to [dependencies], or import it only from tests."),
                        import,
                    ),
                });
            }

            // Manifest is authoritative; no inline merge needed.
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
            if !merged_spec.spec.features.is_empty() {
                errors.push(DependencyError {
                    file_path: merged_spec.first_site.file_path.clone(),
                    error: with_rust_import_context(
                        CompileError::new(
                            format!("Rust import features for `{}` require a version annotation", crate_name),
                            merged_spec.first_site.span,
                        )
                        .with_hint("Add `@ \"version\"` to the rust import."),
                        &merged_spec.first_site,
                    ),
                });
                continue;
            }

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
            merged_spec.spec = default;
        }

        resolved.insert(crate_name, merged_spec);
    }

    resolved
}

fn inline_spec_from_import(import: &InlineRustImport) -> DependencySpec {
    DependencySpec {
        crate_name: import.crate_name.clone(),
        version: import.version.clone(),
        features: import.features.clone(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
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
                        "dependency `{}` is declared in both [dependencies] and [dev-dependencies] with incompatible specs",
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
        let spec = deps.get(&crate_name).or_else(|| dev_deps.get(&crate_name));
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
    let (version, features): (&str, Vec<&str>) = match crate_name {
        "serde" => ("1.0", vec!["derive"]),
        "serde_json" => ("1.0", vec![]),
        "tokio" => ("1", vec!["rt-multi-thread", "macros", "time", "sync"]),
        "time" => ("0.3", vec!["formatting", "macros"]),
        "chrono" => ("0.4", vec!["serde"]),
        "reqwest" => ("0.11", vec!["json"]),
        "uuid" => ("1.0", vec!["v4", "serde"]),
        "rand" => ("0.8", vec![]),
        "regex" => ("1.0", vec![]),
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
        // For any crate not in the hardcoded list above, fall through to the stdlib registry.
        // STDLIB_NAMESPACES is the single source of truth for stdlib-managed crate versions,
        // so we derive the spec from there rather than duplicating version strings here.
        _ => return known_good_spec_from_stdlib(crate_name),
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
/// This makes `STDLIB_NAMESPACES` the single source of truth for stdlib-managed crate versions.
/// When a stdlib `.incn` file writes `from rust::axum import ...` without an inline version annotation, the resolver
/// finds the version here rather than requiring a duplicate hardcoded entry in `known_good_spec`.
fn known_good_spec_from_stdlib(crate_name: &str) -> Option<DependencySpec> {
    for ns in STDLIB_NAMESPACES {
        for dep in ns.extra_crate_deps {
            if dep.crate_name == crate_name {
                let StdlibExtraCrateSource::Version(version) = dep.source else {
                    // Path dependencies are not registry crates; skip.
                    continue;
                };
                return Some(
                    DependencySpec {
                        crate_name: crate_name.to_string(),
                        version: Some(version.to_string()),
                        features: vec![],
                        default_features: true,
                        source: DependencySource::Registry,
                        optional: false,
                        package: None,
                    }
                    .normalized(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::frontend::ast::Span;
    use crate::lockfile::CargoFeatureSelection;

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

    // ---- Phase 2: Feature union across multiple import sites ----

    #[test]
    fn features_from_multiple_sites_are_unioned() {
        let imports = vec![
            inline("tokio", Some("1.0"), &["rt"], false),
            inline("tokio", Some("1.0"), &["macros"], false),
        ];

        let resolved = resolve_dependencies(None, &imports, false, &default_cargo_features()).unwrap();
        let tokio = resolved.dependencies.iter().find(|d| d.crate_name == "tokio").unwrap();
        assert!(
            tokio.features.contains(&"macros".to_string()),
            "expected 'macros' feature"
        );
        assert!(tokio.features.contains(&"rt".to_string()), "expected 'rt' feature");
    }

    // ---- Phase 2: Version conflict across inline sites ----

    #[test]
    fn version_conflict_across_sites_is_error() {
        let imports = vec![
            inline("tokio", Some("1.0"), &[], false),
            inline("tokio", Some("2.0"), &[], false),
        ];

        let err = resolve_dependencies(None, &imports, false, &default_cargo_features()).unwrap_err();
        assert!(!err.is_empty(), "expected at least one error");
        let msg = &err[0].error.message;
        assert!(msg.contains("conflicting"), "expected conflict error, got: {msg}");
    }

    // ---- Phase 3: Manifest overrides inline (error if both present) ----

    #[test]
    fn manifest_forbids_inline_annotation() {
        let toml_str = r#"
[dependencies]
serde = "1.0"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        let imports = vec![inline("serde", Some("2.0"), &[], false)];

        let err = resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            err[0].error.message.contains("not allowed"),
            "expected 'not allowed' error, got: {}",
            err[0].error.message
        );
    }

    // ---- Phase 3: Manifest crate without inline annotation is accepted ----

    #[test]
    fn manifest_crate_without_inline_is_ok() {
        let toml_str = r#"
[dependencies]
serde = "1.0"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        // Import the crate but no version/features annotation
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()).unwrap();
        let serde = resolved.dependencies.iter().find(|d| d.crate_name == "serde").unwrap();
        assert_eq!(serde.version.as_deref(), Some("1.0"));
    }

    // ---- Phase 3: Dev-dep gating (test context only) ----

    #[test]
    fn dev_dep_in_production_code_is_error() {
        let toml_str = r#"
[dev-dependencies]
test_lib = "0.5"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        // Import from production code (is_test_context = false)
        let imports = vec![inline("test_lib", None, &[], false)];

        let err = resolve_dependencies(Some(&manifest), &imports, true, &default_cargo_features()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            err[0].error.message.contains("dev-only"),
            "expected dev-only error, got: {}",
            err[0].error.message
        );
    }

    #[test]
    fn dev_dep_in_test_context_is_ok() {
        let toml_str = r#"
[dev-dependencies]
test_lib = "0.5"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        // Import from test code (is_test_context = true)
        let imports = vec![inline("test_lib", None, &[], true)];

        let resolved = resolve_dependencies(Some(&manifest), &imports, true, &default_cargo_features()).unwrap();
        let test_lib = resolved
            .dev_dependencies
            .iter()
            .find(|d| d.crate_name == "test_lib")
            .unwrap();
        assert_eq!(test_lib.version.as_deref(), Some("0.5"));
    }

    // ---- Phase 3: Known-good defaults ----

    #[test]
    fn known_good_default_applied_when_no_version() {
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_dependencies(None, &imports, false, &default_cargo_features()).unwrap();
        let serde = resolved.dependencies.iter().find(|d| d.crate_name == "serde").unwrap();
        assert_eq!(serde.version.as_deref(), Some("1.0"));
        assert!(serde.features.contains(&"derive".to_string()));
    }

    #[test]
    fn unknown_crate_without_version_is_error() {
        let imports = vec![inline("unknown_crate_xyz", None, &[], false)];

        let err = resolve_dependencies(None, &imports, false, &default_cargo_features()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            err[0].error.message.contains("unknown Rust crate"),
            "expected unknown crate error, got: {}",
            err[0].error.message
        );
        assert!(
            err[0].error.notes.iter().any(|n| n.contains("import site:")),
            "expected import-site note, got: {:?}",
            err[0].error.notes
        );
        assert!(
            err[0]
                .error
                .hints
                .iter()
                .any(|h| h.contains("Verify the Rust crate/module/item path")),
            "expected path/item verification hint, got: {:?}",
            err[0].error.hints
        );
    }

    #[test]
    fn rust_std_import_does_not_create_dependency() {
        let imports = vec![inline("std", None, &[], false)];

        let resolved = resolve_dependencies(None, &imports, false, &default_cargo_features()).unwrap();
        assert!(
            !resolved.dependencies.iter().any(|d| d.crate_name == "std"),
            "rust::std must not be emitted as Cargo dependency"
        );
        assert!(
            !resolved.dev_dependencies.iter().any(|d| d.crate_name == "std"),
            "rust::std must not be emitted as Cargo dev-dependency"
        );
    }

    // ---- Phase 3: Resolution precedence (incan.toml > inline > known-good) ----

    #[test]
    fn manifest_takes_precedence_over_known_good() {
        let toml_str = r#"
[dependencies]
serde = { version = "2.0", features = ["custom"] }
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        let imports = vec![inline("serde", None, &[], false)];

        let resolved = resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()).unwrap();
        let serde = resolved.dependencies.iter().find(|d| d.crate_name == "serde").unwrap();
        // Should be manifest version, not known-good "1.0"
        assert_eq!(serde.version.as_deref(), Some("2.0"));
        assert!(serde.features.contains(&"custom".to_string()));
    }

    // ---- Phase 4: Strict mode rejects git branch deps ----
    // (This is tested via CLI layer, but we test the resolver's output is correct
    //  for the strict_git_source_error function to catch)

    #[test]
    fn git_branch_dep_appears_in_resolved_for_strict_check() {
        let toml_str = r#"
[dependencies]
my_lib = { git = "https://github.com/example/my_lib.git", branch = "main" }
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        let imports: Vec<InlineRustImport> = vec![];

        let resolved = resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()).unwrap();
        let my_lib = resolved.dependencies.iter().find(|d| d.crate_name == "my_lib").unwrap();
        assert!(
            matches!(my_lib.source, DependencySource::Git { .. }),
            "expected git source"
        );
    }

    // ---- Phase 5: Optional dependency validation ----

    #[test]
    fn optional_dep_without_feature_flag_is_error() {
        let toml_str = r#"
[dependencies.optional]
extra_lib = "1.0"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        let imports = vec![inline("extra_lib", None, &[], false)];

        let err = resolve_dependencies(Some(&manifest), &imports, false, &default_cargo_features()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            err[0].error.message.contains("optional"),
            "expected optional dep error, got: {}",
            err[0].error.message
        );
    }

    #[test]
    fn optional_dep_with_feature_flag_is_ok() {
        let toml_str = r#"
[dependencies.optional]
extra_lib = "1.0"
"#;
        let manifest = ProjectManifest::from_str(toml_str, Path::new(".")).unwrap();
        let imports = vec![inline("extra_lib", None, &[], false)];

        let features = CargoFeatureSelection {
            cargo_features: vec!["extra_lib".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        };

        let resolved = resolve_dependencies(Some(&manifest), &imports, false, &features).unwrap();
        assert!(
            resolved.dependencies.iter().any(|d| d.crate_name == "extra_lib"),
            "expected extra_lib in resolved deps"
        );
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
    fn invalid_inline_version_produces_error() {
        let imports = vec![inline("my_crate", Some("banana"), &[], false)];
        let result = resolve_dependencies(None, &imports, false, &default_cargo_features());
        assert!(result.is_err(), "expected error for invalid version");
        let errors = result.unwrap_err();
        assert!(
            errors[0].error.message.contains("invalid Cargo SemVer requirement"),
            "expected SemVer error, got: {}",
            errors[0].error.message
        );
    }
}
