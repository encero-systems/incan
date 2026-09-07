//! Golden snapshot tests for codegen
//!
//! These tests generate Rust code from `.incn` input files and compare the output against stored snapshots.
//! This ensures codegen changes are reviewed and intentional.
//!
//! Run with: `cargo test --test codegen_snapshot_tests`
//! Review changes: `cargo insta review`

use incan::backend::IrCodegen;
use incan::frontend::{lexer, parser};
use incan_semantics_core::{
    CanonicalSymbolId, SemanticSourceTargetKind, decode_incan_symbol_identity, encode_incan_symbol_identity,
};
use quote::ToTokens;
use std::collections::HashSet;
use std::error::Error;
use std::fs;

type TestResult = Result<(), Box<dyn Error>>;

#[path = "support/builtin_stdlib.rs"]
mod builtin_stdlib_support;

fn codegen_with_builtin_stdlib_inventory() -> IrCodegen<'static> {
    let mut codegen = IrCodegen::new();
    codegen.set_sdk_provider_module_paths(builtin_stdlib_support::artifact_module_paths());
    codegen
}

/// Generate Rust code from Incan source
fn generate_rust(source: &str) -> String {
    normalize_projected_symbols_for_readable_codegen(&generate_projected_rust(source))
}

/// Generate Rust while retaining RFC 120's exact physical symbol projections.
fn generate_projected_rust(source: &str) -> String {
    let source = source.to_string();
    incan::compiler_stack::run_on_compiler_stack(move || {
        let Ok(tokens) = lexer::lex(&source) else {
            panic!("lexer failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parser failed");
        };
        let code = match codegen_with_builtin_stdlib_inventory().try_generate(&ast) {
            Ok(code) => code,
            Err(e) => panic!("codegen snapshot inputs must typecheck: {e:?}"),
        };
        normalize_projected_codegen_output(&code)
    })
}

/// Generate Rust with the same source-module and package identity context that the CLI supplies for registry code.
fn generate_registry_rust(source: &str, module_name: &str) -> String {
    normalize_projected_symbols_for_readable_codegen(&generate_projected_registry_rust(source, module_name))
}

/// Generate registry Rust while retaining RFC 120's exact physical symbol projections.
fn generate_projected_registry_rust(source: &str, module_name: &str) -> String {
    let source = source.to_string();
    let module_name = module_name.to_string();
    incan::compiler_stack::run_on_compiler_stack(move || {
        let Ok(tokens) = lexer::lex(&source) else {
            panic!("lexer failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parser failed");
        };
        let mut codegen = IrCodegen::new();
        codegen.set_root_source_module_name(Some(module_name.clone()));
        codegen.set_registry_package_identity(Some(module_name));
        let code = match codegen.try_generate(&ast) {
            Ok(code) => code,
            Err(error) => panic!("registry codegen snapshot inputs must typecheck: {error:?}"),
        };
        normalize_projected_codegen_output(&code)
    })
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
        CanonicalIdentityExport, ConstExport, ExportIdentity, ExportIdentityKind, ExportIdentityProjection,
        FunctionExport, LibraryIdentityGraph, LibraryManifest, ModelExport, ParamExport, ParamKindExport, StaticExport,
        TypeRef,
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
        properties: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.functions.push(FunctionExport {
        name: "make_widget".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "name".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: false,
            default: None,
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
    let identity_entry = |name: &str, kind: ExportIdentityKind, canonical_kind: SemanticSourceTargetKind, start| {
        let canonical = CanonicalSymbolId::module_declaration(
            Vec::new(),
            name,
            canonical_kind,
            incan_semantics_core::HirSourceSpan::new(start, start + name.len()),
        );
        ExportIdentity {
            public_name: name.to_string(),
            public_path: vec!["widgets_core".to_string(), name.to_string()],
            source_path: vec![name.to_string()],
            kind,
            projection: ExportIdentityProjection::Direct,
            canonical: CanonicalIdentityExport::from_canonical("widgets_core", &canonical),
        }
    };
    manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
        exports: vec![
            identity_entry("Widget", ExportIdentityKind::Model, SemanticSourceTargetKind::Model, 0),
            identity_entry(
                "make_widget",
                ExportIdentityKind::Function,
                SemanticSourceTargetKind::Function,
                10,
            ),
            identity_entry(
                "DEFAULT_NAME",
                ExportIdentityKind::Const,
                SemanticSourceTargetKind::Const,
                30,
            ),
            identity_entry(
                "SHARED_COUNT",
                ExportIdentityKind::Static,
                SemanticSourceTargetKind::Static,
                50,
            ),
            identity_entry(
                "SHARED_ITEMS",
                ExportIdentityKind::Static,
                SemanticSourceTargetKind::Static,
                70,
            ),
        ],
        ..LibraryIdentityGraph::default()
    };

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "widgets".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("widgets", "widgets_core", artifact_root),
        },
    )]));

    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.set_library_manifest_index(index);
    let code = match codegen.try_generate(&ast) {
        Ok(c) => c,
        Err(e) => panic!("codegen snapshot inputs must typecheck: {e:?}"),
    };
    normalize_codegen_output(&code)
}

#[cfg(feature = "rust_inspect")]
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
    let mut codegen = codegen_with_builtin_stdlib_inventory();
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.set_library_manifest_index(index);
    let code = match codegen.try_generate(&ast) {
        Ok(code) => code,
        Err(err) => panic!("codegen failed: {err}"),
    };
    normalize_codegen_output(&code)
}

/// Generate Rust from source desugared through a helper-backed vocab WASM artifact.
fn generate_rust_with_helper_backed_vocab_wasm_desugaring(source: &str, keyword_names: &[&str]) -> String {
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
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Named {
                name: "int".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: false,
            default: None,
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
            keywords: keyword_names
                .iter()
                .map(|name| incan_vocab::KeywordSpec {
                    name: (*name).to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                })
                .collect(),
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.set_library_manifest_index(index);
    let code = match codegen.try_generate(&ast) {
        Ok(code) => code,
        Err(err) => panic!("codegen failed: {err}"),
    };
    normalize_codegen_output(&code)
}

/// Normalize generated output so tests don't churn on version bumps while retaining physical symbol projections.
fn normalize_projected_codegen_output(code: &str) -> String {
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

/// Normalize generated output for source-readable assertions and broad snapshots.
fn normalize_codegen_output(code: &str) -> String {
    normalize_projected_symbols_for_readable_codegen(&normalize_projected_codegen_output(code))
}

fn recover_incan_identities_from_generated_rust(code: &str) -> HashSet<CanonicalSymbolId> {
    code.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| token.starts_with("__incan_v"))
        .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
        .collect()
}

/// Keep broad codegen snapshots readable while RFC 120's dedicated tests assert the exact physical projections.
///
/// Canonical identities include declaration spans, so snapshotting their encoded Rust spellings would turn an
/// unrelated source-line move into hundreds of opaque golden-file changes. Decode only for snapshot presentation;
/// every generated string returned by the helpers above retains the real projection for focused assertions.
fn normalize_projected_symbols_for_readable_codegen(code: &str) -> String {
    let presentation_code = strip_rust_facing_projection_shims(code).unwrap_or_else(|| code.to_string());
    let mut normalized = String::with_capacity(presentation_code.len());
    let mut token = String::new();
    let flush = |token: &mut String, normalized: &mut String| {
        if token.starts_with("__incan_v")
            && let Ok(Some(identity)) = decode_incan_symbol_identity(token)
        {
            normalized.push_str(&identity.declaration_name);
        } else {
            normalized.push_str(token);
        }
        token.clear();
    };

    for character in presentation_code.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            token.push(character);
        } else {
            flush(&mut token, &mut normalized);
            normalized.push(character);
        }
    }
    flush(&mut token, &mut normalized);

    // The physical projection is intentionally long and therefore changes prettyplease's wrapping decisions before
    // this presentation-only decode runs. Format the decoded syntax once more so the historical snapshots continue
    // to show the source-level shape rather than projection-induced whitespace churn.
    let Ok(syntax) = syn::parse_file(&normalized) else {
        return normalized;
    };
    format!(
        "// Generated by the Incan compiler v<INCAN_VERSION>\n\n// __INCAN_INSERT_MODS__\n\n{}",
        prettyplease::unparse(&syntax)
    )
}

/// Remove separately tested native-Rust compatibility shims before rendering broad source-readable snapshots.
///
/// Decoding a canonical target back to its source spelling makes an intentional forwarding method appear recursive
/// and makes `use canonical as source` appear self-referential. Artifact tests inspect those physical declarations;
/// broad snapshots should continue to present only the authored implementation and its generated Incan call sites.
fn strip_rust_facing_projection_shims(code: &str) -> Option<String> {
    fn is_projection_alias(tree: &syn::UseTree) -> bool {
        match tree {
            syn::UseTree::Path(path) => is_projection_alias(&path.tree),
            syn::UseTree::Rename(rename) => decode_incan_symbol_identity(&rename.ident.to_string())
                .ok()
                .flatten()
                .is_some_and(|identity| {
                    let alias = rename.rename.to_string();
                    let static_alias = identity
                        .declaration_name
                        .chars()
                        .map(|character| {
                            if character.is_ascii_alphanumeric() {
                                character.to_ascii_uppercase()
                            } else {
                                '_'
                            }
                        })
                        .collect::<String>();
                    alias == identity.declaration_name || alias == static_alias
                }),
            syn::UseTree::Group(group) => group.items.iter().any(is_projection_alias),
            syn::UseTree::Name(_) | syn::UseTree::Glob(_) => false,
        }
    }

    fn is_projection_method(method: &syn::ImplItemFn) -> bool {
        if method.sig.ident.to_string().starts_with("__incan_v")
            || !method.attrs.iter().any(|attribute| attribute.path().is_ident("inline"))
        {
            return false;
        }
        let source_name = method.sig.ident.to_string();
        method
            .block
            .to_token_stream()
            .to_string()
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| token.starts_with("__incan_v"))
            .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
            .any(|identity| identity.declaration_name == source_name)
    }

    /// Report whether a free function is only a source-facing forwarder onto its own projection.
    ///
    /// Emission publishes a projected declaration twice: the implementation under its encoded name, and a thin
    /// function under the source spelling whose entire body calls that projection. Decoding both for presentation
    /// turns the pair into two same-named functions, the second calling itself, so a reader of the golden sees Rust
    /// that could not compile. Only the forwarder is dropped; the implementation keeps the snapshot honest.
    fn is_projection_forwarder(function: &syn::ItemFn) -> bool {
        let source_name = function.sig.ident.to_string();
        if source_name.starts_with("__incan_v") || function.block.stmts.len() != 1 {
            return false;
        }
        function
            .block
            .to_token_stream()
            .to_string()
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| token.starts_with("__incan_v"))
            .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
            .any(|identity| identity.declaration_name == source_name)
    }

    fn strip_items(items: &mut Vec<syn::Item>) {
        items.retain_mut(|item| match item {
            syn::Item::Use(item_use) => !is_projection_alias(&item_use.tree),
            syn::Item::Impl(item_impl) => {
                let original_len = item_impl.items.len();
                item_impl
                    .items
                    .retain(|item| !matches!(item, syn::ImplItem::Fn(method) if is_projection_method(method)));
                original_len == item_impl.items.len() || !item_impl.items.is_empty()
            }
            syn::Item::Fn(item_fn) => !is_projection_forwarder(item_fn),
            syn::Item::Mod(item_mod) => {
                if let Some((_, nested)) = &mut item_mod.content {
                    strip_items(nested);
                }
                true
            }
            _ => true,
        });
    }

    let mut syntax = syn::parse_file(code).ok()?;
    strip_items(&mut syntax.items);
    Some(prettyplease::unparse(&syntax))
}

macro_rules! assert_codegen_snapshot {
    ($name:expr, $code:expr $(,)?) => {
        insta::assert_snapshot!($name, normalize_projected_symbols_for_readable_codegen(&$code));
    };
}

fn compact_rust(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
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
    assert!(
        rust_code.contains("widgets::make_widget") && !rust_code.contains("widgets_core::make_widget"),
        "canonical package identity must not replace the consumer's linked dependency name:\n{rust_code}"
    );
    assert_codegen_snapshot!("pub_import_expressions", rust_code);
}

#[test]
fn test_pub_import_module_alias_codegen() {
    let source = load_test_file("pub_import_module_alias");
    let rust_code = generate_rust_with_widgets_manifest(&source);
    assert_codegen_snapshot!("pub_import_module_alias", rust_code);
}

#[test]
fn test_vocab_block_desugaring_codegen() {
    let source = load_test_file("vocab_block_desugaring");
    let rust_code = generate_rust_with_vocab_wasm_desugaring(&source);
    assert_codegen_snapshot!("vocab_block_desugaring", rust_code);
}

#[test]
fn test_vocab_helper_backed_desugaring_codegen() {
    let source = "import pub::query\n\ndef main() -> None:\n  where true:\n    pass\n";
    let rust_code = generate_rust_with_helper_backed_vocab_wasm_desugaring(source, &["where"]);
    assert_codegen_snapshot!("vocab_helper_backed_desugaring", rust_code);
}

#[test]
fn test_equivalent_helper_backed_keywords_codegen_identically() {
    let where_source = "import pub::query\n\ndef main() -> None:\n  where true:\n    pass\n";
    let screen_source = "import pub::query\n\ndef main() -> None:\n  screen true:\n    pass\n";

    let where_rust = generate_rust_with_helper_backed_vocab_wasm_desugaring(where_source, &["where", "screen"]);
    let screen_rust = generate_rust_with_helper_backed_vocab_wasm_desugaring(screen_source, &["where", "screen"]);
    assert_eq!(
        where_rust, screen_rust,
        "equivalent helper-backed keywords must generate identical Rust"
    );
}

