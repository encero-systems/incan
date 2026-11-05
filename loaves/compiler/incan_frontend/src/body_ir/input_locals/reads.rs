//! Alias reads preserve their checked source owner instead of reusing inferred last-use ownership selection.

use incan_semantics_core::SemanticSourceTargetKind;

use super::super::{BodyBuilder, ast, bir, hir_span};
use crate::typechecker::CheckedInputSource;

impl BodyBuilder<'_, '_> {
    /// Retained operation dependencies cover direct borrowed parameters as well as local aliases. A parameter's
    /// native reference carrier alone is insufficient; the completed source proof must own the read relationship.
    pub(in super::super) fn checked_borrow_source(
        &self,
        expression: &ast::Spanned<ast::Expr>,
    ) -> Option<Result<CheckedInputSource, &'static str>> {
        if let Ok(Some(proof)) = &self.input_body {
            match proof.operation_operand_source(expression.span) {
                Ok(Some(source)) => return Some(Ok(source)),
                Err(reason) => return Some(Err(reason)),
                Ok(None) => {}
            }
        }
        self.input_alias_source(expression)
    }

    /// Resolve a canonical alias through static stored fields. Each extra field must have its own checked identity;
    /// a property, index or display-name match cannot extend a loan's authenticated physical place.
    pub(in super::super) fn input_alias_source(
        &self,
        expression: &ast::Spanned<ast::Expr>,
    ) -> Option<Result<CheckedInputSource, &'static str>> {
        match &expression.node {
            ast::Expr::Ident(_) => {
                let identity = self.type_info.resolved_identity(expression.span)?;
                let alias = self.input_alias(identity)?;
                Some((|| {
                    let root = alias
                        .place
                        .local_id()
                        .and_then(|id| self.locals.get(id.index()))
                        .and_then(|local| local.identity.as_ref())
                        .ok_or("local alias has no canonical physical owner")?;
                    let mut fields = Vec::new();
                    let mut elements = Vec::new();
                    for (position, projection) in alias.place.projection.iter().enumerate() {
                        match projection {
                            bir::PlaceElem::Field {
                                canonical: Some(identity),
                                structural: false,
                                ..
                            } => fields.push(identity.clone()),
                            bir::PlaceElem::Index(_) => elements.push(position),
                            _ => return Err("local alias source is not checked stored storage"),
                        }
                    }
                    Ok(CheckedInputSource::Place {
                        root: root.clone(),
                        fields,
                        elements,
                    })
                })())
            }
            ast::Expr::Paren(inner) => self.input_alias_source(inner),
            ast::Expr::Field(base, _) => {
                let source = self.input_alias_source(base)?;
                Some((|| {
                    if self.type_info.computed_property_access(expression.span).is_some() {
                        return Err("local alias property needs its selected callable access");
                    }
                    let identity = self
                        .type_info
                        .resolved_identity(expression.span)
                        .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
                        .ok_or("local alias projection has no checked field identity")?;
                    let CheckedInputSource::Place {
                        root,
                        mut fields,
                        elements,
                    } = source?
                    else {
                        return Err("local alias cannot project an invocation temporary");
                    };
                    fields.push(identity.clone());
                    Ok(CheckedInputSource::Place { root, fields, elements })
                })())
            }
            _ => None,
        }
    }

    /// Keep alias reads independent of the original owner's source-name countdown, while direct parameter reads
    /// consume their own occurrence. Copy referents produce values; non-Copy reads retain the checked source borrow.
    /// The source proof rejects consuming or escaping uses, so this cannot manufacture ownership from a loan.
    pub(in super::super) fn lower_checked_borrow_value(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let source = self.checked_borrow_source(expression)?;
        let result = (|| {
            let source = source?;
            let place = self.lower_expr_to_place(expression, scope, out);
            if matches!(source, CheckedInputSource::CarriedLocal) {
                let ty = self.resolve_ty(expression.span);
                let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
                    bir::OwnershipFact::Copy
                } else {
                    bir::OwnershipFact::Borrow
                };
                self.consume_place_read(&place);
                return Ok(bir::Operand::place(place, fact, false));
            }
            let CheckedInputSource::Place { root, fields, elements } = source else {
                return Err("local alias has no original source storage".to_owned());
            };
            self.authenticate_input_place(&place, &root, &fields, &elements)?;
            let ty = self.resolve_ty(expression.span);
            let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Borrow
            };
            self.consume_input_source_read(expression, &place);
            Ok(bir::Operand::place(place, fact, false))
        })();
        Some(match result {
            Ok(value) => value,
            Err(reason) => self.unsupported_operand(reason, scope, hir_span(expression.span), out),
        })
    }

    /// Only a direct source occurrence consumes the physical owner's name-based read budget. Alias uses were
    /// counted under another canonical binding and must not make the original owner appear to reach its last use.
    pub(in super::super) fn consume_input_source_read(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        place: &bir::Place,
    ) {
        if self.input_alias_source(expression).is_none() {
            self.consume_place_read(place);
        }
    }
}
