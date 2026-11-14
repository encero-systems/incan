//! A member belongs to the owner its identity names, wherever either declaration's source site lies.

use super::declares_member;
use crate::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

/// Build one module-level owner of the `library` module.
fn owner(name: &str, start: usize, end: usize) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["library".to_owned()],
        name,
        SemanticSourceTargetKind::Enum,
        HirSourceSpan::new(start, end),
    )
}

/// A member stays its owner's when its site no longer lies inside the owner's, as after an edit moves either one.
#[test]
fn a_member_belongs_to_its_owner_wherever_its_site_lies() {
    let color = owner("Color", 10, 50);
    let mut value = color.generated_member("value", SemanticSourceTargetKind::Method);
    value.declaration_span = HirSourceSpan::new(60, 70);
    assert!(declares_member(&color, &value));
}

/// A same-named member of another owner never becomes this owner's, even when its site lies inside this owner's.
#[test]
fn a_member_of_another_owner_never_belongs_to_this_one() {
    let color = owner("Color", 10, 50);
    let shade = owner("Shade", 60, 90);
    let mut value = shade.generated_member("value", SemanticSourceTargetKind::Method);
    value.declaration_span = HirSourceSpan::new(20, 30);
    assert!(!declares_member(&color, &value));
}
