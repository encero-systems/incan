//! Alias reads preserve their checked source owner instead of reusing inferred last-use ownership selection.

use incan_semantics_core::SemanticSourceTargetKind;

use super::super::{BodyBuilder, ast, bir, hir_span};
use crate::typechecker::CheckedInputSource;

impl BodyBuilder<'_, '_> {
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
                    for field in &alias.place.projection {
                        let bir::PlaceElem::Field {
                            canonical: Some(identity),
                            structural: false,
                            ..
                        } = field
                        else {
                            return Err("local alias source is not checked static storage");
                        };
                        fields.push(identity.clone());
                    }
                    Ok(CheckedInputSource::Place {
                        root: root.clone(),
                        fields,
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
                    let CheckedInputSource::Place { root, mut fields } = source? else {
                        return Err("local alias cannot project an invocation temporary");
                    };
                    fields.push(identity.clone());
                    Ok(CheckedInputSource::Place { root, fields })
                })())
            }
            _ => None,
        }
    }

    /// Keep alias reads independent of the original owner's source-name countdown. Reading a Copy referent
    /// produces a value; every non-Copy alias read remains a borrow. The source proof has already rejected consuming
    /// or escaping uses, so this cannot manufacture ownership from a shared or exclusive loan.
    pub(in super::super) fn lower_input_alias_value(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let source = self.input_alias_source(expression)?;
        let result = (|| {
            let CheckedInputSource::Place { root, fields } = source? else {
                return Err("local alias has no original source storage".to_owned());
            };
            let place = self.lower_expr_to_place(expression, scope, out);
            self.authenticate_input_place(&place, &root, &fields)?;
            let ty = self.resolve_ty(expression.span);
            let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Borrow
            };
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
