use super::TypeChecker;
use crate::ast::Program;

/// Parse a source module without changing its declarations or depending on a published standard library.
fn program(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
}

/// A standard-library trait default unwraps a sibling default's closed result with `?`, as a program's does (#3153).
#[test]
fn standard_trait_default_unwraps_a_sibling_default_result() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub trait ValueReading:\n    def aaa(mut self) -> Result[int, str]:\n        return Ok(1)\n\n    def bbb(mut self) -> Result[int, str]:\n        return Ok(self.aaa()? + 1)\n";
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["std".to_string(), "probe".to_string()]));
    let checked = checker.check_program(&parsed);
    assert!(checked.is_ok(), "standard trait default refused: {checked:?}");
    Ok(())
}

/// Probing an adopted trait for a generated enum member must leave the published nominal map identical to a restored
/// body that never enters the probe. An actual declared method still resolves through the standard package.
#[test]
fn absent_standard_trait_method_preserves_checked_nominal_scope() -> Result<(), Box<dyn std::error::Error>> {
    use crate::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        materialize_checked_api_public_namespaces,
    };
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::LoafIndex;

    let equality = incan_lang::lang::traits::as_str(incan_lang::lang::traits::TraitId::Eq);
    let package = "incan_stdlib_primitives";
    let path = vec!["derives".to_owned(), "comparison".to_owned()];
    let declarations = program(&format!(
        "pub trait {equality}:\n    def __eq__(self, other: Self) -> bool: ...\n"
    ))?;
    let mut provider = TypeChecker::new();
    provider.set_current_package_identity(Some(package.into()));
    provider.set_current_module_path(Some(path.clone()));
    provider
        .check_program(&declarations)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&declarations, &provider);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&declarations, &provider, path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new(package, "0.6.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(package, &api, &[(path, exports)])?;
    manifest.contract_metadata.api = Some(api);
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join(format!("{package}.incnlib")),
        manifest.to_json_string()?,
    )?;
    let entry = LoafIndex::standard(package, directory.path())?;
    let mut consumer = TypeChecker::new();
    consumer.set_loaves(std::sync::Arc::new(LoafIndex::new([entry])?));
    let initial = consumer.checked_nominal_type_identities();
    let span = crate::ast::Span::new(4, 16);
    assert!(
        consumer
            .trait_method_info_resolved(equality, "from_value", span)
            .is_none()
    );
    assert_eq!(consumer.checked_nominal_type_identities(), initial);
    assert!(consumer.trait_method_info_resolved(equality, "__eq__", span).is_some());
    Ok(())
}
