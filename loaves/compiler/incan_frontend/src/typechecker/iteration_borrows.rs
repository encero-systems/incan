//! Refuse replacement of a list or its parent while a statement loop borrows its storage.

use crate::ast::{Expr, Span, Spanned};
use crate::diagnostics::errors;
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;
use super::helpers::collection_type_id;
use crate::symbols::ResolvedType;

/// A resolved binding and field path borrowed by a direct list loop. Binding identity distinguishes shadowed names.
#[derive(Debug)]
pub(super) struct IterationBorrow {
    /// Declaration of the root binding, resolved before the loop pattern enters scope.
    root: Span,
    /// Field projections from that binding to the borrowed collection.
    fields: Vec<String>,
    /// Source spelling used to explain which collection remains borrowed.
    name: String,
}

impl TypeChecker {
    /// Retain a direct list place until its loop body finishes. Constructed iterables own their separate storage.
    pub(super) fn enter_iteration_borrow(&mut self, iter: &Spanned<Expr>) -> usize {
        let previous = self.iteration_borrows.len();
        if matches!(self.type_info.expr_type(iter.span), Some(ResolvedType::Generic(base, _))
            if collection_type_id(base) == Some(CollectionTypeId::List))
            && let Some(place) = self.iteration_place(iter)
        {
            self.iteration_borrows.push(place);
        }
        previous
    }

    /// Resolve a name and its field projections using symbol identity rather than display spelling.
    fn iteration_place(&self, expression: &Spanned<Expr>) -> Option<IterationBorrow> {
        match &expression.node {
            Expr::Ident(name) => Some(IterationBorrow {
                root: self.lookup_symbol(name)?.span,
                fields: Vec::new(),
                name: name.to_owned(),
            }),
            Expr::Paren(inner) => self.iteration_place(inner),
            Expr::Field(object, field) => {
                let mut place = self.iteration_place(object)?;
                place.fields.push(field.to_owned());
                place.name.push('.');
                place.name.push_str(field);
                Some(place)
            }
            _ => None,
        }
    }

    /// Reject replacing the borrowed collection or an ancestor; writes to independent sibling fields remain valid.
    pub(super) fn refuse_iterated_place_replacement(
        &mut self,
        object: &Spanned<Expr>,
        field: Option<&str>,
        span: Span,
    ) {
        let Some(mut target) = self.iteration_place(object) else {
            return;
        };
        if let Some(field) = field {
            target.fields.push(field.to_owned());
        }
        if let Some(borrowed) = self
            .iteration_borrows
            .iter()
            .rev()
            .find(|borrowed| target.root == borrowed.root && borrowed.fields.starts_with(&target.fields))
        {
            self.errors
                .push(errors::iterated_place_replacement(&borrowed.name, span));
        }
    }
}
