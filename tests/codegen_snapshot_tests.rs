//! Golden snapshot tests for codegen
//!
//! These tests generate Rust code from `.incn` input files and compare the output against stored snapshots.
//! This ensures codegen changes are reviewed and intentional.
//!
//! Run with: `cargo test --test codegen_snapshot_tests`
//! Review changes: `cargo insta review`

use incan::backend::IrCodegen;
use incan::frontend::{lexer, parser};
use std::fs;

/// Generate Rust code from Incan source
fn generate_rust(source: &str) -> String {
    let Ok(tokens) = lexer::lex(source) else {
        panic!("lexer failed");
    };
    let Ok(ast) = parser::parse(&tokens) else {
        panic!("parser failed");
    };
    let Ok(code) = IrCodegen::new().try_generate(&ast) else {
        panic!("codegen snapshot inputs must typecheck");
    };
    normalize_codegen_output(&code)
}

/// Generate Rust code from Incan source with a populated library index
fn generate_rust_with_widgets_manifest(source: &str) -> String {
    use incan::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan::library_manifest::{ConstExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, TypeRef};
    use std::collections::HashMap;

    let Ok(tokens) = lexer::lex(source) else {
        panic!("lexer failed");
    };
    let Ok(ast) = parser::parse(&tokens) else {
        panic!("parser failed");
    };

    let mut artifact_root = std::env::temp_dir();
    artifact_root.push("incan_test_widgets_artifacts");
    artifact_root.push("target");
    artifact_root.push("lib");

    let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Widget".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        fields: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.functions.push(FunctionExport {
        name: "make_widget".to_string(),
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "name".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
        }],
        return_type: TypeRef::Named {
            name: "Widget".to_string(),
        },
        is_async: false,
    });
    manifest.exports.consts.push(ConstExport {
        name: "DEFAULT_NAME".to_string(),
        ty: TypeRef::Named {
            name: "str".to_string(),
        },
    });

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "widgets".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("widgets", "widgets_core", artifact_root),
        },
    )]));

    let mut codegen = IrCodegen::new();
    codegen.set_library_manifest_index(index);
    let code = match codegen.try_generate(&ast) {
        Ok(c) => c,
        Err(e) => panic!("codegen snapshot inputs must typecheck: {e:?}"),
    };
    normalize_codegen_output(&code)
}

