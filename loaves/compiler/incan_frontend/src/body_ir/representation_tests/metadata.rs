use super::*;

/// The actual checked generic metadata body must survive publication, not merely a fabricated method target.
#[test]
fn generic_field_metadata_source_publishes_a_covered_body() -> Result<(), Box<dyn std::error::Error>> {
    let metadata_members = "pub def field_names[T](value: T) -> list[str]:\n    return [str(field.name) for field in value.__fields__()]\n";
    let source =
        format!("pub def count_fields[T](value: T) -> int:\n    return len(value.__fields__())\n{metadata_members}");
    let (facts, module) = checked_metadata_seed(&source)?;
    assert_metadata_publication(&facts, &module)?;
    let source = format!(
        "model FieldInfo:\n    pub own_value: int\npub def count_fields[T](value: T) -> int:\n    return len(value.__fields__())\npub def local_value(value: FieldInfo) -> int:\n    return value.own_value\n{metadata_members}"
    );
    let (facts, module) = checked_metadata_seed(&source)?;
    assert_metadata_publication(&facts, &module)?;
    let local = module
        .bodies
        .iter()
        .find(|body| body.name == "local_value")
        .ok_or("missing local FieldInfo consumer")?;
    let super::IncanType::Named(key) = &local.params[0].ty else {
        return Err("local FieldInfo lost its nominal carrier".into());
    };
    let owner = local.named_type_identities.get(key).ok_or("missing local owner")?;
    assert!(
        matches!(&owner.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "metadata")
    );
    Ok(())
}

/// Inspect the source-produced body and its published coverage together so omitted carrier facts cannot pass.
fn assert_metadata_publication(
    facts: &TypeCheckInfo,
    module: &bir::BodyIrModule,
) -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};
    use std::collections::BTreeSet;

    assert!(
        facts
            .declarations
            .body_ir_native_type_identities
            .values()
            .any(|identity| identity.declaration_name == "FieldInfo")
    );
    let mut helpers = 0;
    for body in module
        .bodies
        .iter()
        .filter(|body| matches!(body.name.as_str(), "count_fields" | "field_names"))
    {
        helpers += 1;
        let identity = body.canonical.clone().ok_or("missing canonical helper identity")?;
        let public = BTreeSet::from([identity]);
        let bytes = build_surface(
            std::slice::from_ref(module),
            "metadata",
            "0.1.0",
            &public,
            &BTreeSet::new(),
        )?;
        let reader = SurfaceReader::open(&bytes)?;
        let (_, coverage) = reader
            .index()
            .declarations
            .iter()
            .find(|(identity, _)| identity.declaration_name == body.name)
            .ok_or("missing public helper coverage")?;
        assert!(
            matches!(coverage, DeclarationCoverage::Covered { .. }),
            "actual coverage: {coverage:?}; native identities: {:#?}; checked body: {body:#?}",
            facts.declarations.body_ir_native_type_identities
        );
    }
    assert_eq!(helpers, 2, "missing checked metadata helpers");
    Ok(())
}

/// Admit the real SDK metadata declaration before checking a source helper that obtains it without an import.
fn checked_metadata_seed(source: &str) -> Result<(TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    use crate::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        materialize_checked_api_public_namespaces,
    };
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::LoafIndex;
    use std::sync::Arc;

    let sdk = include_str!("../../../../../stdlib/primitives/src/primitives/reflection.incn");
    let facade = include_str!("../../../../../stdlib/primitives/src/reflection.incn");
    let tokens = crate::lexer::lex(sdk).map_err(|errors| format!("{errors:?}"))?;
    let declaration = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let tokens = crate::lexer::lex(facade).map_err(|errors| format!("{errors:?}"))?;
    let facade = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let owner_path = vec!["primitives".to_owned(), "reflection".to_owned()];
    let mut modules = Vec::new();
    let mut checked_exports = Vec::new();
    for (module_path, program) in [
        (owner_path.clone(), &declaration),
        (vec!["reflection".to_owned()], &facade),
    ] {
        let mut declaring = TypeChecker::new();
        declaring.set_current_package_identity(Some("incan_stdlib_primitives".into()));
        declaring.set_current_module_path(Some(module_path.clone()));
        if module_path == owner_path {
            declaring
                .check_program(program)
                .map_err(|errors| format!("{errors:?}"))?;
        } else {
            declaring.register_dependency_module_path_segments("primitives_reflection", owner_path.clone());
            declaring
                .check_with_imports(program, &[("primitives_reflection", &declaration)])
                .map_err(|errors| format!("{errors:?}"))?;
        }
        checked_exports.push((module_path.clone(), collect_checked_public_exports(program, &declaring)));
        modules.push(collect_checked_api_metadata(program, &declaring, module_path));
    }
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new("incan_stdlib_primitives", "0.6.0");
    manifest.contract_metadata.identity_graph.extend_checked_api_exports(
        "incan_stdlib_primitives",
        &api,
        &checked_exports,
    )?;
    manifest.contract_metadata.api = Some(api);
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join("incan_stdlib_primitives.incnlib"),
        manifest.to_json_string()?,
    )?;
    let entry = LoafIndex::standard("incan_stdlib_primitives", directory.path())?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("metadata".into()));
    checker.set_current_module_path(Some(Vec::new()));
    checker.set_loaves(Arc::new(LoafIndex::new([entry])?));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: Some("metadata"),
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    Ok((facts, module))
}
