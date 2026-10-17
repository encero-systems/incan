//! Checked tuple spread components and their single source evaluation at the call binding boundary.

use super::args::{DeclaredSlot, expand_shaped_spread, fixed_elements, plan_declared_args};
use super::{BodyBuilder, IncanCallableParamKind, IncanType, TypeCheckInfo, ast, bir, hir_span};
use crate::typechecker::FixedUnpackPlan;
use std::collections::HashMap;

/// Whether a checked nonliteral tuple spread needs component projection rather than literal expansion.
pub(super) fn has_checked_tuple_spread(info: &TypeCheckInfo, args: &[ast::CallArg]) -> bool {
    args.iter().any(|arg| {
        matches!(arg, ast::CallArg::PositionalUnpack(source)
            if matches!(info.fixed_unpack_plan(source.span), Some(FixedUnpackPlan::Positional(_)))
                && expand_shaped_spread(info, arg).is_none())
    })
}

/// An ordinary argument or one checked tuple component, sharing the original source span for evaluation grouping.
struct ShapedArgument {
    argument: ast::CallArg,
    component: Option<(usize, IncanType)>,
}

impl BodyBuilder<'_, '_> {
    /// Expand literal shapes and checked tuple shapes without evaluating any argument or changing checker facts.
    fn shaped_arguments(&self, args: &[ast::CallArg]) -> Result<Vec<ShapedArgument>, String> {
        let mut shaped = Vec::new();
        for argument in args {
            if let Some(expansion) = expand_shaped_spread(self.type_info, argument) {
                shaped.extend(expansion.into_iter().map(|argument| ShapedArgument {
                    argument,
                    component: None,
                }));
                continue;
            }
            if let ast::CallArg::PositionalUnpack(source) = argument
                && let Some(FixedUnpackPlan::Positional(elements)) = self.type_info.fixed_unpack_plan(source.span)
            {
                let IncanType::Tuple(retained) = self.resolve_ty(source.span) else {
                    return Err("unsupported checked fixed spread without tuple storage".to_string());
                };
                if retained.len() != elements.len() {
                    return Err("unsupported checked fixed spread with mismatched tuple arity".to_string());
                }
                for (index, (actual, checked)) in retained.iter().zip(elements).enumerate() {
                    let ty = self.checked_type(checked);
                    if *actual != ty {
                        return Err("unsupported checked fixed spread with mismatched tuple component".to_string());
                    }
                    shaped.push(ShapedArgument {
                        argument: ast::CallArg::Positional(source.clone()),
                        component: Some((index, ty)),
                    });
                }
                continue;
            }
            shaped.push(ShapedArgument {
                argument: argument.clone(),
                component: None,
            });
        }
        Ok(shaped)
    }

    /// Evaluate each tuple source once and retain typed component reads beside ordinary written arguments.
    fn lower_shaped_arguments(
        &mut self,
        shaped: &[ShapedArgument],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<(bir::Operand, IncanType)> {
        let mut tuple_places: HashMap<(usize, usize), bir::Place> = HashMap::new();
        let mut values = Vec::with_capacity(shaped.len());
        for entry in shaped {
            let expr = match &entry.argument {
                ast::CallArg::Positional(expr)
                | ast::CallArg::Named(_, expr)
                | ast::CallArg::PositionalUnpack(expr)
                | ast::CallArg::KeywordUnpack(expr) => expr,
            };
            let Some((index, ty)) = &entry.component else {
                let operand = self.lower_expr_to_operand(expr, scope, out);
                values.push((self.owned_parameter_operand(expr, operand), self.resolve_ty(expr.span)));
                continue;
            };
            let key = (expr.span.start, expr.span.end);
            let mut place = match tuple_places.entry(key) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    let place = self.materialize_operand_to_place(
                        operand,
                        self.resolve_ty(expr.span),
                        scope,
                        hir_span(expr.span),
                        out,
                    );
                    entry.insert(place).clone()
                }
            };
            place
                .projection
                .push(bir::PlaceElem::structural_field(index.to_string()));
            let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Clone
            };
            values.push((bir::Operand::place(place, fact, false), ty.clone()));
        }
        values
    }

    /// Bind checked tuple components to ordinary/rest slots while preserving single evaluation and slot ordering.
    pub(super) fn bind_checked_tuple_spreads(
        &mut self,
        callee: &str,
        slots: &[DeclaredSlot],
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let shaped = self.shaped_arguments(args)?;
        let arguments: Vec<_> = shaped.iter().map(|entry| entry.argument.clone()).collect();
        if slots.iter().any(|slot| slot.kind != IncanCallableParamKind::Normal) {
            let values = self.lower_shaped_arguments(&shaped, scope, out);
            return self.bind_prepared_rest_args(callee, slots, &arguments, Some(values), scope, out);
        }
        let planned = plan_declared_args(callee, slots, &arguments)?;
        let values = self.lower_shaped_arguments(&shaped, scope, out);
        let mut supplied = vec![None; slots.len()];
        for (written, ((slot, _), (operand, _))) in planned.into_iter().zip(values).enumerate() {
            let operand = if slots[slot].is_mut {
                self.borrow_for_mut_parameter(operand)
            } else {
                operand
            };
            supplied[slot] = Some((operand, written));
        }
        let mut operands = Vec::new();
        let mut bindings = Vec::new();
        let mut defaulted_slots = Vec::new();
        for (slot, value) in supplied.into_iter().enumerate() {
            match value {
                Some((operand, written_position)) => {
                    operands.push(operand);
                    bindings.push(bir::BoundArgument { slot, written_position });
                }
                None => defaulted_slots.push(slot),
            }
        }
        Ok((
            fixed_elements(operands),
            bir::ArgumentBinding::Resolved {
                arguments: bindings,
                defaulted_slots,
            },
        ))
    }
}
