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
    let code = match IrCodegen::new().try_generate(&ast) {
        Ok(code) => code,
        Err(e) => panic!("codegen snapshot inputs must typecheck: {e:?}"),
    };
    normalize_codegen_output(&code)
}

fn parse_incan_program(source: &str, context: &str) -> incan::frontend::ast::Program {
    let tokens = lexer::lex(source).unwrap_or_else(|errs| panic!("{context} lexer failed: {errs:?}"));
    parser::parse(&tokens).unwrap_or_else(|errs| panic!("{context} parser failed: {errs:?}"))
}

/// Generate Rust code from Incan source with a populated library index
fn generate_rust_with_widgets_manifest(source: &str) -> String {
    use incan::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan::library_manifest::{
        ConstExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, ParamKindExport, StaticExport, TypeRef,
    };
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
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
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
            kind: ParamKindExport::Normal,
            has_default: false,
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
    manifest.exports.statics.push(StaticExport {
        name: "SHARED_COUNT".to_string(),
        ty: TypeRef::Named {
            name: "int".to_string(),
        },
    });
    manifest.exports.statics.push(StaticExport {
        name: "SHARED_ITEMS".to_string(),
        ty: TypeRef::Applied {
            name: "list".to_string(),
            args: vec![TypeRef::Named {
                name: "int".to_string(),
            }],
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

fn generate_rust_with_substrait_probe(source: &str) -> String {
    let tmp = match tempfile::tempdir() {
        Ok(tmp) => tmp,
        Err(err) => panic!("failed to create substrait probe tempdir: {err}"),
    };
    let root = tmp.path();
    if let Err(err) = fs::create_dir_all(root.join("src")) {
        panic!("failed to create probe src dir: {err}");
    }
    if let Err(err) = fs::create_dir_all(root.join("substrait").join("src")) {
        panic!("failed to create substrait src dir: {err}");
    }
    if let Err(err) = fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "ra_substrait_probe"
version = "0.1.0"
edition = "2021"

[dependencies]
substrait = { path = "substrait" }
"#,
    ) {
        panic!("failed to write probe Cargo.toml: {err}");
    }
    if let Err(err) = fs::write(
        root.join("src/lib.rs"),
        "pub fn touch() { let _ = substrait::proto::PlanRel; }\n",
    ) {
        panic!("failed to write probe lib.rs: {err}");
    }
    if let Err(err) = fs::write(
        root.join("substrait").join("Cargo.toml"),
        r#"[package]
name = "substrait"
version = "0.63.0"
edition = "2021"
"#,
    ) {
        panic!("failed to write substrait Cargo.toml: {err}");
    }
    if let Err(err) = fs::write(
        root.join("substrait").join("src/lib.rs"),
        r#"pub mod proto {
    pub struct PlanRel;

    pub struct Rel {
        pub rel_type: std::option::Option<rel::RelType>,
    }

    pub struct ReadRel;

    pub mod rel {
        pub enum RelType {
            Read(Box<super::ReadRel>),
        }
    }
}
"#,
    ) {
        panic!("failed to write substrait lib.rs: {err}");
    }

    let Ok(tokens) = lexer::lex(source) else {
        panic!("lexer failed");
    };
    let Ok(ast) = parser::parse(&tokens) else {
        panic!("parser failed");
    };
    let mut codegen = IrCodegen::new();
    codegen.set_rust_inspect_manifest_dir(root.to_path_buf());
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
    let wat_bytes_string = |bytes: &[u8]| {
        let mut escaped = String::new();
        for byte in bytes {
            escaped.push('\\');
            escaped.push_str(&format!("{byte:02x}"));
        }
        escaped
    };
    let wat_i32_cell = |value: i32| wat_bytes_string(&value.to_le_bytes());

    let output_ptr_cell = 0usize;
    let output_len_cell = 4usize;
    let error_ptr_cell = 8usize;
    let error_len_cell = 12usize;
    let input_ptr_cell = 16usize;
    let input_capacity_cell = 20usize;
    let input_len_cell = 24usize;
    let output_offset = 128usize;
    let error_offset = 256usize;
    let input_offset = 384usize;
    let input_capacity = 4096usize;
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{out_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    (i32.const 0)
  )
)"#,
        input_ptr_cell = input_ptr_cell,
        input_capacity_cell = input_capacity_cell,
        input_len_cell = input_len_cell,
        output_ptr_cell = output_ptr_cell,
        output_len_cell = output_len_cell,
        error_ptr_cell = error_ptr_cell,
        error_len_cell = error_len_cell,
        output_ptr_data = wat_i32_cell(output_offset as i32),
        output_len_data = wat_i32_cell(output_payload.len() as i32),
        error_ptr_data = wat_i32_cell(error_offset as i32),
        error_len_data = wat_i32_cell(0),
        input_ptr_data = wat_i32_cell(input_offset as i32),
        input_capacity_data = wat_i32_cell(input_capacity as i32),
        input_len_data = wat_i32_cell(0),
        output_offset = output_offset,
        out_data = wat_bytes_string(output_payload.as_bytes()),
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
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
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

/// Generate Rust from source desugared through a helper-backed vocab WASM artifact.
fn generate_rust_with_helper_backed_vocab_wasm_desugaring(source: &str) -> String {
    use incan::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan::frontend::vocab_desugar_pass::desugar_program_vocab_blocks;
    use incan::library_manifest::{
        FunctionExport, LibraryManifest, ParamExport, ParamKindExport, TypeRef, VocabDesugarerArtifact, VocabExports,
    };
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
        callee: Box::new(incan_vocab::IncanExpr::Helper("filter".to_string())),
        args: vec![incan_vocab::IncanExpr::Int(1)],
    });
    let output_payload = match serde_json::to_string(&response) {
        Ok(payload) => payload,
        Err(err) => panic!("failed to serialize desugar response: {err}"),
    };
    let wat_bytes_string = |bytes: &[u8]| {
        let mut escaped = String::new();
        for byte in bytes {
            escaped.push('\\');
            escaped.push_str(&format!("{byte:02x}"));
        }
        escaped
    };
    let wat_i32_cell = |value: i32| wat_bytes_string(&value.to_le_bytes());

    let output_ptr_cell = 0usize;
    let output_len_cell = 4usize;
    let error_ptr_cell = 8usize;
    let error_len_cell = 12usize;
    let input_ptr_cell = 16usize;
    let input_capacity_cell = 20usize;
    let input_len_cell = 24usize;
    let output_offset = 128usize;
    let error_offset = 256usize;
    let input_offset = 384usize;
    let input_capacity = 4096usize;
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{out_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    (i32.const 0)
  )
)"#,
        input_ptr_cell = input_ptr_cell,
        input_capacity_cell = input_capacity_cell,
        input_len_cell = input_len_cell,
        output_ptr_cell = output_ptr_cell,
        output_len_cell = output_len_cell,
        error_ptr_cell = error_ptr_cell,
        error_len_cell = error_len_cell,
        output_ptr_data = wat_i32_cell(output_offset as i32),
        output_len_data = wat_i32_cell(output_payload.len() as i32),
        error_ptr_data = wat_i32_cell(error_offset as i32),
        error_len_data = wat_i32_cell(0),
        input_ptr_data = wat_i32_cell(input_offset as i32),
        input_capacity_data = wat_i32_cell(input_capacity as i32),
        input_len_data = wat_i32_cell(0),
        output_offset = output_offset,
        out_data = wat_bytes_string(output_payload.as_bytes()),
    );
    let wasm_bytes = match wat::parse_str(wat_source) {
        Ok(bytes) => bytes,
        Err(err) => panic!("failed to compile wat: {err}"),
    };

    let mut artifact_root = std::env::temp_dir();
    artifact_root.push("incan_test_vocab_helper_artifacts");
    artifact_root.push("target");
    artifact_root.push("lib");
    let desugarer_dir = artifact_root.join("desugarers");
    if let Err(err) = std::fs::create_dir_all(&desugarer_dir) {
        panic!("failed to create desugarer artifact dir: {err}");
    }
    let desugarer_path = desugarer_dir.join("query_desugarer.wasm");
    if let Err(err) = std::fs::write(&desugarer_path, &wasm_bytes) {
        panic!("failed to write desugarer artifact: {err}");
    }
    if let Err(err) = std::fs::create_dir_all(artifact_root.join("src")) {
        panic!("failed to create crate src dir: {err}");
    }
    if let Err(err) = std::fs::write(
        artifact_root.join("Cargo.toml"),
        "[package]\nname = \"query_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ) {
        panic!("failed to write Cargo.toml: {err}");
    }
    if let Err(err) = std::fs::write(
        artifact_root.join("src/lib.rs"),
        "pub fn filter(value: i64) -> i64 { value }\n",
    ) {
        panic!("failed to write lib.rs: {err}");
    }

    let mut manifest = LibraryManifest::new("query_core", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "filter".to_string(),
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Named {
                name: "int".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: false,
        }],
        return_type: TypeRef::Named {
            name: "int".to_string(),
        },
        is_async: false,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "query.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "where".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/query_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(&wasm_bytes)),
        }),
    });

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "query".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("query", "query_core", artifact_root),
        },
    )]));
    let imported_vocab = index.library_imported_vocab();

    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(errs) => panic!("lexer failed: {errs:?}"),
    };
    let mut ast = match parser::parse_with_context(
        &tokens,
        Some("tests/codegen_snapshots/vocab_helper_backed_desugaring.incn"),
        Some(&imported_vocab),
    ) {
        Ok(ast) => ast,
        Err(errs) => panic!("parser failed: {errs:?}"),
    };
    if let Err(errs) = desugar_program_vocab_blocks(
        &mut ast,
        Some("tests/codegen_snapshots/vocab_helper_backed_desugaring.incn"),
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
fn test_vocab_helper_backed_desugaring_codegen() {
    let source = "import pub::query\n\ndef main() -> None:\n  where true:\n    pass\n";
    let rust_code = generate_rust_with_helper_backed_vocab_wasm_desugaring(source);
    insta::assert_snapshot!("vocab_helper_backed_desugaring", rust_code);
}

#[test]
fn test_basic_function_codegen() {
    let source = load_test_file("basic_function");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("basic_function", rust_code);
}

#[test]
fn test_function_references_codegen() {
    let source = load_test_file("function_references");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("function_references", rust_code);
}

#[test]
fn test_user_defined_decorators_codegen() {
    let source = load_test_file("user_defined_decorators");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("user_defined_decorators", rust_code);
}

#[test]
fn test_user_defined_method_decorators_codegen() {
    let source = load_test_file("user_defined_method_decorators");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("user_defined_method_decorators", rust_code);
}

#[test]
fn test_user_defined_mutable_method_decorators_codegen() {
    let source = load_test_file("user_defined_mutable_method_decorators");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("user_defined_mutable_method_decorators", rust_code);
}

#[test]
fn test_rfc070_result_combinators_codegen() {
    let source = r#"
def double(value: int) -> int:
  return value * 2

def keep_positive(value: int) -> Result[int, str]:
  if value > 0:
    return Ok(value)
  return Err("not positive")

def observe_int(_value: int) -> None:
  pass

from std.traits.callable import Callable1

model Observer with Callable1[int, None]:
  def __call__(self, value: int) -> None:
    pass

def main(result: Result[int, str]) -> Result[int, str]:
  observer = Observer()
  return result.map(double).and_then(keep_positive).inspect(observe_int).inspect(observer)
"#;
    let rust_code = generate_rust(source);
    assert!(
        rust_code.contains("crate::__incan_std::result::map(result, double)"),
        "map with a named function callback should dogfood the std.result helper:\n{rust_code}"
    );
    assert!(
        rust_code.contains("crate::__incan_std::result::and_then"),
        "and_then with a named function callback should dogfood the std.result helper:\n{rust_code}"
    );
    assert!(
        rust_code.contains("crate::__incan_std::result::inspect"),
        "inspect with a named function callback should dogfood the std.result helper:\n{rust_code}"
    );
    assert!(
        rust_code.contains("observe_int"),
        "inspect should pass Copy named observers through the std.result helper without cloning:\n{rust_code}"
    );
    assert!(
        rust_code.contains(".inspect(|__incan_result_value|"),
        "callable-object inspect should use Rust's borrowed Result observer surface:\n{rust_code}"
    );
    assert!(
        rust_code.contains("observer.__call__(*__incan_result_value)"),
        "callable objects should route through __call__ inside Result combinators:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("clone()"),
        "Copy observer adaptation should not introduce clone calls:\n{rust_code}"
    );
}

#[test]
fn test_rfc070_result_inspect_non_copy_observer_borrows_payload() {
    let source = r#"
model Payload:
  name: str

def observe_payload(_payload: Payload) -> None:
  pass

from std.traits.callable import Callable1

model PayloadObserver with Callable1[Payload, None]:
  def __call__(self, _payload: Payload) -> None:
    pass

pub def transform(result: Result[Payload, str]) -> Result[Payload, str]:
  return result.inspect(observe_payload)

pub def transform_with_observer(result: Result[Payload, str]) -> Result[Payload, str]:
  observer = PayloadObserver()
  return result.inspect(observer)
"#;
    let rust_code = generate_rust(source);
    assert!(
        rust_code.contains("fn __incan_borrow_adapter_observe_payload_0(_: &Payload)"),
        "non-Copy named observer callbacks should get a generated borrowed function adapter:\n{rust_code}"
    );
    assert!(
        rust_code.contains("crate::__incan_std::result::inspect(")
            && rust_code.contains("__incan_borrow_adapter_observe_payload_0"),
        "inspect should pass the borrowed adapter into the Incan-authored std.result helper:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_result_observer_borrow_observe_payload"),
        "named function observers should use the generic borrowed adapter, not the old Result-specific helper:\n{rust_code}"
    );
    assert!(
        rust_code.contains("fn __incan_result_observer_borrow___call__(&self, _: &Payload)"),
        "non-Copy callable observers should get a generated borrowed __call__ helper:\n{rust_code}"
    );
    assert_eq!(
        rust_code.matches("fn __incan_result_observer_borrow___call__").count(),
        1,
        "callable-object borrowed observer helper should be emitted once:\n{rust_code}"
    );
    assert!(
        rust_code.contains("observer.__incan_result_observer_borrow___call__(__incan_result_value)"),
        "inspect should route non-Copy callable objects through the borrowed __call__ helper:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_result_value).clone()"),
        "non-Copy inspect observers must not clone the payload:\n{rust_code}"
    );
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
fn test_std_web_routing_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/web/routing.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("incan_stdlib::errors::__private::raise_runtime_misuse"),
        "proc-macro decorator runtime misuse should route through a named helper:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("panic!(\"decorator marker"),
        "proc-macro decorator runtime misuse must not emit raw panic!:\n{rust_code}"
    );
    insta::assert_snapshot!("std_web_routing_compiled", rust_code);
}

