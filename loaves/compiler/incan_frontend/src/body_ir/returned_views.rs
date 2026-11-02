//! Returned views: a view body returns a borrow of its declared source place, and a caller aliases the
//! substituted place of a view call rather than holding the returned reference. A carried view travels as one payload
//! of a `Result` or `Option`; `?` on a success-carried view continues on the substituted place without reading the
//! carrier's reference.

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess, IncanType};

use super::{BodyBuilder, ast, bir, hir_span};
use crate::ownership_metadata::SourceSlot;

/// The declared view of the body being lowered: the borrowed parameter local and its stored-field path.
#[derive(Debug, Clone)]
pub(super) struct ReturnedViewSource {
    pub view: bir::ReturnedView,
    pub local: bir::LocalId,
    pub fields: Vec<CanonicalSymbolId>,
}

impl BodyBuilder<'_, '_> {
    /// Select the owner's declared view once its parameter locals exist. A method's receiver is slot 0.
    pub(super) fn prepare_returned_view(
        &mut self,
        owner: Option<&CanonicalSymbolId>,
        params: &[bir::CallableParam],
        has_receiver: bool,
    ) {
        let contract = owner.and_then(|owner| self.type_info.declarations.ownership_contracts.get(owner));
        let Ok(Some(view)) = crate::ownership_metadata::returned_view(contract) else {
            return;
        };
        let slot = match view.source.slot {
            SourceSlot::Receiver if has_receiver => 0,
            SourceSlot::Receiver => return,
            SourceSlot::Parameter(slot) => slot + usize::from(has_receiver),
        };
        let Some(parameter) = params.get(slot) else { return };
        self.returned_view = Some(ReturnedViewSource {
            view: bir::ReturnedView {
                slot,
                access: view.access,
                carried: view.carried,
            },
            local: parameter.local,
            fields: view.source.fields.clone(),
        });
    }

