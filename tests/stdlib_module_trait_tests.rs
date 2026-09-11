//! Source-backed module imports retain the semantic traits used by their qualified codec bounds.

use incan::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use incan::frontend::ast::Program;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan::library_manifest::LibraryManifest;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse a source-only consumer without loading a CLI session or an installed SDK.
fn parsed(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?)
}

/// Exercise both directions of a module's codec bounds on the same nominal model.
fn codec_consumer(import: &str, module: &str, derive: &str) -> String {
    format!(
        "{import}\n\n{derive}\nmodel Settings:\n    name: str\n\ndef codecs(source: str) -> None:\n    decoded = {module}.deserialize[Settings](source)\n    encoded = {module}.serialize(Settings(name=\"example\"))\n    pretty = {module}.serialize_pretty(Settings(name=\"example\"))\n"
    )
}

/// Plain imports, root-module imports and aliases must use the module's declared derive contract.
#[test]
fn source_module_codec_bounds_accept_derived_models() -> TestResult {
    for (import, module) in [
        ("from std import toml", "toml"),
        ("from std import toml as codec", "codec"),
        ("import std.toml", "toml"),
        ("import std.toml as codec", "codec"),
    ] {
        let source = codec_consumer(import, module, &format!("@derive({module})"));
        TypeChecker::new()
            .check_program(&parsed(&source)?)
            .map_err(|errors| format!("{import}: {errors:?}"))?;
    }
    Ok(())
}

/// Loading semantic metadata must not make an ordinary model satisfy codec traits it never adopted.
#[test]
fn source_module_codec_bounds_reject_models_without_derives() -> TestResult {
    for (import, module) in [
        ("from std import toml", "toml"),
        ("from std import toml as codec", "codec"),
        ("import std.toml", "toml"),
        ("import std.toml as codec", "codec"),
    ] {
        let source = codec_consumer(import, module, "");
        let errors = TypeChecker::new()
            .check_program(&parsed(&source)?)
            .err()
            .ok_or_else(|| format!("{import} accepted a model without codec traits"))?;
        assert!(
            errors.iter().any(|error| error.message.contains("TomlDeserialize")),
            "{import}: {errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.message.contains("TomlSerialize")),
            "{import}: {errors:?}"
        );
    }
    Ok(())
}

/// Explicit item imports remain a control for the existing semantic-cache path.
#[test]
fn explicit_codec_trait_imports_remain_valid() -> TestResult {
    let source = codec_consumer(
        "from std import toml\nfrom std.toml import TomlSerialize, TomlDeserialize",
        "toml",
        "@derive(TomlSerialize, TomlDeserialize)",
    );
    TypeChecker::new()
        .check_program(&parsed(&source)?)
        .map_err(|errors| format!("explicit trait control: {errors:?}"))?;
    Ok(())
}

/// Replay the exact source-only fixture that failed in the Linux integration root.
#[test]
fn original_toml_module_fixture_passes_bare_frontend_checking() -> TestResult {
    let source = include_str!("fixtures/valid/std_toml_module_import.incn");
    TypeChecker::new()
        .check_program(&parsed(source)?)
        .map_err(|errors| format!("original fixture: {errors:?}"))?;
    Ok(())
}

/// A checked SDK module owns its empty requirements even when a richer source stub exists at that spelling.
#[test]
fn checked_sdk_module_imports_do_not_acquire_source_trait_requirements() -> TestResult {
    let source = "const __derives__ = [TomlSerialize]\n\npub trait TomlSerialize:\n    pass\n";
    let ast = parsed(source)?;
    let module_path = vec!["toml".into()];
    let mut producer = TypeChecker::new();
    producer.set_current_module_path(Some(module_path.clone()));
    producer
        .check_program(&ast)
        .map_err(|errors| format!("provider: {errors:?}"))?;
    let metadata = collect_checked_api_metadata(&ast, &producer, module_path);
    let mut manifest = LibraryManifest::new("codec_provider", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![metadata],
        public_namespaces: Vec::new(),
    });
    for import in ["from std import toml as codec", "import std.toml as codec"] {
        let mut consumer = TypeChecker::new();
        consumer.set_in_memory_sdk_manifest(manifest.clone());
        let source = format!("{import}\n\n@derive(codec)\nmodel Record:\n    name: str\n");
        consumer
            .check_program(&parsed(&source)?)
            .map_err(|errors| format!("{import}: {errors:?}"))?;
        assert_eq!(
            consumer
                .type_info()
                .derivations
                .trait_rust_derive_paths
                .get("std.toml.TomlSerialize"),
            Some(&Vec::new()),
            "{import} recovered source requirements for an artifact-owned trait"
        );
    }
    Ok(())
}