#[test]
fn imported_stdlib_static_method_defaults_expand_at_call_site_issue500() {
    let source = r#"
from std.web import App

def main() -> None:
  App.run()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("App::run(\"127.0.0.1\".to_string(),8080)"),
        "imported stdlib static method call should expand omitted defaults:\n{rust_code}"
    );
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
fn test_web_route_private_nested_module_codegen() {
    let main_source = r#"
import std.async
import api::routes
from std.web import App

def main() -> None:
  App.run(host="127.0.0.1", port=0)
"#;
    let routes_source = r#"
import std.async
from std.web import route, Json
from std.serde import json

@derive(json)
model User:
  id: int
  name: str

@route("/users/{id}")
async def list_user(id: int) -> Json[User]:
  return Json(User(id=id, name="Ada"))
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

    let routes_path = vec!["api".to_string(), "routes".to_string()];
    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(false);
    codegen.add_module_with_path_segments("api_routes", &routes_ast, routes_path.clone());
    let Ok((main_code, modules)) =
        codegen.try_generate_multi_file_nested(&main_ast, std::slice::from_ref(&routes_path))
    else {
        panic!("codegen must succeed");
    };
    let Some(routes_code) = modules.get(&routes_path) else {
        panic!("routes module should be emitted");
    };
    let main_code = normalize_codegen_output(&main_code);
    let routes_code = normalize_codegen_output(routes_code);

    assert!(
        routes_code.contains("#[incan_web_macros::route(\"/users/{id}\")]"),
        "route proc-macro attribute should be retained in dependency module:\n{routes_code}"
    );
    assert!(
        routes_code.contains("struct User"),
        "private response model should be retained in dependency module:\n{routes_code}"
    );
    assert!(
        !routes_code.contains("pub struct User"),
        "route response model should not be forced public:\n{routes_code}"
    );
    assert!(
        routes_code.contains("async fn list_user"),
        "private route handler should be retained in dependency module:\n{routes_code}"
    );
    assert!(
        !routes_code.contains("pub async fn list_user"),
        "route handler should not be forced public:\n{routes_code}"
    );
    assert!(
        !main_code.contains("api::routes::list_user"),
        "main module should not call dependency route handler directly:\n{main_code}"
    );
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
fn test_user_defined_operators_codegen() {
    let source = load_test_file("user_defined_operators");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("user_defined_operators", rust_code);
}

#[test]
fn test_rfc068_protocol_hooks_lower_to_method_calls() {
    let source = r#"
model Flag:
  ready: bool

  def __bool__(self) -> bool:
    return self.ready

model Bag:
  size: int

  def __len__(self) -> int:
    return self.size

  def __contains__(self, item: int) -> bool:
    return item == self.size

model CallableBox:
  seed: int

  def __call__(self, value: int) -> int:
    return self.seed + value

model CounterIter:
  def __next__(self) -> Option[int]:
    return None

model Counter:
  def __iter__(self) -> CounterIter:
    return CounterIter()

def main() -> None:
  flag = Flag(ready=true)
  bag = Bag(size=3)
  callable = CallableBox(seed=4)
  if flag:
    pass
  n = len(bag)
  present = 3 in bag
  called = callable(5)
  for item in Counter():
    seen = item
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    for expected in [
        "flag.__bool__()",
        "bag.__len__()",
        "bag.__contains__(3)",
        "callable.__call__(5)",
        "Counter{}.__iter__()",
        ".__next__()",
    ] {
        assert!(
            compact.contains(expected),
            "expected generated protocol hook call {expected}; generated:\n{rust_code}"
        );
    }
}

#[test]
fn test_mixed_numeric_codegen() {
    let source = load_test_file("mixed_numeric");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("mixed_numeric", rust_code);
}

#[test]
fn test_std_math_codegen() {
    let source = load_test_file("std_math");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_math", rust_code);
}

#[test]
fn test_std_fs_import_codegen() {
    let source = load_test_file("std_fs_import");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("pub use crate::__incan_std::fs::Path;"),
        "std.fs Path import should emit through the generated stdlib fs module; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_std::web::Path"),
        "std.fs Path must not reuse the std.web Path extractor path; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_fs_import", rust_code);
}

