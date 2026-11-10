//! Access projection for fresh model values and primitive text formatting. Neither route calls user hooks implicitly.

use std::collections::BTreeSet;

use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

use super::{Builder, ResolvedType, Unavailable};
use crate::ast::{CallArg, Expr, FStringFormat, FStringPart, ListEntry, Span, Spanned};
use crate::symbols::TypeInfo;

impl Builder<'_> {
    /// Mark constructor arguments whose declared field is exactly the type parameter selected by a carried view.
    /// The checked argument-slot binding and nominal declaration own both orders; source field spellings are not
    /// re-resolved here.
    pub(super) fn constructor_view_arguments(&self, span: Span, component: &[usize]) -> Option<Vec<bool>> {
        let (&parameter_slot, _) = component.split_first()?;
        let ResolvedType::Generic(owner, _) = self.checker.type_info.expr_type(span)? else {
            return None;
        };
        let (parameters, fields, order) = match self.checker.lookup_type_info(owner)? {
            TypeInfo::Model(info) => (&info.type_params, &info.fields, &info.field_order),
            TypeInfo::Class(info) => (&info.type_params, &info.fields, &info.field_order),
            _ => return None,
        };
        let parameter = parameters.get(parameter_slot)?;
        let binding = self.checker.type_info.constructor_field_binding(span)?;
        Some(
            binding
                .argument_slots
                .iter()
                .map(|slot| {
                    order.get(*slot).and_then(|name| fields.get(name)).is_some_and(|field| {
                        matches!(&field.ty,
                                ResolvedType::TypeVar(name) | ResolvedType::Named(name) if name == parameter)
                    })
                })
                .collect(),
        )
    }

    /// Evaluate a list that stores returned views, retaining each substituted source loan under the list value.
    /// Non-view elements keep the same Copy-scalar boundary as an ordinary admitted list literal.
    pub(super) fn dependent_list(
        &mut self,
        entries: &[ListEntry],
        incoming: &[usize],
    ) -> Result<Option<(Vec<usize>, Vec<super::LoanId>)>, Unavailable> {
        let mut has_dependency = false;
        for entry in entries {
            let ListEntry::Element(value) = entry else { continue };
            if let Some(path) = self.optional_place(value)?
                && let super::Root::Binding(root) = path.place.root
                && self.graph.dependent_locals.contains_key(&root)
            {
                has_dependency = true;
                continue;
            }
            let call = super::super::builtins::view_call(value);
            let native = self
                .checker
                .type_info
                .protocols
                .explicit_input_calls
                .get(&call.span)
                .is_some_and(|call| call.kind == super::super::InputCallKind::Native);
            has_dependency |= !native && self.view_source(value)?.is_some();
        }
        if !has_dependency {
            return Ok(None);
        }
        let mut incoming = incoming.to_vec();
        let mut loans = Vec::new();
        for entry in entries {
            let value = match entry {
                ListEntry::Element(value) => value,
                ListEntry::Spread(value) => {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "a reference-carrying list cannot infer spread dependencies".into(),
                    });
                }
            };
            let call = super::super::builtins::view_call(value);
            let native = self
                .checker
                .type_info
                .protocols
                .explicit_input_calls
                .get(&call.span)
                .is_some_and(|call| call.kind == super::super::InputCallKind::Native);
            if let Some(stored) = self.stored_result(value, incoming.clone())? {
                incoming = stored.incoming;
                for dependency in stored.loans {
                    let (next, _, loan) = self.carry_dependency(dependency, value.span, incoming)?;
                    incoming = next;
                    loans.push(loan);
                }
                continue;
            }
            if !native && let Some(source) = self.view_source(value)? {
                let (outgoing, path) = self.evaluate_view_source(value, source, incoming)?;
                incoming = outgoing;
                loans.push(path.through.ok_or(Unavailable {
                    span: value.span,
                    reason: "stored returned view has no result loan".into(),
                })?);
                continue;
            }
            if !matches!(
                self.checker.type_info.expr_type(value.span),
                Some(
                    ResolvedType::Int
                        | ResolvedType::Float
                        | ResolvedType::Numeric(_)
                        | ResolvedType::Bool
                        | ResolvedType::Unit
                )
            ) {
                return Err(Unavailable {
                    span: value.span,
                    reason: "a reference-carrying list needs returned views or Copy scalar elements".into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(Some((incoming, loans)))
    }

    /// A fresh list evaluates each Copy or independently fresh element in written order and retains no borrowed
    /// dependency. Spreads and non-Copy stored places need separate retained-owner projection.
    pub(super) fn scalar_list(
        &mut self,
        entries: &[ListEntry],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for entry in entries {
            let value = match entry {
                ListEntry::Element(value) => value,
                ListEntry::Spread(value) => {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "list expansion needs checked element access".into(),
                    });
                }
            };
            let copy_scalar = matches!(
                self.checker.type_info.expr_type(value.span),
                Some(
                    ResolvedType::Int
                        | ResolvedType::Float
                        | ResolvedType::Numeric(_)
                        | ResolvedType::Bool
                        | ResolvedType::Unit
                )
            );
            if !copy_scalar && !self.fresh_or_copy_field(value) && !self.allow_fresh_aggregate_elements {
                return Err(Unavailable {
                    span: value.span,
                    reason: "list element needs checked retained-owner projection".into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

    /// A complete model or class field binding, declared in this project or by a provider, constructs fresh storage
    /// after evaluating supplied values in written order. Declaration-owned defaults add no caller-side access;
    /// supplied and defaulted slots must form an exact partition. Native constructors and non-Copy place transfers
    /// retain their separate proof requirements.
    pub(super) fn model_construction(
        &mut self,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let owner = self.checker.type_info.resolved_identity(callee.span);
        if !matches!(callee.node, Expr::Ident(_))
            || owner.is_none_or(|owner| {
                !matches!(
                    owner.kind,
                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                ) || !matches!(owner.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
            })
        {
            return Err(Unavailable {
                span,
                reason: "construction needs a checked source model or class declaration".into(),
            });
        }
        let binding = self
            .checker
            .type_info
            .constructor_field_binding(span)
            .ok_or(Unavailable {
                span,
                reason: "construction has no checked field binding".into(),
            })?;
        let mut selected = BTreeSet::new();
        if binding.argument_slots.len() != arguments.len()
            || binding
                .argument_slots
                .iter()
                .chain(&binding.defaulted_slots)
                .any(|slot| *slot >= binding.field_count || !selected.insert(*slot))
            || selected.len() != binding.field_count
            || binding.defaulted_slots.windows(2).any(|slots| slots[0] >= slots[1])
        {
            return Err(Unavailable {
                span,
                reason: "construction needs a complete distinct supplied/defaulted field partition".into(),
            });
        }
        let view_arguments = self
            .checker
            .type_info
            .expr_type(span)
            .and_then(|ty| match ty {
                ResolvedType::Generic(_, arguments) => arguments.iter().position(|argument| {
                    matches!(argument,
                        ResolvedType::Generic(name, payload)
                            if payload.len() == 1 && self.checker.explicit_ownership_mode(name).is_some())
                }),
                _ => None,
            })
            .and_then(|slot| self.constructor_view_arguments(span, &[slot]));
        for (index, argument) in arguments.iter().enumerate() {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                _ => {
                    return Err(Unavailable {
                        span,
                        reason: "construction expansion lacks checked field access".into(),
                    });
                }
            };
            let stores_view = view_arguments
                .as_ref()
                .and_then(|arguments| arguments.get(index))
                .copied()
                .unwrap_or(false);
            if !stores_view && !self.fresh_or_copy_field(value) {
                let borrowed = self
                    .optional_place(value)?
                    .is_some_and(|path| path.through.is_some() || self.borrowed_inputs.contains(&path.place.root));
                return Err(Unavailable {
                    span: value.span,
                    reason: if borrowed {
                        super::BORROWED_VALUE_IN_OWNED_FIELD.into()
                    } else {
                        "non-Copy constructor field needs checked ownership transfer materialization".into()
                    },
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

    /// Authenticate a source enum construction by its retained variant and result-owner identities. A method with
    /// the same spelling, a native variant or an ordinary field is not a source constructor. Payload evaluation is
    /// handled separately so recognizing fresh aggregate storage never authorizes an unproved payload transfer.
    pub(super) fn source_enum_payload_count(&self, value: &Spanned<Expr>) -> Option<usize> {
        super::super::builtins::source_enum_construction(&self.checker.type_info, value)?;
        let identity = self.checker.type_info.resolved_identity(value.span)?;
        if identity.kind != SemanticSourceTargetKind::Variant
            || !matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
        {
            return None;
        }
        let owner = match self.checker.type_info.expr_type(value.span)? {
            ResolvedType::Named(owner) | ResolvedType::Generic(owner, _) => owner,
            _ => return None,
        };
        let TypeInfo::Enum(declaration) = self.checker.lookup_semantic_type_info(owner)? else {
            return None;
        };
        let name = declaration
            .variants
            .iter()
            .find(|name| declaration.variant_identities.get(*name) == Some(identity))?;
        declaration.variant_fields.get(name).map(Vec::len)
    }

    /// Evaluate fresh enum payloads in written order after authenticating the variant's complete checked arity.
    /// Non-Copy places and stored views retain their ownership proof requirements, just as model fields do.
    pub(super) fn source_enum_construction(
        &mut self,
        value: &Spanned<Expr>,
        count: usize,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let arguments = match &value.node {
            Expr::MethodCall(_, _, _, arguments) => arguments.as_slice(),
            Expr::Field(..) if count == 0 => return Ok(incoming),
            _ => {
                return Err(Unavailable {
                    span: value.span,
                    reason: "source enum construction lacks its checked payload arguments".into(),
                });
            }
        };
        if arguments.len() != count {
            return Err(Unavailable {
                span: value.span,
                reason: "source enum construction needs its complete checked payload".into(),
            });
        }
        for argument in arguments {
            let payload = match argument {
                CallArg::Positional(payload) | CallArg::Named(_, payload) => payload,
                _ => {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "source enum construction expansion lacks checked payload access".into(),
                    });
                }
            };
            if !self.fresh_or_copy_field(payload) {
                return Err(Unavailable {
                    span: payload.span,
                    reason: "non-Copy enum payload needs checked ownership transfer materialization".into(),
                });
            }
            incoming = self.expression(payload, incoming, false)?;
        }
        Ok(incoming)
    }

    /// Copy values need no transfer; literal, formatted, list-literal, explicitly cloned and nested constructed
    /// values own their fresh storage (a list literal's elements are Copy scalars, which its own evaluation checks).
    /// A source place with a non-Copy type is never reclassified as fresh merely because its last use is here.
    pub(super) fn fresh_or_copy_field(&self, value: &Spanned<Expr>) -> bool {
        if self.source_enum_payload_count(value).is_some() {
            return true;
        }
        if self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|ty| self.copy_value(ty))
        {
            return true;
        }
        match &value.node {
            Expr::Literal(_) | Expr::FString(_) | Expr::List(_) => true,
            Expr::Paren(inner) => self.fresh_or_copy_field(inner),
            Expr::Call(_, _, _) => {
                self.checker.type_info.constructor_field_binding(value.span).is_some()
                    || self
                        .checker
                        .type_info
                        .protocols
                        .explicit_input_calls
                        .get(&value.span)
                        .and_then(|call| self.checker.type_info.selected_input_contract(call).ok())
                        .is_some_and(|contract| {
                            crate::ownership_metadata::owned_result(contract).is_ok_and(|result| result.is_some())
                                && contract.is_some_and(|contract| contract.result_source.is_none())
                        })
            }
            Expr::MethodCall(..) => self.checked_clone_call(value.span),
            _ => false,
        }
    }

    /// Primitive formatting observes each interpolation in source order and creates a new string. Nominal/native
    /// display hooks and unsupported format expressions cannot acquire ordinary read semantics through this route.
    pub(super) fn formatted_text(
        &mut self,
        parts: &[FStringPart],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let mut operands = Vec::new();
        for part in parts {
            let FStringPart::Expr { expr, format } = part else {
                continue;
            };
            if !matches!(format, FStringFormat::Display | FStringFormat::Debug)
                || !matches!(
                    self.checker.type_info.expr_type(expr.span),
                    Some(
                        ResolvedType::Str
                            | ResolvedType::FrozenStr
                            | ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Unit
                    )
                )
            {
                return Err(Unavailable {
                    span: expr.span,
                    reason: "formatting needs selected display-hook access facts".into(),
                });
            }
            incoming = self.expression(expr, incoming, false)?;
            let (evaluated, dependency) = self.retain_operation_operand(expr, incoming)?;
            incoming = evaluated;
            operands.extend(dependency);
        }
        self.finish_operation(span, operands, incoming)
    }
}