#[test]
fn test_basic_function_codegen() {
    let source = load_test_file("basic_function");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("basic_function", rust_code);
}

#[test]
fn test_function_references_codegen() {
    let source = load_test_file("function_references");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("function_references", rust_code);
}

#[test]
fn test_user_defined_decorators_codegen() {
    let source = load_test_file("user_defined_decorators");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("user_defined_decorators", rust_code);
}

#[deny(clippy::expect_used, clippy::unwrap_used)]
mod emitted_symbol_projection_tests {
    use super::*;

    fn generate_rust(source: &str) -> String {
        generate_projected_rust(source)
    }

    fn generate_registry_rust(source: &str, module_name: &str) -> String {
        generate_projected_registry_rust(source, module_name)
    }

    #[test]
    fn decorated_function_emits_one_source_projection_and_distinct_generated_helpers() -> TestResult {
        let source = load_test_file("user_defined_decorators");
        let rust_code = generate_rust(&source);
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Function && identity.declaration_name == "label"
            })
            .ok_or_else(|| "decorated source wrapper must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(label);

        assert_eq!(
            rust_code.matches(&format!("fn {projection}")).count(),
            1,
            "one source declaration must emit exactly one projected function definition:\n{rust_code}"
        );
        assert!(
            rust_code.contains("fn __incan_original_label"),
            "decorator original must retain its separate compiler-helper name:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn decorator_factory_calls_use_the_resolved_projection_for_free_and_method_wrappers() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
def preserve[F]() -> ((F) -> F):
  return (func) => func

@preserve()
def decorated_total(first: int, second: int) -> int:
  return first + second

class Box:
  base: int

  @preserve()
  def total(self, extra: int) -> int:
    return self.base + extra

def main() -> None:
  box = Box(base=5)
  _ = decorated_total(1, 2) + box.total(6)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let preserve = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Function && identity.declaration_name == "preserve"
            })
            .ok_or_else(|| "decorator factory must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(preserve);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert_eq!(
            compact.matches(&format!("{projection}()")).count(),
            2,
            "both decorator initializers must call the exact compiler-derived factory projection:\n{rust_code}"
        );
        assert!(
            !compact.contains("preserve()"),
            "decorator lowering must not fall back to the raw source name:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn ordinary_function_declaration_and_call_share_one_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
def calculate(value: int) -> int:
  return value + 1

def main() -> None:
  _ = calculate(41)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let calculate = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Function && identity.declaration_name == "calculate"
            })
            .ok_or_else(|| "ordinary source function must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(calculate);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(compact.contains(&format!("{projection}(41,)")), "{rust_code}");
        Ok(())
    }

    #[test]
    fn same_module_function_alias_calls_the_target_projection_without_a_second_definition() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
def calculate(value: int) -> int:
  return value + 1

compute = calculate

def main() -> None:
  _ = compute(41)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let calculate = identities
            .iter()
            .find(|identity| identity.declaration_name == "calculate")
            .ok_or_else(|| "alias target identity must survive codegen".to_string())?;
        let projection = encode_incan_symbol_identity(calculate);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(compact.contains(&format!("{projection}(41,)")), "{rust_code}");
        assert!(
            !rust_code.contains("fn compute"),
            "a binding alias must not create another declaration:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn partial_wrapper_declaration_and_call_share_one_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
def route(method: str, path: str) -> str:
  return method + path

get = partial route(method="GET")

def main() -> None:
  _ = get(path="/")
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let get = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Partial && identity.declaration_name == "get")
            .ok_or_else(|| "source partial wrapper must retain its own canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(get);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact.contains(&format!("{projection}(\"GET\".to_string(),\"/\".to_string(),)")),
            "{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn cross_module_function_alias_imports_and_calls_the_provider_projection() -> TestResult {
        let root_source = r#"
from helpers import calculate as compute

def main() -> None:
  _ = compute(41)
"#;
        let helper_source = r#"
pub def calculate(value: int) -> int:
  return value + 1
"#;
        let root_ast = parse_incan_program(root_source, "root alias fixture");
        let helper_ast = parse_incan_program(helper_source, "helper alias fixture");
        let helper_path = vec!["helpers".to_string()];
        let mut codegen = codegen_with_builtin_stdlib_inventory();
        codegen.add_module_with_path_segments("helpers", &helper_ast, helper_path.clone());
        let (root_code, modules) = codegen
            .try_generate_multi_file_nested(&root_ast, std::slice::from_ref(&helper_path))
            .map_err(|error| format!("cross-module alias fixture must typecheck and lower: {error:?}"))?;
        let helper_code = modules
            .get(&helper_path)
            .ok_or_else(|| "helper module must be emitted".to_string())?;
        let identities = recover_incan_identities_from_generated_rust(helper_code);
        let calculate = identities
            .iter()
            .find(|identity| identity.declaration_name == "calculate")
            .ok_or_else(|| "provider function must carry a projection".to_string())?;
        let projection = encode_incan_symbol_identity(calculate);
        let compact_root = compact_rust(&root_code);

        assert!(helper_code.contains(&format!("fn {projection}")), "{helper_code}");
        assert!(
            compact_root.contains(&format!("usecrate::helpers::{projection};")),
            "{root_code}"
        );
        assert!(
            !compact_root.contains(&format!("{projection}as{projection}")),
            "{root_code}"
        );
        assert!(compact_root.contains(&format!("{projection}(41,)")), "{root_code}");
        Ok(())
    }

    #[test]
    fn decorated_method_declaration_and_call_share_one_projection() -> TestResult {
        let source = load_test_file("user_defined_method_decorators");
        let rust_code = generate_rust(&source);
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "label")
            .ok_or_else(|| "decorated source method must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(label);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            rust_code.matches(&format!(".{projection}")).count() >= 1,
            "method call must use the compiler-derived declaration projection:\n{rust_code}"
        );
        assert!(
            rust_code.contains("fn __incan_original_label"),
            "decorator method helper must remain non-Incan and distinct:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn ordinary_method_declaration_and_call_share_one_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
class Counter:
  value: int

  def next(self) -> int:
    return self.value + 1

def main() -> None:
  counter: Counter = Counter(value=41)
  _ = counter.next()
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let next = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "next")
            .ok_or_else(|| "ordinary source method must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(next);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact_rust(&rust_code).contains(&format!(".{projection}()")),
            "concrete method call must use the compiler-derived declaration projection:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn public_inherent_methods_keep_source_spelled_rust_forwarders() -> TestResult {
        let rust_code = generate_projected_registry_rust(
            r#"
pub model Counter:
  value: int

  @staticmethod
  def make(value: int) -> Counter:
    return Counter(value=value)

  def next(self) -> int:
    return self.value + 1

  pub property current -> int:
    return self.value

def main() -> None:
  counter = Counter.make(41)
  println(counter.next())
  println(counter.current)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let compact = compact_rust(&rust_code);

        let mut projections = std::collections::HashMap::new();
        for (source_name, kind) in [
            ("make", SemanticSourceTargetKind::Method),
            ("next", SemanticSourceTargetKind::Method),
            ("current", SemanticSourceTargetKind::Property),
        ] {
            let identity = identities
                .iter()
                .find(|identity| identity.kind == kind && identity.declaration_name == source_name)
                .ok_or_else(|| format!("missing canonical identity for Counter.{source_name}"))?;
            let projection = encode_incan_symbol_identity(identity);
            assert_eq!(
                rust_code.matches(&format!("fn {projection}")).count(),
                1,
                "the canonical method must remain the sole authored implementation:\n{rust_code}"
            );
            assert_eq!(
                rust_code.matches(&format!("fn {source_name}")).count(),
                1,
                "the native Rust surface must retain one source-spelled forwarding entry point:\n{rust_code}"
            );
            projections.insert(source_name, projection);
        }
        assert!(
            compact.contains(&format!(
                "pubfnmake(value:i64)->Counter{{Self::{}(value,)}}",
                projections["make"]
            )),
            "the static source entry point must forward to its canonical target:\n{rust_code}"
        );
        for source_name in ["next", "current"] {
            assert!(
                compact.contains(&format!(
                    "pubfn{source_name}(&self)->i64{{self.{}()}}",
                    projections[source_name]
                )),
                "the instance source entry point must forward to its canonical target:\n{rust_code}"
            );
        }
        Ok(())
    }

    #[test]
    fn classmethod_and_staticmethod_declarations_and_calls_share_their_projections() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
class Factory:
  @classmethod
  def answer(cls) -> int:
    return 42

  @staticmethod
  def twice(value: int) -> int:
    return value * 2

def main() -> None:
  answer = Factory.answer()
  twice = Factory.twice(21)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let answer = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "answer")
            .ok_or_else(|| "classmethod must retain its canonical identity".to_string())?;
        let twice = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "twice")
            .ok_or_else(|| "staticmethod must retain its canonical identity".to_string())?;
        let answer_projection = encode_incan_symbol_identity(answer);
        let twice_projection = encode_incan_symbol_identity(twice);
        let compact = compact_rust(&rust_code);

        assert_eq!(
            rust_code.matches(&format!("fn {answer_projection}")).count(),
            1,
            "{rust_code}"
        );
        assert_eq!(
            rust_code.matches(&format!("fn {twice_projection}")).count(),
            1,
            "{rust_code}"
        );
        assert!(
            compact.contains(&format!("Factory::{answer_projection}()")),
            "{rust_code}"
        );
        assert!(
            compact.contains(&format!("Factory::{twice_projection}(21,)")),
            "{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn static_factory_projection_coexists_with_same_named_instance_field() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
type Days = newtype int

model TimeDelta:
  pub days: Days

  @staticmethod
  def days(value: Days) -> TimeDelta:
    return TimeDelta(days=value)

def main() -> None:
  delta = TimeDelta.days(-7)
  println(f"{delta.days.0}")
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let factory = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "days")
            .ok_or_else(|| "same-named static factory must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(factory);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact.contains("pubdays:Days"),
            "the instance field must remain ordinary stored data:\n{rust_code}"
        );
        assert!(
            compact.contains(&format!("TimeDelta::{projection}(Days(-7),)")),
            "the type-owned call must select the factory and preserve implicit newtype construction:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn type_owned_and_instance_owned_methods_with_one_spelling_project_separately() -> TestResult {
        let rust_code = generate_projected_registry_rust(
            r#"
model Counter:
  value: int

  @staticmethod
  def next(value: int) -> Counter:
    return Counter(value=value)

  def next(self) -> int:
    return self.value + 1

def main() -> None:
  counter = Counter.next(4)
  println(counter.next())
"#,
            "app.main",
        );
        let projections = recover_incan_identities_from_generated_rust(&rust_code)
            .into_iter()
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "next")
            .map(|identity| encode_incan_symbol_identity(&identity))
            .collect::<Vec<_>>();
        let compact = compact_rust(&rust_code);

        assert_eq!(
            projections.len(),
            2,
            "both method declarations must retain identities:\n{rust_code}"
        );
        assert!(
            projections
                .iter()
                .any(|projection| compact.contains(&format!("Counter::{projection}(4"))),
            "the type receiver must select the static declaration:\n{rust_code}"
        );
        assert!(
            projections
                .iter()
                .any(|projection| compact.contains(&format!("counter.{projection}()"))),
            "the instance receiver must select the receiver-bearing declaration:\n{rust_code}"
        );
        assert_eq!(
            rust_code.matches("fn next").count(),
            0,
            "Rust cannot expose one raw inherent name for distinct type-owned and instance-owned declarations:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn field_factory_and_accumulator_with_one_spelling_codegen_as_a_fluent_chain() -> TestResult {
        let rust_code = generate_projected_registry_rust(
            r#"
model TimeDelta:
  days: int

  @staticmethod
  def days(value: int) -> TimeDelta:
    return TimeDelta(days=value)

  def days(self, value: int) -> TimeDelta:
    return TimeDelta(days=self.days + value)

def main() -> None:
  delta = TimeDelta.days(-7).days(2)
  println(delta.days)
"#,
            "app.main",
        );
        let projections = recover_incan_identities_from_generated_rust(&rust_code)
            .into_iter()
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "days")
            .map(|identity| encode_incan_symbol_identity(&identity))
            .collect::<Vec<_>>();
        let compact = compact_rust(&rust_code);

        assert_eq!(
            projections.len(),
            2,
            "the type-owned factory and instance accumulator must retain distinct identities:\n{rust_code}"
        );
        assert!(
            projections
                .iter()
                .any(|projection| compact.contains(&format!("TimeDelta::{projection}(-7"))),
            "the first call in the chain must select the type-owned factory:\n{rust_code}"
        );
        assert!(
            projections
                .iter()
                .any(|projection| compact.contains(&format!(".{projection}(2"))),
            "the second call in the chain must select the instance accumulator:\n{rust_code}"
        );
        assert!(
            compact.contains("println!(\"{}\",delta.days)"),
            "the terminal field access must select stored data rather than either callable member:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn newtype_associated_declaration_and_call_share_one_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
type Positive = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value <= 0:
      return Err(ValidationError("value must be positive"))
    return Ok(Positive(value))

def main() -> None:
  _ = Positive.from_underlying(1)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let from_underlying = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "from_underlying"
            })
            .ok_or_else(|| "newtype associated method must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(from_underlying);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact.contains(&format!("Positive::{projection}(1,)")),
            "newtype associated call must use the compiler-derived declaration projection:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn validated_newtype_implicit_coercion_calls_the_hook_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
type Positive = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value <= 0:
      return Err(ValidationError("value must be positive"))
    return Ok(Positive(value))

def accept(value: Positive) -> None:
  return

def main() -> None:
  accept(1)
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let from_underlying = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "from_underlying"
            })
            .ok_or_else(|| "validated-newtype hook must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(from_underlying);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact.contains(&format!("Positive::{projection}(1,)")),
            "implicit coercion must call the exact checked-hook projection:\n{rust_code}"
        );
        assert!(
            !compact.contains("Positive::from_underlying("),
            "implicit coercion must not bypass the projected declaration:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn method_alias_calls_the_target_projection_without_a_second_definition() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
model User:
  name: str
  short = label

  def label(self) -> str:
    return self.name

def main() -> None:
  user = User(name="Ada")
  _ = user.short()
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "label")
            .ok_or_else(|| "method alias target must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(label);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(compact.contains(&format!(".{projection}()")), "{rust_code}");
        assert!(
            !rust_code.contains("fn short"),
            "binding alias must not create a method declaration:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn method_partial_uses_an_explicit_generated_wrapper_and_preserves_the_target_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
model User:
  name: str
  short = partial label(prefix=1)

  def label(self, prefix: int) -> str:
    return self.name

def main() -> None:
  user = User(name="Ada")
  _ = user.short()
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "label")
            .ok_or_else(|| format!("method partial target must retain its canonical identity:\n{rust_code}"))?;
        let projection = encode_incan_symbol_identity(label);
        let compact = compact_rust(&rust_code);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            rust_code.contains("fn short"),
            "method partial must emit its generated forwarding helper:\n{rust_code}"
        );
        assert!(
            compact.contains(".short(1)"),
            "method partial call must target its forwarding helper:\n{rust_code}"
        );
        assert!(compact.contains(&format!(".{projection}(prefix,)")), "{rust_code}");
        assert!(
            identities.iter().all(|identity| identity.declaration_name != "short"),
            "a method-partial binding must not mint a second source declaration identity: {identities:?}"
        );
        Ok(())
    }

    #[test]
    fn computed_property_getter_and_access_share_one_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