#[test]
fn test_std_tempfile_import_codegen() {
    let source = load_test_file("std_tempfile_import");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::NamedTemporaryFile;"),
        "std.tempfile NamedTemporaryFile import should emit through the generated stdlib tempfile module; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::TemporaryDirectory;"),
        "std.tempfile TemporaryDirectory import should emit through the generated stdlib tempfile module; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::SpooledTemporaryFile;"),
        "std.tempfile SpooledTemporaryFile import should emit through the generated stdlib tempfile module; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::fs::Path;"),
        "std.tempfile call sites should still use std.fs Path for path values; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_std::web::Path"),
        "std.tempfile must not reuse the std.web Path extractor path; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_tempfile_import", rust_code);
}

#[test]
fn test_function_calls_codegen() {
    let source = load_test_file("function_calls");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("function_calls", rust_code);
}

#[test]
fn test_variadic_calls_codegen() {
    let source = load_test_file("variadic_calls");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("variadic_calls", rust_code);
}

#[test]
fn test_collections_codegen() {
    let source = load_test_file("collections");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("collections", rust_code);
    assert!(
        rust_code.contains("(1, \"one\".to_string())"),
        "expected tuple[str] literal elements to materialize owned String values"
    );
    assert!(
        rust_code.contains("(\"a\".to_string(), 1)"),
        "expected dict[str, _] literal keys to materialize owned String values"
    );
    assert!(
        rust_code.contains("(2, \"two\".to_string())"),
        "expected dict[_, str] literal values to materialize owned String values"
    );
}

#[test]
fn test_list_repeat_codegen() {
    let source = load_test_file("list_repeat");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("list_repeat", rust_code);
}

#[test]
fn test_rfc088_iterator_adapters_codegen() {
    let source = load_test_file("rfc088_iterator_adapters");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc088_iterator_adapters", rust_code);
}

#[test]
fn test_empty_list_string_arg_codegen() {
    let source = load_test_file("empty_list_string_arg");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("empty_list_string_arg", rust_code);
}

#[test]
fn test_generic_model_field_access_codegen() {
    let source = load_test_file("generic_model_field_access");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("generic_model_field_access", rust_code);
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
fn test_pattern_alternation_codegen() {
    let source = load_test_file("pattern_alternation");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("pattern_alternation", rust_code);
}

#[test]
fn test_rfc049_if_let_while_let_codegen() {
    let source = load_test_file("rfc049_if_let_while_let");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc049_if_let_while_let", rust_code);
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
fn test_issue492_rust_result_match_does_not_clone_scrutinee() {
    let rust_code = generate_rust(
        r#"
from rust::std::fs import read_dir
from rust::std::path import Path as RustPath

def main() -> None:
    match read_dir(RustPath.new(".")):
        Ok(entries) =>
            for entry_result in entries:
                match entry_result:
                    Ok(entry) => println(entry.path().to_string_lossy().into_owned())
                    Err(err) => println(err.to_string())
        Err(err) => println(err.to_string())
"#,
    );

    assert!(
        !rust_code.contains("entry_result.clone()"),
        "Rust Result match scrutinee must not be cloned for non-Clone payloads:\n{rust_code}"
    );
}

#[test]
fn test_type_annotations_codegen() {
    let source = load_test_file("type_annotations");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("type_annotations", rust_code);
}

#[test]
fn test_rfc029_union_types_codegen() {
    let source = load_test_file("rfc029_union_types");
    let rust_code = generate_rust(&source);
    assert!(
        !rust_code.contains("isinstance("),
        "union isinstance chains must fully lower before Rust emission:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc029_union_types", rust_code);
}

#[test]
fn test_issue501_option_union_isinstance_codegen_no_raw_call() {
    let rust_code = generate_rust(
        r#"
@derive(Clone)
type LocalPath = newtype str

pub def describe(value: Option[LocalPath | str]) -> str:
  if value is not None:
    if isinstance(value, str):
      return value.upper()
    elif isinstance(value, LocalPath):
      return value.0
  return "missing"
"#,
    );

    assert!(
        !rust_code.contains("isinstance("),
        "Option[Union] isinstance narrowing must fully lower before Rust emission:\n{rust_code}"
    );
}

#[test]
fn test_issue502_exhaustive_independent_isinstance_branches_codegen_has_no_unit_fallback() {
    let rust_code = generate_rust(
        r#"
@derive(Clone)
type LocalPath = newtype str

pub def normalize_path_like(value: LocalPath | str) -> LocalPath:
  if isinstance(value, str):
    return LocalPath(value)
  if isinstance(value, LocalPath):
    return value
"#,
    );

    assert!(
        !rust_code.contains("=> {}"),
        "exhaustive independent isinstance branches must not emit unit fallthrough arms:\n{rust_code}"
    );
}

#[test]
fn test_issue457_461_cross_module_union_codegen_uses_crate_wrapper() {
    let main_source = r#"
from producers import parse_value
from consumers import describe

def main() -> None:
  println(describe(parse_value(False)))
  println(describe("literal"))
"#;
    let producers_source = r#"
pub def parse_value(flag: bool) -> int | str:
  if flag:
    return 1
  return "fallback"
"#;
    let consumers_source = r#"
pub def describe(value: int | str) -> str:
  if isinstance(value, int):
    return "number"
  else:
    return value.upper()
"#;

    let Ok(main_tokens) = lexer::lex(main_source) else {
        panic!("lexer failed")
    };
    let Ok(main_ast) = parser::parse(&main_tokens) else {
        panic!("parser failed")
    };
    let Ok(producers_tokens) = lexer::lex(producers_source) else {
        panic!("lexer failed")
    };
    let Ok(producers_ast) = parser::parse(&producers_tokens) else {
        panic!("parser failed")
    };
    let Ok(consumers_tokens) = lexer::lex(consumers_source) else {
        panic!("lexer failed")
    };
    let Ok(consumers_ast) = parser::parse(&consumers_tokens) else {
        panic!("parser failed")
    };

    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("producers", &producers_ast, vec!["producers".to_string()]);
    codegen.add_module_with_path_segments("consumers", &consumers_ast, vec!["consumers".to_string()]);
    let (main_code, modules) = codegen
        .try_generate_multi_file_nested(
            &main_ast,
            &[vec!["producers".to_string()], vec!["consumers".to_string()]],
        )
        .unwrap_or_else(|err| panic!("codegen must succeed: {err:?}"));
    let main_code = normalize_codegen_output(&main_code);
    let Some(producers_module) = modules.get(&vec!["producers".to_string()]) else {
        panic!("missing producers module");
    };
    let producers_code = normalize_codegen_output(producers_module);
    let Some(consumers_module) = modules.get(&vec!["consumers".to_string()]) else {
        panic!("missing consumers module");
    };
    let consumers_code = normalize_codegen_output(consumers_module);

    assert!(
        main_code.contains("pub enum __IncanUnion"),
        "root module should own generated ordinary union wrappers:\n{main_code}"
    );
    assert!(
        main_code.contains("describe(parse_value(false))"),
        "same-shaped union forwarding should not need an adapter at source level:\n{main_code}"
    );
    assert!(
        main_code.contains("describe(crate ::__IncanUnion"),
        "literal calls to imported union-typed functions should use the root wrapper:\n{main_code}"
    );
    assert!(
        producers_code.contains("-> crate::__IncanUnion"),
        "producer module signatures should refer to the crate-level wrapper:\n{producers_code}"
    );
    assert!(
        consumers_code.contains("value: crate::__IncanUnion"),
        "consumer module signatures should refer to the crate-level wrapper:\n{consumers_code}"
    );
    assert!(
        !producers_code.contains("pub enum __IncanUnion") && !consumers_code.contains("pub enum __IncanUnion"),
        "dependency modules must not emit nominally distinct local union wrappers:\nproducers:\n{producers_code}\nconsumers:\n{consumers_code}"
    );
}

#[test]
fn test_string_operations_codegen() {
    let source = load_test_file("string_operations");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("string_operations", rust_code);
}

#[test]
fn test_issue236_non_string_join_codegen() {
    let source = load_test_file("issue236_non_string_join");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue236_non_string_join", rust_code);
}

/// Issue #244: recursive call with `mut` list args inside `while` must not emit `.clone()` for those args (snapshot is
/// the contract).
#[test]
fn test_issue244_recursive_mut_list_codegen() {
    let source = load_test_file("issue244_recursive_mut_list");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue244_recursive_mut_list", rust_code);
}

/// Issue #244 regression: mutable `str` params are passed by `&mut` and keep string conversions.
#[test]
fn test_issue244_mut_str_param_codegen() {
    let source = load_test_file("issue244_mut_str_param");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue244_mut_str_param", rust_code);
}

/// Issue #241: field-backed values passed to by-value methods must clone via the ownership planner.
#[test]
fn test_issue241_field_backed_method_arg_clone_codegen() {
    let source = load_test_file("issue241_field_backed_method_arg_clone");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("self._cursor.join(other._cursor.clone(),true)"),
        "expected field-backed by-value method arg to clone through planner-owned call lowering; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("self._cursor.join(&other._cursor,true)"),
        "unexpected borrowed field-backed method arg for by-value call; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue241_field_backed_method_arg_clone", rust_code);
}

/// Issue #364: filtered list comprehensions over non-Copy values must not destructure `&item` in `filter(...)`.
#[test]
fn test_issue364_filtered_list_comp_borrow_codegen() {
    let source = load_test_file("issue364_filtered_list_comp_borrow");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains(".iter().filter_map(|stored|{letstored=(*stored).clone();ifstored.store_id_raw==store_id{Some(stored.node)}else{None}})"),
        "expected filtered list comprehension to clone inside filter_map for non-Copy items; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains(".filter(|&stored|"),
        "filtered list comprehension must not destructure `&stored`; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue364_filtered_list_comp_borrow", rust_code);
}

#[test]
fn test_rfc006_generator_expression_codegen() {
    let source = load_test_file("rfc006_generator_expression");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("incan_stdlib::iter::Generator::new"),
        "expected generator expression to construct stdlib Generator; generated:\n{rust_code}"
    );
    assert!(
        compact.contains(".flat_map(move|x|"),
        "expected nested generator expression to preserve second for-clause lazily; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("ifx>0") && compact.contains("ify>x") && compact.contains("std::iter::empty()"),
        "expected generator expression filters to stay lazy inside the iterator chain; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("u64::try_from(3)"),
        "generator take() must keep its i64 argument instead of using Rust Read::take u64 coercion; generated:\n{rust_code}"
    );
}

