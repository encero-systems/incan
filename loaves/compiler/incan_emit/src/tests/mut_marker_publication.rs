//! Publication of the `mut` marker (#1790): a published function type keeps the marker on its parameter, and the
//! native union inside a marked parameter is projected like any other.

use std::collections::HashMap;

use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use incan_frontend::library_manifest::{LibraryManifest, TypeRef};
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

/// Issue #1790: a public function whose parameter is a function type with a `mut`-marked parameter publishes the
/// marker, and the union inside the marked parameter carries its emitted native representation.
#[test]
fn marked_callable_parameter_publishes_its_marker_and_native_union() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub def run(step: (mut list[int | str]) -> int) -> int:\n    mut items: list[int | str] = [1, \"a\"]\n    return step(items)\n";
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("producer".into()));
    checker.set_current_module_path(Some(module_path.clone()));
    checker.check_program(&ast).map_err(|errors| format!("{errors:?}"))?;
    let exports = incan_frontend::library_exports::collect_checked_public_exports(&ast, &checker);
    let mut manifest = LibraryManifest::from_checked_exports("producer", "1.0.0", &exports);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&ast, &checker, module_path.clone())],
        public_namespaces: Vec::new(),
    });
    let mut codegen = crate::IrCodegen::new();
    codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
    codegen.set_publication_api(manifest.contract_metadata.api.clone());
    codegen.set_publication_identities(manifest.name.clone(), manifest.contract_metadata.identity_graph.clone());
    let (_, metadata) = codegen.try_generate_with_metadata(&ast, &module_path)?;
    metadata.apply_to_library_manifest(&mut manifest)?;

    let run = manifest
        .exports
        .functions
        .iter()
        .find(|function| function.name == "run")
        .ok_or("run absent")?;
    let step = run.params.first().ok_or("step param absent")?;
    let TypeRef::Function { params, .. } = &step.ty else {
        return Err(format!("`step` must publish a function type, got {:?}", step.ty).into());
    };
    let [TypeRef::MutParam { inner }] = params.as_slice() else {
        return Err(format!("the step's parameter must keep its `mut` marker, got {params:?}").into());
    };
    let TypeRef::Applied { name, args, .. } = inner.as_ref() else {
        return Err(format!("the marked parameter must stay a list, got {inner:?}").into());
    };
    assert_eq!(name, "List");
    assert!(
        matches!(args.as_slice(), [TypeRef::NativeUnion(_)]),
        "the union inside the marked parameter must carry its native representation, got {args:?}"
    );
    Ok(())
}
