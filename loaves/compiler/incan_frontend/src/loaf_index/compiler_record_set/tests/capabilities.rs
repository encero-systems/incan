//! The typed reader rejects mismatched scopes and unadmitted global contributors before exposing any package.
use crate::library_manifest::capabilities::{self, Contributors};
use std::collections::BTreeSet;

#[test]
fn compiler_contributions_are_bound_to_the_exact_package_scope() -> Result<(), Box<dyn std::error::Error>> {
    let names = BTreeSet::from(["one".to_owned(), "two".to_owned()]);
    let key = capabilities::key("release", &names);
    let scope = key.rsplit('/').next().ok_or("scope digest")?;
    let data = || Contributors {
        profile: "release".into(),
        scope: scope.into(),
        vocabulary: vec!["one".into()],
        operations: Vec::new(),
    };
    let selected = capabilities::select(data(), "release", &names)?;
    assert!(selected["one"].vocabulary);
    assert!(!selected["two"].vocabulary);
    assert!(!selected["one"].provider_operations);
    assert!(capabilities::select(data(), "debug", &names).is_err());
    assert!(capabilities::select(data(), "release", &BTreeSet::from(["one".into()])).is_err());
    let mut changed = data();
    changed.operations.push("absent".into());
    assert!(capabilities::select(changed, "release", &names).is_err());
    Ok(())
}

/// Rust ABI content identities are tagged. Catalog publication neither parses these as bare record hashes nor
/// includes Rust ABI details among the two global discovery facts.
#[test]
fn tagged_native_abi_identity_is_not_a_compiler_record_digest() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_manifest::{LibraryManifest, capabilities::CapabilityCatalog};
    let name = "incan_stdlib_async";
    let bytes = LibraryManifest::encode_compiler_record_value(&serde_json::json!({
        "name": name, "vocabulary": false, "provider_operations": false,
        "rust_abi_cache_digest": "sha256:7b6f5e6dc848f0d0d29ba0295714fa96eabd473ad4fd7814e65b75c2f00504d7"
    }))?;
    let mut catalog = CapabilityCatalog::default();
    assert!(catalog.insert("different_package", &bytes).is_err());
    catalog.insert(name, &bytes)?;
    let (_, facts) = catalog.contributors("debug");
    assert!(facts.vocabulary.is_empty() && facts.operations.is_empty());
    Ok(())
}