#[test]
fn test_rfc006_generator_function_yield_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def numbers() -> Generator[int]:
  yield 1

def main() -> None:
  values = numbers().collect()
  println(values[0])
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lexer failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parser failed: {errs:?}")))?;

    let rust_code = normalize_codegen_output(
        &IrCodegen::new()
            .try_generate(&ast)
            .map_err(|err| std::io::Error::other(format!("generator function codegen failed: {err:?}")))?,
    );
    assert!(
        rust_code.contains("incan_stdlib::iter::Generator::spawn"),
        "expected generator function to use runtime generator spawn; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("__incan_yield.yield_value(1)"),
        "expected yield statement to send generator item; generated:\n{rust_code}"
    );
    Ok(())
}

/// Issue #366: struct fields initialized from `self.<owned_field>` inside `clone(self) -> Self` must clone the field.
#[test]
fn test_issue366_clone_self_string_field_codegen() {
    let source = load_test_file("issue366_clone_self_string_field");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("logical_name:self.logical_name.clone()"),
        "expected clone(self)->Self struct field emission to clone borrowed string fields; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("logical_name:self.logical_name,"),
        "unexpected raw move from borrowed self field in clone(self)->Self emission; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue366_clone_self_string_field", rust_code);
}

/// Filtered dict comprehensions over borrowed iterables must own the item before evaluating the predicate.
#[test]
fn test_filtered_dict_comp_predicate_codegen() {
    let source = load_test_file("filtered_dict_comp_predicate");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains(
            ".iter().filter_map(|x|{letx=*x;ifincan_stdlib::num::py_mod_i64(x,2)==0{Some((x,x*x))}else{None}})"
        ),
        "expected filtered dict comprehension over Copy items to copy inside filter_map before evaluating the predicate; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains(".filter(|x|incan_stdlib::num::py_mod_i64(x,2)==0)"),
        "filtered dict comprehension must not leave the predicate closure borrowing `x`; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("letx=(*x).clone()"),
        "filtered dict comprehension over Copy items should not call clone; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("filtered_dict_comp_predicate", rust_code);
}

/// Issue #602: comprehensions over Copy item types should use copied values rather than `.clone()` hot paths.
#[test]
fn test_issue602_comprehension_copy_hotpaths_codegen() {
    let source = load_test_file("issue602_comprehension_copy_hotpaths");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("(xs).iter().copied().map(|x|x*x).collect::<Vec<_>>()"),
        "expected unfiltered Copy list comprehension to use copied(), generated:\n{rust_code}"
    );
    assert!(
        compact.contains(".iter().filter_map(|x|{letx=*x;ifx>0{Some(x*x)}else{None}})"),
        "expected filtered Copy list comprehension to copy the borrowed item without clone; generated:\n{rust_code}"
    );
    assert!(
        compact.contains(".iter().filter_map(|x|{letx=*x;ifx>0{Some((x,x*x))}else{None}})"),
        "expected filtered Copy dict comprehension to copy the borrowed item without clone; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("(*x).clone()") && !compact.contains(".iter().cloned().map(|x|x*x)"),
        "Copy comprehension hot paths should not emit clone calls; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue602_comprehension_copy_hotpaths", rust_code);
}

#[test]
fn test_issue602_owned_iterator_source_hotpaths_codegen() {
    let source = load_test_file("issue602_owned_iterator_source_hotpaths");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("ListIterator{items:(xs),") && !compact.contains("ListIterator{items:(xs).clone()"),
        "last-use list iterator sources should move into ListIterator instead of cloning; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("(vec![1,2,3]).into_iter()"),
        "one-shot generator iterable sources should move into the generator instead of cloning; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("ListIterator{items:(values).clone(),") && compact.contains("(values).clone().into_iter()"),
        "reused list iterator and generator sources should still clone; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("(xs).clone().into_iter()"),
        "generator iterable variables remain cloned until lazy generator capture gets broader move analysis; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue602_owned_iterator_source_hotpaths", rust_code);
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
fn test_issue246_class_field_visibility_codegen() {
    let source = load_test_file("issue246_class_field_visibility");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue246_class_field_visibility", rust_code);
}

#[test]
fn test_generic_methods_codegen() {
    let source = load_test_file("generic_methods");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("generic_methods", rust_code);
}

#[test]
fn test_explicit_call_site_generics_codegen() {
    let source = load_test_file("explicit_call_site_generics");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("explicit_call_site_generics", rust_code);
}

#[test]
fn test_models_codegen() {
    let source = load_test_file("models");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("models", rust_code);
}

#[test]
fn test_rfc046_computed_properties_codegen() {
    let source = load_test_file("rfc046_computed_properties");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("pub fn dollars(&self) -> i64"),
        "public computed properties should emit as Rust methods:\n{rust_code}"
    );
    assert!(
        rust_code.contains("value.dollars() + value.cents"),
        "computed property reads must emit getter calls, not field reads:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc046_computed_properties", rust_code);
}

#[test]
fn test_list_pop_clone_only_model_codegen() {
    let source = load_test_file("list_pop_clone_only_model");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("incan_stdlib::collections::__private::list_pop"),
        "expected list.pop() emission to route through the stdlib helper; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains(".pop().unwrap_or_else"),
        "list.pop() emission must not inline unwrap_or_else fallback logic; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("list_pop_clone_only_model", rust_code);
}

#[test]
fn test_list_clone_model_codegen() {
    let source = load_test_file("list_clone_model");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("letcopy=nodes.clone();"),
        "expected list.clone() to emit a normal Vec clone; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("list_clone_model", rust_code);
}

/// Issue #380: `len(...)` must lower to a parse-safe expression so comparisons compile as Rust.
#[test]
fn test_issue380_len_comparison_codegen() {
    let source = load_test_file("issue380_len_comparison");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("return ::std::convert::identity(xs.len() as i64) < 2;"),
        "expected len(list) comparison to isolate the cast in a parse-safe expression; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("if ::std::convert::identity(expr.arguments.len() as i64) < 2 {"),
        "expected recursive field len comparison to isolate the cast in a parse-safe expression; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue380_len_comparison", rust_code);
}

/// Issue #383: shared `list[str]` loop args must not lower through consuming `into_iter()` inside repeated helper
/// calls.
#[test]
fn test_issue383_loop_helper_shared_string_list_codegen() {
    let source = load_test_file("issue383_loop_helper_shared_string_list");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("out.push(match_index(xs.clone(), y));"),
        "expected loop helper call to preserve the shared string list via clone, not move it; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("xs.into_iter().map(|s| s.to_string()).collect()"),
        "expected shared string-list helper calls to avoid consuming into_iter lowering; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue383_loop_helper_shared_string_list", rust_code);
}

/// Issue #383 follow-on: dict comprehensions must clone non-Copy keys before reading them in the value expression.
#[test]
fn test_issue383_dict_comp_reuses_noncopy_key_codegen() {
    let source = load_test_file("issue383_dict_comp_reuses_noncopy_key");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains(".map(|name| (name.clone(), ::std::convert::identity(name.len() as i64)))"),
        "expected dict comprehension to clone the non-Copy key before reading it again in the value expression; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("issue383_dict_comp_reuses_noncopy_key", rust_code);
}