model Account:
  cents: int

  property dollars -> int:
    return self.cents

def main() -> None:
  account: Account = Account(cents=100)
  _ = account.dollars
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let dollars = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Property && identity.declaration_name == "dollars"
            })
            .ok_or_else(|| "computed property getter must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(dollars);

        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact_rust(&rust_code).contains(&format!(".{projection}()")),
            "computed property access must call the compiler-derived projection:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn trait_method_keeps_abi_slot_and_exposes_one_recoverable_concrete_projection() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
trait Labelled:
  def label(self) -> str

class Item with Labelled:
  value: str

  def label(self) -> str:
    return self.value

def main() -> None:
  item: Item = Item(value="ready")
  _ = item.label()
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "label")
            .ok_or_else(|| "concrete trait implementation method must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(label);

        assert!(
            rust_code.contains("fn label(&self)"),
            "Rust trait ABI slot must retain its declared name:\n{rust_code}"
        );
        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact_rust(&rust_code).contains(&format!(".{projection}()")),
            "concrete method call must use the recoverable projection rather than guess the trait slot name:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn trait_targeted_method_overloads_keep_distinct_recoverable_projections() {
        let rust_code = generate_rust(&load_test_file("rfc043_newtype_trait_targets"));
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let convert_identities = identities
            .iter()
            .filter(|identity| {
                identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "convert"
            })
            .collect::<Vec<_>>();

        assert_eq!(
            convert_identities.len(),
            2,
            "each targeted source method needs its own canonical projection"
        );
        for identity in convert_identities {
            let projection = encode_incan_symbol_identity(identity);
            assert_eq!(
                rust_code.matches(&format!("fn {projection}")).count(),
                1,
                "each targeted method declaration must materialize once:\n{rust_code}"
            );
        }
    }

    #[test]
    fn a_type_owned_call_with_explicit_type_arguments_projects_like_its_declaration() -> TestResult {
        // `FactoryBox[int].make(1)` and `FactoryBox.make(1)` call the same declaration; the subscript only supplies
        // type arguments. The subscripted receiver used to resolve to `Unknown`, so method resolution matched no
        // declaration and recorded no identity, and lowering emitted the source spelling against a declaration that
        // was emitted under its projection.
        let rust_code = generate_registry_rust(
            r#"
@derive(Clone)
class FactoryBox[T with Clone]:
  pub value: T

  @classmethod
  def make(cls, value: T) -> Self:
    return cls(value=value)

def main() -> None:
  boxed = FactoryBox[int].make(1)
  println(f"{boxed.value}")
"#,
            "app.main",
        );

        assert!(
            rust_code.contains("FactoryBox::__incan_v1_"),
            "an explicitly instantiated type-owned call must name its declaration's projection:\n{rust_code}"
        );
        assert!(
            !rust_code.contains("FactoryBox::make("),
            "the pre-projection source spelling names no emitted function:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn a_derived_method_keeps_its_source_spelling_instead_of_a_wrapper_that_does_not_exist() -> TestResult {
        // `@derive(Default)` produces a Rust `Default` impl, not an Incan wrapper, so `Point.default()` has no
        // recoverable slot on `Point`. Projecting it named `std.derives.copying.default` -- a real declaration with
        // nothing emitted for it here -- and the generated Rust did not compile. This is the shape that broke the
        // RFC 023 parity fixture, minimized to run without Oven.
        let rust_code = generate_registry_rust(
            r#"
@derive(Default)
model Point:
  x: int = 0

def main() -> None:
  p = Point.default()
  println(f"{p.x}")
"#,
            "app.main",
        );

        assert!(
            rust_code.contains("Point::default()"),
            "a derived associated function must keep its source spelling:\n{rust_code}"
        );
        assert!(
            !rust_code.contains("Point::__incan_v1_"),
            "a derived associated function has no recoverable wrapper, so none may be named:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn adopted_default_method_exposes_a_recoverable_projection_beside_the_trait_slot() -> TestResult {
        let rust_code = generate_registry_rust(
            r#"
trait Labelled:
  def label(self) -> str:
    return "default"

class Item with Labelled:
  value: str

def main() -> None:
  item: Item = Item(value="ready")
  _ = item.label()
"#,
            "app.main",
        );
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let label = identities
            .iter()
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "label")
            .ok_or_else(|| "an adopted Incan default method must retain its declaration identity".to_string())?;
        let projection = encode_incan_symbol_identity(label);

        assert!(
            rust_code.contains("fn label(&self)"),
            "Rust trait ABI slot must retain its declared name:\n{rust_code}"
        );
        assert_eq!(rust_code.matches(&format!("fn {projection}")).count(), 1, "{rust_code}");
        assert!(
            compact_rust(&rust_code).contains(&format!(".{projection}()")),
            "concrete calls to an adopted default must use the recoverable projection:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn rust_extern_wrapper_is_projected_but_delegated_rust_symbol_is_not() -> TestResult {
        let source = load_test_file("rust_extern_delegation");
        let rust_code = generate_rust(&source);
        let identities = recover_incan_identities_from_generated_rust(&rust_code);
        let fail_t = identities
            .iter()
            .find(|identity| {
                identity.kind == SemanticSourceTargetKind::Function && identity.declaration_name == "fail_t"
            })
            .ok_or_else(|| "Incan extern wrapper must retain its canonical identity".to_string())?;
        let projection = encode_incan_symbol_identity(fail_t);

        assert!(rust_code.contains(&format!("fn {projection}")), "{rust_code}");
        assert!(rust_code.contains("incan_stdlib::testing::fail_t"), "{rust_code}");
        assert!(
            !rust_code.contains(&format!("incan_stdlib::testing::{projection}")),
            "{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn reexport_chain_preserves_the_provider_projection_without_alias_guessing() -> TestResult {
        let provider_ast = parse_incan_program(
            "pub def calculate(value: int) -> int:\n  return value + 1\n",
            "function reexport provider",
        );
        let facade_ast = parse_incan_program(
            "pub from provider import calculate as facade_calculate\n",
            "function reexport facade",
        );
        let public_api_ast = parse_incan_program(
            "pub from facade import facade_calculate as exported_calculate\n",
            "function reexport public API",
        );
        let consumer_ast = parse_incan_program(
            "from crate.public_api import exported_calculate as compute\n\ndef main() -> None:\n  _ = compute(41)\n",
            "function reexport consumer",
        );
        let provider_path = vec!["provider".to_string()];
        let facade_path = vec!["facade".to_string()];
        let public_api_path = vec!["public_api".to_string()];
        let dependency_paths = vec![provider_path.clone(), facade_path.clone(), public_api_path.clone()];
        let mut codegen = codegen_with_builtin_stdlib_inventory();
        codegen.add_module_with_path_segments("provider", &provider_ast, provider_path.clone());
        codegen.add_module_with_path_segments("facade", &facade_ast, facade_path.clone());
        codegen.add_module_with_path_segments("public_api", &public_api_ast, public_api_path.clone());
        let (consumer_code, modules) = codegen
            .try_generate_multi_file_nested(&consumer_ast, &dependency_paths)
            .map_err(|error| format!("function reexport chain must typecheck and lower: {error:?}"))?;
        let provider_code = modules
            .get(&provider_path)
            .ok_or_else(|| "provider module must be emitted".to_string())?;
        let facade_code = modules
            .get(&facade_path)
            .ok_or_else(|| "facade module must be emitted".to_string())?;
        let public_api_code = modules
            .get(&public_api_path)
            .ok_or_else(|| "public API module must be emitted".to_string())?;
        let identities = recover_incan_identities_from_generated_rust(provider_code);
        let calculate = identities
            .iter()
            .find(|identity| identity.declaration_name == "calculate")
            .ok_or_else(|| "provider declaration must carry a projection".to_string())?;
        let projection = encode_incan_symbol_identity(calculate);
        let compact_facade = compact_rust(facade_code);
        let compact_public_api = compact_rust(public_api_code);
        let compact_consumer = compact_rust(&consumer_code);

        assert!(provider_code.contains(&format!("fn {projection}")), "{provider_code}");
        assert!(compact_facade.contains(&projection), "{facade_code}");
        assert!(compact_public_api.contains(&projection), "{public_api_code}");
        assert!(
            compact_consumer.contains(&format!("{projection}(41,)")),
            "{consumer_code}"
        );
        assert!(
            !compact_facade.contains(&format!("{projection}as{projection}")),
            "{facade_code}"
        );
        assert!(
            !compact_public_api.contains(&format!("{projection}as{projection}")),
            "{public_api_code}"
        );
        Ok(())
    }
}

#[test]
fn test_decorated_variadic_function_codegen() {
    let source = load_test_file("decorated_variadic_function");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("decorated_variadic_function", rust_code);
}

#[test]
fn test_user_defined_method_decorators_codegen() {
    let source = load_test_file("user_defined_method_decorators");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("user_defined_method_decorators", rust_code);
}

#[test]
fn test_user_defined_mutable_method_decorators_codegen() {
    let source = load_test_file("user_defined_mutable_method_decorators");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("user_defined_mutable_method_decorators", rust_code);
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
fn test_rfc070_result_unwrap_codegen_does_not_require_debug_err() {
    let source = r#"
model PlainError:
  message: str

pub def direct(result: Result[int, PlainError]) -> int:
  return result.unwrap()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.split_whitespace().collect::<String>();
    assert!(
        compact.contains("matchresult{Ok(__incan_ok)=>__incan_ok,Err(_)=>panic!"),
        "Result.unwrap should lower to an explicit match that discards Err without a Debug bound:\n{rust_code}"
    );
    assert!(
        !compact.contains("result.unwrap()"),
        "Result.unwrap should not lower to Rust unwrap(), which requires E: Debug:\n{rust_code}"
    );
}

#[test]
fn test_rfc070_result_inspect_non_copy_observer_borrows_payload() -> TestResult {
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
    let borrowed_adapter = rust_code
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .find(|token| token.starts_with("__incan_borrow_adapter_"))
        .ok_or_else(|| {
            std::io::Error::other(
                "non-Copy named observer callbacks should retain one generated borrowed function adapter",
            )
        })?;
    assert!(
        rust_code.contains(&format!("fn {borrowed_adapter}(\n    _: &Payload")),
        "non-Copy named observer callbacks should get a generated borrowed function adapter:\n{rust_code}"
    );
    assert!(
        rust_code.contains("crate::__incan_std::result::inspect(") && rust_code.matches(borrowed_adapter).count() == 2,
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
    Ok(())
}

#[test]
fn test_dict_operations_codegen() {
    let source = load_test_file("dict_operations");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("dict_operations", rust_code);
}

#[test]
fn test_model_struct_codegen() {
    let source = load_test_file("model_struct");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("model_struct", rust_code);
}

#[test]
fn test_uppercase_var_field_access_codegen() {
    let source = load_test_file("uppercase_var_field_access");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("uppercase_var_field_access", rust_code);
}

#[test]
fn test_model_with_alias_codegen() {
    let source = load_test_file("model_with_alias");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("model_with_alias", rust_code);
}

#[test]
fn test_model_with_serde_alias_codegen() {
    let source = load_test_file("model_with_serde_alias");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("model_with_serde_alias", rust_code);
}

#[test]
fn test_model_alias_expressions_codegen() {
    // RFC 021: Test alias-aware expression lowering (constructor, field access, patterns)
    let source = load_test_file("model_alias_expressions");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("model_alias_expressions", rust_code);
}

#[test]
fn test_model_alias_self_access_codegen() {
    // RFC 021: Ensure `self.<alias>` field access lowers to canonical field name
    let source = load_test_file("model_alias_self_access");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("model_alias_self_access", rust_code);
}

#[test]
fn test_web_route_extractors_codegen() {
    let source = load_test_file("web_route_extractors");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("web_route_extractors", rust_code);
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
    assert_codegen_snapshot!("std_web_routing_compiled", rust_code);
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
fn imported_stdlib_associated_function_defaults_expand_in_generated_rust() -> TestResult {
    let source = r#"
from std.collections import OrdinalMapError

def main() -> None:
  error = OrdinalMapError.invalid_key_record("bad key")
  print(error.message())
"#;
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture should lex: {errors:?}")))?;
    let ast =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture should parse: {errors:?}")))?;
    let plan = incan::provider::ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["collections".to_string()]);
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(std::sync::Arc::new(plan));
    let generated = codegen.try_generate(&ast).map_err(|error| {
        std::io::Error::other(format!(
            "provider-bootstrap fixture should typecheck and lower: {error:?}"
        ))
    })?;
    let rust_code = normalize_projected_codegen_output(&generated);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    let identities = recover_incan_identities_from_generated_rust(&rust_code);
    let invalid_key_record = identities
        .iter()
        .find(|identity| {
            identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "invalid_key_record"
        })
        .ok_or_else(|| std::io::Error::other("the imported associated function must retain its canonical identity"))?;
    let projection = encode_incan_symbol_identity(invalid_key_record);
    assert!(
        compact.contains(&format!("OrdinalMapError::{projection}(\"badkey\".to_string(),-1,)")),
        "imported stdlib associated-function calls must expand omitted defaults:\n{rust_code}"
    );
    Ok(())
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.add_module_with_path_segments("api_routes", &routes_ast, vec!["api".to_string(), "routes".to_string()]);
    let Ok((main_code, _modules)) =
        codegen.try_generate_multi_file_nested(&main_ast, &[vec!["api".to_string(), "routes".to_string()]])
    else {
        panic!("codegen must succeed");
    };
    let rust_code = normalize_codegen_output(&main_code);
    assert_codegen_snapshot!("web_route_extractors_nested_module", rust_code);
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
    let mut codegen = codegen_with_builtin_stdlib_inventory();
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
    assert_codegen_snapshot!("async_main_runtime_bootstrap", rust_code);
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
    assert_codegen_snapshot!("literals", rust_code);
}

#[test]
fn test_operators_codegen() {
    let source = load_test_file("operators");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("operators", rust_code);
}

#[test]
fn test_user_defined_operators_codegen() {
    let source = load_test_file("user_defined_operators");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("user_defined_operators", rust_code);
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
        "(Counter{}).__iter__()",
        ".__next__()",
    ] {
        assert!(
            compact.contains(expected),
            "expected generated protocol hook call {expected}; generated:\n{rust_code}"
        );
    }
}

#[test]
fn test_fallible_iteration_protocol_propagates_next_errors_codegen() {
    let source = r#"
model ChunkStream:
  def __iter__(self) -> ChunkStream:
    return self

  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def main() -> Result[None, str]:
  for chunk in ChunkStream()?:
    seen = chunk
  return Ok(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains(".__next__()?"),
        "expected fallible for-loop lowering to propagate each __next__ error; generated:\n{rust_code}"
    );
}

#[test]
fn test_fallible_iteration_protocol_lowers_trait_typed_receiver_codegen() {
    let source = r#"
trait FallibleStream[T, E]:
  def __iter__(self) -> Self:
    return self

  def __next__(self) -> Result[Option[T], E]: ...

model ChunkStream with FallibleStream[int, str]:
  def __next__(self) -> Result[Option[int], str]:
    return Ok(None)

def chunks() -> FallibleStream[int, str]:
  return ChunkStream()

def main() -> Result[None, str]:
  for chunk in chunks()?:
    seen = chunk
  return Ok(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains(".__next__()?"),
        "expected trait-typed fallible loop lowering to propagate each __next__ error; generated:\n{rust_code}"
    );
}

#[test]
fn test_source_callable_bound_preserves_nominal_trait_with_native_callable_blanket_codegen() {
    let source = r#"
from std.traits.callable import Callable1

def apply[Mapper with (Clone, Callable1[int, str])](mapper: Mapper, value: int) -> str:
  return mapper(value)

@derive(Clone)
model Prefixer with Callable1[int, str]:
  prefix: str

  def __call__(self, value: int) -> str:
    return f"{self.prefix}:{value}"

def main() -> str:
  prefix = "item"
  closure_value = apply((value) => f"{prefix}:{value}", 3)
  return f"{closure_value}:{apply(Prefixer(prefix="model"), 4)}"
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("Mapper:Clone+Callable1<i64,String>"),
        "source Callable1 bounds must remain nominal in generated Rust:\n{rust_code}"
    );
    assert!(
        compact.contains("Callable1::<i64,String,>::__call__(&mapper,value)")
            && !compact.contains("Mapper:Clone+Fn(i64)->String"),
        "generic source callables must dispatch through their nominal hook:\n{rust_code}"
    );
    assert!(
        compact.contains("apply(|value:i64|"),
        "a direct closure checked through Callable1 must retain its resolved parameter type in Rust:\n{rust_code}"
    );
}

#[test]
fn test_std_fallible_loop_preserves_qualified_trait_dispatch_codegen() {
    let source = r#"
from std.derives.collection import FallibleIterator

model NumberStream with FallibleIterator[int, str]:
  def __next__(mut self) -> Result[Option[int], str]:
    return Ok(None)

def main() -> Result[None, str]:
  for value in NumberStream()?:
    println(value)
  return Ok(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    for method in ["__iter__", "__next__"] {
        let expected = format!("FallibleIterator::<i64,String,>::{method}");
        assert!(
            compact.contains(&expected),
            "stdlib fallible-loop hook {method} must retain qualified trait dispatch; generated:\n{rust_code}"
        );
    }
}

#[test]
fn generic_fallible_iterator_consumer_inherits_implementation_bounds_issue1280() {
    let source = r#"
from std.derives.collection import FallibleIterator

model Stream[R] with FallibleIterator[int, str]:
    value: R

    def __next__(mut self) -> Result[Option[int], str]:
        return Ok(None)

pub def drain[R](value: R) -> Result[int, str]:
    mut count = 0
    for _item in Stream[R](value=value)?:
        count += 1
    return Ok(count)

pub def relay[R](value: R) -> Result[int, str]:
    return drain(value)

def main() -> None:
    return
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("impl<R:Clone>FallibleIterator<i64,String>forStream<R>"),
        "the generic trait implementation must retain its backend-inferred Clone requirement:\n{rust_code}"
    );
    assert!(
        compact.contains("pubfndrain<R:Clone>(value:R)->Result<i64,String>"),
        "a generic caller consuming that implementation must inherit the same Clone requirement:\n{rust_code}"
    );
    assert!(
        compact.contains("pubfnrelay<R:Clone>(value:R)->Result<i64,String>"),
        "ordinary transitive callers must inherit the implementation requirement at the same fixed point:\n{rust_code}"
    );
}

#[test]
fn compiled_provider_consumer_inherits_manifest_implementation_bounds_issue1280()
-> Result<(), Box<dyn std::error::Error>> {
    use incan::frontend::library_manifest_index::{
        LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use incan::library_manifest::{
        ImplementationTraitBoundExport, ImplementationTraitBoundOriginExport, ImplementationTypeParamExport,
        LibraryManifest, ModelExport, TypeBoundExport, TypeParamExport, TypeRef,
    };
    use std::collections::HashMap;

    let source = r#"
from pub::streams import Stream

pub def drain[R](stream: Stream[R]) -> Result[int, str]:
    mut count = 0
    for _item in stream?:
        count += 1
    return Ok(count)

def main() -> None:
    return
"#;
    let ast = parse_incan_program(source, "compiled provider implementation-bound consumer");
    let mut manifest = LibraryManifest::new("streams_core", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Stream".to_string(),
        type_params: vec![TypeParamExport {
            name: "R".to_string(),
            bounds: Vec::new(),
        }],
        traits: vec!["FallibleIterator".to_string()],
        trait_adoptions: vec![TypeBoundExport {
            name: "FallibleIterator".to_string(),
            source_name: Some("FallibleIterator".to_string()),
            module_path: Some(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]),
            type_args: vec![
                TypeRef::Named {
                    name: "int".to_string(),
                },
                TypeRef::Named {
                    name: "str".to_string(),
                },
            ],
            implementation_type_params: vec![ImplementationTypeParamExport {
                name: "R".to_string(),
                bounds: vec![ImplementationTraitBoundExport {
                    trait_path: "Clone".to_string(),
                    type_args: Vec::new(),
                    associated_types: Vec::new(),
                    origin: ImplementationTraitBoundOriginExport::Standard,
                }],
            }],
        }],
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "streams".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "streams",
                "streams_core",
                std::env::temp_dir().join("incan_test_streams_artifacts"),
            ),
        },
    )]));
    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.set_library_manifest_index(index);
    let rust_code = normalize_projected_symbols_for_readable_codegen(&codegen.try_generate(&ast)?);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("pubfndrain<R:Clone>(stream:Stream<R>)->Result<i64,String>"),
        "a manifest-only consumer must inherit the exact implementation requirement:\n{rust_code}"
    );
    Ok(())
}

