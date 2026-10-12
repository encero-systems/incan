//! Retain checked numeric union member conversions before the plan's exact-member injection boundary.

use incan_lang::numeric_values::numeric_type_losslessly_widens_to;

use super::operators::numeric_carrier_id;
use super::{BodyBuilder, IncanType, ast, bir, hir_span};

/// Choose legacy's first eligible member of a closed primitive union, after looking through Option payload layers.
///
/// An exact numeric carrier needs no conversion. Open generic or nominal member profiles leave the operand unchanged;
/// no member is chosen before declaration instantiation or canonical type resolution.
fn numeric_union_member(source: &IncanType, destination: &IncanType) -> Option<IncanType> {
    let source_id = numeric_carrier_id(source)?;
    let mut destination = destination;
    while let IncanType::Generic { base, args } = destination {
        if base != "Option" || args.len() != 1 {
            break;
        }
        destination = &args[0];
    }
    let IncanType::Generic { base, args } = destination else {
        return None;
    };
    if base != "Union" || !args.iter().all(|member| matches!(member, IncanType::Primitive(_))) {
        return None;
    }
    if args.iter().any(|member| numeric_carrier_id(member) == Some(source_id)) {
        return None;
    }
    args.iter()
        .find(|member| {
            numeric_carrier_id(member).is_some_and(|target| numeric_type_losslessly_widens_to(source_id, target))
        })
        .cloned()
}

impl BodyBuilder<'_, '_> {
    /// Evaluate a boundary value once and retain a numeric union conversion selected by its checked destination.
    ///
    /// Explicit destination types belong to bindings and callable slots; otherwise the recorded value destination
    /// covers field writes and constructor fields. Expressions without either fact retain their existing operand.
    pub(super) fn lower_boundary_value(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        destination: Option<&IncanType>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let operand = self.lower_expr_to_operand(expr, scope, out);
        let recorded = self
            .type_info
            .value_destination_type(expr.span)
            .map(|ty| self.checked_type(ty));
        match destination.or(recorded.as_ref()) {
            Some(destination) => self.retain_numeric_union_member(expr, operand, destination, scope, out),
            None => operand,
        }
    }

    /// Give an accepted numeric value its selected primitive union-member storage before assignment or call binding.
    ///
    /// The destination comes from a checked binding, checked callable slot or recorded value destination. Reading an
    /// already materialized temporary's retained type prevents the same value from being adapted twice. The plan then
    /// receives an exact member and only injects it; no legacy fact, inferred variable type or numeric policy changes.
    pub(super) fn retain_numeric_union_member(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        operand: bir::Operand,
        destination: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let source = match &operand {
            bir::Operand::Place(read) if read.place.projection.is_empty() => read
                .place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .map_or_else(|| self.resolve_ty(expr.span), |local| local.ty.clone()),
            _ => self.resolve_ty(expr.span),
        };
        let Some(member) = numeric_union_member(&source, destination) else {
            return operand;
        };
        self.push_assign_temp(bir::Rvalue::Use(operand), member, scope, hir_span(expr.span), out)
    }
}
