//! Documentation split from checked API compile facts and restored by anchor (RFC 179).

use super::metadata_for;
use crate::api_metadata::{CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage};
use crate::library_manifest::LibraryContractMetadata;

/// Contract metadata whose checked API documents a function, a model and one of its methods.
fn documented_contract() -> Result<LibraryContractMetadata, String> {
    let source = r#"
pub def price() -> int:
    """The list price."""
    return 1999

pub model Money:
    """An amount in cents."""

    cents: int

    def doubled(self) -> int:
        """Twice the amount."""
        return self.cents * 2
"#;
    let module = metadata_for(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(LibraryContractMetadata {
        api: Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![module],
            public_namespaces: Vec::new(),
        }),
        ..LibraryContractMetadata::default()
    })
}

#[test]
fn documentation_splits_from_api_compile_facts_and_restores_by_anchor() -> Result<(), String> {
    let original = documented_contract()?;
    let (mut facts, documentation) = original.split_documentation();
    let anchors = documentation
        .iter()
        .map(|entry| entry.anchor.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        anchors.len(),
        3,
        "the function, the model and its method are documented: {anchors:?}"
    );
    let encoded = serde_json::to_string(&facts).map_err(|error| error.to_string())?;
    for docstring in ["The list price.", "An amount in cents.", "Twice the amount."] {
        assert!(
            !encoded.contains(docstring),
            "the compile facts carry no documentation, found {docstring:?}"
        );
    }
    facts.restore_documentation(documentation)?;
    assert_eq!(
        facts, original,
        "restoring the documentation gives back the checked API"
    );
    Ok(())
}

#[test]
fn documentation_for_another_anchor_is_refused() -> Result<(), String> {
    let (mut facts, mut documentation) = documented_contract()?.split_documentation();
    let entry = documentation.first_mut().ok_or("the contract is documented")?;
    entry.anchor = "elsewhere".to_owned();
    let refusal = match facts.restore_documentation(documentation) {
        Ok(()) => return Err("documentation for another anchor was restored".to_owned()),
        Err(refusal) => refusal,
    };
    assert!(refusal.contains("elsewhere"), "the refusal names the entry: {refusal}");
    Ok(())
}
