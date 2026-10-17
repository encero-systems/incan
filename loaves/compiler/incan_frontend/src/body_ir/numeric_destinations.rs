//! Retain checked numeric destination carriers before native assignments, calls and collection probes.

use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::numeric_values::numeric_type_losslessly_widens_to;

use super::operators::numeric_carrier_id;
use super::primitives::collection_member_types;
use super::{BodyBuilder, IncanType, ast, bir, hir_span};

/// Choose a checked numeric destination or legacy's first eligible closed primitive union member through Option layers.
///
/// An exact numeric carrier needs no conversion. Open generic or nominal member profiles leave the operand unchanged;
/// no member is chosen before declaration instantiation or canonical type resolution.
fn numeric_destination(source: &IncanType, destination: &IncanType) -> Option<IncanType> {
    let source_id = numeric_carrier_id(source)?;
    let mut destination = destination;
    while let IncanType::Generic { base, args } = destination {
        if base != "Option" || args.len() != 1 {
            break;
        }
        destination = &args[0];
    }
    if let Some(target_id) = numeric_carrier_id(destination) {
        return (source_id != target_id && numeric_type_losslessly_widens_to(source_id, target_id))
            .then(|| destination.clone());
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

/// Retain the numeric element/key type a checked builtin membership probe compares against.
///
/// Lists and sets compare their element type, dictionaries their key type. Non-numeric, unresolved and unsupported
/// collection shapes select nothing, and the shared lossless-widening registry prevents a narrowing conversion.
pub(super) fn collection_numeric_destination(source: &IncanType, collection: &IncanType) -> Option<IncanType> {
    [CollectionTypeId::List, CollectionTypeId::Set, CollectionTypeId::Dict]
        .into_iter()
        .find_map(|kind| collection_member_types(collection, kind).and_then(|members| members.first()))
        .and_then(|member| numeric_destination(source, member))
}

impl BodyBuilder<'_, '_> {
    /// Evaluate a boundary value once and retain a numeric conversion selected by its checked destination.
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
            Some(destination) => self.retain_numeric_destination(expr, operand, destination, scope, out),
            None => operand,
        }
    }

    /// Give an accepted numeric value its checked destination or selected primitive union-member storage before
    /// binding.
    ///
    /// The destination comes from a checked binding, checked callable slot or recorded value destination. Reading an
    /// already materialized temporary's retained type prevents the same value from being adapted twice. The plan then
    /// receives an exact numeric carrier/member; no legacy fact, inferred variable type or numeric policy changes.
    pub(super) fn retain_numeric_destination(
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
        let Some(member) = numeric_destination(&source, destination) else {
            return operand;
        };
        self.push_assign_temp(bir::Rvalue::Use(operand), member, scope, hir_span(expr.span), out)
    }
}
