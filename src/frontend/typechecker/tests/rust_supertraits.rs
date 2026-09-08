//! Foreign supertrait validation at the source/Rust trait boundary (#1427).

use super::*;
use incan_core::interop::{RustItemKind, RustItemMetadata, RustTraitInfo, RustVisibility};

/// Imported Rust bounds are preserved for native checking, including primitive and collection instantiations.
#[test]
fn imported_rust_generic_bounds_accept_native_scalars_and_collections() {
    assert_check_ok(
        r#"
from rust::serde::de import DeserializeOwned as Owned

def identity[T with Owned](value: T) -> T:
    return value

class Reader:
    def identity[T with Owned](self, value: T) -> T:
        return identity(value)

def main() -> None:
    reader = Reader()
    assert identity("demo") == "demo"
    assert reader.identity(["linux", "macos"]) == ["linux", "macos"]
"#,
    );
}

/// A same-spelled local trait still requires an explicit source adoption.
#[test]
fn local_trait_bound_is_not_treated_as_a_foreign_capability() {
    let errors = check_str(
        r#"
trait DeserializeOwned:
    pass

def identity[T with DeserializeOwned](value: T) -> T:
    return value

def main() -> None:
    identity("demo")
"#,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("violates generic bound"))
    );
}

/// Build a checker with one imported foreign item whose metadata has a known kind.
fn checker_with_foreign_parent(kind: RustItemKind) -> TypeChecker {
    let mut checker = TypeChecker::new();
    checker.symbols.define(Symbol {
        name: "ForeignParent".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "foreign".to_string(),
            path: "foreign::Parent".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "foreign::Parent".to_string(),
                definition_path: Some("foreign::Parent".to_string()),
                visibility: RustVisibility::Public,
                kind,
            }),
        }),
        span: Span::default(),
        scope: 0,
    });
    checker
}

/// Positive Rust item metadata distinguishes a deferred trait obligation from a value masquerading as a bound.
#[test]
fn imported_rust_generic_bound_requires_trait_metadata() {
    let checker = checker_with_foreign_parent(RustItemKind::Trait(RustTraitInfo {
        items: vec![],
        derive_macro: None,
    }));
    assert!(checker.type_satisfies_explicit_bound(&ResolvedType::Str, "ForeignParent"));
    let checker = checker_with_foreign_parent(RustItemKind::Constant {
        type_display: "i32".to_string(),
    });
    assert!(!checker.type_satisfies_explicit_bound(&ResolvedType::Str, "ForeignParent"));
    assert!(checker.imported_generic_rust_bound_path("ForeignParent").is_none());
}

/// Foreign traits remain usable when metadata is absent, with native validation delegated to rustc.
#[test]
fn imported_rust_supertrait_without_metadata_is_accepted() {
    assert_check_ok(
        r#"
from rust::std::fmt import Display as RustDisplay

trait Labeled with RustDisplay:
    pass
"#,
    );
}

/// Known Rust trait metadata satisfies the supertrait kind check without fabricating an Incan declaration.
#[test]
fn imported_rust_supertrait_metadata_is_accepted() {
    let mut checker = checker_with_foreign_parent(RustItemKind::Trait(RustTraitInfo {
        items: vec![],
        derive_macro: None,
    }));
    let bound = Spanned::new(
        TraitBound {
            name: "ForeignParent".to_string(),
            type_args: vec![],
        },
        Span::default(),
    );
    assert_eq!(
        checker.resolve_trait_supertrait_bound(&bound),
        Some(("::foreign::Parent".to_string(), vec![]))
    );
    assert!(checker.errors.is_empty());
}

/// Positive metadata identifying a value must not be mistaken for an opaque foreign trait.
#[test]
fn imported_rust_supertrait_rejects_known_nontrait_metadata() {
    let mut checker = checker_with_foreign_parent(RustItemKind::Constant {
        type_display: "i32".to_string(),
    });
    let bound = Spanned::new(
        TraitBound {
            name: "ForeignParent".to_string(),
            type_args: vec![],
        },
        Span::default(),
    );
    assert!(checker.resolve_trait_supertrait_bound(&bound).is_none());
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.message.contains("is not a trait"))
    );
}

/// Foreign identity matching follows the imported path, never an alias or an unrelated local suffix.
#[test]
fn imported_rust_supertrait_identity_distinguishes_aliases_and_local_names() -> Result<(), Box<dyn std::error::Error>> {
    let mut checker = checker_with_foreign_parent(RustItemKind::Trait(RustTraitInfo {
        items: vec![],
        derive_macro: None,
    }));
    let mut alias = checker
        .lookup_symbol("ForeignParent")
        .cloned()
        .ok_or("missing seeded foreign parent")?;
    alias.name = "AnotherAlias".to_string();
    checker.symbols.define(alias);
    assert!(checker.trait_name_matches("ForeignParent", "AnotherAlias"));
    assert!(checker.trait_name_matches("ForeignParent", "::foreign::Parent"));
    assert!(!checker.trait_name_matches("ForeignParent", "::other::Parent"));
    assert!(!checker.trait_name_matches("ForeignParent", "Parent"));
    assert!(!checker.trait_name_matches("::foreign::Parent", "Parent"));
    Ok(())
}