#[test]
fn test_std_fallible_adapter_loop_inherits_qualified_trait_dispatch_codegen() {
    let source = r#"
from std.io import BinaryReader

pub def consume[R with BinaryReader](reader: R) -> Result[None, str]:
  prefix = "read failed: "
  for chunk in reader.chunks(16).map_err((error) => f"{prefix}{error.detail}")?:
    println(len(chunk))
  return Ok(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    for method in ["__iter__", "__next__"] {
        assert!(
            compact.contains(&format!("FallibleIterator::<Vec<u8>,String,>::{method}")),
            "a fallible adapter loop must retain qualified {method} dispatch from the source trait call:\n{rust_code}"
        );
    }
}

#[test]
fn test_fallible_iterator_adapter_chain_codegen() {
    let source = r#"
trait FallibleStream[T, E]:
  def __next__(mut self) -> Result[Option[T], E]: ...

  def map[U with Clone](self, f: (T) -> U) -> FallibleStream[U, E]:
    return MappedStream[T, E, Self, U](source=self, f=f)

  def collect(mut self) -> Result[list[T], E]:
    mut items: list[T] = []
    while true:
      match self.__next__():
        Ok(Some(item)) => items.append(item)
        Ok(None) => return Ok(items)
        Err(error) => return Err(error)

  def take(self, count: int) -> FallibleStream[T, E]:
    return self

  def inspect(self, f: (T) -> None) -> FallibleStream[T, E]:
    return self

  def inspect_err(self, f: (E) -> None) -> FallibleStream[T, E]:
    return self

  def map_err[F with Clone](self, f: (E) -> F) -> FallibleStream[T, F]:
    return ErrorMappedStream[T, E, Self, F](source=self, f=f, item_marker=None)

model NumberStream with FallibleStream[int, str]:
  items: list[int]
  index: int

  def __next__(mut self) -> Result[Option[int], str]:
    if self.index >= len(self.items):
      return Ok(None)
    item = self.items[self.index]
    self.index += 1
    return Ok(Some(item))

model MappedStream[T, E, Source with FallibleStream[T, E], Output] with FallibleStream[Output, E]:
  source: Source
  f: (T) -> Output
  error_marker: Option[E] = None

  def __next__(mut self) -> Result[Option[Output], E]:
    match self.source.__next__():
      Ok(Some(item)) =>
        transform = self.f
        return Ok(Some(transform(item)))
      Ok(None) => return Ok(None)
      Err(error) => return Err(error)

model ErrorMappedStream[T, E, Source with FallibleStream[T, E], MappedError] with FallibleStream[T, MappedError]:
  source: Source
  f: (E) -> MappedError
  item_marker: Option[T] = None

  def __next__(mut self) -> Result[Option[T], MappedError]:
    match self.source.__next__():
      Ok(next) => return Ok(next)
      Err(error) =>
        transform = self.f
        return Err(transform(error))

def double(value: int) -> int:
  return value * 2

def observe_value(value: int) -> None:
  pass

def observe_error(error: str) -> None:
  pass

def main() -> None:
  pipeline = NumberStream(items=[1, 2], index=0).map(double).take(1).inspect(observe_value).inspect_err(observe_error)
  match pipeline.collect():
    Ok(values) => println(len(values))
    Err(error) => println(error)

  stream = NumberStream(items=[3], index=0).map(double)
  match stream.collect():
    Ok(values) => println(len(values))
    Err(error) => println(error)

  errors = NumberStream(items=[], index=0).map_err((error) => f"mapped:{error}")
  match errors.collect():
    Ok(values) => println(len(values))
    Err(error) => println(error)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains(".map(")
            && compact.contains(".take(1).inspect(observe_value).inspect_err(observe_error)")
            && compact.contains(".map_err(|error|"),
        "expected fallible adapter chain to remain source-owned; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("u64::try_from") && !compact.contains("__incan_std::result::inspect"),
        "semantic trait dispatch must win over same-name Iterator and Result helpers; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("letmutstream=") && compact.contains("stream.collect()"),
        "a selected mut-self trait terminal must make its local receiver mutable; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("implFallibleStream<U,String>+use<U>"),
        "opaque adapter returns must exclude the receiver lifetime through precise captures; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("pubfnmap<U:Clone>(&self,f:fn(i64)->U,)->implFallibleStream<U,String>+use<U>"),
        "expected the projected trait adapter to specialize adopter arguments and exclude the receiver lifetime; generated:\n{rust_code}"
    );
}

