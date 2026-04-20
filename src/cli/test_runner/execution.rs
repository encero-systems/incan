use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::commands;
use crate::cli::commands::common;
use crate::cli::commands::common::ProjectRequirements;
#[cfg(feature = "rust_inspect")]
use crate::cli::commands::common::{
    collect_rust_inspect_query_paths, ensure_rust_inspect_workspace, prewarm_rust_inspect_workspace,
};
use crate::cli::prelude::ParsedModule;
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::ast::Program;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::vocab_desugar_pass;
use crate::frontend::{lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use sha2::{Digest, Sha256};

use super::module_graph::collect_source_modules_for_test;
use super::types::{TestInfo, TestResult};

/// Generated `#[cfg(test)]` module that wraps Incan test functions as Rust `#[test]` cases, one `cargo test` per file.
const INCAN_FILE_TEST_MOD: &str = "__incan_file_tests";

fn parse_isolated_target_env(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Shared Cargo `target/` directory for generated test crates in a package.
///
/// By default this reuses the project's main `target/` so existing dependency artifacts are shared across regular
/// builds and `incan test` runs for better DX.
///
/// Set `INCAN_TEST_ISOLATED_TARGET_DIR` to one of `1|true|yes|on` to use `target/incan_test_runner` instead.
fn shared_cargo_target_dir(project_root: &Path) -> PathBuf {
    let absolute_project_root = if project_root.is_absolute() {
        project_root.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(project_root)
    } else {
        project_root.to_path_buf()
    };

    if parse_isolated_target_env(std::env::var("INCAN_TEST_ISOLATED_TARGET_DIR").ok().as_deref()) {
        absolute_project_root.join("target").join("incan_test_runner")
    } else {
        absolute_project_root.join("target")
    }
}

/// Shared front-end + dependency work for one test file, reused across parametrized variants and multiple tests in the
/// same `.incn` file within a single `incan test` session.
pub(super) struct PreparedTestFile {
    pub library_manifest_index: LibraryManifestIndex,
    pub ast: Program,
    pub source_modules: Vec<ParsedModule>,
    pub project_root: PathBuf,
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
    pub lock_payload: Option<String>,
    #[cfg(feature = "rust_inspect")]
    pub rust_inspect_manifest_dir: PathBuf,
}

fn canonical_path_for_cache_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn absolute_project_root(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

/// Infer a package root for manifest-less test runs.
///
/// Prefer conventional package anchors like `tests/` or `src/` so a file such as
/// `/repo/tests/test_cwd.incn` resolves its runtime cwd to `/repo`, not `/repo/tests`.
/// If no conventional anchor is present, fall back to the caller cwd when the test
/// file lives underneath it; otherwise use the test file's parent directory.
fn infer_project_root_without_manifest(test_path: &Path) -> PathBuf {
    let absolute_test_path = if test_path.is_absolute() {
        test_path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(test_path)
    } else {
        test_path.to_path_buf()
    };
    let absolute_test_path = fs::canonicalize(&absolute_test_path).unwrap_or(absolute_test_path);

    for ancestor in absolute_test_path.ancestors().skip(1) {
        if ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "tests" | "src"))
            && let Some(parent) = ancestor.parent()
        {
            return parent.to_path_buf();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
        if absolute_test_path.starts_with(&cwd) {
            return cwd;
        }
    }

    absolute_test_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn compute_test_prep_cache_key(
    test_path: &Path,
    source: &str,
    source_modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    cargo: &CargoFeatureSelection,
    locked: bool,
    frozen: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_test_prep/1\0");
    hasher.update(canonical_path_for_cache_key(test_path).to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(source.as_bytes());
    hasher.update(b"\0");

    let mut sorted_mods: Vec<&ParsedModule> = source_modules.iter().collect();
    sorted_mods.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    for m in sorted_mods {
        hasher.update(canonical_path_for_cache_key(&m.file_path).to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(m.source.as_bytes());
        hasher.update(b"\0|\0");
    }

    match manifest {
        Some(m) => {
            hasher.update(b"manifest\0");
            hasher.update(m.path().to_string_lossy().as_bytes());
            hasher.update(b"\0");
            // Distinguish read errors from an empty manifest file (both would otherwise contribute no bytes).
            match fs::read_to_string(m.path()) {
                Ok(body) => {
                    hasher.update(b"ok\0");
                    hasher.update(body.as_bytes());
                }
                Err(_) => hasher.update(b"err\0"),
            }
            hasher.update(b"\0");
        }
        None => hasher.update(b"nomanifest\0"),
    }

    for f in &cargo.cargo_features {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([cargo.cargo_no_default_features as u8]);
    hasher.update([cargo.cargo_all_features as u8]);
    hasher.update([locked as u8]);
    hasher.update([frozen as u8]);

    format!("v1:{}", hex::encode(hasher.finalize()))
}

/// Merge stdlib feature flags from previously prepared files with the current file requirements.
///
/// The rust-inspect workspace lives under one shared `target/incan_lock` directory per package. If files in a single
/// `incan test` session require different stdlib features, a non-monotonic feature set can cause workspace
/// fingerprint churn and expensive mid-run rewrites. Keeping a session-local feature union avoids that churn.
fn merge_rust_inspect_stdlib_features<'a>(
    existing_feature_sets: impl Iterator<Item = &'a [String]>,
    current_features: &[String],
) -> Vec<String> {
    let mut merged: BTreeSet<String> = current_features.iter().cloned().collect();
    for features in existing_feature_sets {
        merged.extend(features.iter().cloned());
    }
    merged.into_iter().collect()
}

/// Promote project dev dependencies into ordinary dependencies for generated test-runner crates.
///
/// `incan test` generates a library crate and runs `cargo test` against that crate. Because the generated user/test
/// code lives under `src/`, anything it imports must be available as a normal dependency, not only under
/// `[dev-dependencies]`.
fn merge_test_runner_dependencies(
    dependencies: &[crate::manifest::DependencySpec],
    dev_dependencies: &[crate::manifest::DependencySpec],
) -> Result<Vec<crate::manifest::DependencySpec>, String> {
    let mut merged = dependencies.to_vec();
    for candidate in dev_dependencies {
        if let Some(existing) = merged.iter().find(|dep| dep.crate_name == candidate.crate_name) {
            if existing != candidate {
                return Err(format!(
                    "test runner dependency `{}` conflicts between dependencies and dev-dependencies",
                    candidate.crate_name
                ));
            }
            continue;
        }
        merged.push(candidate.clone());
    }
    merged.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(merged)
}

fn file_batch_dir_suffix(file_path: &Path) -> String {
    let p = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("batch_{}", &digest[..16])
}

/// Stable Rust crate name for one generated per-file test runner crate.
///
/// We derive this from the per-file batch suffix so shared `CARGO_TARGET_DIR` reuse does not alias crate identities
/// across different `.incn` files.
fn runner_crate_name_for_batch_suffix(batch_suffix: &str) -> String {
    let normalized = batch_suffix
        .strip_prefix("batch_")
        .unwrap_or(batch_suffix)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    format!("test_runner_{}", normalized)
}

/// Normalize a libtest test name by stripping any leading crate/module qualifiers before
/// [`INCAN_FILE_TEST_MOD`].
///
/// Examples:
/// - `__incan_file_tests::incan_harness_0_case` (unchanged)
/// - `test_runner::__incan_file_tests::incan_harness_0_case` (crate prefix removed)
fn normalize_libtest_test_name(name: &str) -> String {
    let trimmed = name.trim();
    if let Some(pos) = trimmed.find(INCAN_FILE_TEST_MOD) {
        trimmed[pos..].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Stable `#[test]` function name inside [`INCAN_FILE_TEST_MOD`] (indexed for guaranteed uniqueness).
fn harness_fn_name(test: &TestInfo, index: usize) -> String {
    let raw = test
        .parametrize_call
        .as_ref()
        .map(|p| p.display_id.as_str())
        .unwrap_or_else(|| test.function_name.as_str());
    let mut slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while slug.contains("__") {
        slug = slug.replace("__", "_");
    }
    slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        slug = "unnamed".to_string();
    }
    if slug.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        slug = format!("case_{slug}");
    }
    format!("incan_harness_{index}_{slug}")
}

/// Append a `#[cfg(test)]` module with one `#[test]` per collected case so `cargo test` runs an entire file in one
/// shot.
///
/// The generated harness resets the process cwd to the source project root before each test so fixture paths behave
/// the same way as ordinary `incan run/build/test` entrypoints rather than inheriting the generated temp crate path.
fn inject_file_test_harness(rust_code: &str, tests: &[TestInfo], project_root: &Path) -> String {
    let mut out = rust_code.to_string();
    let project_root_literal = format!("{:?}", project_root.to_string_lossy());
    out.push_str("\n\n#[cfg(test)]\nmod ");
    out.push_str(INCAN_FILE_TEST_MOD);
    out.push_str(" {\nuse super::*;\n");
    for (index, t) in tests.iter().enumerate() {
        let fname = harness_fn_name(t, index);
        let call = match &t.parametrize_call {
            Some(pc) => format!("    super::{}({});\n", t.function_name, pc.rust_args),
            None => format!("    super::{}();\n", t.function_name),
        };
        out.push_str("    #[test]\n    fn ");
        out.push_str(&fname);
        out.push_str("() {\n");
        out.push_str("        if let Err(err) = std::env::set_current_dir(");
        out.push_str(&project_root_literal);
        out.push_str(") {\n");
        out.push_str("            panic!(\"failed to set generated test cwd: {}\", err);\n");
        out.push_str("        }\n");
        out.push_str(&call);
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// Parse `cargo test` / libtest lines: `test <name> ... ok|FAILED`.
fn parse_libtest_outcomes(combined: &str) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    for line in combined.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(" ... ") else {
            continue;
        };
        let status = tail.trim();
        let passed = status.starts_with("ok");
        let failed = status.starts_with("FAILED");
        if passed || failed {
            map.insert(normalize_libtest_test_name(name), passed);
        }
    }
    map
}

/// Fully qualified Rust test name as libtest prints it for harness functions under [`INCAN_FILE_TEST_MOD`].
fn libtest_qualified_name(fn_name: &str) -> String {
    format!("{INCAN_FILE_TEST_MOD}::{fn_name}")
}

/// Best-effort extraction of failure output for one harness `fn_name` from combined `cargo test` stdout/stderr.
///
/// Looks for libtest `---- <qualified> stdout ----` sections, then falls back to panic/assertion heuristics or the
/// full trimmed output.
fn extract_libtest_failure_detail(combined: &str, full_name: &str) -> String {
    for line in combined.lines() {
        let line = line.trim();
        if line.starts_with("---- ")
            && line.ends_with(" stdout ----")
            && normalize_libtest_test_name(line).contains(full_name)
            && let Some(pos) = combined.find(line)
        {
            let after = &combined[pos + line.len()..];
            let end = after
                .find("\n---- ")
                .unwrap_or_else(|| after.find("\nfailures:").unwrap_or(after.len()));
            let body = after[..end].trim();
            if !body.is_empty() {
                return body.to_string();
            }
        }
    }
    if combined.contains("panicked at") {
        return extract_panic_message(combined);
    }
    if combined.contains("assertion") {
        return extract_assertion_error(combined);
    }
    combined.trim().to_string()
}

/// Turn one batched `cargo test` run into per-[`TestInfo`] results.
///
/// On compile failure, every test shares `compile_message`. Otherwise outcomes come from [`parse_libtest_outcomes`];
/// wall time is split evenly across tests for display.
fn map_batch_results(
    tests: &[TestInfo],
    combined_output: &str,
    elapsed: std::time::Duration,
    compile_failed: bool,
    compile_message: &str,
    manifest_path: &Path,
    crate_name: &str,
) -> Vec<(TestInfo, TestResult)> {
    if compile_failed {
        let msg = compile_message.to_string();
        return tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(elapsed, msg.clone())))
            .collect();
    }

    let outcomes = parse_libtest_outcomes(combined_output);
    let per_test_ms = elapsed.as_millis() / tests.len().max(1) as u128;

    tests
        .iter()
        .enumerate()
        .map(|(index, t)| {
            let fname = harness_fn_name(t, index);
            let full = libtest_qualified_name(&fname);
            let result = match outcomes.get(&full) {
                Some(true) => TestResult::Passed(std::time::Duration::from_millis(per_test_ms as u64)),
                Some(false) => {
                    let detail = extract_libtest_failure_detail(combined_output, &full);
                    TestResult::Failed(std::time::Duration::from_millis(per_test_ms as u64), detail)
                }
                None => TestResult::Failed(
                    elapsed,
                    if combined_output.contains(INCAN_FILE_TEST_MOD) {
                        format!(
                            "Test runner did not report outcome for `{full}`.\nmanifest=`{}` crate=`{}`\nThis may indicate stale/shared test-runner artifacts.\n{combined_output}",
                            manifest_path.display(),
                            crate_name,
                        )
                    } else {
                        format!(
                            "Test runner did not report outcome for `{full}` (see cargo output below)\n{combined_output}"
                        )
                    },
                ),
            };
            (t.clone(), result)
        })
        .collect()
}

/// Run every collected test in `tests` that lives in the same `.incn` file with **one** `cargo test` invocation (#271).
///
/// Returns an empty vector when `tests` is empty. Otherwise every entry must share the same [`TestInfo::file_path`].
/// Skip/xfail handling stays in [`super::run_tests`].
#[allow(clippy::too_many_arguments)]
pub(super) fn run_file_tests_batch(
    tests: &[TestInfo],
    prep_cache: &mut HashMap<String, Arc<PreparedTestFile>>,
    locked: bool,
    frozen: bool,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    fail_fast: bool,
) -> Vec<(TestInfo, TestResult)> {
    if tests.is_empty() {
        return Vec::new();
    }
    if tests.windows(2).any(|w| w[0].file_path != w[1].file_path) {
        let start = Instant::now();
        let msg = "internal error: `run_file_tests_batch` received tests from multiple files".to_string();
        return tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), msg.clone())))
            .collect();
    }

    let start = Instant::now();
    let first = &tests[0];

    // ---- Context: load test source, discover manifest, parse and vocab-desugar the test file ----
    let source = match fs::read_to_string(&first.file_path) {
        Ok(s) => s,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Failed to read file: {}", e)),
                    )
                })
                .collect();
        }
    };

    let manifest = match ProjectManifest::discover(first.file_path.parent().unwrap_or_else(|| Path::new("."))) {
        Ok(manifest) => manifest,
        Err(err) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Manifest error: {}", err)),
                    )
                })
                .collect();
        }
    };
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Lexer error: {:?}", e)),
                    )
                })
                .collect();
        }
    };

    let path_display = first.file_path.to_string_lossy();
    let mut ast = match parser::parse_with_context(&tokens, Some(path_display.as_ref()), Some(&library_imported_vocab))
    {
        Ok(a) => a,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Parser error: {:?}", e)),
                    )
                })
                .collect();
        }
    };
    if let Err(errors) =
        vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, Some(path_display.as_ref()), &library_manifest_index)
    {
        return tests
            .iter()
            .map(|t| {
                (
                    t.clone(),
                    TestResult::Failed(start.elapsed(), format!("Vocab desugar error: {:?}", errors)),
                )
            })
            .collect();
    }

    let cargo_feature_selection = CargoFeatureSelection {
        cargo_features: cargo_features.to_vec(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    // ---- Context: resolve project paths and collect transitive Incan modules for the test ----
    let project_root = manifest
        .as_ref()
        .map(|m| m.project_root().to_path_buf())
        .unwrap_or_else(|| infer_project_root_without_manifest(&first.file_path));
    let project_root = absolute_project_root(&project_root);
    let source_root = common::resolve_source_root(&project_root, manifest.as_ref());
    let source_modules = match collect_source_modules_for_test(
        &ast,
        &source_root,
        Some(&library_imported_vocab),
        Some(&library_manifest_index),
    ) {
        Ok(m) => m,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Failed to collect source modules: {}", e)),
                    )
                })
                .collect();
        }
    };

    // ---- Context: session prep cache — reuse deps / lock / rust-inspect when key matches ----
    let cache_key = compute_test_prep_cache_key(
        &first.file_path,
        &source,
        &source_modules,
        manifest.as_ref(),
        &cargo_feature_selection,
        locked,
        frozen,
    );

    let prepared: Arc<PreparedTestFile> = if let Some(hit) = prep_cache.get(&cache_key) {
        Arc::clone(hit)
    } else {
        // ---- Context: cold prep — inline imports, resolve and merge Cargo deps, lock + rust-inspect workspace ----
        let module_for_imports = ParsedModule {
            name: "test".to_string(),
            path_segments: vec!["test".to_string()],
            file_path: first.file_path.clone(),
            source: source.clone(),
            ast: ast.clone(),
        };

        let mut dependency_modules: Vec<ParsedModule> = Vec::with_capacity(1 + source_modules.len());
        dependency_modules.push(module_for_imports);
        dependency_modules.extend(source_modules.iter().map(|m| ParsedModule {
            name: m.name.clone(),
            path_segments: m.path_segments.clone(),
            file_path: m.file_path.clone(),
            source: m.source.clone(),
            ast: m.ast.clone(),
        }));

        let mut inline_imports = Vec::new();
        for module in &dependency_modules {
            inline_imports.extend(common::collect_inline_rust_imports(module, true));
        }

        let project_requirements =
            match common::collect_project_requirements(&dependency_modules, &library_manifest_index) {
                Ok(requirements) => requirements,
                Err(err) => {
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                        .collect();
                }
            };

        let mut resolved =
            match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_feature_selection) {
                Ok(resolved) => resolved,
                Err(errors) => {
                    let mut sources = HashMap::new();
                    sources.insert(first.file_path.clone(), source.clone());
                    let mut msg = String::new();
                    for err in &errors {
                        msg.push_str(&common::format_dependency_error(err, &sources));
                    }
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), msg.clone())))
                        .collect();
                }
            };
        if let Err(err) = common::merge_project_requirement_dependencies(&mut resolved, &project_requirements) {
            return tests
                .iter()
                .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                .collect();
        }

        let project_name = manifest
            .as_ref()
            .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
            .or_else(|| {
                first
                    .file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "incan_test".to_string());
        let lock_payload = match commands::resolve_lock_payload(commands::LockResolutionRequest {
            project_root: &project_root,
            project_name: &project_name,
            manifest: manifest.as_ref(),
            resolved: &resolved,
            project_requirements: &project_requirements,
            cargo_features: &cargo_feature_selection,
            locked,
            frozen,
        }) {
            Ok(payload) => payload,
            Err(err) => {
                return tests
                    .iter()
                    .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                    .collect();
            }
        };

        #[cfg(feature = "rust_inspect")]
        let rust_inspect_manifest_dir = {
            let metadata_query_paths = collect_rust_inspect_query_paths(&dependency_modules);
            let mut rust_inspect_requirements = project_requirements.clone();
            rust_inspect_requirements.stdlib_features = merge_rust_inspect_stdlib_features(
                prep_cache
                    .values()
                    .map(|prepared| prepared.project_requirements.stdlib_features.as_slice()),
                &project_requirements.stdlib_features,
            );
            let rust_inspect_manifest_dir = match ensure_rust_inspect_workspace(
                &project_root,
                &project_name,
                manifest
                    .as_ref()
                    .and_then(|m| m.build.as_ref().and_then(|build| build.rust_edition.clone())),
                &resolved,
                &rust_inspect_requirements,
                lock_payload.clone(),
            ) {
                Ok(dir) => dir,
                Err(err) => {
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                        .collect();
                }
            };
            if let Err(err) = prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, &metadata_query_paths) {
                return tests
                    .iter()
                    .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                    .collect();
            }
            rust_inspect_manifest_dir
        };

        let prepared = PreparedTestFile {
            library_manifest_index,
            ast,
            source_modules,
            project_root,
            resolved,
            project_requirements,
            lock_payload,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir,
        };
        let arc = Arc::new(prepared);
        prep_cache.insert(cache_key, Arc::clone(&arc));
        arc
    };

    // ---- Codegen + unified file harness (library + #[cfg(test)] module) ----
    let mut codegen = IrCodegen::new();
    codegen.set_library_manifest_index(prepared.library_manifest_index.clone());
    #[cfg(feature = "rust_inspect")]
    {
        codegen.set_rust_inspect_manifest_dir(prepared.rust_inspect_manifest_dir.clone());
    }

    for module in &prepared.source_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let dir_suffix = file_batch_dir_suffix(&first.file_path);
    let runner_crate_name = runner_crate_name_for_batch_suffix(&dir_suffix);
    let temp_dir = format!("target/incan_tests/{}", dir_suffix);
    let temp_dir_path = PathBuf::from(&temp_dir);
    let manifest_path = if temp_dir_path.is_absolute() {
        temp_dir_path.join("Cargo.toml")
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(&temp_dir).join("Cargo.toml")
    } else {
        temp_dir_path.join("Cargo.toml")
    };

    let mut generator = ProjectGenerator::new(&temp_dir, &runner_crate_name, false);
    generator.set_stdlib_features(prepared.project_requirements.stdlib_features.clone());
    generator.set_cargo_lock_payload(prepared.lock_payload.clone());
    generator.set_cargo_policy_flags(common::cargo_command_flags(locked, frozen, &cargo_feature_selection));

    let gen_err = |msg: String| {
        tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), msg.clone())))
            .collect::<Vec<_>>()
    };

    let runner_dependencies =
        match merge_test_runner_dependencies(&prepared.resolved.dependencies, &prepared.resolved.dev_dependencies) {
            Ok(deps) => deps,
            Err(message) => return gen_err(message),
        };
    generator.set_include_dev_dependencies(false);
    generator.set_dependencies(runner_dependencies);
    generator.set_dev_dependencies(Vec::new());

    if prepared.source_modules.is_empty() {
        let rust_code = match codegen.try_generate(&prepared.ast) {
            Ok(code) => code,
            Err(e) => return gen_err(format!("Code generation error: {}", e)),
        };
        let rust_code = inject_file_test_harness(&rust_code, tests, &prepared.project_root);
        if let Err(e) = generator.generate(&rust_code) {
            return gen_err(format!("Failed to generate project: {}", e));
        }
    } else {
        let module_paths: Vec<Vec<String>> = prepared
            .source_modules
            .iter()
            .map(|m| m.path_segments.clone())
            .collect();
        let (main_code, rust_modules) = match codegen.try_generate_multi_file_nested(&prepared.ast, &module_paths) {
            Ok(result) => result,
            Err(e) => return gen_err(format!("Code generation error: {}", e)),
        };
        let main_code = inject_file_test_harness(&main_code, tests, &prepared.project_root);
        if let Err(e) = generator.generate_nested(&main_code, &rust_modules) {
            return gen_err(format!("Failed to generate project: {}", e));
        }
    }

    let shared_target_dir = shared_cargo_target_dir(&prepared.project_root);
    let mut command = std::process::Command::new("cargo");
    command.arg("test");
    command.arg("--manifest-path");
    command.arg(&manifest_path);
    for flag in common::cargo_command_flags(locked, frozen, &cargo_feature_selection) {
        command.arg(flag);
    }
    // Batched per-file execution shares one process across all generated #[test] fns.
    // Force deterministic single-thread libtest execution to preserve historical
    // isolation assumptions for tests that use shared global runtime state.
    command.arg("--");
    command.arg("--test-threads=1");
    if fail_fast {
        command.arg("--fail-fast");
    }

    let output = match command
        .env("CARGO_TARGET_DIR", &shared_target_dir)
        // Keep runtime-relative fixture paths anchored to the caller's project, not the generated test crate.
        .current_dir(&prepared.project_root)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Failed to run cargo test: {}", e)),
                    )
                })
                .collect();
        }
    };

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    let compile_failed = !output.status.success()
        && (combined.contains("could not compile")
            || combined.contains("error: could not compile")
            || combined.contains("error[E")
            || combined.contains("error: aborting due to"));

    if compile_failed {
        return map_batch_results(
            tests,
            &combined,
            elapsed,
            true,
            &combined,
            &manifest_path,
            &runner_crate_name,
        );
    }

    map_batch_results(tests, &combined, elapsed, false, "", &manifest_path, &runner_crate_name)
}