/// Generate Rust from source that includes imported vocab blocks desugared via a WASM artifact.
fn generate_rust_with_vocab_wasm_desugaring(source: &str) -> String {
    use incan::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan::frontend::vocab_desugar_pass::desugar_program_vocab_blocks;
    use incan::library_manifest::{LibraryManifest, VocabDesugarerArtifact, VocabExports};
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
        name: "generated".to_string(),
        mutable: false,
        value: incan_vocab::IncanExpr::Int(1),
    }]);
    let output_payload = match serde_json::to_string(&response) {
        Ok(payload) => payload,
        Err(err) => panic!("failed to serialize desugar response: {err}"),
    };
    let mut output_bytes = String::new();
    for byte in output_payload.as_bytes() {
        output_bytes.push('\\');
        output_bytes.push_str(&format!("{byte:02x}"));
    }
    let output_offset = 0usize;
    let error_offset = 128usize;
    let input_offset = 256usize;
    let input_capacity = 4096usize;
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global (export "__incan_input_ptr") i32 (i32.const {input_offset}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity}))
  (global (export "__incan_input_len") (mut i32) (i32.const 0))
  (global (export "__incan_output_ptr") i32 (i32.const {output_offset}))
  (global (export "__incan_output_len") i32 (i32.const {out_len}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_offset}))
  (global (export "__incan_error_len") i32 (i32.const 0))
  (data (i32.const {output_offset}) "{out_data}")
  (func (export "desugar_block") (result i32)
    (i32.const 0)
  )
)"#,
        input_capacity = input_capacity,
        input_offset = input_offset,
        out_len = output_payload.len(),
        output_offset = output_offset,
        error_offset = error_offset,
        out_data = output_bytes,
    );
    let wasm_bytes = match wat::parse_str(wat_source) {
        Ok(bytes) => bytes,
        Err(err) => panic!("failed to compile wat: {err}"),
    };

    let mut artifact_root = std::env::temp_dir();
    artifact_root.push("incan_test_vocab_desugar_artifacts");
    artifact_root.push("target");
    artifact_root.push("lib");
    let desugarer_dir = artifact_root.join("desugarers");
    if let Err(err) = std::fs::create_dir_all(&desugarer_dir) {
        panic!("failed to create desugarer artifact dir: {err}");
    }
    let desugarer_path = desugarer_dir.join("routes_desugarer.wasm");
    if let Err(err) = std::fs::write(&desugarer_path, &wasm_bytes) {
        panic!("failed to write desugarer artifact: {err}");
    }
    if let Err(err) = std::fs::create_dir_all(artifact_root.join("src")) {
        panic!("failed to create crate src dir: {err}");
    }
    if let Err(err) = std::fs::write(
        artifact_root.join("Cargo.toml"),
        "[package]\nname = \"routes_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ) {
        panic!("failed to write Cargo.toml: {err}");
    }
    if let Err(err) = std::fs::write(artifact_root.join("src/lib.rs"), "pub fn ready() {}\n") {
        panic!("failed to write lib.rs: {err}");
    }

    let mut manifest = LibraryManifest::new("routes_core", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "route".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            relative_path: "desugarers/routes_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(&wasm_bytes)),
        }),
    });

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "routes".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("routes", "routes_core", artifact_root),
        },
    )]));
    let imported_vocab = index.library_imported_vocab();

    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(errs) => panic!("lexer failed: {errs:?}"),
    };
    let mut ast = match parser::parse_with_context(
        &tokens,
        Some("tests/codegen_snapshots/vocab_block_desugaring.incn"),
        Some(&imported_vocab),
    ) {
        Ok(ast) => ast,
        Err(errs) => panic!("parser failed: {errs:?}"),
    };
    if let Err(errs) = desugar_program_vocab_blocks(
        &mut ast,
        Some("tests/codegen_snapshots/vocab_block_desugaring.incn"),
        &index,
    ) {
        panic!("desugar pass failed: {errs:?}");
    }

    let mut codegen = IrCodegen::new();
    codegen.set_library_manifest_index(index);
    let code = match codegen.try_generate(&ast) {
        Ok(code) => code,
        Err(err) => panic!("codegen failed: {err}"),
    };
    normalize_codegen_output(&code)
}

