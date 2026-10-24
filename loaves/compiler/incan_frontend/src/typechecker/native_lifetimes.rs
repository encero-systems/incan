//! Refuse borrowed native source signatures whose erased regions have no retained lifetime contract.

use incan_lang::interop::RustItemKind;

use super::TypeChecker;
use crate::ast::Span;
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;

impl TypeChecker {
    /// Check authored parameters, including native values whose own type carries a borrow. Native call temporaries
    /// keep their metadata-selected borrowing; source declarations need a retained cross-call lifetime relationship.
    pub(super) fn refuse_uncontracted_rust_borrows(&mut self, ty: &ResolvedType, span: Span) {
        if let Some(owner) = self.uncontracted_rust_borrow(ty) {
            self.errors
                .push(errors::rust_borrow_without_lifetime_contract(&owner, span));
        }
    }

    /// Find a lifetime-bearing native owner, including aliases, references and nested type arguments. A value such
    /// as `Captures<'a>` still borrows its input when passed by value; removing an outer reference cannot erase that
    /// relationship. Owner metadata supplies lifetime presence, independently of the source spelling.
    fn uncontracted_rust_borrow(&self, ty: &ResolvedType) -> Option<String> {
        match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                return self.uncontracted_rust_borrow(inner);
            }
            ResolvedType::Tuple(elements) => {
                return elements
                    .iter()
                    .find_map(|element| self.uncontracted_rust_borrow(element));
            }
            _ => {}
        }
        if let Some((path, _, _, _)) = self.rust_identity_for_type(ty)
            && let Some(metadata) = self.rust_item_metadata_for_complete_type(&path)
            && let RustItemKind::Type(info) = &metadata.kind
            && info.has_lifetime_params
        {
            return Some(path);
        }
        match ty {
            ResolvedType::Generic(_, arguments) => arguments
                .iter()
                .find_map(|argument| self.uncontracted_rust_borrow(argument)),
            _ => None,
        }
    }
}
