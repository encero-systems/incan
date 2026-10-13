//! Checked rest-parameter storage and call packing for Body IR only.

use super::args::{DeclaredSlot, fixed_elements};
use super::*;

/// Represent a rest annotation by the collection the declaration actually receives.
pub(super) fn parameter_storage_type(ty: IncanType, kind: ast::ParamKind) -> IncanType {
    match kind {
        ast::ParamKind::Normal => ty,
        ast::ParamKind::RestPositional => IncanType::Generic {
            base: "List".to_string(),
            args: vec![ty],
        },
        ast::ParamKind::RestKeyword => IncanType::Generic {
            base: "Dict".to_string(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Str), ty],
        },
    }
}

/// Select declared destinations using legacy's rest-aware binding: spreads belong wholly to rest slots.
fn rest_destinations(callee: &str, slots: &[DeclaredSlot], args: &[ast::CallArg]) -> Result<Vec<usize>, String> {
    let ordinary: Vec<_> = slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| (slot.kind == IncanCallableParamKind::Normal).then_some(index))
        .collect();
    let positional_rest = slots
        .iter()
        .position(|slot| slot.kind == IncanCallableParamKind::RestPositional);
    let keyword_rest = slots
        .iter()
        .position(|slot| slot.kind == IncanCallableParamKind::RestKeyword);
    let mut positional = 0;
    args.iter()
        .map(|arg| {
            let destination = match arg {
                ast::CallArg::Positional(_) => {
                    let slot = ordinary.get(positional).copied().or(positional_rest);
                    positional += 1;
                    slot
                }
                ast::CallArg::Named(name, _) => ordinary
                    .iter()
                    .copied()
                    .find(|index| slots[*index].name.as_deref() == Some(name.node.as_str()))
                    .or(keyword_rest),
                ast::CallArg::PositionalUnpack(_) => positional_rest,
                ast::CallArg::KeywordUnpack(_) => keyword_rest,
            };
            destination.ok_or_else(|| format!("{callee}: unsupported rest argument without its declared destination"))
        })
        .collect()
}

/// Keep legacy's unbuildable repeated rest spread refused instead of introducing a clone legacy never emits.
fn rest_spread_moves_reused_local(arg: &ast::CallArg, operand: &bir::Operand, locals: &[bir::LocalDecl]) -> bool {
    if !matches!(arg, ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_)) {
        return false;
    }
    let bir::Operand::Place(read) = operand else {
        return false;
    };
    read.fact == bir::OwnershipFact::Clone
        && !read.last_use
        && read.place.projection.is_empty()
        && read
            .place
            .local_id()
            .and_then(|local| locals.get(local.index()))
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::UserBinding))
}

impl BodyBuilder<'_, '_> {
    /// Evaluate written arguments once, then pack rest values into explicit aggregates and bind complete slots.
    ///
    /// The existing kernel vocabulary represents the result without unresolved arity: one list or dictionary
    /// operand fills each rest slot, including an empty aggregate. Spreads and duplicate keyword overwrite order
    /// remain on those aggregates. No legacy checker fact or callable surface is rewritten.
    pub(super) fn bind_general_rest_args(
        &mut self,
        callee: &str,
        slots: &[DeclaredSlot],
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        self.bind_prepared_rest_args(callee, slots, args, None, scope, out)
    }

    /// Pack rest slots from either source expressions or previously evaluated checked tuple components.
    ///
    /// Prepared values carry their own retained component type; the tuple operand's expression span must not give a
    /// component the enclosing tuple's storage layout. Both paths retain the same written argument order.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_prepared_rest_args(
        &mut self,
        callee: &str,
        slots: &[DeclaredSlot],
        args: &[ast::CallArg],
        prepared: Option<Vec<(bir::Operand, IncanType)>>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        // ---- Bind checked destinations before evaluating source expressions ----
        let destinations = rest_destinations(callee, slots, args)?;
        let mut ordinary = vec![None; slots.len()];
        let mut sequences: Vec<Vec<bir::ArgumentElement>> = vec![Vec::new(); slots.len()];
        let mut mappings: Vec<Vec<bir::DictEntry>> = vec![Vec::new(); slots.len()];
        let span = args.first().map_or(ast::Span::default(), |arg| match arg {
            ast::CallArg::Positional(expr)
            | ast::CallArg::Named(_, expr)
            | ast::CallArg::PositionalUnpack(expr)
            | ast::CallArg::KeywordUnpack(expr) => expr.span,
        });

