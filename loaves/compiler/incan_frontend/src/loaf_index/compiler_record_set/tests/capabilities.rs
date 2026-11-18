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