/// Issue #195: `for x in list[E]` must iterate owned `E` (via `.iter().cloned()`) so `==` against `E` compiles.
#[test]
fn test_for_in_list_enum_equality_codegen() {
    let source = load_test_file("for_in_list_enum_equality");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("for expected in required.iter().cloned()"),
        "expected enum list for-loop to use .iter().cloned(); generated:\n{rust_code}"
    );
    insta::assert_snapshot!("for_in_list_enum_equality", rust_code);
}

/// Issue #372: imported enums must still iterate as owned values in borrowed list loops.
#[test]
fn test_issue372_imported_enum_loop_ownership_codegen() {
    let main_source = r#"
from rels import ConformanceRel

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    _ =>
      return "Other"

def scenario_matches(required: list[ConformanceRel]) -> bool:
  for expected in required:
    if expected == ConformanceRel.Read:
      if relation_kind_name_from_conformance(expected) == "ReadRel":
        return true
  return false

def main() -> None:
  println(scenario_matches([ConformanceRel.Read]))
"#;
    let rels_source = r#"
@derive(Clone)
pub enum ConformanceRel:
  Read
  Filter
"#;

    let Ok(main_tokens) = lexer::lex(main_source) else {
        panic!("lexer failed")
    };
    let Ok(main_ast) = parser::parse(&main_tokens) else {
        panic!("parser failed")
    };
    let Ok(rels_tokens) = lexer::lex(rels_source) else {
        panic!("lexer failed")
    };
    let Ok(rels_ast) = parser::parse(&rels_tokens) else {
        panic!("parser failed")
    };

    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("rels", &rels_ast, vec!["rels".to_string()]);
    let Ok((main_code, _modules)) = codegen.try_generate_multi_file_nested(&main_ast, &[vec!["rels".to_string()]])
    else {
        panic!("codegen must succeed");
    };
    let rust_code = normalize_codegen_output(&main_code);

    assert!(
        rust_code.contains("for expected in required.iter().cloned()"),
        "expected imported enum loop to use .iter().cloned(); generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("for expected in required.iter() {"),
        "imported enum loop must not iterate borrowed enum refs; generated:\n{rust_code}"
    );

    insta::assert_snapshot!("issue372_imported_enum_loop_ownership", rust_code);
}

#[test]
fn test_issue377_imported_sum_shadows_builtin_codegen() {
    let main_source = r#"
from functions import col, sum

def selected_column_name() -> str:
  amount = col("amount")
  result = sum(amount)
  return result.column_name

def main() -> None:
  println(selected_column_name())
"#;
    let functions_source = r#"
pub model ColumnRef:
  pub name: str

pub model AggregateMeasure:
  pub column_name: str

pub def col(name: str) -> ColumnRef:
  return ColumnRef(name=name)

pub def sum(expr: ColumnRef) -> AggregateMeasure:
  return AggregateMeasure(column_name=expr.name)
"#;

    let Ok(main_tokens) = lexer::lex(main_source) else {
        panic!("lexer failed")
    };
    let Ok(main_ast) = parser::parse(&main_tokens) else {
        panic!("parser failed")
    };
    let Ok(function_tokens) = lexer::lex(functions_source) else {
        panic!("lexer failed")
    };
    let Ok(functions_ast) = parser::parse(&function_tokens) else {
        panic!("parser failed")
    };

    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("functions", &functions_ast, vec!["functions".to_string()]);
    let Ok((main_code, _modules)) = codegen.try_generate_multi_file_nested(&main_ast, &[vec!["functions".to_string()]])
    else {
        panic!("codegen must succeed");
    };
    let rust_code = normalize_codegen_output(&main_code);

    assert!(
        rust_code.contains("let result = sum(amount);"),
        "expected imported helper call to remain a normal function call; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains(".iter().sum::<i64>()"),
        "expected imported helper call to avoid builtin sum lowering; generated:\n{rust_code}"
    );

    insta::assert_snapshot!("issue377_imported_sum_shadows_builtin", rust_code);
}

#[test]
fn test_traits_codegen() {
    let source = load_test_file("traits");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("traits", rust_code);
}

#[test]
fn test_trait_supertraits_codegen() {
    let source = load_test_file("trait_supertraits");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("impl<T:Clone>BoxedValue<T>{"),
        "expected generic inherent impl to inherit Clone bound for backend-owned returns; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("impl<T:Clone>OrderedCollection<T>forBoxedValue<T>{"),
        "expected generic trait impl to inherit Clone bound for backend-owned Self returns; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("impl<T:Clone>Collection<T>forBoxedValue<T>{"),
        "expected generic trait impl to inherit Clone bound for backend-owned field returns; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("returnself.value.clone();"),
        "expected trait-supertrait field return to materialize ownership via clone; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("returnself.clone();"),
        "expected trait-supertrait Self return to materialize ownership via clone; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("trait_supertraits", rust_code);
}

#[test]
fn test_trait_supertrait_assignability_codegen() {
    let source = load_test_file("trait_supertrait_assignability");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("trait_supertrait_assignability", rust_code);
}

#[test]
fn test_enums_codegen() {
    let source = load_test_file("enums");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("enums", rust_code);
}

#[test]
fn test_enum_methods_traits_codegen() {
    let source = load_test_file("enum_methods_traits");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("pubfndefault()->Self{"),
        "expected enum inherent methods to emit in an impl block; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implLabelledforSignal{"),
        "expected enum trait adoption to emit a trait impl block; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("pubfnmessage(&self)->String{"),
        "expected existing enum message helper to remain emitted; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("enum_methods_traits", rust_code);
}

#[test]
fn test_rfc043_newtype_trait_targets_codegen() {
    let source = load_test_file("rfc043_newtype_trait_targets");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implToIntforValue{fnconvert(&self)->i64{return1;}}"),
        "expected ToInt impl to select the int-targeted convert body; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implToStrforValue{fnconvert(&self)->String{return\"value\".to_string();}}"),
        "expected ToStr impl to select the str-targeted convert body; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("typeOutput="),
        "local Incan traits do not declare associated type items; generated impls must not emit one:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc043_newtype_trait_targets", rust_code);
}

#[test]
fn test_rfc043_imported_trait_associated_type_codegen() {
    let source = load_test_file("rfc043_imported_trait_associated_type");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implAssocforBoxed{typeItem=i64;}"),
        "expected imported Rust trait impl to include associated type item; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc043_imported_trait_associated_type", rust_code);
}

#[test]
fn test_rfc043_rust_derive_passthrough_codegen() {
    let source = load_test_file("rfc043_rust_derive_passthrough");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("#[derive(serde::Serialize,Default,Eq,Hash,PartialEq,Debug,Clone"),
        "expected @rust.derive to emit imported and built-in Rust derives; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc043_rust_derive_passthrough", rust_code);
}

#[test]
fn test_value_enums_codegen() {
    let source = load_test_file("value_enums");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("value_enums", rust_code);
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
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("incan_stdlib::collections::__private::list_min_copy")
            || compact.contains("incan_stdlib::collections::__private::list_min_clone")
            || compact.contains("incan_stdlib::collections::__private::list_min_f64"),
        "expected min() emission to route through stdlib helpers; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("incan_stdlib::collections::__private::list_max_copy")
            || compact.contains("incan_stdlib::collections::__private::list_max_clone")
            || compact.contains("incan_stdlib::collections::__private::list_max_f64"),
        "expected max() emission to route through stdlib helpers; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains(".unwrap_or_else"),
        "builtins codegen must not inline unwrap_or_else fallback paths for list min/max; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("builtins", rust_code);
}

#[test]
fn test_pub_const_codegen() {
    let source = load_test_file("pub_const");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("pub_const", rust_code);
}

#[test]
fn test_consts_codegen() {
    let source = load_test_file("consts");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("consts", rust_code);
}

#[test]
fn test_rfc052_module_static_storage_codegen() {
    let source = load_test_file("rfc052_module_static_storage");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc052_module_static_storage", rust_code);
}

#[test]
fn test_rfc052_pub_static_codegen() {
    let source = load_test_file("rfc052_pub_static");
    let rust_code = generate_rust_with_widgets_manifest(&source);
    insta::assert_snapshot!("rfc052_pub_static", rust_code);
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
fn test_rust_associated_call_in_elif_codegen() {
    let source = load_test_file("rust_associated_call_in_elif");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_associated_call_in_elif", rust_code);
}

#[test]
fn test_issue367_result_ok_string_literal_codegen() {
    let source = load_test_file("issue367_result_ok_string_literal");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue367_result_ok_string_literal", rust_code);
}

#[test]
fn test_issue367_result_ok_string_literal_emits_owned_strings() {
    let source = load_test_file("issue367_result_ok_string_literal");
    let rust_code = generate_rust(&source);

    assert!(
        rust_code.contains("(\"from_call\").to_string()"),
        "expected call-argument seeding path to coerce Ok string literals to owned String"
    );
    assert!(
        rust_code.contains("(\"from_local\").to_string()"),
        "expected assignment seeding path to coerce Ok string literals to owned String"
    );
    assert!(
        rust_code.contains("(\"from_return\").to_string()"),
        "expected return-context seeding path to coerce Ok string literals to owned String"
    );
    assert!(
        !rust_code.contains("Ok::<std::string::String, std::string::String>(\"from_call\")"),
        "unexpected raw &str Ok payload in call-argument seeding path"
    );
    assert!(
        !rust_code.contains("Ok::<std::string::String, std::string::String>(\"from_local\")"),
        "unexpected raw &str Ok payload in assignment seeding path"
    );
    assert!(
        !rust_code.contains("Ok::<std::string::String, std::string::String>(\"from_return\")"),
        "unexpected raw &str Ok payload in return-context seeding path"
    );
}

/// Issue #374: qualified enum constructor patterns in `Pattern =>` arms must resolve for same-enum scrutinees.
#[test]
fn test_issue374_enum_constructor_match_codegen() {
    let source = load_test_file("issue374_enum_constructor_match");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue374_enum_constructor_match", rust_code);
}

#[test]
fn test_issue389_for_tuple_unpack_enumerate_codegen() {
    let source = load_test_file("issue389_for_tuple_unpack_enumerate");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue389_for_tuple_unpack_enumerate", rust_code);
    let compact_code = rust_code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        compact_code
            .contains("for (idx, name) in xs .iter() .enumerate() .map(|(idx, value)| (idx as i64, value.clone()))"),
        "expected enumerate loop to emit Incan int indices for tuple binding"
    );
    assert!(
        !compact_code.contains("map(|(idx, value)| (idx as i64, value))"),
        "enumerate loop must clone borrowed iterator values before tuple binding"
    );
}

