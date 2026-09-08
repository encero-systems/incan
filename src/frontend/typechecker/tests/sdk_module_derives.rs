//! Checked SDK metadata retains explicit module derive membership and backend requirements.

use crate::backend::IrCodegen;
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage, api_declaration_public_name,
    collect_checked_api_metadata,
};
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};
use crate::library_manifest::LibraryManifest;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Publish a synthetic module with no source-stdlib fallback and a deliberately excluded derivable trait.
fn provider_metadata() -> Result<CheckedApiMetadata, Box<dyn std::error::Error>> {
    let source = r#"
const __derives__ = [Included]

@rust.derive("Debug")
pub trait Included:
    pass

@rust.derive("Clone")
pub trait Omitted:
    pass
"#;
    metadata_from_source(source, &["bundle_probe".into()])
}

/// Collect the same checked module representation used by SDK publishers.
fn metadata_from_source(
    source: &str,
    module_path: &[String],
) -> Result<CheckedApiMetadata, Box<dyn std::error::Error>> {
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?)
        .map_err(|errors| format!("parse: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.to_vec()));
    checker
        .check_program(&ast)
        .map_err(|errors| format!("producer: {errors:?}"))?;
    Ok(collect_checked_api_metadata(&ast, &checker, module_path.to_vec()))
}

/// Metadata encodes the declared list, keeps its module owner, and reads legacy modules as having no bundle.
#[test]
fn sdk_module_derives_metadata_round_trip_preserves_exact_membership() -> TestResult {
    let metadata = provider_metadata()?;
    assert_eq!(metadata.derivable_traits, ["Included"]);
    assert_eq!(metadata.module_path, ["bundle_probe"]);
    assert!(
        metadata
            .declarations
            .iter()
            .all(|declaration| api_declaration_public_name(declaration) != Some("__derives__"))
    );
    let encoded = serde_json::to_value(&metadata)?;
    let decoded: CheckedApiMetadata = serde_json::from_value(encoded.clone())?;
    assert_eq!(decoded, metadata);
    let mut legacy = encoded;
    legacy
        .as_object_mut()
        .ok_or("module metadata is not an object")?
        .remove("derivable_traits");
    let decoded: CheckedApiMetadata = serde_json::from_value(legacy)?;
    assert!(decoded.derivable_traits.is_empty());
    Ok(())
}

/// Public SDK module imports and aliases use checked membership without recovering producer source.
#[test]
fn sdk_module_derives_imports_and_aliases_preserve_backend_requirements() -> TestResult {
    for import in [
        "import std.bundle_probe as chosen",
        "from std import bundle_probe as chosen",
    ] {
        let mut manifest = LibraryManifest::new("derive_provider", "0.1.0");
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![provider_metadata()?],
            public_namespaces: Vec::new(),
        });
        let mut checker = TypeChecker::new();
        checker.set_in_memory_sdk_manifest(manifest);
        let source = format!("{import}\n\n@derive(chosen)\npub model Record:\n    value: int\n");
        let ast = parser::parse(&lexer::lex(&source).map_err(|errors| format!("lex: {errors:?}"))?)
            .map_err(|errors| format!("parse: {errors:?}"))?;
        checker
            .check_program(&ast)
            .map_err(|errors| format!("consumer {import}: {errors:?}"))?;
        let info = checker.type_info();
        assert_eq!(
            info.derivations.derivable_modules.get("std.bundle_probe"),
            Some(&vec!["Included".into()])
        );
        assert_eq!(
            info.derivations
                .trait_rust_derive_paths
                .get("std.bundle_probe.Included"),
            Some(&vec!["Debug".into()])
        );
        let mut generator = IrCodegen::new();
        generator.set_prechecked_type_info(info.clone(), Default::default());
        let rust = generator
            .try_generate(&ast)
            .map_err(|error| format!("codegen: {error:?}"))?;
        assert!(
            rust.lines()
                .any(|line| line.trim_start().starts_with("#[derive(") && line.contains("Debug")),
            "{rust}"
        );
        assert!(rust.contains("Included for Record"), "{rust}");
        assert!(
            !rust.contains("Omitted for Record"),
            "bundle membership must not be inferred from other derivable traits: {rust}"
        );
    }
    Ok(())
}

