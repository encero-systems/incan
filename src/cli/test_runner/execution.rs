use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::commands;
use crate::cli::commands::common;
use crate::cli::prelude::ParsedModule;
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::{lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;

use super::module_graph::collect_source_modules_for_test;
use super::types::{ParametrizeCall, TestInfo, TestResult};

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

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Lexer error: {:?}", e)),
    };

    let ast = match parser::parse(&tokens) {
        Ok(a) => a,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Parser error: {:?}", e)),
    };

    let module_for_imports = ParsedModule {
        name: "test".to_string(),
        path_segments: vec!["test".to_string()],
        file_path: test.file_path.clone(),
        source: source.clone(),
        ast: ast.clone(),
    };
    let inline_imports = common::collect_inline_rust_imports(&module_for_imports, true);
    let manifest = match ProjectManifest::discover(test.file_path.parent().unwrap_or_else(|| Path::new("."))) {
        Ok(manifest) => manifest,
        Err(err) => {
            return TestResult::Failed(start.elapsed(), format!("Manifest error: {}", err));
        }
    };

    let cargo_feature_selection = CargoFeatureSelection {
        cargo_features: cargo_features.to_vec(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let resolved = match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_feature_selection) {
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

    let project_root = manifest
        .as_ref()
        .map(|m| m.project_root().to_path_buf())
        .unwrap_or_else(|| test.file_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());
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
    let lock_payload = match commands::resolve_lock_payload(
        &project_root,
        &project_name,
        manifest.as_ref(),
        &resolved,
        &cargo_feature_selection,
        locked,
        frozen,
    ) {
        Ok(payload) => payload,
        Err(err) => {
            return TestResult::Failed(start.elapsed(), err.message);
        }
    };

    // ---- Collect source modules referenced by the test ----
    let source_root = common::resolve_source_root(&project_root, manifest.as_ref());
    let source_modules = match collect_source_modules_for_test(&ast, &source_root) {
        Ok(m) => m,
        Err(e) => {
            return TestResult::Failed(start.elapsed(), format!("Failed to collect source modules: {}", e));
        }
    };

    // ---- Setup codegen ----
    let mut codegen = IrCodegen::new();

    for module in &source_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    // Scan all modules for feature flags.
    codegen.scan_for_serde(&ast);
    codegen.scan_for_async(&ast);
    codegen.scan_for_list_helpers(&ast);
    for module in &source_modules {
        codegen.scan_for_serde(&module.ast);
        codegen.scan_for_async(&module.ast);
        codegen.scan_for_list_helpers(&module.ast);
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
    generator.set_needs_serde(codegen.needs_serde());
    generator.set_needs_tokio(codegen.needs_tokio());
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
    let mut command = std::process::Command::new("cargo");
    command.arg("run");
    for flag in common::cargo_command_flags(locked, frozen, &cargo_feature_selection) {
        command.arg(flag);
    }
    let output = command.current_dir(&temp_dir).output();

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