#[test]
fn test_issue483_list_comp_tuple_unpack_enumerate_codegen() {
    let source = load_test_file("issue483_list_comp_tuple_unpack_enumerate");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue483_list_comp_tuple_unpack_enumerate", rust_code);
    assert!(
        rust_code.contains(".map(|(idx, name)| Binding"),
        "expected enumerate list comprehension to destructure tuple bindings in the map closure"
    );
}

#[test]
fn test_fixed_call_unpack_codegen() {
    let source = load_test_file("fixed_call_unpack");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("fixed_call_unpack", rust_code);
    assert!(
        rust_code.contains("combine(\n        1,\n        \"Ada\".to_string()"),
        "expected shaped positional unpack to emit ordinary fixed arguments"
    );
    assert!(
        rust_code.contains("__incan_rest_args.push(7);"),
        "expected leftover shaped positional entries to feed *rest"
    );
    assert!(
        rust_code.contains("__incan_rest_kwargs.insert(\"city\".to_string(), \"London\".to_string());"),
        "expected unknown shaped keyword entries to feed **kwargs"
    );
    assert!(
        rust_code.contains("route(\"/status\".to_string(), \"GET\".to_string())"),
        "expected shaped keyword unpack to emit ordinary fixed keyword arguments"
    );
    assert!(
        rust_code.contains("counter.add(5, 6)"),
        "expected fixed method unpack to emit ordinary method arguments"
    );
}

#[test]
fn test_collection_literal_spread_codegen() {
    let source = load_test_file("collection_literal_spread");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("collection_literal_spread", rust_code);
    assert!(
        rust_code.contains("__incan_list.extend((vec![2, 3]).into_iter());"),
        "expected list literal spread to emit Vec::extend"
    );
    assert!(
        rust_code.contains("__incan_list.push(tail.0);") && rust_code.contains("__incan_list.push(tail.1);"),
        "expected tuple-shaped list spread to emit field pushes"
    );
    assert!(
        rust_code.contains("for (__incan_key, __incan_value) in (defaults).into_iter()"),
        "expected dict literal spread to emit insertion loop"
    );
    assert!(
        rust_code.contains("__incan_dict.insert(\"trace\".to_string(), \"enabled\".to_string());"),
        "expected later direct dict entry to overwrite earlier spread entry"
    );
}

#[test]
fn test_issue391_list_str_append_literal_codegen() {
    let source = load_test_file("issue391_list_str_append_literal");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue391_list_str_append_literal", rust_code);
    assert!(
        rust_code.contains("columns.push(\"count\".to_string())"),
        "expected list[str].append(\"...\") to materialize an owned String element"
    );
    assert!(
        !rust_code.contains("columns.push(\"count\".clone())"),
        "string literal append must not clone a borrowed &str"
    );
}

#[test]
fn test_rust_interop_field_access_codegen() {
    let source = load_test_file("rust_interop_field_access");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_interop_field_access", rust_code);
}

#[test]
fn test_issue217_rust_enum_match_bindings_codegen() {
    let source = load_test_file("issue217_rust_enum_match_bindings");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("issue217_rust_enum_match_bindings", rust_code);
}

#[test]
fn test_issue459_rust_enum_pattern_import_codegen() {
    let source = load_test_file("issue459_rust_enum_pattern_import");
    let rust_code = generate_rust_with_substrait_probe(&source);
    insta::assert_snapshot!("issue459_rust_enum_pattern_import", rust_code);
    assert!(
        rust_code.contains("use ::substrait::proto::rel::RelType;"),
        "expected Rust enum import used only by a match pattern to be retained:\n{rust_code}"
    );
}

#[test]
fn test_rfc041_std_rust_capability_bounds_codegen() {
    let source = load_test_file("rfc041_std_rust_capability_bounds");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_std_rust_capability_bounds", rust_code);
}

#[test]
fn test_rfc041_rusttype_interop_codegen() {
    let source = load_test_file("rfc041_rusttype_interop");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_rusttype_interop", rust_code);
}

#[test]
fn test_rfc041_rusttype_rebinding_codegen() {
    let source = load_test_file("rfc041_rusttype_rebinding");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_rusttype_rebinding", rust_code);
}

#[test]
fn test_rfc041_interop_from_try_codegen() {
    let source = load_test_file("rfc041_interop_from_try");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_interop_from_try", rust_code);
}

#[test]
fn test_rfc041_interop_into_via_codegen() {
    let source = load_test_file("rfc041_interop_into_via");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_interop_into_via", rust_code);
}

#[test]
fn test_rfc041_capability_bounds_full_codegen() {
    let source = load_test_file("rfc041_capability_bounds_full");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_capability_bounds_full", rust_code);
}

#[test]
fn test_rfc041_structural_coercion_codegen() {
    let source = load_test_file("rfc041_structural_coercion");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_structural_coercion", rust_code);
}

#[test]
fn test_rfc041_rust_coercions_codegen() {
    let source = load_test_file("rfc041_rust_coercions");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_rust_coercions", rust_code);
}

#[test]
fn test_rfc041_emit_rust_path_type_codegen() {
    let source = load_test_file("rfc041_emit_rust_path_type");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_emit_rust_path_type", rust_code);
}

#[test]
fn test_rfc041_emit_static_bound_codegen() {
    let source = load_test_file("rfc041_emit_static_bound");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rfc041_emit_static_bound", rust_code);
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
    assert!(
        !rust_code.contains(".expect(\"validated newtype construction failed"),
        "checked newtype construction should not emit .expect():\n{rust_code}"
    );
    assert!(
        rust_code.contains("incan_stdlib::validation::raise_validation_error"),
        "checked newtype construction should route validation failures through the runtime helper:\n{rust_code}"
    );
    insta::assert_snapshot!("newtype_checked_construction", rust_code);
}

#[test]
fn test_newtype_implicit_coercion_codegen() {
    let source = load_test_file("newtype_implicit_coercion");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.matches("Attempts::from_underlying").count() >= 4,
        "implicit coercion should route int inputs through Attempts::from_underlying:\n{rust_code}"
    );
    assert!(
        rust_code.contains("let retry: RetryAttempts = RetryAttempts("),
        "transitive coercion should wrap the checked Attempts value in RetryAttempts:\n{rust_code}"
    );
    insta::assert_snapshot!("newtype_implicit_coercion", rust_code);
}

#[test]
fn test_constrained_newtype_generated_validation_codegen() {
    let source = r#"
type PositiveInt = newtype int[gt=0]

def take_positive(value: PositiveInt) -> None:
    println(f"{value.0}")

def main() -> None:
    take_positive(1)
    explicit = PositiveInt(2)
    println(f"{explicit.0}")
"#;
    let rust_code = generate_rust(source);
    assert!(
        rust_code.contains("incan_stdlib::validation::raise_constraint_error"),
        "generated constrained newtype validation should use the runtime helper:\n{rust_code}"
    );
    assert!(
        rust_code.contains("if __incan_newtype_input > 0"),
        "generated validation should enforce the gt constraint before wrapping:\n{rust_code}"
    );
}

#[test]
fn test_user_defined_panic_function_codegen() {
    let source = load_test_file("panic_function_name");
    let rust_code = generate_rust(&source);
    assert!(
        !rust_code.contains("println!(\"{}\", panic!(\"not the macro\"));"),
        "user-defined panic function must not emit panic! macro:\n{rust_code}"
    );
    insta::assert_snapshot!("panic_function_name", rust_code);
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

/// RFC 041 / Phase E: compile `std.async.task` from `.incn` source.
#[test]
fn test_std_async_task_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/task.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_async_task_compiled", rust_code);
}

/// RFC 041 / Phase E: compile `std.async.time` from `.incn` source.
#[test]
fn test_std_async_time_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/time.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_async_time_compiled", rust_code);
}

/// Compile `std.async.channel` from `.incn` source.
#[test]
fn test_std_async_channel_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/channel.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_async_channel_compiled", rust_code);
}

/// Compile `std.async.sync` from `.incn` source.
#[test]
fn test_std_async_sync_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/sync.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_async_sync_compiled", rust_code);
}