/// Inject a `fn main()` into generated Rust code that calls the test function (legacy single-case `cargo run` path).
///
/// Kept for unit tests only; the runner uses [`inject_file_test_harness`] + `cargo test` per source file (#271).
#[cfg(test)]
fn inject_test_main(
    rust_code: &str,
    function_name: &str,
    parametrize: Option<&super::types::ParametrizeCall>,
) -> String {
    let call = if let Some(pc) = parametrize {
        format!("    {}({});", function_name, pc.rust_args)
    } else {
        format!("    {}();", function_name)
    };
    format!("{}\nfn main() {{\n{}\n}}\n", rust_code, call)
}

/// Extract an assertion error from stderr.
fn extract_assertion_error(stderr: &str) -> String {
    for line in stderr.lines() {
        if line.contains("assertion") || line.contains("AssertionError") {
            return line.trim().to_string();
        }
    }
    stderr.to_string()
}

/// Extract a panic message from stdout.
fn extract_panic_message(stdout: &str) -> String {
    let mut in_panic = false;
    let mut msg = String::new();

    for line in stdout.lines() {
        if line.contains("panicked at") {
            in_panic = true;
            msg.push_str(line.trim());
            msg.push('\n');
        } else if in_panic && line.starts_with("  ") {
            msg.push_str(line);
            msg.push('\n');
        } else if in_panic && line.is_empty() {
            break;
        }
    }

    if msg.is_empty() { stdout.to_string() } else { msg }
}