/// Normalize generated output so snapshots don't churn on version bumps.
fn normalize_codegen_output(code: &str) -> String {
    let from = format!(
        "// Generated by the Incan compiler v{}\n\n",
        incan::version::INCAN_VERSION
    );
    let to = "// Generated by the Incan compiler v<INCAN_VERSION>\n\n";
    code.replace(&from, to)
        .lines()
        .map(|line| {
            if line.starts_with("incan_stdlib::__incan_stdlib_version_check!(") {
                "incan_stdlib::__incan_stdlib_version_check!(\"<INCAN_STDLIB_VERSION>\");"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Load a test file from the codegen_snapshots directory
fn load_test_file(name: &str) -> String {
    let path = format!("tests/codegen_snapshots/{}.incn", name);
    let Ok(content) = fs::read_to_string(&path) else {
        panic!("Failed to read test file: {}", path);
    };
    content
}

#[test]
fn test_pub_import_expressions_codegen() {
    let source = load_test_file("pub_import_expressions");
    let rust_code = generate_rust_with_widgets_manifest(&source);
    insta::assert_snapshot!("pub_import_expressions", rust_code);
}

#[test]
fn test_pub_import_module_alias_codegen() {
    let source = load_test_file("pub_import_module_alias");
    let rust_code = generate_rust_with_widgets_manifest(&source);
    insta::assert_snapshot!("pub_import_module_alias", rust_code);
}

#[test]
fn test_vocab_block_desugaring_codegen() {
    let source = load_test_file("vocab_block_desugaring");
    let rust_code = generate_rust_with_vocab_wasm_desugaring(&source);
    insta::assert_snapshot!("vocab_block_desugaring", rust_code);
}

#[test]
fn test_basic_function_codegen() {
    let source = load_test_file("basic_function");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("basic_function", rust_code);
}

#[test]
fn test_dict_operations_codegen() {
    let source = load_test_file("dict_operations");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("dict_operations", rust_code);
}

#[test]
fn test_model_struct_codegen() {
    let source = load_test_file("model_struct");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("model_struct", rust_code);
}

#[test]
fn test_uppercase_var_field_access_codegen() {
    let source = load_test_file("uppercase_var_field_access");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("uppercase_var_field_access", rust_code);
}

#[test]
fn test_model_with_alias_codegen() {
    let source = load_test_file("model_with_alias");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("model_with_alias", rust_code);
}

#[test]
fn test_model_with_serde_alias_codegen() {
    let source = load_test_file("model_with_serde_alias");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("model_with_serde_alias", rust_code);
}

#[test]
fn test_model_alias_expressions_codegen() {
    // RFC 021: Test alias-aware expression lowering (constructor, field access, patterns)
    let source = load_test_file("model_alias_expressions");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("model_alias_expressions", rust_code);
}

#[test]
fn test_model_alias_self_access_codegen() {
    // RFC 021: Ensure `self.<alias>` field access lowers to canonical field name
    let source = load_test_file("model_alias_self_access");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("model_alias_self_access", rust_code);
}

#[test]
fn test_web_route_extractors_codegen() {
    let source = load_test_file("web_route_extractors");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("web_route_extractors", rust_code);
}

#[test]
fn test_web_route_extractors_nested_module_codegen() {
    let main_source = r#"
import std.async
import api::routes

def main() -> None:
  pass
"#;
    let routes_source = r#"
import std.async
from std.web import route, POST

@route("/things", methods=[POST])
async def create(id: int) -> int:
  return id

@route("/search")
async def search(id: int) -> int:
  return id
"#;

    let Ok(main_tokens) = lexer::lex(main_source) else {
        panic!("lexer failed")
    };
    let Ok(main_ast) = parser::parse(&main_tokens) else {
        panic!("parser failed")
    };
    let Ok(routes_tokens) = lexer::lex(routes_source) else {
        panic!("lexer failed")
    };
    let Ok(routes_ast) = parser::parse(&routes_tokens) else {
        panic!("parser failed")
    };

    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("api_routes", &routes_ast, vec!["api".to_string(), "routes".to_string()]);
    let Ok((main_code, _modules)) =
        codegen.try_generate_multi_file_nested(&main_ast, &[vec!["api".to_string(), "routes".to_string()]])
    else {
        panic!("codegen must succeed");
    };
    let rust_code = normalize_codegen_output(&main_code);
    insta::assert_snapshot!("web_route_extractors_nested_module", rust_code);
}

#[test]
fn test_async_main_runtime_bootstrap_codegen() {
    let source = r#"
import std.async

async def main() -> None:
  println("hello")
"#;
    let rust_code = generate_rust(source);
    insta::assert_snapshot!("async_main_runtime_bootstrap", rust_code);
}

// ============================================================================
// RFC 022: Codegen emits incan_stdlib handoff, not framework crate references
// ============================================================================

#[test]
fn test_web_route_codegen_no_framework_crate_leakage() {
    // RFC 022 requires that generated Rust for web programs references incan_stdlib::web::... but never directly
    // references framework crates like axum::, actix_web::, etc.
    let source = load_test_file("web_route_extractors");
    let rust_code = generate_rust(&source);

    // Must reference the stdlib handoff
    assert!(
        rust_code.contains("incan_stdlib"),
        "Generated web code should reference incan_stdlib"
    );
    assert!(
        rust_code.contains("incan_web_macros::route"),
        "Generated web code should use incan_web_macros::route passthrough"
    );

    // Must NOT directly reference framework crates
    assert!(
        !rust_code.contains("axum::"),
        "Generated web code should not directly reference axum::"
    );
    assert!(
        !rust_code.contains("actix_web::"),
        "Generated web code should not directly reference actix_web::"
    );
}

// ============================================================================
// Tests migrated from legacy codegen/expressions/mod.rs tests
// ============================================================================

#[test]
fn test_literals_codegen() {
    let source = load_test_file("literals");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("literals", rust_code);
}

#[test]
fn test_operators_codegen() {
    let source = load_test_file("operators");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("operators", rust_code);
}

#[test]
fn test_mixed_numeric_codegen() {
    let source = load_test_file("mixed_numeric");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("mixed_numeric", rust_code);
}

#[test]
fn test_function_calls_codegen() {
    let source = load_test_file("function_calls");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("function_calls", rust_code);
}

#[test]
fn test_collections_codegen() {
    let source = load_test_file("collections");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("collections", rust_code);
}

#[test]
fn test_lowercase_types_codegen() {
    let source = load_test_file("lowercase_types");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("lowercase_types", rust_code);
}

// ============================================================================
// Tests migrated from legacy codegen/statements/mod.rs tests
// ============================================================================

#[test]
fn test_assignments_codegen() {
    let source = load_test_file("assignments");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("assignments", rust_code);
}

#[test]
fn test_control_flow_codegen() {
    let source = load_test_file("control_flow");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("control_flow", rust_code);
}

#[test]
fn test_returns_codegen() {
    let source = load_test_file("returns");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("returns", rust_code);
}

#[test]
fn test_loops_codegen() {
    let source = load_test_file("loops");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("loops", rust_code);
}

#[test]
fn test_match_statements_codegen() {
    let source = load_test_file("match_statements");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("match_statements", rust_code);
}

#[test]
fn test_type_annotations_codegen() {
    let source = load_test_file("type_annotations");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("type_annotations", rust_code);
}

#[test]
fn test_string_operations_codegen() {
    let source = load_test_file("string_operations");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("string_operations", rust_code);
}

// ============================================================================
// Tests for declarations (functions, classes, models, traits, enums)
// ============================================================================

#[test]
fn test_functions_codegen() {
    let source = load_test_file("functions");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("functions", rust_code);
}

#[test]
fn test_classes_codegen() {
    let source = load_test_file("classes");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("classes", rust_code);
}

#[test]
fn test_models_codegen() {
    let source = load_test_file("models");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("models", rust_code);
}

#[test]
fn test_traits_codegen() {
    let source = load_test_file("traits");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("traits", rust_code);
}

#[test]
fn test_enums_codegen() {
    let source = load_test_file("enums");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("enums", rust_code);
}

// ============================================================================
// Additional migration tests
// ============================================================================

#[test]
fn test_patterns_codegen() {
    let source = load_test_file("patterns");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("patterns", rust_code);
}

#[test]
fn test_param_mut_unused_codegen() {
    let source = load_test_file("param_mut_unused");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("param_mut_unused", rust_code);
}

#[test]
fn test_imports_codegen() {
    let source = load_test_file("imports");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("imports", rust_code);
}

#[test]
fn test_builtins_codegen() {
    let source = load_test_file("builtins");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("builtins", rust_code);
}

#[test]
fn test_pub_const_codegen() {
    let source = load_test_file("pub_const");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("pub_const", rust_code);
}

#[test]
fn test_const_str_chain_codegen() {
    let source = load_test_file("const_str_chain");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("const_str_chain", rust_code);
}

#[test]
fn test_const_bytes_codegen() {
    let source = load_test_file("const_bytes");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("const_bytes", rust_code);
}

#[test]
fn test_inferred_reassign_codegen() {
    // Snapshot test to keep style consistent with this file.
    let source = load_test_file("inferred_reassign");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("inferred_reassign", rust_code);
}

#[test]
fn test_rust_interop_associated_functions_codegen() {
    let source = load_test_file("rust_interop_associated_functions");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_interop_associated_functions", rust_code);
}

#[test]
fn test_rust_interop_field_access_codegen() {
    let source = load_test_file("rust_interop_field_access");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_interop_field_access", rust_code);
}

#[test]
fn test_titlecase_var_not_type_codegen() {
    let source = load_test_file("titlecase_var_not_type");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("titlecase_var_not_type", rust_code);
}

// ============================================================================
// Construction semantics: defaults + newtype checked construction
// ============================================================================

#[test]
fn test_constructor_field_defaults_codegen() {
    let source = load_test_file("constructor_field_defaults");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("constructor_field_defaults", rust_code);
}

#[test]
fn test_newtype_checked_construction_codegen() {
    let source = load_test_file("newtype_checked_construction");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_checked_construction", rust_code);
}

#[test]
fn test_newtype_builder_methods_codegen() {
    let source = load_test_file("newtype_builder_methods");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_builder_methods", rust_code);
}

#[test]
fn test_newtype_with_override_codegen() {
    let source = load_test_file("newtype_with_override");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_with_override", rust_code);
}

#[test]
fn test_newtype_axum_response_codegen() {
    let source = load_test_file("newtype_axum_response");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_axum_response", rust_code);
}

#[test]
fn test_newtype_generic_json_codegen() {
    let source = load_test_file("newtype_generic_json");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_generic_json", rust_code);
}

#[test]
fn test_newtype_generic_simple_codegen() {
    let source = load_test_file("newtype_generic_simple");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_generic_simple", rust_code);
}

#[test]
fn test_newtype_generic_builder_methods_codegen() {
    let source = load_test_file("newtype_generic_builder_methods");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_generic_builder_methods", rust_code);
}

#[test]
fn test_newtype_web_response_codegen() {
    let source = load_test_file("newtype_web_response");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("newtype_web_response", rust_code);
}

// ============================================================================
/// RFC 023: `rust.module()` + `@rust.extern` delegation codegen.
// ============================================================================
///
/// Verifies that `@rust.extern` functions emit delegation calls to the declared Rust module path, while pure Incan
/// functions in the same module compile normally.
#[test]
fn test_rust_extern_delegation_codegen() {
    let source = load_test_file("rust_extern_delegation");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_extern_delegation", rust_code);
}

/// RFC 023 Phase 5: compile the real `std.testing` module source.
#[test]
fn test_std_testing_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/testing.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_testing_compiled", rust_code);
}

// ============================================================================
// RFC 023: Compile std.derives.* trait definitions from Incan source
// ============================================================================

/// compile `std.derives.comparison` (Eq, Ord, Hash) from `.incn` source.
///
/// Verifies that trait declarations with `@rust.extern` abstract methods and pure-Incan default methods
/// (`__ne__`, `__le__`, `__gt__`, `__ge__`) compile through the full pipeline.
#[test]
fn test_std_derives_comparison_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/comparison.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_derives_comparison_compiled", rust_code);
}

/// compile `std.derives.copying` (Clone, Copy, Default) from `.incn` source.
#[test]
fn test_std_derives_copying_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/copying.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_derives_copying_compiled", rust_code);
}

/// compile `std.derives.string` (Debug, Display) from `.incn` source.
#[test]
fn test_std_derives_string_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/string.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_derives_string_compiled", rust_code);
}

