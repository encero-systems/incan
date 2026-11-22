//! Retain checked numeric destination carriers before native assignments, calls and collection probes.

use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::numeric_values::numeric_type_losslessly_widens_to;
use incan_semantics_core::body_ir::nominal_carrier_key;

use super::operators::numeric_carrier_id;
use super::{BodyBuilder, IncanType, ast, bir, hir_span};

/// Choose a checked numeric destination or legacy's first eligible closed primitive union member through Option layers.
///
/// An exact numeric carrier needs no conversion. Open generic or nominal member profiles leave the operand unchanged;
/// no member is chosen before declaration instantiation or canonical type resolution.
fn numeric_destination(source: &IncanType, destination: &IncanType) -> Option<IncanType> {
    let source_id = numeric_carrier_id(source)?;
    let mut destination = destination;
    while let Some([payload]) = destination.collection_arguments(CollectionTypeId::Option) {
        destination = payload;
    }
    if let Some(target_id) = numeric_carrier_id(destination) {
        return (source_id != target_id && numeric_type_losslessly_widens_to(source_id, target_id))
            .then(|| destination.clone());
    }
    let args = destination.union_members()?;
    if !args.iter().all(|member| matches!(member, IncanType::Primitive(_))) {
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
        .find_map(|kind| {
            collection
                .collection_arguments(kind)
                .and_then(|members| members.first())
        })
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

    /// Clone a `Borrowed[T]` argument whose checked destination owns a `T` (a stored list element), leaving every
    /// other argument as lowered; ordinary argument lowering keeps its own numeric and probe adaptations.
    pub(super) fn retain_borrowed_value_destination(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        operand: bir::Operand,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some(destination) = self
            .type_info
            .value_destination_type(expr.span)
            .map(|ty| self.checked_type(ty))
        else {
            return operand;
        };
        let source = match &operand {
            bir::Operand::Place(read) if read.place.projection.is_empty() => read
                .place
                .local_id()
                .and_then(|local| self.locals.get(local.index()))
                .map_or_else(|| self.resolve_ty(expr.span), |local| local.ty.clone()),
            _ => self.resolve_ty(expr.span),
        };
        match &source {
            IncanType::Ref(borrowed) | IncanType::RefMut(borrowed)
                if !matches!(destination, IncanType::Ref(_) | IncanType::RefMut(_))
                    && !self.borrowed_rust_copy(borrowed)
                    && self.same_nominal_carrier(borrowed, &destination) =>
            {
                self.materialize_borrowed_value(expr, operand, &destination, scope, out)
            }
            _ => operand,
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
        let Some(destination) = self.type_info.expressions.numeric_conversion_destinations.get(&span) else {
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
        // RFC 164: an owned use of a borrowed Copy scalar copies the value it borrows, read through the reference.
        if let IncanType::Ref(borrowed) | IncanType::RefMut(borrowed) = &source
            && !matches!(destination, IncanType::Ref(_) | IncanType::RefMut(_))
            && super::operators::borrowed_scalar(borrowed)
            && matches!(&operand, bir::Operand::Place(read) if read.place.projection.is_empty())
        {
            let value = super::operators::read_through_scalar_borrow(&source, operand);
            return match numeric_destination(borrowed, destination) {
                Some(member) => self.push_assign_temp(bir::Rvalue::Use(value), member, scope, hir_span(expr.span), out),
                None => value,
            };
        }
        // RFC 164: a `Borrowed[T]` value at a destination that owns a `T` receives a clone, for every `T`; the checker
        // refused the owned destinations whose `T` has no clone.
        if let IncanType::Ref(borrowed) | IncanType::RefMut(borrowed) = &source
            && !matches!(destination, IncanType::Ref(_) | IncanType::RefMut(_))
            && !self.borrowed_rust_copy(borrowed)
            && self.same_nominal_carrier(borrowed, destination)
        {
            return self.materialize_borrowed_value(expr, operand, destination, scope, out);
        }
        let Some(member) = numeric_destination(&source, destination) else {
            return operand;
        };
        self.push_assign_temp(bir::Rvalue::Use(operand), member, scope, hir_span(expr.span), out)
    }

    /// Whether `ty` is a Rust type whose checked layout is `Copy`. An owned use of a borrow of it copies the value, as
    /// a by-value receiver does, and never calls its `Clone`, which may be observable.
    fn borrowed_rust_copy(&self, ty: &IncanType) -> bool {
        let (IncanType::Named(name) | IncanType::Generic { base: name, .. }) = ty else {
            return false;
        };
        self.type_info
            .rust
            .checked_layouts
            .get(name.as_str())
            .is_some_and(|layout| layout.is_copy)
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

    /// Clone a `Borrowed[T]` value into the owned `T` its checked destination requires (RFC 164).
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