#[cfg(test)]
mod tests {
    use super::super::types::ParametrizeCall;
    use super::*;
    use std::path::Path;

    #[test]
    fn shared_target_dir_stays_under_project_target() {
        let target_dir = shared_cargo_target_dir(Path::new("/tmp/incan_project"));
        assert_eq!(target_dir, PathBuf::from("/tmp/incan_project/target"));
    }

    #[test]
    fn parse_isolated_target_env_requires_truthy_values() {
        assert!(parse_isolated_target_env(Some("1")));
        assert!(parse_isolated_target_env(Some("true")));
        assert!(parse_isolated_target_env(Some(" yes ")));
        assert!(parse_isolated_target_env(Some("on")));
        assert!(!parse_isolated_target_env(None));
        assert!(!parse_isolated_target_env(Some("0")));
        assert!(!parse_isolated_target_env(Some("false")));
        assert!(!parse_isolated_target_env(Some("off")));
        assert!(!parse_isolated_target_env(Some("")));
    }

    #[test]
    fn inject_main_plain_test() {
        let rust = "fn test_add() { assert_eq!(1 + 1, 2); }";
        let result = inject_test_main(rust, "test_add", None);
        assert!(result.contains("fn main()"), "should inject fn main()");
        assert!(result.contains("test_add();"), "should call test function");
        assert!(!result.contains("test_add(1"), "plain test has no args");
    }