/// An artifact without declared membership must not invent a bundle from its derivable traits.
#[test]
fn sdk_module_derives_missing_membership_stays_rejected() -> TestResult {
    let mut metadata = provider_metadata()?;
    metadata.derivable_traits.clear();
    // A real source module exists here; installed metadata must still be authoritative.
    metadata.module_path = vec!["serde".into(), "json".into()];
    let mut manifest = LibraryManifest::new("legacy_derive_provider", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![metadata],
        public_namespaces: Vec::new(),
    });
    let mut checker = TypeChecker::new();
    checker.set_in_memory_sdk_manifest(manifest);
    let source = "import std.serde.json as chosen\n\n@derive(chosen)\npub model Record:\n    value: int\n";
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?)
        .map_err(|errors| format!("parse: {errors:?}"))?;
    let errors = checker
        .check_program(&ast)
        .err()
        .ok_or("missing derive membership was accepted")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("does not declare `__derives__`")),
        "{errors:?}"
    );
    Ok(())
}

/// A public alias of a derivable trait retains the target's explicit backend requirement.
#[test]
fn sdk_module_derives_trait_alias_retains_target_macro() -> TestResult {
    use crate::frontend::api_metadata::{ApiAlias, ApiDeclaration, SourceAnchor, SourceSpan};
    let mut facade = provider_metadata()?;
    facade.module_path = vec!["bundle_facade".into()];
    facade.derivable_traits = vec!["Exported".into()];
    facade.declarations = vec![ApiDeclaration::Alias(ApiAlias {
        name: "Exported".into(),
        anchor: SourceAnchor {
            id: "bundle_facade::Exported".into(),
            span: SourceSpan { start: 0, end: 1 },
        },
        target_path: vec!["std".into(), "bundle_probe".into(), "Included".into()],
        is_public: true,
        projected_function: None,
        projected_type: None,
    })];
    let mut manifest = LibraryManifest::new("aliased_derive_provider", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![provider_metadata()?, facade],
        public_namespaces: Vec::new(),
    });
    let mut checker = TypeChecker::new();
    checker.set_in_memory_sdk_manifest(manifest);
    let source = "import std.bundle_facade as chosen\n\n@derive(chosen)\npub model Record:\n    value: int\n";
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?)
        .map_err(|errors| format!("parse: {errors:?}"))?;
    checker
        .check_program(&ast)
        .map_err(|errors| format!("consumer: {errors:?}"))?;
    assert_eq!(
        checker
            .type_info()
            .derivations
            .trait_rust_derive_paths
            .get("std.bundle_facade.Exported"),
        Some(&vec!["Debug".into()])
    );
    let mut generator = IrCodegen::new();
    generator.set_prechecked_type_info(checker.type_info().clone(), Default::default());
    let rust = generator
        .try_generate(&ast)
        .map_err(|error| format!("codegen: {error:?}"))?;
    assert!(
        rust.lines()
            .any(|line| line.trim_start().starts_with("#[derive(") && line.contains("Debug")),
        "{rust}"
    );
    Ok(())
}

/// Empty checked backend requirements and changed trait contracts must override current stdlib source.
#[test]
fn sdk_module_derives_empty_artifact_requirements_do_not_fall_back_to_source() -> TestResult {
    let source = "const __derives__ = [Serialize]\n\npub trait Serialize:\n    pass\n";
    let mut manifest = LibraryManifest::new("artifact_owns_serde", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![metadata_from_source(source, &["serde".into(), "json".into()])?],
        public_namespaces: Vec::new(),
    });
    let mut checker = TypeChecker::new();
    checker.set_in_memory_sdk_manifest(manifest);
    let source = "import std.serde.json as chosen\n\n@derive(chosen)\npub model Record:\n    value: int\n";
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?)
        .map_err(|errors| format!("parse: {errors:?}"))?;
    checker
        .check_program(&ast)
        .map_err(|errors| format!("consumer: {errors:?}"))?;
    let info = checker.type_info();
    assert_eq!(
        info.derivations.trait_rust_derive_paths.get("std.serde.json.Serialize"),
        Some(&Vec::new())
    );
    let trait_info = checker
        .lookup_imported_module_trait(&["std".into(), "serde".into(), "json".into()], "Serialize")
        .ok_or("artifact trait missing")?;
    assert!(
        trait_info.supertraits.is_empty(),
        "source trait requirements leaked into artifact contract"
    );
    let mut generator = IrCodegen::new();
    generator.set_prechecked_type_info(checker.type_info().clone(), Default::default());
    let rust = generator
        .try_generate(&ast)
        .map_err(|error| format!("codegen: {error:?}"))?;
    assert!(
        !rust.contains("serde::Serialize"),
        "source macro must not leak into checked artifact: {rust}"
    );
    Ok(())
}
