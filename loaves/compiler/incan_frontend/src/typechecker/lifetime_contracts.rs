//! DD-0006 lifetime contracts: a source return or field type never names a lifetime-bearing Rust type.

use std::cell::RefCell;

use incan_lang::interop::RustItemKind;

use super::TypeChecker;
use crate::ast::Span;
use crate::diagnostics::errors;
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;

impl TypeChecker {
    /// Refuse a source return or field type that names a Rust type with lifetime parameters, before Body IR runs, and
    /// return whether it did.
    ///
    /// A source signature cannot state how such a value relates to the borrows it was made from, so the compiler has
    /// no lifetime contract to give rustc (DD-0006, `INCAN-T0120`). Parameters are not judged here: each call binds a
    /// parameter's lifetimes independently, borrowed or by value (#2368).
    pub(in crate::typechecker) fn refuse_lifetime_bearing_rust_type(&mut self, ty: &ResolvedType, span: Span) -> bool {
        let Some(path) = self.lifetime_bearing_rust_path(ty) else {
            return false;
        };
        self.errors
            .push(errors::rust_borrow_without_lifetime_contract(&path, span));
        true
    }

    /// Return the path of the first Rust type `ty` names, at any depth, whose metadata declares lifetime parameters.
    fn lifetime_bearing_rust_path(&self, ty: &ResolvedType) -> Option<String> {
        let found = RefCell::new(None);
        map_resolved_type_leaves(ty, &|leaf| {
            if found.borrow().is_none()
                && let Some(path) = self.rust_type_path_with_lifetimes(leaf)
            {
                *found.borrow_mut() = Some(path);
            }
            None
        });
        found.into_inner()
    }

    /// Return the Rust path one type node names when that Rust type declares lifetime parameters.
    fn rust_type_path_with_lifetimes(&self, node: &ResolvedType) -> Option<String> {
        if !matches!(
            node,
            ResolvedType::RustPath(_) | ResolvedType::Generic(..) | ResolvedType::Named(_)
        ) {
            return None;
        }
        let path = self.rust_canonical_path_for_receiver_type(node)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        info.has_lifetime_params.then_some(path)
    }
}