    #[test]
    fn inject_main_parametrized_int_args() {
        let rust = "fn test_add(x: i64, y: i64, expected: i64) { assert_eq!(x + y, expected); }";
        let pc = ParametrizeCall {
            display_id: "test_add[1-2-3]".to_string(),
            rust_args: "1, 2, 3".to_string(),
        };
        let result = inject_test_main(rust, "test_add", Some(&pc));
        assert!(result.contains("fn main()"), "should inject fn main()");
        assert!(result.contains("test_add(1, 2, 3);"), "should call with int args");
    }

    #[test]
    fn inject_main_parametrized_string_args() {
        let rust = "fn test_len(input: String, expected: i64) {}";
        let pc = ParametrizeCall {
            display_id: "test_len[hello-5]".to_string(),
            rust_args: "\"hello\".to_string(), 5".to_string(),
        };
        let result = inject_test_main(rust, "test_len", Some(&pc));
        assert!(
            result.contains("test_len(\"hello\".to_string(), 5);"),
            "should call with string + int args"
        );
    }

    #[test]
    fn inject_main_parametrized_negative_args() {
        let rust = "fn test_sub(a: i64, b: i64, expected: i64) {}";
        let pc = ParametrizeCall {
            display_id: "test_sub[-1-1-0]".to_string(),
            rust_args: "-1, 1, 0".to_string(),
        };
        let result = inject_test_main(rust, "test_sub", Some(&pc));
        assert!(
            result.contains("test_sub(-1, 1, 0);"),
            "should call with negative int args"
        );
    }