/// Compile `std.async.race` from `.incn` source.
#[test]
fn test_std_async_race_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/race.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_async_race_compiled", rust_code);
}

/// Compile `race for value:` through the shared `std.async.race` runtime helper surface.
#[test]
fn test_race_for_expression_codegen() {
    let source = r#"
import std.async

pub async def fast() -> int:
  return 1

pub async def slow() -> int:
  return 2

pub async def fastest() -> int:
  return race for value:
    await fast() => value
    await slow() => value
"#;
    let rust_code = generate_rust(source);
    insta::assert_snapshot!("race_for_expression_codegen", rust_code);
}

/// Awaiting a declared wrapper must delegate to the proven awaitable field.
#[test]
fn test_awaitable_wrapper_delegation_codegen() {
    let source = r#"
import std.async
from std.async.task import JoinHandle, TaskJoinError

pub model TaskBox[T] with Awaitable[Result[T, TaskJoinError]]:
  handle: JoinHandle[T]

pub async def wait_for(box: TaskBox[int]) -> Result[int, TaskJoinError]:
  return await box
"#;
    let rust_code = generate_rust(source);
    assert!(
        rust_code.contains("r#box.handle.await"),
        "awaitable wrapper should lower through its awaitable field, got:\n{rust_code}"
    );
}

// ============================================================================
// RFC 023: Compile std.derives.* trait definitions from Incan source
// ============================================================================

/// compile `std.derives.comparison` (Eq, Ord, Hash) from `.incn` source.
///
/// Verifies that source-defined abstract methods and pure-Incan default methods (`__ne__`, `__le__`, `__gt__`,
/// `__ge__`) compile through the full pipeline without a fake `rust.module()` boundary.
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

/// compile `std.derives.collection` (collection/iterator protocols and adapters) from `.incn` source.
#[test]
fn test_std_derives_collection_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/collection.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_derives_collection_compiled", rust_code);
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

/// RFC 024: verify `@derive(json)` resolves through stdlib derive metadata and compiles.
///
/// Exercises the stdlib import path for the json module-level derive.
#[test]
fn test_std_serde_json_import_codegen() {
    let source = load_test_file("std_serde_json_import");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("incan_stdlib::json::__private::stringify_or_raise"),
        "expected JSON stringify emission to route through stdlib helper; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("incan_stdlib::json::__private::parse_or_error"),
        "expected JSON decode emission to route through stdlib helper; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("serde_json::to_string"),
        "generated JSON stringify paths should no longer inline serde_json::to_string fallbacks; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("serde_json::from_str"),
        "generated JSON decode paths should no longer inline serde_json::from_str fallbacks; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_serde_json_import", rust_code);
}

/// RFC 047: compile `std.graph` declarations from `.incn` source.
#[test]
fn test_std_graph_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/graph.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_graph_compiled", rust_code);
}

/// RFC 061: compile the `std.compression` source modules.
#[test]
fn test_std_compression_modules_compile_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let paths = [
        "crates/incan_stdlib/stdlib/compression/prelude.incn",
        "crates/incan_stdlib/stdlib/compression/_core.incn",
        "crates/incan_stdlib/stdlib/compression/_auto.incn",
        "crates/incan_stdlib/stdlib/compression/gzip.incn",
        "crates/incan_stdlib/stdlib/compression/zlib.incn",
        "crates/incan_stdlib/stdlib/compression/deflate.incn",
        "crates/incan_stdlib/stdlib/compression/zstd.incn",
        "crates/incan_stdlib/stdlib/compression/bz2.incn",
        "crates/incan_stdlib/stdlib/compression/lzma.incn",
        "crates/incan_stdlib/stdlib/compression/snappy.incn",
        "crates/incan_stdlib/stdlib/compression/snappy/raw.incn",
    ];

    for path in paths {
        let source = fs::read_to_string(path)?;
        let rust_code = generate_rust(&source);
        assert!(
            rust_code.contains("__incan"),
            "expected {path} to compile into Incan-generated Rust, got:\n{rust_code}"
        );
    }
    Ok(())
}

/// RFC 047: verify `std.graph` imports, direct constructors, DAGs, and multigraph edge ids lower to Rust.
#[test]
fn test_std_graph_import_codegen() {
    let source = load_test_file("std_graph_import");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("DiGraph::<String>::__incan_new()"),
        "expected DiGraph constructor syntax to lower through __incan_new; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("Dag::<String>::__incan_new()"),
        "expected Dag constructor syntax to lower through __incan_new; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("MultiDiGraph::<String>::__incan_new()"),
        "expected MultiDiGraph constructor syntax to lower through __incan_new; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("Result<EdgeId,GraphError>"),
        "expected multigraph add_edge to preserve EdgeId result; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_graph_import", rust_code);
}

/// RFC 060: compile `std.uuid` declarations from `.incn` source.
#[test]
fn test_std_uuid_compiled_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let path = "crates/incan_stdlib/stdlib/uuid.incn";
    let source = fs::read_to_string(path)?;
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("pubstructUUID(pubu128);"),
        "expected UUID to remain a source-defined u128 newtype; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("uuid::Uuid::") && !rust_code.contains("uuid::Uuid;"),
        "std.uuid must not lower to a Rust uuid::Uuid-backed type; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_uuid_compiled", rust_code);
    Ok(())
}

/// RFC 060: verify `std.uuid` imports and method calls lower without a Rust-backed UUID type.
#[test]
fn test_std_uuid_import_codegen() {
    let source = load_test_file("std_uuid_import");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("UUID::parse"),
        "expected parse call to route through the source-defined UUID type; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("uuid::Uuid::") && !rust_code.contains("uuid::Uuid;"),
        "std.uuid import path must not introduce a Rust uuid::Uuid-backed type; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("std_uuid_import", rust_code);
}

/// RFC 059: direct imported constructors lower through the generic `__incan_new` hook.
#[test]
fn test_std_regex_import_constructor_hook_codegen() {
    let source = r#"
from std.regex import Regex, RegexError

def main() -> Result[None, RegexError]:
  _regex = Regex("x+", ignore_case=true)?
  return Ok(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("Regex::__incan_new(\"x+\".to_string(),true,false,false,false)?"),
        "expected imported Regex constructor syntax to lower through the generic __incan_new hook; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("__incan_std::regex::compile("),
        "direct constructor lowering should not hardcode std.regex.compile; generated:\n{rust_code}"
    );
}

/// RFC 023 (#303): explicit `with Serialize` adoption should expand the stdlib default `to_json` body into the
/// generated impl while also forwarding the Rust serde derive.
#[test]
fn test_std_serde_with_serialize_trait_codegen() {
    let source = load_test_file("std_serde_with_serialize_trait");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_serde_with_serialize_trait", rust_code);
}

#[test]
fn test_newtype_with_serialize_trait_forwards_rust_derive() {
    let source = r#"
from std.serde.json import Serialize

type Wrapped = newtype str with Serialize

def main() -> None:
  println(Wrapped("ok").to_json())
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("#[derive(Debug,serde::Serialize)]structWrapped(pubString);")
            || compact.contains("#[derive(Debug,Clone,serde::Serialize)]structWrapped(pubString);"),
        "expected newtype `with Serialize` to forward the Rust serde derive; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implSerializeforWrapped"),
        "expected newtype `with Serialize` to expand the stdlib Serialize impl; generated:\n{rust_code}"
    );
}

