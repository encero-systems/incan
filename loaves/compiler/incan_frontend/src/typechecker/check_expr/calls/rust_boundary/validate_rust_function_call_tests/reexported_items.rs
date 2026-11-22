//! One Rust definition named through its re-exports is one type.

use super::*;

/// A re-export and the definition it names are the same type in either direction; another definition is not.
#[test]
fn reexported_rust_items_are_their_definition() {
    let facts = [
        ("dep::Token", Some("dep::inner::Token")),
        ("dep::inner::Token", None),
        ("dep::Other", Some("dep::inner::Other")),
    ];
    let mut loaves = crate::loaf_index::LoafIndex::default();
    loaves.add_rust_facts(
        &Default::default(),
        facts
            .into_iter()
            .map(|(path, definition)| {
                (
                    path.to_owned(),
                    RustItemMetadata {
                        canonical_path: path.to_owned(),
                        definition_path: definition.map(str::to_owned),
                        visibility: RustVisibility::Public,
                        kind: RustItemKind::Type(RustTypeInfo::default()),
                    },
                )
            })
            .collect(),
    );
    let mut checker = TypeChecker::new();
    checker.set_loaves(std::sync::Arc::new(loaves));
    let path = |path: &str| ResolvedType::RustPath(path.to_owned());
    assert!(checker.types_compatible(&path("dep::Token"), &path("dep::inner::Token")));
    assert!(checker.types_compatible(&path("dep::inner::Token"), &path("rust::dep::Token")));
    assert!(!checker.types_compatible(&path("dep::Other"), &path("dep::inner::Token")));
}
