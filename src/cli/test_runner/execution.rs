use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::commands;
use crate::cli::commands::common;
#[cfg(feature = "rust-metadata")]
use crate::cli::commands::common::ensure_rust_metadata_workspace;
use crate::cli::prelude::ParsedModule;
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::vocab_desugar_pass;
use crate::frontend::{lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;

use super::module_graph::collect_source_modules_for_test;
use super::types::{ParametrizeCall, TestInfo, TestResult};

/// Reuse Cargo artifacts across generated test projects for the same project root.
///
/// `incan test` materializes one generated Cargo project per discovered test under `target/incan_tests/<case>`. If each
/// generated project also gets its own Cargo `target/` directory, dependencies like Tokio and Axum are rebuilt over and
/// over. Pointing them all at a shared target directory keeps per-test recompiles mostly limited to the tiny generated
/// crate instead of the full dependency graph.
fn shared_cargo_target_dir(project_root: &Path) -> PathBuf {
    let absolute_project_root = if project_root.is_absolute() {
        project_root.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(project_root)
    } else {
        project_root.to_path_buf()
    };

    absolute_project_root.join("target").join("incan_test_runner")
}

/// Run a single test.
pub(super) fn run_single_test(
    test: &TestInfo,
    locked: bool,
    frozen: bool,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> TestResult {
    let start = Instant::now();

    let source = match fs::read_to_string(&test.file_path) {
        Ok(s) => s,
        Err(e) => {
            return TestResult::Failed(start.elapsed(), format!("Failed to read file: {}", e));
        }
    };

    let manifest = match ProjectManifest::discover(test.file_path.parent().unwrap_or_else(|| Path::new("."))) {
        Ok(manifest) => manifest,
        Err(err) => {
            return TestResult::Failed(start.elapsed(), format!("Manifest error: {}", err));
        }
    };
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Lexer error: {:?}", e)),
    };

    let path_display = test.file_path.to_string_lossy();
    let mut ast = match parser::parse_with_context(&tokens, Some(path_display.as_ref()), Some(&library_imported_vocab))
    {
        Ok(a) => a,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Parser error: {:?}", e)),
    };
    if let Err(errors) =
        vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, Some(path_display.as_ref()), &library_manifest_index)
    {
        return TestResult::Failed(start.elapsed(), format!("Vocab desugar error: {:?}", errors));
    }

    let module_for_imports = ParsedModule {
        name: "test".to_string(),
        path_segments: vec!["test".to_string()],
        file_path: test.file_path.clone(),
        source: source.clone(),
        ast: ast.clone(),
    };

    let cargo_feature_selection = CargoFeatureSelection {
        cargo_features: cargo_features.to_vec(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    // ---- Collect source modules referenced by the test ----
    let project_root = manifest
        .as_ref()
        .map(|m| m.project_root().to_path_buf())
        .unwrap_or_else(|| test.file_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());
    let source_root = common::resolve_source_root(&project_root, manifest.as_ref());
    let source_modules = match collect_source_modules_for_test(
        &ast,
        &source_root,
        Some(&library_imported_vocab),
        Some(&library_manifest_index),
    ) {
        Ok(m) => m,
        Err(e) => {
            return TestResult::Failed(start.elapsed(), format!("Failed to collect source modules: {}", e));
        }
    };

    // ---- Collect dependency imports from test + transitive source modules ----
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

    let project_requirements = match common::collect_project_requirements(&dependency_modules, &library_manifest_index)
    {
        Ok(requirements) => requirements,
        Err(err) => {
            return TestResult::Failed(start.elapsed(), err.message);
        }
    };

    let mut resolved = match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_feature_selection) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut sources = HashMap::new();
            sources.insert(test.file_path.clone(), source.clone());
            let mut msg = String::new();
            for err in &errors {
                msg.push_str(&common::format_dependency_error(err, &sources));
            }
            return TestResult::Failed(start.elapsed(), msg);
        }
    };
    if let Err(err) = common::merge_project_requirement_dependencies(&mut resolved, &project_requirements) {
        return TestResult::Failed(start.elapsed(), err.message);
    }

    let project_name = manifest
        .as_ref()
        .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
        .or_else(|| {
            test.file_path
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
            return TestResult::Failed(start.elapsed(), err.message);
        }
    };

    // ---- Setup codegen ----
    let mut codegen = IrCodegen::new();
    codegen.set_library_manifest_index(library_manifest_index.clone());
    #[cfg(feature = "rust-metadata")]
    {
        let rust_metadata_manifest_dir = match ensure_rust_metadata_workspace(
            &project_root,
            &project_name,
            manifest
                .as_ref()
                .and_then(|m| m.build.as_ref().and_then(|build| build.rust_edition.clone())),
            &resolved,
            &project_requirements,
            lock_payload.clone(),
        ) {
            Ok(dir) => dir,
            Err(err) => return TestResult::Failed(start.elapsed(), err.message),
        };
        codegen.set_rust_metadata_manifest_dir(rust_metadata_manifest_dir);
    }

    for module in &source_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    // ---- Determine unique temp dir ----
    // Parametrized variants include the case ID in the directory name to avoid collisions.
    let dir_suffix = test
        .parametrize_call
        .as_ref()
        .map_or_else(|| test.function_name.clone(), |p| p.display_id.clone())
        .replace(['[', ']', '-'], "_");
    let temp_dir = format!("target/incan_tests/{}", dir_suffix);

    let mut generator = ProjectGenerator::new(&temp_dir, "test_runner", true);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);
    generator.set_cargo_lock_payload(lock_payload);
    generator.set_cargo_policy_flags(common::cargo_command_flags(locked, frozen, &cargo_feature_selection));

    // ---- Generate project (multi-file when source modules are present) ----
    if source_modules.is_empty() {
        let rust_code = match codegen.try_generate(&ast) {
            Ok(code) => code,
            Err(e) => {
                return TestResult::Failed(start.elapsed(), format!("Code generation error: {}", e));
            }
        };
        let rust_code = inject_test_main(&rust_code, &test.function_name, test.parametrize_call.as_ref());
        if let Err(e) = generator.generate(&rust_code) {
            return TestResult::Failed(start.elapsed(), format!("Failed to generate project: {}", e));
        }
    } else {
        let module_paths: Vec<Vec<String>> = source_modules.iter().map(|m| m.path_segments.clone()).collect();
        let (main_code, rust_modules) = match codegen.try_generate_multi_file_nested(&ast, &module_paths) {
            Ok(result) => result,
            Err(e) => {
                return TestResult::Failed(start.elapsed(), format!("Code generation error: {}", e));
            }
        };
        let main_code = inject_test_main(&main_code, &test.function_name, test.parametrize_call.as_ref());
        if let Err(e) = generator.generate_nested(&main_code, &rust_modules) {
            return TestResult::Failed(start.elapsed(), format!("Failed to generate project: {}", e));
        }
    }

    // ---- Run the generated project ----
    // Use `cargo run` rather than `cargo test` — the injected `fn main()` calls the test function directly. A panic
    // (assertion failure) exits non-zero; a clean return exits zero.
    let shared_target_dir = shared_cargo_target_dir(&project_root);
    let mut command = std::process::Command::new("cargo");
    command.arg("run");
    for flag in common::cargo_command_flags(locked, frozen, &cargo_feature_selection) {
        command.arg(flag);
    }
    let output = command
        .env("CARGO_TARGET_DIR", &shared_target_dir)
        .current_dir(&temp_dir)
        .output();

    match output {
        Ok(output) => {
            let duration = start.elapsed();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                TestResult::Passed(duration)
            } else {
                let msg = if stderr.contains("assertion") {
                    extract_assertion_error(&stderr)
                } else if stdout.contains("panicked") {
                    extract_panic_message(&stdout)
                } else {
                    format!("Test failed\n{}\n{}", stdout, stderr)
                };
                TestResult::Failed(duration, msg)
            }
        }
        Err(e) => TestResult::Failed(start.elapsed(), format!("Failed to run test: {}", e)),
    }
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

/// Inject a `fn main()` into generated Rust code that calls the test function.
///
/// The generated code from the IR pipeline has no `fn main()` for test files. This function appends one that calls the
/// discovered test function (optionally with parametrized arguments), so that `cargo run` executes the test. A panic
/// (assertion failure) causes a non-zero exit; a clean return signals success.
fn inject_test_main(rust_code: &str, function_name: &str, parametrize: Option<&ParametrizeCall>) -> String {
    let call = if let Some(pc) = parametrize {
        format!("    {}({});", function_name, pc.rust_args)
    } else {
        format!("    {}();", function_name)
    };
    format!("{}\nfn main() {{\n{}\n}}\n", rust_code, call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn shared_target_dir_stays_under_project_target() {
        let target_dir = shared_cargo_target_dir(Path::new("/tmp/incan_project"));
        assert_eq!(target_dir, PathBuf::from("/tmp/incan_project/target/incan_test_runner"));
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
}