        // ---- Evaluate each argument once in written order ----
        let mut prepared = prepared.map(Vec::into_iter);
        for (written, (arg, slot)) in args.iter().zip(destinations).enumerate() {
            let expr = match arg {
                ast::CallArg::Positional(expr)
                | ast::CallArg::Named(_, expr)
                | ast::CallArg::PositionalUnpack(expr)
                | ast::CallArg::KeywordUnpack(expr) => expr,
            };
            let (mut operand, operand_ty) = match prepared.as_mut() {
                Some(values) => values.next().ok_or("missing prepared rest argument")?,
                None => (self.lower_expr_to_operand(expr, scope, out), self.resolve_ty(expr.span)),
            };
            if rest_spread_moves_reused_local(arg, &operand, &self.locals) {
                return Err("rest spread of a reused owned local: legacy moves its source".to_string());
            }
            if !slots[slot].is_mut {
                operand = self.owned_parameter_operand(expr, operand);
                operand = self.push_assign_temp(bir::Rvalue::Use(operand), operand_ty, scope, hir_span(expr.span), out);
            }
            if slots[slot].kind == IncanCallableParamKind::Normal {
                let operand = if slots[slot].is_mut {
                    self.borrow_for_mut_parameter(operand)
                } else {
                    self.owned_parameter_operand(expr, operand)
                };
                ordinary[slot] = Some((operand, written));
                continue;
            }
            match arg {
                ast::CallArg::Positional(_) => sequences[slot].push(bir::ArgumentElement::One(operand)),
                ast::CallArg::PositionalUnpack(_) => {
                    sequences[slot].push(bir::ArgumentElement::Spread(bir::SpreadElement {
                        source: operand,
                        kind: bir::SpreadKind::Sequence,
                    }))
                }
                ast::CallArg::Named(name, _) => mappings[slot].push(bir::DictEntry::Pair(
                    bir::Operand::Constant(bir::Constant::Str(name.node.clone())),
                    Box::new(operand),
                )),
                ast::CallArg::KeywordUnpack(_) => mappings[slot].push(bir::DictEntry::Spread(bir::SpreadElement {
                    source: operand,
                    kind: bir::SpreadKind::Mapping,
                })),
            }
        }

        // ---- Materialize one collection per rest slot, including empty collections ----
        let mut operands = Vec::new();
        let mut arguments = Vec::new();
        let mut defaulted_slots = Vec::new();
        for (slot, declaration) in slots.iter().enumerate() {
            let supplied = if declaration.kind != IncanCallableParamKind::Normal {
                let rvalue = match declaration.kind {
                    IncanCallableParamKind::RestPositional => {
                        bir::Rvalue::Aggregate(bir::AggregateKind::List, std::mem::take(&mut sequences[slot]))
                    }
                    IncanCallableParamKind::RestKeyword => bir::Rvalue::Dict(std::mem::take(&mut mappings[slot])),
                    IncanCallableParamKind::Normal => return Err("invalid Body IR rest declaration kind".to_string()),
                };
                self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
                let source_kind = match declaration.kind {
                    IncanCallableParamKind::Normal => ast::ParamKind::Normal,
                    IncanCallableParamKind::RestPositional => ast::ParamKind::RestPositional,
                    IncanCallableParamKind::RestKeyword => ast::ParamKind::RestKeyword,
                };
                let ty = parameter_storage_type(declaration.ty.clone(), source_kind);
                Some((
                    self.push_assign_temp(rvalue, ty, scope, hir_span(span), out),
                    args.len() + slot,
                ))
            } else {
                ordinary[slot].take()
            };
            if let Some((operand, written_position)) = supplied {
                operands.push(operand);
                arguments.push(bir::BoundArgument { slot, written_position });
            } else if declaration.has_default {
                defaulted_slots.push(slot);
            } else {
                return Err(format!(
                    "{callee}: unsupported missing ordinary argument beside rest parameters"
                ));
            }
        }
        Ok((
            fixed_elements(operands),
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            },
        ))
    }
}