/// RFC 023: compile `std.serde.json` (Serialize, Deserialize) from `.incn` source.
///
/// Verifies that trait declarations with `@rust.extern` methods compile through the full pipeline when serde namespace
/// is in IncanSource mode.
#[test]
fn test_std_serde_json_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/serde/json.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_serde_json_compiled", rust_code);
}

/// RFC 023: verify `from std.serde.json import Serialize, Deserialize` resolves and compiles.
///
/// Exercises the stdlib import path for serde traits alongside @derive(Serialize, Deserialize).
#[test]
fn test_std_serde_json_import_codegen() {
    let source = load_test_file("std_serde_json_import");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_serde_json_import", rust_code);
}

// ============================================================================
/// Issue #145: Full surface-semantics path for `assert` statements.
// ============================================================================
///
/// Exercises: parser `Statement::Surface` -> typechecker -> lowering to `IrExprKind::Call` with `canonical_path` ->
/// emission via `emit_canonical_callee_path()`.
#[test]
fn test_assert_surface_codegen() {
    let source = load_test_file("assert_surface");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("assert_surface", rust_code);
}

// ============================================================================
// RFC 023: Trait Bound Inference and `with` Annotation
// ============================================================================

/// RFC 023: Inferred trait bounds from usage (`==`/`!=` -> PartialEq, f-string -> Display, etc.)
#[test]
fn test_trait_bound_inference_codegen() {
    let source = load_test_file("trait_bound_inference");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("trait_bound_inference", rust_code);
}

/// RFC 023: Explicit `with` bounds on type parameters.
#[test]
fn test_trait_bound_explicit_codegen() {
    let source = load_test_file("trait_bound_explicit");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("trait_bound_explicit", rust_code);
}

/// RFC 023: Additional inference cases (Display, Dict key hashing, arithmetic, transitive propagation).
#[test]
fn test_trait_bound_inference_more_codegen() {
    let source = load_test_file("trait_bound_inference_more");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("trait_bound_inference_more", rust_code);
}

// Glob-based test that auto-discovers all .incn files
// To enable: uncomment the test below and run `cargo test --test codegen_snapshot_tests`
//
// #[test]
// fn test_all_codegen_snapshots() {
//     insta::glob!("codegen_snapshots/*.incn", |path| {
//         let source = fs::read_to_string(path).expect("failed to read file");
//         let rust_code = generate_rust(&source);
//         let name = path.file_stem().unwrap().to_string_lossy();
//         insta::assert_snapshot!(name.to_string(), rust_code);
//     });
// }