#[test]
fn test_with_serialize_keeps_ordinary_methods_inherent() {
    let source = r#"
from std.serde.json import Serialize

model Payload with Serialize:
  value: str

  def display_text(self) -> str:
    return self.value

def main() -> None:
  payload = Payload(value="ok")
  println(payload.display_text())
  println(payload.to_json())
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implPayload{pubfndisplay_text(&self)->String"),
        "expected ordinary method on `with Serialize` model to emit as inherent impl; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("implSerializeforPayload{fndisplay_text"),
        "ordinary methods must not be emitted into the Serialize trait impl; generated:\n{rust_code}"
    );
}

/// RFC 024: module-level derive metadata should let `@derive(json)` adopt serde traits and emit Rust derives.
#[test]
fn test_rfc024_module_derive_json_codegen() {
    let source = load_test_file("rfc024_module_derive_json");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("serde::Serialize,serde::Deserialize"),
        "expected @derive(json) to forward serde derives; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("impljson::SerializeforPayload{fnto_json(&self)->String"),
        "expected @derive(json) to emit the adopted json.Serialize trait impl with its serde adapter; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc024_module_derive_json", rust_code);
}

/// RFC 024: imported trait aliases should work as partial derives.
#[test]
fn test_rfc024_partial_alias_derive_codegen() {
    let source = load_test_file("rfc024_partial_alias_derive");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implJsonSerializeforPayload{fnto_json(&self)->String"),
        "expected @derive(JsonSerialize) to emit the adopted aliased trait impl with its serde adapter; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("rfc024_partial_alias_derive", rust_code);
}

/// RFC 024: user modules can define a second serde-backed format without compiler changes.
#[test]
fn test_rfc024_user_module_serde_format_codegen() {
    let yaml_source = r#"
__derives__ = [Serialize]

@rust.derive("serde::Serialize")
pub trait Serialize:
  def to_yaml(self) -> str:
    return str("yaml")
"#;
    let source = r#"
from std.serde import json
import yaml

@derive(json, yaml)
model Payload:
  value: int

def encode_yaml[T with yaml.Serialize](value: T) -> str:
  return value.to_yaml()

def encode_json[T with json.Serialize](value: T) -> str:
  return value.to_json()

def main() -> str:
  return encode_yaml(Payload(value=1))
"#;

    let yaml_ast = parse_incan_program(yaml_source, "yaml module");
    let main_ast = parse_incan_program(source, "consumer");
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("yaml", &yaml_ast, vec!["yaml".to_string()]);
    let (main_code, _modules) = codegen
        .try_generate_multi_file_nested(&main_ast, &[vec!["yaml".to_string()]])
        .unwrap_or_else(|err| panic!("user serde derivable module should codegen: {err:?}"));
    let rust_code = normalize_codegen_output(&main_code);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert_eq!(
        rust_code.matches("serde::Serialize").count(),
        1,
        "expected duplicate serde derive paths to be deduplicated; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("impljson::SerializeforPayload"),
        "expected stdlib json.Serialize impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implyaml::SerializeforPayload"),
        "expected user yaml.Serialize impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("fnto_yaml(&self)->String"),
        "expected yaml default method body to expand into the impl; generated:\n{rust_code}"
    );
}

/// RFC 024: derivable modules are not limited to serde-backed Rust derives.
#[test]
fn test_rfc024_user_module_pure_incan_derivable_codegen() {
    let schema_source = r#"
__derives__ = [Named]

pub trait Named:
  def schema_name(self) -> str:
    return str("schema")
"#;
    let source = r#"
import schema

@derive(schema)
model Payload:
  value: int

def name[T with schema.Named](value: T) -> str:
  return value.schema_name()

def main() -> str:
  return name(Payload(value=1))
"#;

    let schema_ast = parse_incan_program(schema_source, "schema module");
    let main_ast = parse_incan_program(source, "consumer");
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("schema", &schema_ast, vec!["schema".to_string()]);
    let (main_code, _modules) = codegen
        .try_generate_multi_file_nested(&main_ast, &[vec!["schema".to_string()]])
        .unwrap_or_else(|err| panic!("pure Incan derivable module should codegen: {err:?}"));
    let rust_code = normalize_codegen_output(&main_code);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implschema::NamedforPayload"),
        "expected user schema.Named impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("fnschema_name(&self)->String"),
        "expected pure Incan default method body to expand into the impl; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("serde::Serialize"),
        "pure derivable fixture should not emit serde derives; generated:\n{rust_code}"
    );
}

#[test]
fn test_multi_instantiation_trait_methods_codegen_trait_impls_only() {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

model Reading with Convert[int], Convert[float]:
  value: int

  def convert(self) -> int:
    return self.value

  def convert(self) -> float:
    return 1.0

def main() -> None:
  reading = Reading(value=1)
  precise: float = reading.convert()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implConvert<i64>forReading"),
        "expected Convert[int] trait impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implConvert<f64>forReading"),
        "expected Convert[float] trait impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("let_precise:f64=reading.convert();"),
        "typed local binding must preserve the Rust return hint for same-family trait impl dispatch; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("implReading{fnconvert"),
        "same-name trait methods must not also lower as duplicate inherent methods; generated:\n{rust_code}"
    );
}

#[test]
fn test_std_json_value_indexing_emits_checked_helpers() {
    let source = r#"
from std.json import JsonValue

pub def by_name(data: JsonValue) -> Option[JsonValue]:
  return data["name"]

pub def by_index(data: JsonValue) -> Option[JsonValue]:
  return data[0]
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains(
            "crate::__incan_std::traits::indexing::Index::<String,Option<JsonValue>,>::__getitem__(&data,\"name\".to_string())"
        ),
        "expected object-style JsonValue indexing to use source-authored Index.__getitem__; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("crate::__incan_std::traits::indexing::Index::<i64,Option<JsonValue>,>::__getitem__(&data,0)"),
        "expected array-style JsonValue indexing to use source-authored Index.__getitem__; generated:\n{rust_code}"
    );
}

#[test]
fn test_enum_multi_instantiation_trait_methods_codegen_trait_impls_only() {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

enum Token with Convert[int], Convert[float]:
  Number

  def convert(self) -> int:
    return 1

  def convert(self) -> float:
    return 1.0

def main() -> None:
  token: Token = Token.Number
  precise: float = token.convert()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implConvert<i64>forToken"),
        "expected Convert[int] enum trait impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implConvert<f64>forToken"),
        "expected Convert[float] enum trait impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("let_precise:f64=token.convert();"),
        "typed enum local binding must preserve the Rust return hint for same-family trait impl dispatch; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("implToken{pubfnconvert") && !compact.contains("implToken{fnconvert"),
        "same-name enum trait methods must not also lower as duplicate inherent methods; generated:\n{rust_code}"
    );
}

// ============================================================================
// RFC 023: Compile std.traits.* trait definitions from Incan source
// ============================================================================

#[test]
fn test_std_traits_ops_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/ops.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_ops_compiled", rust_code);
}

#[test]
fn test_std_traits_error_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/error.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_error_compiled", rust_code);
}

#[test]
fn test_std_traits_indexing_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/indexing.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_indexing_compiled", rust_code);
}

#[test]
fn test_std_traits_callable_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/callable.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_callable_compiled", rust_code);
}

#[test]
fn test_std_traits_prelude_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/prelude.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_prelude_compiled", rust_code);
}

#[test]
fn test_std_traits_convert_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/convert.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_convert_compiled", rust_code);
}

#[test]
fn test_std_traits_convert_usage_codegen() {
    let source = load_test_file("std_traits_convert_usage");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("std_traits_convert_usage", rust_code);
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
/// RFC 057: Targeted Rust lint suppression.
// ============================================================================
#[test]
fn test_rust_allow_codegen() {
    let source = load_test_file("rust_allow");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("rust_allow", rust_code);
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

#[test]
fn test_ordinal_key_builtin_impls_codegen() {
    let source = load_test_file("ordinal_key_builtin_impls");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("pubusecrate::__incan_std::collections::OrdinalKey;"),
        "expected imported std.collections.OrdinalKey re-export; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implcrate::__incan_std::collections::OrdinalKeyforStatus{")
            && compact
                .contains("fnordinal_hash(&self)->i64{incan_stdlib::collections::__private::ordinal_key_hash_bytes")
            && compact
                .contains("fnordinal_bytes_equal(&self,data:Vec<u8>)->bool{self.value().as_bytes()==data.as_slice()}"),
        "expected generated OrdinalKey impl for string value enum; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implcrate::__incan_std::collections::OrdinalKeyforHttpStatus{")
            && compact.contains(
                "fnordinal_hash(&self)->i64{incan_stdlib::collections::__private::ordinal_key_hash_bytes"
            )
            && compact.contains("fnordinal_bytes_equal(&self,data:Vec<u8>)->bool{data.as_slice()==self.value().to_le_bytes().as_slice()}"),
        "expected generated OrdinalKey impl for integer value enum; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("ordinal_key_builtin_impls", rust_code);
}

#[test]
fn test_ordinal_map_str_fast_lookup_codegen() {
    let source = load_test_file("ordinal_map_str_fast_lookup");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("columns.require(key)"),
        "expected OrdinalMap[str].require to keep the source-defined lookup path; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("columns.require_many(keys)"),
        "expected OrdinalMap[str].require_many to keep the source-defined batch lookup path; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("__incan_require_str_fast") && !compact.contains("__incan_require_many_str_fast"),
        "OrdinalMap[str] calls should not route through generated method specializations; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("vec![\"id\".to_string(),\"status\".to_string()]"),
        "expected direct string-list construction to materialize owned strings; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("vec![(\"id\".to_string(),10),(\"status\".to_string(),20)]"),
        "expected direct string-pair construction to materialize owned strings; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("ordinal_map_str_fast_lookup", rust_code);
}

#[test]
fn test_imported_stdlib_value_fragment_codegen() {
    let source = load_test_file("imported_stdlib_value_fragment");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("::ordinal_key_byte(7);"),
        "expected imported stdlib value fragment helper to be called directly; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("ordinal_key_append_byte"),
        "stale datetime ordinal append helper leaked into generated code; generated:\n{rust_code}"
    );
    insta::assert_snapshot!("imported_stdlib_value_fragment", rust_code);
}

/// RFC 023: Additional inference cases (Display, Dict key hashing, arithmetic, transitive propagation).
#[test]
fn test_trait_bound_inference_more_codegen() {
    let source = load_test_file("trait_bound_inference_more");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("trait_bound_inference_more", rust_code);
}

#[test]
fn test_loop_expressions_codegen() {
    let source = load_test_file("loop_expressions");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("loop_expressions", rust_code);
}

/// RFC 023: Generic bounds in return types (issue #196).
///
/// Verifies that trait bounds from return types (e.g., `impl BoundedDataSet<T>`) are properly inferred and emitted in
/// the Rust codegen, even when the bounds aren't used in the function body.
#[test]
fn test_generic_bounds_return_type_codegen() {
    let source = load_test_file("generic_bounds_return_type");
    let rust_code = generate_rust(&source);
    insta::assert_snapshot!("generic_bounds_return_type", rust_code);
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