    /// Return a borrow of exactly the declared source place. The checker proved the relation; this authenticates
    /// the lowered place against the declared local and field identities before borrowing it.
    pub(super) fn lower_view_return(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(), String> {
        let (place, access) = self.authenticated_view_place(value, scope, out)?;
        out.push(bir::Statement {
            kind: bir::StatementKind::Return {
                value: Some(bir::Operand::place(place, view_fact(access), false)),
            },
            span,
        });
        Ok(())
    }

    /// A carried view's body returns its carrier: the variant holding the view (`Ok` or `Some`, or `Err` for a view
    /// carried in the error) holds a borrow of exactly the declared source place, the other variant holds its owned
    /// payload, and a carried view call or `None` is returned as lowered.
    pub(super) fn lower_carried_return(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        carried: usize,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(), String> {
        let carrier = self.owner_return_type.clone();
        // The same constructor authentication every other Body IR construction uses.
        let constructed = match &value.node {
            ast::Expr::Call(callee, type_args, arguments) if type_args.is_empty() => match arguments.as_slice() {
                [ast::CallArg::Positional(payload)] => {
                    if let Some(kind) = self.intrinsic_result_constructor(callee, value.span) {
                        Some((Some(kind), payload))
                    } else if self.intrinsic_option_constructor(callee) {
                        Some((None, payload))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let operand = match (constructed, &carrier) {
            (Some((Some(kind), payload)), IncanType::Generic { base, args }) if base == "Result" && args.len() == 2 => {
                let payload = if (kind == bir::ResultVariantKind::Ok) == (carried == 0) {
                    let (place, access) = self.authenticated_view_place(payload, scope, out)?;
                    bir::Operand::place(place, view_fact(access), false)
                } else {
                    let owned = self.lower_expr_to_operand(payload, scope, out);
                    self.owned_parameter_operand(payload, owned)
                };
                self.carried_variant(kind, payload, carried, value.span, &carrier, scope, out)
            }
            (Some((None, payload)), IncanType::Generic { base, args })
                if base == "Option" && args.len() == 1 && carried == 0 =>
            {
                let (place, access) = self.authenticated_view_place(payload, scope, out)?;
                self.push_assign_temp(
                    bir::Rvalue::Aggregate(
                        bir::AggregateKind::OptionSome,
                        super::args::fixed_elements(vec![bir::Operand::place(place, view_fact(access), false)]),
                    ),
                    carrier.clone(),
                    scope,
                    hir_span(value.span),
                    out,
                )
            }
            _ => self.lower_return_operand(value, scope, out),
        };
        out.push(bir::Statement {
            kind: bir::StatementKind::Return { value: Some(operand) },
            span,
        });
        Ok(())
    }

    /// Construct one variant of the body's reference-carrying `Result`, whose `carried` payload type is the view's
    /// reference.
    #[allow(clippy::too_many_arguments)]
    fn carried_variant(
        &mut self,
        kind: bir::ResultVariantKind,
        payload: bir::Operand,
        carried: usize,
        span: ast::Span,
        carrier: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let IncanType::Generic { args, .. } = carrier else {
            return bir::Operand::Constant(bir::Constant::Unit);
        };
        let canonical_types = reference_carrier_identities(
            self.type_info
                .expressions
                .expression_type_identities
                .get(&span)
                .cloned()
                .unwrap_or_default(),
            carried,
        );
        self.push_assign_temp(
            bir::Rvalue::ResultVariant(bir::ResultVariant {
                kind,
                payload,
                ok_type: args[0].clone(),
                error_type: args[1].clone(),
                canonical_types,
            }),
            carrier.clone(),
            scope,
            hir_span(span),
            out,
        )
    }

    /// Lower a returned view's place and authenticate it against the declared local and field identities.
    fn authenticated_view_place(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(bir::Place, IncanInputAccess), String> {
        let source = self.returned_view.clone().ok_or("view return has no declared source")?;
        let place = self.lower_expr_to_place(value, scope, out);
        let authenticated = place.local_id() == Some(source.local)
            && place.projection.len() == source.fields.len()
            && place.projection.iter().zip(&source.fields).all(|(projection, field)| {
                matches!(projection, bir::PlaceElem::Field { canonical: Some(identity), structural: false, .. }
                    if identity == field)
            });
        if !authenticated {
            return Err("returned view differs from its declared source place".into());
        }
        Ok((place, source.view.access))
    }

    /// For a call whose selected callee returns a view, the substituted place: the receiver's or the slot
    /// argument's place followed by the declared fields, with the view's access. Other calls return `None`.
    pub(super) fn view_call_place(
        &self,
        span: ast::Span,
        receiver: Option<&bir::Operand>,
        arguments: &[bir::ArgumentElement],
    ) -> Option<Result<(bir::Place, IncanInputAccess), String>> {
        let (access, slot, fields) = self.type_info.selected_call_view(span)?;
        Some(substituted_place(access, slot, fields, receiver, arguments))
    }

    /// Whether an expression is `?` taking a success-carried view out of an already lowered call, or out of a local
    /// still holding one, through parentheses.
    pub(super) fn carried_view_try(&self, expression: &ast::Spanned<ast::Expr>) -> bool {
        match &unparenthesized(expression).node {
            ast::Expr::Try(inner) => self.carried_view_of(inner).is_some_and(|(.., payload)| payload == 0),
            _ => false,
        }
    }

    /// The substituted place, access and carrying payload of an already lowered call carrying a view, or of the call
    /// a local bound to one still holds, through parentheses.
    pub(super) fn carried_view_of(
        &self,
        call: &ast::Spanned<ast::Expr>,
    ) -> Option<(bir::Place, IncanInputAccess, usize)> {
        let expression = unparenthesized(call);
        let span = match &expression.node {
            ast::Expr::Ident(_) => *self
                .type_info
                .resolved_identity(expression.span)
                .and_then(|identity| self.carried_view_locals.get(identity))?,
            _ => expression.span,
        };
        self.carried_view_places.get(&span).cloned()
    }

    /// The span of a call that carries a view in its result, written without `?`, through parentheses, with the
    /// payload carrying the view.
    pub(super) fn carried_view_call(&self, expression: &ast::Spanned<ast::Expr>) -> Option<(ast::Span, usize)> {
        let expression = unparenthesized(expression);
        if !matches!(expression.node, ast::Expr::Call(..) | ast::Expr::MethodCall(..)) {
            return None;
        }
        let (.., payload) = self.type_info.selected_carried_view(expression.span)?;
        Some((expression.span, payload))
    }

    /// For a call whose selected callee carries a view in its `Result` or `Option`, the same substituted place a
    /// direct view would give, with the payload carrying the view. `?` on a success-carried view continues on it; the
    /// carrier's own reference is never read.
    pub(super) fn carried_view_place(
        &self,
        span: ast::Span,
        receiver: Option<&bir::Operand>,
        arguments: &[bir::ArgumentElement],
    ) -> Option<Result<(bir::Place, IncanInputAccess, usize), String>> {
        let (access, slot, fields, payload) = self.type_info.selected_carried_view(span)?;
        Some(
            substituted_place(access, slot, fields, receiver, arguments)
                .map(|(place, access)| (place, access, payload)),
        )
    }

    /// Emit a view call for its effects only. The returned reference is not retained: the caller uses the
    /// substituted place, so no reference-valued local, wrapper or copy exists.
    pub(super) fn push_view_call(
        &mut self,
        callee: bir::Callee,
        args: Vec<bir::ArgumentElement>,
        place: bir::Place,
        access: IncanInputAccess,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: None,
                callee,
                args,
                may_panic: false,
            },
            span,
        });
        bir::Operand::place(place, view_fact(access), false)
    }
}

/// An expression without its redundant parentheses: the call whose span keys a carried view.
fn unparenthesized(expression: &ast::Spanned<ast::Expr>) -> &ast::Spanned<ast::Expr> {
    match &expression.node {
        ast::Expr::Paren(inner) => unparenthesized(inner),
        _ => expression,
    }
}

/// The receiver's or the slot argument's place followed by the declared fields, with the view's access.
fn substituted_place(
    access: IncanInputAccess,
    slot: SourceSlot,
    fields: Vec<CanonicalSymbolId>,
    receiver: Option<&bir::Operand>,
    arguments: &[bir::ArgumentElement],
) -> Result<(bir::Place, IncanInputAccess), String> {
    let base = match slot {
        SourceSlot::Receiver => receiver,
        SourceSlot::Parameter(slot) => match arguments.get(slot) {
            Some(bir::ArgumentElement::One(operand)) => Some(operand),
            _ => None,
        },
    };
    let Some(bir::Operand::Place(read)) = base else {
        return Err("returned view source is not borrowed from existing storage".to_string());
    };
    let mut place = read.place.clone();
    place.projection.extend(
        fields
            .into_iter()
            .map(|field| bir::PlaceElem::field(field.declaration_name.clone(), Some(field))),
    );
    Ok((place, access))
}

/// A carrier type whose `payload` argument is the view's reference: `Result[&mut T, E]` for `Result[T, E]` with an
/// exclusive view in the success payload, `Result[T, &E]` with a shared view in the error. Other types are returned
/// unchanged.
pub(super) fn reference_carrier(carrier: IncanType, access: IncanInputAccess, payload: usize) -> IncanType {
    match carrier {
        IncanType::Generic { base, mut args } if payload < args.len() => {
            let value = Box::new(args[payload].clone());
            args[payload] = if access == IncanInputAccess::Exclusive {
                IncanType::RefMut(value)
            } else {
                IncanType::Ref(value)
            };
            IncanType::Generic { base, args }
        }
        other => other,
    }
}

/// The checked leaf identities of a value-typed carrier, moved under the reference `reference_carrier` places at the
/// `payload` argument: a leaf at `[payload, ...]` sits at `[payload, 0, ...]` once that payload is a reference to it.
fn reference_carrier_identities(
    identities: std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    payload: usize,
) -> std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId> {
    identities
        .into_iter()
        .map(|(mut path, identity)| {
            if path.first() == Some(&payload) {
                path.insert(1, 0);
            }
            (path, identity)
        })
        .collect()
}

/// The ownership fact of a borrow with the view's access.
pub(super) fn view_fact(access: IncanInputAccess) -> bir::OwnershipFact {
    if access == IncanInputAccess::Exclusive {
        bir::OwnershipFact::MutBorrow
    } else {
        bir::OwnershipFact::Borrow
    }
}