#[test]
fn test_projected_fallible_terminal_uses_mutable_receiver() {
    let source = r#"
from std.derives.collection import FallibleIterator

model NumberStream with FallibleIterator[int, str]:
  items: list[int]
  index: int

  def __next__(mut self) -> Result[Option[int], str]:
    if self.index >= len(self.items):
      return Ok(None)
    item = self.items[self.index]
    self.index += 1
    return Ok(Some(item))

def main() -> None:
  match NumberStream(items=[1], index=0).collect():
    Ok(values) => println(len(values))
    Err(error) => println(error)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("pubfncollect(&mutself)->Result<Vec<i64>,String>")
            && compact.contains("(NumberStream{items:vec![1],index:0,}).collect()"),
        "a projected mut-self trait terminal must preserve a mutable receiver contract; generated:\n{rust_code}"
    );
}

#[test]
fn test_mixed_numeric_codegen() {
    let source = load_test_file("mixed_numeric");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("mixed_numeric", rust_code);
}

#[test]
fn test_std_math_codegen() {
    let source = load_test_file("std_math");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_math", rust_code);
}

#[test]
fn test_std_fs_import_codegen() {
    let source = load_test_file("std_fs_import");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("pub use crate::__incan_std::fs::Path;"),
        "std.fs Path import should emit through the compiled stdlib artifact; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_std::web::Path"),
        "std.fs Path must not reuse the std.web Path extractor path; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("std_fs_import", rust_code);
}

#[test]
fn test_std_tempfile_import_codegen() {
    let source = load_test_file("std_tempfile_import");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::NamedTemporaryFile;"),
        "std.tempfile NamedTemporaryFile import should emit through the compiled stdlib artifact; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::TemporaryDirectory;"),
        "std.tempfile TemporaryDirectory import should emit through the compiled stdlib artifact; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::tempfile::SpooledTemporaryFile;"),
        "std.tempfile SpooledTemporaryFile import should emit through the compiled stdlib artifact; generated:\n{rust_code}"
    );
    assert!(
        rust_code.contains("pub use crate::__incan_std::fs::Path;"),
        "std.tempfile call sites should use the compiled std.fs Path value; generated:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("__incan_std::web::Path"),
        "std.tempfile must not reuse the std.web Path extractor path; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("std_tempfile_import", rust_code);
}

#[test]
fn test_function_calls_codegen() {
    let source = load_test_file("function_calls");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("function_calls", rust_code);
}

#[test]
fn test_variadic_calls_codegen() {
    let source = load_test_file("variadic_calls");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("variadic_calls", rust_code);
}

#[test]
fn test_collections_codegen() {
    let source = load_test_file("collections");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("collections", rust_code);
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
fn test_issue633_question_mark_list_comprehension_codegen_uses_loop() {
    let source = r#"
def parse_value(value: int) -> Result[int, str]:
    return Ok(value)


def parse_all(values: list[int]) -> Result[list[int], str]:
    return Ok([parse_value(value)? for value in values])


def main() -> None:
    match parse_all([1, 2, 3]):
        Ok(values) => println(values[0])
        Err(err) => println(err)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.split_whitespace().collect::<String>();
    assert!(
        compact.contains("letmut__incan_list=Vec::new();forvaluein(values).iter().copied(){__incan_list.push(parse_value(value)?);}__incan_list"),
        "expected issue633 comprehension to lower to an outer-function loop, got:\n{rust_code}"
    );
    assert!(
        !compact.contains(".map(|value|parse_value(value)?)"),
        "question-mark comprehension must not lower into an element-returning Rust map closure:\n{rust_code}"
    );
}

#[test]
fn test_list_repeat_codegen() {
    let source = load_test_file("list_repeat");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("list_repeat", rust_code);
}

#[test]
fn test_rfc088_iterator_adapters_codegen() {
    let source = load_test_file("rfc088_iterator_adapters");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc088_iterator_adapters", rust_code);
}

#[test]
fn test_issue950_953_iterator_adapter_sources_codegen() {
    let source = load_test_file("issue950_953_iterator_adapter_sources");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue950_953_iterator_adapter_sources", rust_code);
}

#[test]
fn test_issue951_set_constructor_codegen() {
    let source = load_test_file("issue951_set_constructor");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue951_set_constructor", rust_code);
}

#[test]
fn test_issue963_set_add_codegen() {
    let source = load_test_file("issue963_set_add");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue963_set_add", rust_code);
}

/// Assert that a public SHA-256 handle can be stored and mutated through a model field.
#[test]
fn test_issue969_storable_sha256_hasher_codegen() {
    let source = load_test_file("issue969_storable_sha256_hasher");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue969_storable_sha256_hasher", rust_code);
}

#[test]
fn test_issue951_set_shadowing_codegen() {
    let source = load_test_file("issue951_set_shadowing");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue951_set_shadowing", rust_code);
}

/// Issue #1116: generated Rust must retain a module `len` call while lowering `std.builtins.len` as the core builtin.
#[test]
fn test_issue1116_builtin_len_shadowing_codegen() {
    let source = load_test_file("issue1116_builtin_len_shadowing");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("fnlen(value:i64)->i64{"),
        "expected the module `len` definition to survive codegen; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("len(4)"),
        "expected an unqualified `len(4)` call to select the module binding; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("vec![10,20,30].len()asi64"),
        "expected `std.builtins.len` to select the core builtin; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("issue1116_builtin_len_shadowing", rust_code);
}

#[test]
fn builtin_json_stringify_evaluates_its_operand_once() {
    for call in [
        "json_stringify(next_value())",
        "std.builtins.json_stringify(next_value())",
    ] {
        let source = format!(
            r#"
def next_value() -> str:
  println("evaluated")
  return "line\né"

def main() -> str:
  return {call}
"#
        );
        let rust_code = generate_rust(&source);
        let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
        assert!(
            compact.contains("let__incan_json_value=&(next_value());"),
            "the emitted {call} must bind its operand before serialization; generated:\n{rust_code}"
        );
        assert_eq!(
            compact.matches("next_value()").count(),
            2,
            "the declaration and one {call} operand evaluation must be the only occurrences; generated:\n{rust_code}"
        );
    }
}

#[test]
fn builtin_json_stringify_gives_untyped_none_a_concrete_rust_type() {
    let source = r#"
def main() -> str:
  return json_stringify(None)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("let__incan_json_value=&(None::<()>);"),
        "an untyped Incan None must have a concrete serializable Rust type; generated:\n{rust_code}"
    );
}

#[test]
fn builtin_json_stringify_preserves_the_incan_int_width() {
    let source = r#"
def main() -> str:
  return json_stringify(9223372036854775807)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("let__incan_json_value:&i64=&(9223372036854775807);"),
        "an Incan int operand must retain its i64 width at the JSON boundary; generated:\n{rust_code}"
    );
}

#[test]
fn test_issue950_builtin_zip_only_codegen() {
    let source = load_test_file("issue950_builtin_zip_only");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue950_builtin_zip_only", rust_code);
}

#[test]
fn test_empty_list_string_arg_codegen() {
    let source = load_test_file("empty_list_string_arg");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("empty_list_string_arg", rust_code);
}

#[test]
fn test_generic_model_field_access_codegen() {
    let source = load_test_file("generic_model_field_access");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("generic_model_field_access", rust_code);
}

#[test]
fn test_lowercase_types_codegen() {
    let source = load_test_file("lowercase_types");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("lowercase_types", rust_code);
}

// ============================================================================
// Tests migrated from legacy codegen/statements/mod.rs tests
// ============================================================================

#[test]
fn test_assignments_codegen() {
    let source = load_test_file("assignments");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("assignments", rust_code);
}

#[test]
fn test_control_flow_codegen() {
    let source = load_test_file("control_flow");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("control_flow", rust_code);
}

#[test]
fn test_pattern_alternation_codegen() {
    let source = load_test_file("pattern_alternation");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("pattern_alternation", rust_code);
}

#[test]
fn test_rfc049_if_let_while_let_codegen() {
    let source = load_test_file("rfc049_if_let_while_let");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc049_if_let_while_let", rust_code);
}

#[test]
fn test_returns_codegen() {
    let source = load_test_file("returns");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("returns", rust_code);
}

#[test]
fn test_loops_codegen() {
    let source = load_test_file("loops");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("loops", rust_code);
}

#[test]
fn test_match_statements_codegen() {
    let source = load_test_file("match_statements");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("match_statements", rust_code);
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
    assert_codegen_snapshot!("type_annotations", rust_code);
}

#[test]
fn test_rfc029_union_types_codegen() {
    let source = load_test_file("rfc029_union_types");
    let rust_code = generate_rust(&source);
    assert!(
        !rust_code.contains("isinstance("),
        "union isinstance chains must fully lower before Rust emission:\n{rust_code}"
    );
    assert_codegen_snapshot!("rfc029_union_types", rust_code);
}

#[test]
fn isinstance_alias_target_uses_the_typecheckers_resolved_target_in_native_lowering()
-> Result<(), Box<dyn std::error::Error>> {
    let rust_code = generate_rust(
        r#"
type Whole = int

const STATIC_TEXT: str = "static text"

def isinstance(value: int, target: int) -> bool:
  return false

pub def probe(value: int | str) -> bool:
  return std.builtins.isinstance(value, Whole)

pub def narrow_union(value: int | str) -> str:
  if std.builtins.isinstance(value, str):
    return value.upper()
  return "number"

pub def narrow_option(value: int | str | None) -> str:
  if std.builtins.isinstance(value, int):
    return "number"
  else:
    if value is None:
      return "missing"
    else:
      return value.upper()

pub def static_text_is_str() -> bool:
  return std.builtins.isinstance(STATIC_TEXT, str)

pub def frozen_union_kind(value: FrozenStr | int) -> str:
  if std.builtins.isinstance(value, str):
    return str(value)
  return "number"

pub def frozen_option_kind(value: FrozenStr | None) -> str:
  if std.builtins.isinstance(value, str):
    return str(value)
  return "missing"

pub def frozen_option_union_kind(value: Option[FrozenStr | int]) -> str:
  if std.builtins.isinstance(value, str):
    return str(value)
  return "other"

pub def mixed_string_union_kind(value: FrozenStr | str | int) -> str:
  if std.builtins.isinstance(value, str):
    return str(value)
  return "number"

pub def mixed_string_option_union_kind(value: Option[FrozenStr | str | int]) -> str:
  if std.builtins.isinstance(value, str):
    return str(value)
  return "other"
"#,
    );
    assert!(
        !rust_code.contains("isinstance("),
        "the native expression route must consume the retained alias-expanded target rather than emit a raw call:\n{rust_code}"
    );
    assert!(
        rust_code.contains("matches!(value"),
        "the checked explicit builtin expression must lower to native union dispatch:\n{rust_code}"
    );
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        compact.contains("let_=STATIC_TEXT;true"),
        "semantic str matching must normalize the native const/static-string storage form:\n{rust_code}"
    );
    let (_, frozen_shapes) = compact
        .split_once("pubfnfrozen_union_kind")
        .ok_or("missing frozen-union isinstance regression function")?;
    let (frozen_union, frozen_options) = frozen_shapes
        .split_once("pubfnfrozen_option_kind")
        .ok_or("missing frozen-option isinstance regression function")?;
    assert!(
        frozen_union.contains("matchvalue") && frozen_union.contains("::V0(value)=>"),
        "frozen-string union statement lowering must select and bind its semantic str variant:\n{rust_code}"
    );
    let (frozen_option, frozen_option_union) = frozen_options
        .split_once("pubfnfrozen_option_union_kind")
        .ok_or("missing frozen option-union isinstance regression function")?;
    assert!(
        frozen_option.contains("matchvalue{Some(value)=>"),
        "optional frozen string must select its semantic str payload:\n{rust_code}"
    );
    assert!(
        frozen_option_union.contains("Some(__IncanUnion") && frozen_option_union.contains("::V0(value))=>"),
        "optional frozen-string union statement lowering must select and bind its semantic str variant:\n{rust_code}"
    );
    let (_, mixed_string_shapes) = compact
        .split_once("pubfnmixed_string_union_kind")
        .ok_or("missing mixed string-storage union isinstance regression function")?;
    let (mixed_string_union, mixed_string_option_union) = mixed_string_shapes
        .split_once("pubfnmixed_string_option_union_kind")
        .ok_or("missing optional mixed string-storage union isinstance regression function")?;
    assert_eq!(
        mixed_string_union.matches("returnvalue.to_string()").count(),
        2,
        "every matching string-storage union variant must execute the true branch:\n{rust_code}"
    );
    assert_eq!(
        mixed_string_option_union.matches("returnvalue.to_string()").count(),
        2,
        "every matching optional string-storage union variant must execute the true branch:\n{rust_code}"
    );
    Ok(())
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
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
    assert_codegen_snapshot!("string_operations", rust_code);
}

#[test]
fn test_issue236_non_string_join_codegen() {
    let source = load_test_file("issue236_non_string_join");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue236_non_string_join", rust_code);
}

/// Issue #244: recursive call with `mut` list args inside `while` must not emit `.clone()` for those args (snapshot is
/// the contract).
#[test]
fn test_issue244_recursive_mut_list_codegen() {
    let source = load_test_file("issue244_recursive_mut_list");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue244_recursive_mut_list", rust_code);
}

