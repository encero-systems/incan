//! Retain checked numeric destination carriers before native assignments, calls and collection probes.

use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::numeric_values::numeric_type_losslessly_widens_to;
use incan_semantics_core::body_ir::nominal_carrier_key;

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
    /// Evaluate a boundary value once, applying retained native adapters before its checked numeric destination.
    ///
    /// Native adapter and explicit numeric decisions precede ordinary lossless destination adaptation. Destination
    /// types belong to bindings and callable slots; otherwise the recorded value destination covers field writes
    /// and constructor fields. Expressions without either fact retain their existing operand.
    pub(super) fn lower_boundary_value(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        destination: Option<&IncanType>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let operand = match self.lower_adjusted_argument(expr, scope, out) {
            Some(Ok(operand)) => operand,
            Some(Err(reason)) => return self.unsupported_operand(reason, scope, hir_span(expr.span), out),
            None => self.lower_expr_to_operand(expr, scope, out),
        };
        if self
            .type_info
            .expressions
            .numeric_conversion_destinations
            .contains_key(&expr.span)
        {
            return self.retain_checked_numeric_result(operand, expr.span, scope, out);
        }
        let recorded = self
            .type_info
            .value_destination_type(expr.span)
            .map(|ty| self.checked_type(ty));
        match destination.or(recorded.as_ref()) {
            Some(destination) => self.retain_numeric_destination(expr, operand, destination, scope, out),
            None => operand,
        }
    }

    /// Apply an explicitly selected numeric result conversion once, including a native length's usize-to-int edge.
    /// Reading the retained temporary type prevents an enclosing assignment from repeating the same conversion.
    pub(super) fn retain_checked_numeric_result(
        &mut self,
        operand: bir::Operand,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some(destination) = self
            .type_info
            .expressions
            .numeric_conversion_destinations
            .get(&span)
        else {
            return operand;
        };
        let destination = self.checked_type(destination);
        if self.retained_operand_type(&operand).as_ref() == Some(&destination) {
            return operand;
        }
        self.push_assign_temp(bir::Rvalue::Use(operand), destination, scope, hir_span(span), out)
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
        if let IncanType::Ref(borrowed) = &source
            && self.is_source_nominal(borrowed)
            && self.same_nominal_carrier(borrowed, destination)
        {
            return self.materialize_borrowed_value(expr, operand, destination, scope, out);
        }
        let Some(member) = numeric_destination(&source, destination) else {
            return operand;
        };
        self.push_assign_temp(bir::Rvalue::Use(operand), member, scope, hir_span(expr.span), out)
    }

    /// Whether `ty` names a nominal declared in source, by its carrier key or by a source name that has one. A type
    /// imported from Rust keeps its native copy semantics, so a borrow of it is never cloned for a destination.
    fn is_source_nominal(&self, ty: &IncanType) -> bool {
        let (IncanType::Named(name) | IncanType::Generic { base: name, .. }) = ty else {
            return false;
        };
        name.starts_with("__incan_carrier_")
            || self
                .type_info
                .declarations
                .named_type_identities
                .get(name)
                .and_then(nominal_carrier_key)
                .is_some()
    }

    /// Whether two types name one source nominal with the same arguments, when one is spelled by its source name and
    /// the other by the carrier key that native storage froze for the declaration.
    fn same_nominal_carrier(&self, left: &IncanType, right: &IncanType) -> bool {
        let carrier = |name: &String| {
            self.type_info
                .declarations
                .named_type_identities
                .get(name)
                .and_then(nominal_carrier_key)
                .unwrap_or_else(|| name.clone())
        };
        match (left, right) {
            (IncanType::Named(left), IncanType::Named(right)) => carrier(left) == carrier(right),
            (
                IncanType::Generic {
                    base: left,
                    args: left_args,
                },
                IncanType::Generic {
                    base: right,
                    args: right_args,
                },
            ) => {
                carrier(left) == carrier(right)
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.same_nominal_carrier(left, right))
            }
            _ => left == right,
        }
    }

    /// Clone a borrowed source model or class into the owned value its checked destination requires.
    ///
    /// The checker admitted the borrow where an owned value is needed; the read of the borrowed place carries the
    /// clone fact, and the owned temporary is the value the destination binds.
    fn materialize_borrowed_value(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        mut operand: bir::Operand,
        destination: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if let bir::Operand::Place(read) = &mut operand {
            read.fact = bir::OwnershipFact::Clone;
            read.last_use = false;
        }
        self.push_assign_temp(
            bir::Rvalue::Use(operand),
            destination.clone(),
            scope,
            hir_span(expr.span),
            out,
        )
    }
}
