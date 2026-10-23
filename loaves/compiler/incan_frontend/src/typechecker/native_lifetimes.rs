//! Refuse borrowed native source signatures whose erased regions have no retained lifetime contract.

use incan_lang::interop::RustItemKind;

use super::TypeChecker;
use crate::ast::Span;
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;

impl TypeChecker {
    /// Check an authored parameter's explicitly borrowed positions. Native call temporaries keep their
    /// metadata-selected borrowing; only source declarations introduce the unsupported cross-call lifetime boundary.
    pub(super) fn refuse_uncontracted_rust_borrows(&mut self, ty: &ResolvedType, span: Span) {
        if let Some(owner) = self.uncontracted_rust_borrow(ty, false) {
            self.errors
                .push(errors::rust_borrow_without_lifetime_contract(&owner, span));
        }
    }

    /// Find a lifetime-bearing native referent under a source borrow, including aliases and nested type arguments.
    /// Owner metadata supplies lifetime presence; spelling and method signatures never serve as a lifetime proof.
    fn uncontracted_rust_borrow(&self, ty: &ResolvedType, borrowed: bool) -> Option<String> {
        match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                return self.uncontracted_rust_borrow(inner, true);
            }
            ResolvedType::Tuple(elements) => {
                return elements
                    .iter()
                    .find_map(|element| self.uncontracted_rust_borrow(element, borrowed));
            }
            _ => {}
        }
        if borrowed
            && let Some((path, _, _, _)) = self.rust_identity_for_type(ty)
            && let Some(metadata) = self.rust_item_metadata_for_complete_type(&path)
            && let RustItemKind::Type(info) = &metadata.kind
            && info.has_lifetime_params
        {
            return Some(path);
        }
        match ty {
            ResolvedType::Generic(_, arguments) => arguments
                .iter()
                .find_map(|argument| self.uncontracted_rust_borrow(argument, borrowed)),
            _ => None,
        }
    }
}