    #[test]
    fn prep_cache_key_stable_for_identical_inputs() {
        let cargo = CargoFeatureSelection {
            cargo_features: vec!["serde".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        }
        .normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let k1 = compute_test_prep_cache_key(
            Path::new("tests/test_x.incn"),
            "source a",
            &mods,
            None,
            &cargo,
            false,
            false,
        );
        let k2 = compute_test_prep_cache_key(
            Path::new("tests/test_x.incn"),
            "source a",
            &mods,
            None,
            &cargo,
            false,
            false,
        );
        assert_eq!(k1, k2);
        assert!(k1.starts_with("v1:"));
    }

    #[test]
    fn prep_cache_key_changes_when_test_source_changes() {
        let cargo = CargoFeatureSelection::default().normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let k1 = compute_test_prep_cache_key(Path::new("t.incn"), "a", &mods, None, &cargo, false, false);
        let k2 = compute_test_prep_cache_key(Path::new("t.incn"), "b", &mods, None, &cargo, false, false);
        assert_ne!(k1, k2);
    }

    #[test]
    fn prep_cache_key_includes_locked_and_frozen_flags() {
        let cargo = CargoFeatureSelection::default().normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let k_unlocked = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, false, false);
        let k_locked = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, true, false);
        assert_ne!(k_unlocked, k_locked);
    }

    #[test]
    fn merge_rust_inspect_stdlib_features_unions_and_sorts() {
        let existing = [
            vec!["json".to_string(), "async".to_string()],
            vec!["web".to_string(), "async".to_string()],
        ];
        let merged = merge_rust_inspect_stdlib_features(
            existing.iter().map(|set| set.as_slice()),
            &["serde".to_string(), "json".to_string()],
        );
        assert_eq!(
            merged,
            vec![
                "async".to_string(),
                "json".to_string(),
                "serde".to_string(),
                "web".to_string()
            ]
        );
    }

    #[test]
    fn merge_test_runner_dependencies_promotes_dev_deps_into_dependencies() {
        use crate::manifest::{DependencySource, DependencySpec};

        let deps = vec![DependencySpec {
            crate_name: "serde".to_string(),
            version: Some("1.0".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];
        let dev_deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["macros".to_string(), "rt-multi-thread".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];

        let merged = match merge_test_runner_dependencies(&deps, &dev_deps) {
            Ok(merged) => merged,
            Err(err) => panic!("expected merge to succeed: {err}"),
        };
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|dep| dep.crate_name == "serde"));
        assert!(merged.iter().any(|dep| dep.crate_name == "tokio"));
    }

    #[test]
    fn merge_test_runner_dependencies_rejects_conflicting_duplicates() {
        use crate::manifest::{DependencySource, DependencySpec};

        let deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["time".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];
        let dev_deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["macros".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];

        let error = match merge_test_runner_dependencies(&deps, &dev_deps) {
            Ok(merged) => panic!("expected conflict, got merged dependencies: {merged:?}"),
            Err(err) => err,
        };
        assert!(error.contains("tokio"));
        assert!(error.contains("conflicts"));
    }

    #[test]
    fn parse_libtest_outcomes_detects_ok_and_failed() {
        let out = r#"
test __incan_file_tests::incan_harness_0_a ... ok
test __incan_file_tests::incan_harness_1_b ... FAILED
test result: FAILED. 1 passed; 1 failed
"#;
        let m = parse_libtest_outcomes(out);
        assert_eq!(m.get("__incan_file_tests::incan_harness_0_a"), Some(&true));
        assert_eq!(m.get("__incan_file_tests::incan_harness_1_b"), Some(&false));
    }

    #[test]
    fn parse_libtest_outcomes_normalizes_prefixed_names() {
        let out = r#"
test test_runner_76001490ba86f677::__incan_file_tests::incan_harness_0_a ... ok
test test_runner_76001490ba86f677::__incan_file_tests::incan_harness_1_b ... FAILED
"#;
        let m = parse_libtest_outcomes(out);
        assert_eq!(m.get("__incan_file_tests::incan_harness_0_a"), Some(&true));
        assert_eq!(m.get("__incan_file_tests::incan_harness_1_b"), Some(&false));
    }

    #[test]
    fn runner_crate_name_is_derived_from_batch_suffix() {
        let name = runner_crate_name_for_batch_suffix("batch_76001490ba86f677");
        assert_eq!(name, "test_runner_76001490ba86f677");
    }

    #[test]
    fn inject_file_test_harness_emits_tests_module() {
        let rust = "fn test_a() {}\nfn test_b() {}\n";
        let tests = vec![
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_a".to_string(),
                markers: vec![],
                required_fixtures: vec![],
                parametrize_call: None,
            },
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_b".to_string(),
                markers: vec![],
                required_fixtures: vec![],
                parametrize_call: None,
            },
        ];
        let g = inject_file_test_harness(rust, &tests, Path::new("."));
        assert!(g.contains("mod __incan_file_tests"));
        assert!(g.contains("fn incan_harness_0_test_a"));
        assert!(g.contains("fn incan_harness_1_test_b"));
        assert!(g.contains("set_current_dir"));
        assert!(g.contains("super::test_a();"));
        assert!(g.contains("super::test_b();"));
    }
}