/// Issue #244 regression: mutable `str` params are passed by `&mut` and keep string conversions.
#[test]
fn test_issue244_mut_str_param_codegen() {
    let source = load_test_file("issue244_mut_str_param");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue244_mut_str_param", rust_code);
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
    assert_codegen_snapshot!("issue241_field_backed_method_arg_clone", rust_code);
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
    assert_codegen_snapshot!("issue364_filtered_list_comp_borrow", rust_code);
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
        &codegen_with_builtin_stdlib_inventory()
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
    assert_codegen_snapshot!("issue366_clone_self_string_field", rust_code);
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
    assert_codegen_snapshot!("filtered_dict_comp_predicate", rust_code);
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
    assert_codegen_snapshot!("issue602_comprehension_copy_hotpaths", rust_code);
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
    assert_codegen_snapshot!("issue602_owned_iterator_source_hotpaths", rust_code);
}

// ============================================================================
// Tests for declarations (functions, classes, models, traits, enums)
// ============================================================================

#[test]
fn test_functions_codegen() {
    let source = load_test_file("functions");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("functions", rust_code);
}

#[test]
fn test_classes_codegen() {
    let source = load_test_file("classes");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("classes", rust_code);
}

#[test]
fn test_issue246_class_field_visibility_codegen() {
    let source = load_test_file("issue246_class_field_visibility");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue246_class_field_visibility", rust_code);
}

#[test]
fn test_generic_methods_codegen() {
    let source = load_test_file("generic_methods");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("generic_methods", rust_code);
}

#[test]
fn test_issue731_generic_method_defaults_codegen() {
    let source = load_test_file("issue731_generic_method_defaults");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue731_generic_method_defaults", rust_code);
}

#[test]
fn test_explicit_call_site_generics_codegen() {
    let source = load_test_file("explicit_call_site_generics");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("explicit_call_site_generics", rust_code);
}

#[test]
fn test_models_codegen() {
    let source = load_test_file("models");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("models", rust_code);
}

/// Power lowers as a Rust method call, so its receiver must retain the source expression's grouping.
#[test]
fn test_power_receiver_parenthesisation_codegen() {
    let source = r#"
def compound(left: float, right: float) -> float:
    return (left + right) ** 0.5


def coerced(base: int, exponent: int) -> float:
    return base ** exponent


def main() -> None:
    println(compound(3.0, 4.0))
    println(coerced(2, 8))
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("return(left+right).powf(0.5);"),
        "compound power receiver must remain grouped; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("return((base)asf64).powf((exponent)asf64);"),
        "coerced power receiver must be parenthesized before the method call; generated:\n{rust_code}"
    );
}

#[test]
fn exact_float_boundaries_emit_finite_guards_without_changing_ordinary_float() {
    let source = r#"
pub def parsed_exact(value: str) -> f64:
    return float(value)

pub def widened_exact(value: f32) -> f64:
    return value

pub def exact_f32(value: f32) -> f32:
    return value

pub def ordinary(value: str) -> float:
    return float(value)
"#;
    let rust_code = generate_rust(source);
    assert_eq!(
        rust_code.matches("incan_stdlib::num::require_finite_f64").count(),
        2,
        "ordinary float must remain unguarded while exact f64 returns and f32 widening are guarded:\n{rust_code}"
    );
    assert_eq!(
        rust_code.matches("incan_stdlib::num::require_finite_f32").count(),
        3,
        "public exact f32 inputs and exact f32 returns must be guarded:\n{rust_code}"
    );
}

#[test]
fn exact_float_arithmetic_is_validated_before_every_observable_use() {
    let source = r#"
pub def returned_f32(left: f32, right: f32) -> f32:
    return left * right

pub def stored_f64(left: f64, right: f64) -> f64:
    value: f64 = left * right
    return value

pub def compared_f32(left: f32, right: f32) -> bool:
    return left * right > left

pub def printed_f64(left: f64, right: f64) -> None:
    println(left * right)
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(
        compact.contains("require_finite_f32(left*right)"),
        "exact f32 arithmetic must be checked before return or comparison:\n{rust_code}"
    );
    assert!(
        compact.contains("require_finite_f64(left*right)"),
        "exact f64 arithmetic must be checked before storage or printing:\n{rust_code}"
    );
    assert!(
        compact.contains(
            "println!(\"{}\",incan_stdlib::num::require_finite_f64(incan_stdlib::num::require_finite_f64(left*right)))"
        ),
        "print must not observe a non-finite exact f64 arithmetic result:\n{rust_code}"
    );
    assert!(
        compact.contains(">incan_stdlib::num::require_finite_f32(left)"),
        "comparison must not observe a non-finite exact f32 arithmetic result:\n{rust_code}"
    );
}

#[test]
fn exact_float_public_and_rust_ingress_is_guarded_before_observation() {
    let source = r#"
rust.module("incan_stdlib::num")

@rust.extern
pub def require_finite_f32(value: f32) -> f32:
    ...

pub def observe_exact(left: f32, right: f64) -> bool:
    println(left)
    rendered = str(right)
    formatted = f"{left}"
    return left < right

pub def observe_ieee(value: float) -> bool:
    println(value)
    return value < value
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    for expected in [
        "let_=incan_stdlib::num::require_finite_f32(value);",
        "incan_stdlib::num::require_finite_f32(incan_stdlib::num::require_finite_f32(value))",
        "let_=incan_stdlib::num::require_finite_f32(left);",
        "let_=incan_stdlib::num::require_finite_f64(right);",
        "println!(\"{}\",incan_stdlib::num::require_finite_f32(left))",
        "incan_stdlib::num::require_finite_f64(right).to_string()",
        "format!(\"{}\",incan_stdlib::num::require_finite_f32(left))",
        "((incan_stdlib::num::require_finite_f32(left))asf64)<incan_stdlib::num::require_finite_f64(right)",
    ] {
        assert!(
            compact.contains(expected),
            "exact public/Rust ingress and observation must be finite-checked ({expected}); generated:\n{rust_code}"
        );
    }
    assert!(
        compact.contains("pubfnobserve_ieee(value:f64)->bool{let_=println!(\"{}\",value);returnvalue<value;"),
        "ordinary float must retain unguarded IEEE observation behavior:\n{rust_code}"
    );
}

#[test]
fn exact_float_scalars_extracted_from_aggregates_are_guarded_before_use() {
    let source = r#"
rust.module("incan_stdlib::num")

@rust.extern
def consume_exact(value: f32) -> f32:
    ...

pub model ExactSamples:
    pub narrow: f32
    pub wide: f64
    pub maybe: Option[f32]

pub def observe_aggregate(samples: ExactSamples, values: list[f64]) -> bool:
    forwarded = consume_exact(samples.narrow)
    return not samples.narrow.is_nan() and values[0].is_finite() and samples.wide.is_finite() and forwarded.is_finite()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    for expected in [
        "require_finite_f32(samples.narrow).is_nan()",
        "require_finite_f64(*incan_stdlib::collections::list_get(&values,(0)asi64)",
        "require_finite_f64(samples.wide).is_finite()",
        "consume_exact(incan_stdlib::num::require_finite_f32",
    ] {
        assert!(
            compact.contains(expected),
            "exact aggregate scalar use must retain its finite guard ({expected}); generated:\n{rust_code}"
        );
    }
    assert_eq!(
        compact.matches("require_finite_f32(self.narrow)").count(),
        2,
        "field value reflection and field-item reflection must guard exact f32 values:\n{rust_code}"
    );
    assert_eq!(
        compact.matches("require_finite_f64(self.wide)").count(),
        2,
        "field value reflection and field-item reflection must guard exact f64 values:\n{rust_code}"
    );
    assert_eq!(
        compact.matches("require_finite_f32(*value)").count(),
        2,
        "optional exact field reflection must guard each present value:\n{rust_code}"
    );
}

#[test]
fn mixed_f32_arithmetic_widens_operands_for_f64_codegen() {
    let source = r#"
pub def with_f64(left: f32, right: f64) -> float:
    added = left + right
    divided = left / right
    floored = left // right
    remainder = left % right
    return left ** right

pub def with_float(left: f32, right: float) -> float:
    return left + right

pub def with_int(left: f32, right: int) -> float:
    return left + right
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    for expected in [
        "(left)asf64+right",
        "incan_stdlib::num::py_div((left)asf64,right)",
        "incan_stdlib::num::py_floor_div_f64((left)asf64,right)",
        "incan_stdlib::num::py_mod_f64((left)asf64,right)",
        "((left)asf64).powf(right)",
        "return(left)asf64+right;",
        "return(left)asf64+(right)asf64;",
    ] {
        assert!(
            compact.contains(expected),
            "mixed exact/broad arithmetic must emit concrete f64 operands ({expected}); generated:\n{rust_code}"
        );
    }
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
    assert_codegen_snapshot!("rfc046_computed_properties", rust_code);
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
    assert_codegen_snapshot!("list_pop_clone_only_model", rust_code);
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
    assert_codegen_snapshot!("list_clone_model", rust_code);
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
    assert_codegen_snapshot!("issue380_len_comparison", rust_code);
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
    assert_codegen_snapshot!("issue383_loop_helper_shared_string_list", rust_code);
}

/// Issue #383 follow-on: dict comprehensions must clone non-Copy keys before reading them in the value expression.
#[test]
fn test_issue383_dict_comp_reuses_noncopy_key_codegen() {
    let source = load_test_file("issue383_dict_comp_reuses_noncopy_key");
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains(".map(|name| (name.clone(), incan_stdlib::strings::str_len(&(name))))"),
        "expected dict comprehension to clone the non-Copy key before reading it again in the value expression; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("issue383_dict_comp_reuses_noncopy_key", rust_code);
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
    assert_codegen_snapshot!("for_in_list_enum_equality", rust_code);
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
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

    assert_codegen_snapshot!("issue372_imported_enum_loop_ownership", rust_code);
}

#[test]
/// Keep source-package class construction on the complete provider-owned constructor surface.
fn test_issue886_imported_private_source_class_constructor_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = r#"
pub class Vault:
  secret: str = "sealed"
  pub label: str
  revision: int = 7
"#;
    let facade_source = "pub from provider import Vault as FacadeVault\n";
    let public_api_source = "pub from facade import FacadeVault as ExportedVault\n";
    let consumer_source = r#"
from crate.public_api import ExportedVault as PublicVault

def main() -> None:
  vault = PublicVault(label="visible", revision=9)
  println(vault.label)
"#;
    let provider_ast = parse_incan_program(provider_source, "private-class provider");
    let facade_ast = parse_incan_program(facade_source, "private-class facade");
    let public_api_ast = parse_incan_program(public_api_source, "private-class public API");
    let consumer_ast = parse_incan_program(consumer_source, "private-class consumer");
    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.add_module_with_path_segments("provider", &provider_ast, vec!["provider".to_string()]);
    codegen.add_module_with_path_segments("facade", &facade_ast, vec!["facade".to_string()]);
    codegen.add_module_with_path_segments("public_api", &public_api_ast, vec!["public_api".to_string()]);
    let (consumer_code, _modules) = codegen
        .try_generate_multi_file_nested(
            &consumer_ast,
            &[
                vec!["provider".to_string()],
                vec!["facade".to_string()],
                vec!["public_api".to_string()],
            ],
        )
        .map_err(|err| std::io::Error::other(format!("private source class should codegen: {err:?}")))?;
    let rust_code = normalize_codegen_output(&consumer_code);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("PublicVault(None,\"visible\".to_string(),Some(9))"),
        "expected imported private class construction to use the provider bridge; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("issue886_imported_private_source_class_constructor", rust_code);
    Ok(())
}

#[test]
/// Keep source-package model construction on the public-field bridge while private defaults remain provider-owned.
fn test_issue964_imported_private_source_model_constructor_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = r#"
pub model Vault:
  secret: str = "sealed"
  pub label: str
  revision: int = 7
"#;
    let facade_source = "pub from provider import Vault as FacadeVault\n";
    let public_api_source = "pub from facade import FacadeVault as ExportedVault\n";
    let consumer_source = r#"
from crate.public_api import ExportedVault as PublicVault

def main() -> None:
  vault = PublicVault(label="visible")
  println(vault.label)
"#;
    let provider_ast = parse_incan_program(provider_source, "private-model provider");
    let facade_ast = parse_incan_program(facade_source, "private-model facade");
    let public_api_ast = parse_incan_program(public_api_source, "private-model public API");
    let consumer_ast = parse_incan_program(consumer_source, "private-model consumer");
    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.add_module_with_path_segments("provider", &provider_ast, vec!["provider".to_string()]);
    codegen.add_module_with_path_segments("facade", &facade_ast, vec!["facade".to_string()]);
    codegen.add_module_with_path_segments("public_api", &public_api_ast, vec!["public_api".to_string()]);
    let (consumer_code, _modules) = codegen
        .try_generate_multi_file_nested(
            &consumer_ast,
            &[
                vec!["provider".to_string()],
                vec!["facade".to_string()],
                vec!["public_api".to_string()],
            ],
        )
        .map_err(|err| std::io::Error::other(format!("private source model should codegen: {err:?}")))?;
    let rust_code = normalize_codegen_output(&consumer_code);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("PublicVault(\"visible\".to_string())"),
        "expected imported private model construction to use the public-field provider bridge; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("issue964_imported_private_source_model_constructor", rust_code);
    Ok(())
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

    let mut codegen = codegen_with_builtin_stdlib_inventory();
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

    assert_codegen_snapshot!("issue377_imported_sum_shadows_builtin", rust_code);
}

