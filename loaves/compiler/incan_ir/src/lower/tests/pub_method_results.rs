//! The result type of a method called on a `pub::` dependency's type keeps every union the provider owns (#1797).

use super::*;
use incan_frontend::library_manifest::{LibraryManifest, MethodExport, ModelExport, ReceiverExport, TypeRef};
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::provider::ProviderPlan;
use std::sync::Arc;

/// Build lowering over one `querykit` provider whose `Box` model has `answer(self) -> int | str`.
fn lowering_with_querykit_box() -> AstLowering {
    let named = |name: &str| TypeRef::Named {
        origin: None,
        name: name.to_string(),
    };
    let mut manifest = LibraryManifest::new("querykit", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Box".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: vec![MethodExport {
            name: "answer".to_string(),
            canonical: None,
            alias_of: None,
            type_params: Vec::new(),
            receiver: Some(ReceiverExport::Immutable),
            params: Vec::new(),
            return_type: TypeRef::Applied {
                origin: None,
                name: crate::types::IR_UNION_TYPE_NAME.to_string(),
                args: vec![named("int"), named("str")],
            },
            is_async: false,
            has_body: true,
        }],
    });
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "querykit".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "querykit",
                "querykit",
                std::env::temp_dir().join("incan_issue1797_querykit"),
            ),
        },
    )]));
    let mut lowering = AstLowering::new();
    lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_library_index(index))));
    lowering
}

/// #1797: the checker types `Box(value=3).answer()` with `int | str` spelled structurally, which this crate would
/// re-own as its own wrapper while the provider's method returns the provider's. The method's signature, rebuilt from
/// the provider's metadata and marked as the provider's, names the owning crate at the union position, and the call's
/// result takes that carrier; the result then crosses into a consumer's own `int | str` parameter the way a dependency
/// function's result does.
#[test]
fn pub_dependency_method_result_keeps_the_providers_union_issue1797() -> Result<(), String> {
    let mut lowering = lowering_with_querykit_box();
    let receiver = IrType::Struct("querykit::Box".to_string());
    let signature = lowering
        .callable_signature_for_imported_pub_type_method("querykit", &receiver, "answer")
        .ok_or("expected the provider's `answer` declaration")?;
    let signature = lowering.pub_external_signature("querykit", signature);
    assert!(
        matches!(&signature.return_type, IrType::ExternalUnion { library, .. } if library == "querykit"),
        "the provider's declaration names the owning crate, got {:?}",
        signature.return_type
    );

    let checked = crate::lower::types::union_ir_type(vec![IrType::Int, IrType::String]);
    let result = AstLowering::retain_provider_owned_union_representation(checked.clone(), &signature.return_type);
    assert_eq!(
        result, signature.return_type,
        "the call's result takes the provider's carrier"
    );
    assert_eq!(
        result.union_type_name(),
        checked.union_type_name(),
        "one wrapper shape, owned by the provider rather than re-owned here"
    );

    let listed = AstLowering::retain_provider_owned_union_representation(
        IrType::List(Box::new(checked)),
        &IrType::List(Box::new(signature.return_type.clone())),
    );
    assert_eq!(
        listed,
        IrType::List(Box::new(signature.return_type)),
        "a union nested in the result is carried the same way"
    );
    Ok(())
}