#[test]
fn test_traits_codegen() {
    let source = load_test_file("traits");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("traits", rust_code);
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
    assert_codegen_snapshot!("trait_supertraits", rust_code);
}

#[test]
fn test_trait_supertrait_assignability_codegen() {
    let source = load_test_file("trait_supertrait_assignability");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("trait_supertrait_assignability", rust_code);
}

#[test]
fn test_enums_codegen() {
    let source = load_test_file("enums");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("enums", rust_code);
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
    assert_codegen_snapshot!("enum_methods_traits", rust_code);
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
    assert_codegen_snapshot!("rfc043_newtype_trait_targets", rust_code);
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
    assert_codegen_snapshot!("rfc043_imported_trait_associated_type", rust_code);
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
    assert_codegen_snapshot!("rfc043_rust_derive_passthrough", rust_code);
}

#[test]
fn test_value_enums_codegen() {
    let source = load_test_file("value_enums");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("value_enums", rust_code);
}

// ============================================================================
// Additional migration tests
// ============================================================================

#[test]
fn test_patterns_codegen() {
    let source = load_test_file("patterns");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("patterns", rust_code);
}

#[test]
fn test_param_mut_unused_codegen() {
    let source = load_test_file("param_mut_unused");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("param_mut_unused", rust_code);
}

#[test]
fn test_imports_codegen() {
    let source = load_test_file("imports");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("imports", rust_code);
}

#[test]
fn test_builtins_codegen() {
    let source = load_test_file("builtins");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    let min_max = rust_code
        .split("fn test_min_max_builtins")
        .nth(1)
        .and_then(|remainder| remainder.split("fn test_abs_builtin").next())
        .expect("builtins fixture must retain its min/max function before abs");
    let compact_min_max = min_max.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
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
        !compact_min_max.contains(".unwrap_or_else"),
        "builtins codegen must not inline unwrap_or_else fallback paths for list min/max; generated:\n{rust_code}"
    );
    assert_codegen_snapshot!("builtins", rust_code);
}

#[test]
fn test_pub_const_codegen() {
    let source = load_test_file("pub_const");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("pub_const", rust_code);
}

#[test]
fn test_consts_codegen() {
    let source = load_test_file("consts");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("consts", rust_code);
}

/// Issue #1001: an empty frozen descriptor field is const-safe even when its element model is not itself a Rust
/// const type.
///
/// The integration test covers compilation and execution. This focused codegen proof keeps the backend admission
/// contract local, so a generated-project runner cannot hide a regression in const representability.
#[test]
fn test_descriptor_const_with_empty_frozen_list_codegen_issue1001() {
    let source = r#"
@derive(Clone, Eq, Descriptor)
model Version:
  major: int
  minor: int

@derive(Clone, Eq, Descriptor)
model Change:
  version: Version
  note: str

@derive(Clone, Eq, Descriptor)
model Deprecation:
  since: Version
  note: str

@derive(Clone, Eq, Descriptor)
model Lifecycle:
  since: Version
  changed: FrozenList[Change]
  deprecated: Option[Deprecation]

const INITIAL_VERSION: Version = Version(major=0, minor=1)
const NO_CHANGES: FrozenList[Change] = []
const INITIAL_LIFECYCLE: Lifecycle = Lifecycle(
  since=INITIAL_VERSION,
  changed=NO_CHANGES,
  deprecated=None,
)

def main() -> None:
  assert INITIAL_LIFECYCLE.changed == NO_CHANGES
"#;

    let rust_code = generate_rust(source);
    let compact = rust_code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        compact.contains(
            "constNO_CHANGES:incan_stdlib::frozen::FrozenList<Change>=incan_stdlib::frozen::FrozenList::new(&[],);"
        ),
        "empty FrozenList descriptor constants must emit a const-safe Rust initializer; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("constINITIAL_LIFECYCLE:Lifecycle=Lifecycle{since:INITIAL_VERSION,changed:NO_CHANGES,deprecated:None::<_>,};"),
        "nested descriptor constants must retain the exact frozen-list binding; generated:\n{rust_code}"
    );
}

#[test]
fn test_rfc052_module_static_storage_codegen() {
    let source = load_test_file("rfc052_module_static_storage");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc052_module_static_storage", rust_code);
}

#[test]
fn test_rfc052_pub_static_codegen() {
    let source = load_test_file("rfc052_pub_static");
    let rust_code = generate_rust_with_widgets_manifest(&source);
    assert_codegen_snapshot!("rfc052_pub_static", rust_code);
}

#[test]
fn test_const_str_chain_codegen() {
    let source = load_test_file("const_str_chain");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("const_str_chain", rust_code);
}

#[test]
fn test_const_bytes_codegen() {
    let source = load_test_file("const_bytes");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("const_bytes", rust_code);
}

#[test]
fn test_inferred_reassign_codegen() {
    // Snapshot test to keep style consistent with this file.
    let source = load_test_file("inferred_reassign");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("inferred_reassign", rust_code);
}

#[test]
fn test_rust_interop_associated_functions_codegen() {
    let source = load_test_file("rust_interop_associated_functions");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rust_interop_associated_functions", rust_code);
}

#[test]
fn test_issue806_rust_receiver_turbofish_codegen() {
    let source = load_test_file("issue_806");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue_806", rust_code);
}

#[test]
fn test_rust_associated_call_in_elif_codegen() {
    let source = load_test_file("rust_associated_call_in_elif");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rust_associated_call_in_elif", rust_code);
}

#[test]
fn test_issue367_result_ok_string_literal_codegen() {
    let source = load_test_file("issue367_result_ok_string_literal");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue367_result_ok_string_literal", rust_code);
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

#[test]
fn test_issue880_map_err_string_literal_closure_codegen() {
    let source = load_test_file("issue880_map_err_string_literal_closure");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue880_map_err_string_literal_closure", rust_code);
}

#[test]
fn test_issue880_map_err_string_literal_closure_emits_owned_error() {
    let source = load_test_file("issue880_map_err_string_literal_closure");
    let rust_code = generate_rust(&source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("map_err(|_error|\"malformed_json\".to_string())"),
        "expected closure return conversion to materialize the error as String; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("map_err(|_error|\"malformed_json\")"),
        "unexpected borrowed &str closure result; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("fnparse_formatted(source:String)->Result<JsonValue,String>")
            && compact.contains("incan_stdlib::strings::fstring(&__parts,&__args)"),
        "expected the existing owned f-string closure path to remain unchanged; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("fnpreserve_error<T:Clone>(value:T)->Result<i64,T>"),
        "expected generic closure return cloning to add its backend Clone bound; generated:\n{rust_code}"
    );
}

/// Issue #374: qualified enum constructor patterns in `Pattern =>` arms must resolve for same-enum scrutinees.
#[test]
fn test_issue374_enum_constructor_match_codegen() {
    let source = load_test_file("issue374_enum_constructor_match");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue374_enum_constructor_match", rust_code);
}

#[test]
fn test_issue389_for_tuple_unpack_enumerate_codegen() {
    let source = load_test_file("issue389_for_tuple_unpack_enumerate");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue389_for_tuple_unpack_enumerate", rust_code);
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
    assert_codegen_snapshot!("issue483_list_comp_tuple_unpack_enumerate", rust_code);
    assert!(
        rust_code.contains(".map(|(idx, name)| Binding"),
        "expected enumerate list comprehension to destructure tuple bindings in the map closure"
    );
}

#[test]
fn test_fixed_call_unpack_codegen() {
    let source = load_test_file("fixed_call_unpack");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("fixed_call_unpack", rust_code);
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
    assert_codegen_snapshot!("collection_literal_spread", rust_code);
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
    assert_codegen_snapshot!("issue391_list_str_append_literal", rust_code);
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
    assert_codegen_snapshot!("rust_interop_field_access", rust_code);
}

#[test]
fn test_issue217_rust_enum_match_bindings_codegen() {
    let source = load_test_file("issue217_rust_enum_match_bindings");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("issue217_rust_enum_match_bindings", rust_code);
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_issue459_rust_enum_pattern_import_codegen() {
    let source = load_test_file("issue459_rust_enum_pattern_import");
    let rust_code = generate_rust_with_substrait_probe(&source);
    assert_codegen_snapshot!("issue459_rust_enum_pattern_import", rust_code);
    assert!(
        rust_code.contains("use ::substrait::proto::rel::RelType;"),
        "expected Rust enum import used only by a match pattern to be retained:\n{rust_code}"
    );
}

#[test]
fn test_rfc041_std_rust_capability_bounds_codegen() {
    let source = load_test_file("rfc041_std_rust_capability_bounds");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_std_rust_capability_bounds", rust_code);
}

#[test]
fn test_rfc041_rusttype_interop_codegen() {
    let source = load_test_file("rfc041_rusttype_interop");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_rusttype_interop", rust_code);
}

#[test]
fn test_rfc041_rusttype_rebinding_codegen() {
    let source = load_test_file("rfc041_rusttype_rebinding");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_rusttype_rebinding", rust_code);
}

#[test]
fn test_rfc041_interop_from_try_codegen() {
    let source = load_test_file("rfc041_interop_from_try");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_interop_from_try", rust_code);
}

#[test]
fn test_rfc041_interop_into_via_codegen() {
    let source = load_test_file("rfc041_interop_into_via");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_interop_into_via", rust_code);
}

#[test]
fn test_rfc041_capability_bounds_full_codegen() {
    let source = load_test_file("rfc041_capability_bounds_full");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_capability_bounds_full", rust_code);
}

#[test]
fn test_rfc041_structural_coercion_codegen() {
    let source = load_test_file("rfc041_structural_coercion");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_structural_coercion", rust_code);
}

#[test]
fn test_rfc041_rust_coercions_codegen() {
    let source = load_test_file("rfc041_rust_coercions");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_rust_coercions", rust_code);
}

#[test]
fn test_rfc041_emit_rust_path_type_codegen() {
    let source = load_test_file("rfc041_emit_rust_path_type");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_emit_rust_path_type", rust_code);
}

#[test]
fn test_rfc041_emit_static_bound_codegen() {
    let source = load_test_file("rfc041_emit_static_bound");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rfc041_emit_static_bound", rust_code);
}

#[test]
fn test_titlecase_var_not_type_codegen() {
    let source = load_test_file("titlecase_var_not_type");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("titlecase_var_not_type", rust_code);
}

// ============================================================================
// Construction semantics: defaults + newtype checked construction
// ============================================================================

#[test]
fn test_constructor_field_defaults_codegen() {
    let source = load_test_file("constructor_field_defaults");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("constructor_field_defaults", rust_code);
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
    assert_codegen_snapshot!("newtype_checked_construction", rust_code);
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
    assert_codegen_snapshot!("newtype_implicit_coercion", rust_code);
}

#[test]
fn test_validated_newtype_json_deserialization_uses_canonical_hook() {
    let source = load_test_file("newtype_json_validation");
    let rust_code = generate_rust(&source);
    assert!(
        !rust_code.contains("serde::Deserialize)]\nstruct ShortId"),
        "validated newtypes must not derive unchecked deserialization:\n{rust_code}"
    );
    assert!(
        rust_code.contains("#[derive(Debug, Clone, serde::Serialize)]\nstruct ShortId"),
        "checked deserialization must preserve newtype serialization:\n{rust_code}"
    );
    assert!(
        rust_code.contains("impl<'de> serde::Deserialize<'de> for ShortId"),
        "validated newtypes should emit checked deserialization:\n{rust_code}"
    );
    assert!(
        rust_code.contains("ShortId::from_underlying") || rust_code.contains("Self::from_underlying"),
        "checked deserialization should call the canonical validation hook:\n{rust_code}"
    );
    assert!(
        rust_code.contains("impl<'de, D> serde::Deserialize<'de> for CheckedBox<D>"),
        "checked deserialization should preserve generic newtype parameters:\n{rust_code}"
    );
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
    assert_codegen_snapshot!("panic_function_name", rust_code);
}

#[test]
fn test_newtype_builder_methods_codegen() {
    let source = load_test_file("newtype_builder_methods");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_builder_methods", rust_code);
}

#[test]
fn test_newtype_with_override_codegen() {
    let source = load_test_file("newtype_with_override");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_with_override", rust_code);
}

#[test]
fn test_newtype_axum_response_codegen() {
    let source = load_test_file("newtype_axum_response");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_axum_response", rust_code);
}

#[test]
fn test_newtype_generic_json_codegen() {
    let source = load_test_file("newtype_generic_json");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_generic_json", rust_code);
}

#[test]
fn test_newtype_generic_simple_codegen() {
    let source = load_test_file("newtype_generic_simple");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_generic_simple", rust_code);
}

#[test]
fn test_newtype_generic_builder_methods_codegen() {
    let source = load_test_file("newtype_generic_builder_methods");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_generic_builder_methods", rust_code);
}

#[test]
fn test_newtype_web_response_codegen() {
    let source = load_test_file("newtype_web_response");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("newtype_web_response", rust_code);
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
    assert_codegen_snapshot!("rust_extern_delegation", rust_code);
}

#[test]
fn rust_extern_method_projection_preserves_rust_abi_symbol() {
    let source = r#"
rust.module("incan_stdlib::web")

pub class App:
    @staticmethod
    @rust.extern
    def run(host: str, port: int) -> None:
        ...
"#;
    let rust_code = generate_projected_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("pubfn__incan_v1_") && compact.contains("incan_stdlib::web::run(host,port)"),
        "the Incan wrapper must be canonical while its Rust ABI target keeps the source-declared name:\n{rust_code}"
    );
    assert!(
        !compact.contains("incan_stdlib::web::__incan_v1_"),
        "canonical Incan identity must not be projected onto a host-owned Rust ABI symbol:\n{rust_code}"
    );
}

/// RFC 023 Phase 5: compile the real `std.testing` module source.
#[test]
fn test_std_testing_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/testing.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_testing_compiled", rust_code);
}

/// RFC 041 / Phase E: compile `std.async.task` from `.incn` source.
#[test]
fn test_std_async_task_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/task.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_async_task_compiled", rust_code);
}

/// RFC 041 / Phase E: compile `std.async.time` from `.incn` source.
#[test]
fn test_std_async_time_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/time.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_async_time_compiled", rust_code);
}

/// Compile `std.async.channel` from `.incn` source.
#[test]
fn test_std_async_channel_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/channel.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_async_channel_compiled", rust_code);
}

/// Compile `std.async.sync` from `.incn` source.
#[test]
fn test_std_async_sync_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/sync.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_async_sync_compiled", rust_code);
}

/// Compile `std.async.race` from `.incn` source.
#[test]
fn test_std_async_race_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/async/race.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_async_race_compiled", rust_code);
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
    assert_codegen_snapshot!("race_for_expression_codegen", rust_code);
}

/// Awaiting a declared wrapper must delegate to the proven awaitable field.
#[test]
fn test_awaitable_wrapper_delegation_codegen() {
    let source = r#"
import std.async
from std.async.task import JoinHandle, TaskJoinError

pub model TaskBox[T] with Awaitable[Result[T, TaskJoinError]]:
  pub handle: JoinHandle[T]

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
    assert_codegen_snapshot!("std_derives_comparison_compiled", rust_code);
}

/// compile `std.derives.copying` (Clone, Copy, Default) from `.incn` source.
#[test]
fn test_std_derives_copying_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/copying.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_derives_copying_compiled", rust_code);
}

/// compile `std.derives.string` (Debug, Display) from `.incn` source.
#[test]
fn test_std_derives_string_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/string.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_derives_string_compiled", rust_code);
}

/// compile `std.derives.collection` (collection/iterator protocols and adapters) from `.incn` source.
#[test]
fn test_std_derives_collection_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/derives/collection.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert!(
        rust_code.contains("StoredMapFn: Clone + Callable1<T, Output>"),
        "fallible adapter storage must retain the nominal source callable bound:\n{rust_code}"
    );
    assert!(
        rust_code.contains("fn filter<Predicate: Clone + Callable1<Output, bool>>"),
        "expanded fallible defaults must substitute the adapter's adopted item type:\n{rust_code}"
    );
    assert!(
        !rust_code.contains("list<"),
        "collection types inside callable bounds must use their canonical Rust representation:\n{rust_code}"
    );
    assert_codegen_snapshot!("std_derives_collection_compiled", rust_code);
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
    assert_codegen_snapshot!("std_serde_json_compiled", rust_code);
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
    assert_codegen_snapshot!("std_serde_json_import", rust_code);
}

/// RFC 113: typed declaration registries remain source-authored while the compiler records `@describe` facts.
#[test]
fn test_std_registry_import_codegen() {
    let source = load_test_file("std_registry_import");
    let rust_code = generate_registry_rust(&source, "std_registry_import");
    assert_codegen_snapshot!("std_registry_import", rust_code);
}

/// RFC 113: method descriptions lower into source-owned registry runtime registration.
#[test]
fn test_std_registry_methods_codegen() {
    let source = load_test_file("std_registry_methods");
    let rust_code = generate_registry_rust(&source, "std_registry_methods");
    assert_codegen_snapshot!("std_registry_methods", rust_code);
}

/// RFC 113: explicit compilation-unit and package entries retain compiler-checked canonical subjects.
#[test]
fn test_std_registry_subjects_codegen() {
    let source = load_test_file("std_registry_subjects");
    let rust_code = generate_registry_rust(&source, "std_registry_subjects");
    assert_codegen_snapshot!("std_registry_subjects", rust_code);
}

/// RFC 113: a structural descriptor can retain a concrete Incan type token without changing registry lowering.
#[test]
fn test_std_registry_type_token_codegen() {
    let source = load_test_file("std_registry_type_token");
    let rust_code = generate_registry_rust(&source, "std_registry_type_token");
    assert_codegen_snapshot!("std_registry_type_token", rust_code);
}

/// RFC 047: compile `std.graph` declarations from `.incn` source.
#[test]
fn test_std_graph_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/graph.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_graph_compiled", rust_code);
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
    assert_codegen_snapshot!("std_graph_import", rust_code);
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
    assert_codegen_snapshot!("std_uuid_compiled", rust_code);
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
    assert_codegen_snapshot!("std_uuid_import", rust_code);
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
    assert_codegen_snapshot!("std_serde_with_serialize_trait", rust_code);
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

#[test]
fn test_qualified_source_trait_dispatch_does_not_double_borrow_self() {
    let source = r#"
from std.serde import json

@derive(json)
model Payload:
  value: int

  def encode(self) -> str:
    return self.to_json()

def main() -> str:
  return Payload(value=1).encode()
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("json::Serialize::to_json(self)"),
        "expected the Rust-native trait slot to reuse the method's borrowed self receiver; generated:\n{rust_code}"
    );
    assert!(
        !compact.contains("returnSerialize::to_json(&self)") && !compact.contains("returnself.to_json(&self)"),
        "source projection dispatch must not borrow an already borrowed self receiver; generated:\n{rust_code}"
    );
}

#[test]
fn test_direct_json_trait_import_keeps_canonical_owner_across_modules_issue946() {
    let models_source = r#"
from std.serde import json

@derive(json)
pub model Item:
  pub value: str
"#;
    let encode_source = r#"
from std.serde.json import Serialize
from crate.models import Item

pub def encode(item: Item) -> str:
  return item.to_json()
"#;
    let root_source = r#"
from crate.encode import encode
from crate.models import Item
"#;
    let models_ast = parse_incan_program(models_source, "JSON model module");
    let encode_ast = parse_incan_program(encode_source, "JSON encoder module");
    let root_ast = parse_incan_program(root_source, "JSON library root");
    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.add_module_with_path_segments("models", &models_ast, vec!["models".to_string()]);
    codegen.add_module_with_path_segments("encode", &encode_ast, vec!["encode".to_string()]);
    let (_root_code, modules) = codegen
        .try_generate_multi_file_nested(&root_ast, &[vec!["models".to_string()], vec!["encode".to_string()]])
        .unwrap_or_else(|err| panic!("multi-module JSON library should codegen: {err:?}"));
    let Some(encode_module) = modules.get(&vec!["encode".to_string()]) else {
        panic!("missing generated encoder module");
    };
    let rust_code = normalize_codegen_output(encode_module);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

    assert!(
        compact.contains("crate::__incan_std::serde::json::Serialize::to_json(&item)"),
        "the encoder module must reach the trait through its canonical owner:\n{rust_code}"
    );
    assert!(
        !compact.contains("returnjson::Serialize::to_json(&item)"),
        "the encoder module does not import the source `json` module:\n{rust_code}"
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
    assert_codegen_snapshot!("rfc024_module_derive_json", rust_code);
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
    assert_codegen_snapshot!("rfc024_partial_alias_derive", rust_code);
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
    let mut codegen = codegen_with_builtin_stdlib_inventory();
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
    let mut codegen = codegen_with_builtin_stdlib_inventory();
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
        compact.contains("data.__getitem__(\"name\".to_string())"),
        "expected object-style JsonValue indexing to use source-authored Index.__getitem__; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("data.__getitem__(0)"),
        "expected array-style JsonValue indexing to use source-authored Index.__getitem__; generated:\n{rust_code}"
    );
}

/// Issue #815: explicit `Index[K, V]` adoption on a generic carrier must produce a Rust trait impl.
#[test]
fn test_issue815_generic_index_adoption_emits_trait_impl() {
    let source = r#"
from std.traits.indexing import Index

class GenericBox[T with Clone] with Index[str, str]:
  pub label: str
  pub witness: list[T]

  def __getitem__(self, key: str) for Index[str, str] -> str:
    return key

pub def indexed_label[T with Clone](box: GenericBox[T]) -> str:
  return box["amount"]
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("impl<T:Clone>Index<String,String>forGenericBox<T>"),
        "generic Index adoption must emit a parameterized Rust trait impl; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("r#box.__getitem__(\"amount\".to_string())"),
        "generic index access must dispatch through the adopted Index implementation; generated:\n{rust_code}"
    );
}

/// Issue #815: `Self` in an adopted Index output must become the concrete carrier in an impl header and call site.
#[test]
fn test_issue815_self_returning_index_adoption_uses_owner_type() {
    let source = r#"
from std.traits.indexing import Index

class PlainBox with Index[list[str], Self]:
  pub label: str

  def __getitem__(self, key: list[str]) for Index[list[str], Self] -> Self:
    return self

pub def nested_box(box: PlainBox) -> PlainBox:
  return box[["name"]]
"#;
    let rust_code = generate_rust(source);
    let compact = rust_code.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
    assert!(
        compact.contains("implIndex<Vec<String>,PlainBox>forPlainBox"),
        "Self in an adopted Index target must emit as the owner type; generated:\n{rust_code}"
    );
    assert!(
        compact.contains("r#box.__getitem__(vec![\"name\".to_string()])"),
        "Self-returning index access must use the concrete Index instantiation; generated:\n{rust_code}"
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
    assert_codegen_snapshot!("std_traits_ops_compiled", rust_code);
}

#[test]
fn test_std_traits_error_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/error.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_error_compiled", rust_code);
}

#[test]
fn test_std_traits_indexing_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/indexing.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_indexing_compiled", rust_code);
}

#[test]
fn test_std_traits_callable_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/callable.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_callable_compiled", rust_code);
}

#[test]
fn test_std_traits_prelude_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/prelude.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_prelude_compiled", rust_code);
}

#[test]
fn test_std_traits_convert_compiled_codegen() {
    let path = "crates/incan_stdlib/stdlib/traits/convert.incn";
    let Ok(source) = fs::read_to_string(path) else {
        panic!("Failed to read stdlib source file: {}", path);
    };
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_convert_compiled", rust_code);
}

#[test]
fn test_std_traits_convert_usage_codegen() {
    let source = load_test_file("std_traits_convert_usage");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("std_traits_convert_usage", rust_code);
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
    assert_codegen_snapshot!("assert_surface", rust_code);
}

// ============================================================================
/// RFC 057: Targeted Rust lint suppression.
// ============================================================================
#[test]
fn test_rust_allow_codegen() {
    let source = load_test_file("rust_allow");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("rust_allow", rust_code);
}

// ============================================================================
// RFC 023: Trait Bound Inference and `with` Annotation
// ============================================================================

/// RFC 023: Inferred trait bounds from usage (`==`/`!=` -> PartialEq, f-string -> Display, etc.)
#[test]
fn test_trait_bound_inference_codegen() {
    let source = load_test_file("trait_bound_inference");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("trait_bound_inference", rust_code);
}

/// RFC 023: Explicit `with` bounds on type parameters.
#[test]
fn test_trait_bound_explicit_codegen() {
    let source = load_test_file("trait_bound_explicit");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("trait_bound_explicit", rust_code);
}

#[test]
fn test_ordinal_key_builtin_impls_codegen() -> TestResult {
    let source = load_test_file("ordinal_key_builtin_impls");
    let collections_source = fs::read_to_string("crates/incan_stdlib/stdlib/collections.incn")?;
    let collections_ast = parse_incan_program(&collections_source, "std.collections metadata");
    let main_ast = parse_incan_program(&source, "ordinal key bridge fixture");
    let mut codegen = codegen_with_builtin_stdlib_inventory();
    codegen.add_dependency_symbol_module_with_path_segments(
        "__incan_std_collections",
        &collections_ast,
        vec!["__incan_std".to_string(), "collections".to_string()],
    );
    let generated = codegen
        .try_generate(&main_ast)
        .map_err(|error| std::io::Error::other(format!("ordinal key bridge fixture must generate: {error:?}")))?;
    let rust_code = normalize_codegen_output(&generated);
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
    assert_codegen_snapshot!("ordinal_key_builtin_impls", rust_code);
    Ok(())
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
    assert_codegen_snapshot!("ordinal_map_str_fast_lookup", rust_code);
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
    assert_codegen_snapshot!("imported_stdlib_value_fragment", rust_code);
}

/// RFC 023: Additional inference cases (Display, Dict key hashing, arithmetic, transitive propagation).
#[test]
fn test_trait_bound_inference_more_codegen() {
    let source = load_test_file("trait_bound_inference_more");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("trait_bound_inference_more", rust_code);
}

#[test]
fn test_loop_expressions_codegen() {
    let source = load_test_file("loop_expressions");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("loop_expressions", rust_code);
}

/// RFC 023: Generic bounds in return types (issue #196).
///
/// Verifies that trait bounds from return types (e.g., `impl BoundedDataSet<T>`) are properly inferred and emitted in
/// the Rust codegen, even when the bounds aren't used in the function body.
#[test]
fn test_generic_bounds_return_type_codegen() {
    let source = load_test_file("generic_bounds_return_type");
    let rust_code = generate_rust(&source);
    assert_codegen_snapshot!("generic_bounds_return_type", rust_code);
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
//         assert_codegen_snapshot!(name.to_string(), rust_code);
//     });
// }
