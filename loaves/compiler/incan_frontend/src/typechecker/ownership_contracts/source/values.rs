//! Access projection for fresh model values and primitive text formatting. Neither route calls user hooks implicitly.

use std::collections::BTreeSet;

use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

use super::{Builder, ResolvedType, Unavailable};
use crate::ast::{CallArg, Expr, FStringFormat, FStringPart, ListEntry, Span, Spanned};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::TypeInfo;

impl Builder<'_> {
    /// Mark constructor arguments whose selected stored field is itself a view type. This is the direct-field form of
    /// the older generic-carrier projection below; both use the checked argument-slot binding and declaration order.
    pub(super) fn constructor_view_fields(&self, span: Span) -> Option<Vec<bool>> {
        let ty = self.checker.type_info.expr_type(span)?;
        let (owner, arguments) = match ty {
            ResolvedType::Named(owner) => (owner, &[][..]),
            ResolvedType::Generic(owner, arguments) => (owner, arguments.as_slice()),
            _ => return None,
        };
        let (parameters, fields, order) = match self.checker.lookup_type_info(owner)? {
            TypeInfo::Model(info) => (&info.type_params, &info.fields, &info.field_order),
            TypeInfo::Class(info) => (&info.type_params, &info.fields, &info.field_order),
            TypeInfo::Newtype(info) => {
                let underlying =
                    substitute_resolved_type(&info.underlying, &type_param_subst_map(&info.type_params, arguments));
                return Some(vec![self.checker.type_holds_borrow(&underlying)]);
            }
            _ => return None,
        };
        let substitution = type_param_subst_map(parameters, arguments);
        let binding = self.checker.type_info.constructor_field_binding(span)?;
        Some(
            binding
                .argument_slots
                .iter()
                .map(|slot| {
                    order.get(*slot).and_then(|name| fields.get(name)).is_some_and(|field| {
                        let ty = substitute_resolved_type(&field.ty, &substitution);
                        self.checker.type_holds_borrow(&ty)
                    })
                })
                .collect(),
        )
    }

    /// Mark the payloads of one selected source enum variant that store a direct or transitive borrow.
    pub(super) fn enum_view_constructor_fields(&self, callee: &Spanned<Expr>, span: Span) -> Option<Vec<bool>> {
        let Expr::Field(base, variant) = &callee.node else {
            return None;
        };
        self.enum_view_method_constructor_fields(base, variant, span)
    }

    /// Mark view payloads for the method-shaped `Enum.Variant(...)` AST used by ordinary source syntax.
    pub(super) fn enum_view_method_constructor_fields(
        &self,
        base: &Spanned<Expr>,
        written_variant: &str,
        span: Span,
    ) -> Option<Vec<bool>> {
        let checked_ty = self
            .checker
            .type_info
            .expr_type(span)
            .or_else(|| self.checker.type_info.expr_type(base.span));
        let (owner, arguments) = match checked_ty {
            Some(ResolvedType::Named(owner)) => (owner.as_str(), &[][..]),
            Some(ResolvedType::Generic(owner, arguments)) => (owner.as_str(), arguments.as_slice()),
            _ => match &base.node {
                Expr::Ident(owner) => (owner.as_str(), &[][..]),
                _ => return None,
            },
        };
        let TypeInfo::Enum(info) = self.checker.lookup_type_info(owner)? else {
            return None;
        };
        let variant = info
            .variant_aliases
            .get(written_variant)
            .map(String::as_str)
            .unwrap_or(written_variant);
        let substitution = type_param_subst_map(&info.type_params, arguments);
        Some(
            info.variant_fields
                .get(variant)?
                .iter()
                .map(|field| {
                    let field = substitute_resolved_type(field, &substitution);
                    self.checker.type_holds_borrow(&field)
                })
                .collect(),
        )
    }

    /// Evaluate a checker-selected payload variant as fresh storage. Each payload is fresh, Copy, or an exact
    /// transferable place; a borrowed place cannot become owned merely by crossing the variant boundary.
    pub(super) fn variant_construction(
        &mut self,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for argument in arguments {
            let (CallArg::Positional(value) | CallArg::Named(_, value)) = argument else {
                return Err(Unavailable {
                    span,
                    reason: "variant construction needs fixed checked payloads".into(),
                });
            };
            if !self.fresh_or_copy_field(value) {
                let place = self.optional_place(value)?.ok_or(Unavailable {
                    span: value.span,
                    reason: "non-Copy variant payload needs checked ownership transfer materialization".into(),
                })?;
                if place.through.is_some() || self.borrowed_inputs.contains(&place.place.root) {
                    return Err(Unavailable {
                        span: value.span,
                        reason: super::BORROWED_VALUE_IN_OWNED_FIELD.into(),
                    });
                }
                self.constrained.insert(place.place.root);
                incoming = self.expression(value, incoming, true)?;
                continue;
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

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
    /// after evaluating supplied values in written order. Deferred defaults, native constructors and non-Copy place
    /// transfer require separate proofs.
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
                    SemanticSourceTargetKind::Model
                        | SemanticSourceTargetKind::Class
                        | SemanticSourceTargetKind::Newtype
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
        if !binding.defaulted_slots.is_empty()
            || binding.field_count != arguments.len()
            || binding.argument_slots.len() != arguments.len()
            || binding
                .argument_slots
                .iter()
                .any(|slot| *slot >= binding.field_count || !selected.insert(*slot))
        {
            return Err(Unavailable {
                span,
                reason: "construction needs complete distinct fields without deferred defaults".into(),
            });
        }
        let view_arguments = self.constructor_view_fields(span);
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
            if stores_view {
                self.validate_view_field_source(value)?;
            }
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

    /// Evaluate one enum construction whose checked payload layout holds a view, enforcing the callable's one source.
    pub(super) fn enum_view_construction(
        &mut self,
        arguments: &[CallArg],
        view_fields: &[bool],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if arguments.len() != view_fields.len() {
            return Err(Unavailable {
                span,
                reason: "enum construction needs its checked payload layout".into(),
            });
        }
        for (argument, stores_view) in arguments.iter().zip(view_fields) {
            let CallArg::Positional(value) = argument else {
                return Err(Unavailable {
                    span,
                    reason: "enum construction expansion lacks one checked payload".into(),
                });
            };
            if *stores_view {
                self.validate_view_field_source(value)?;
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

    /// Identify source-bearing enum arguments from checked input and dependent-local facts when no field binding is
    /// published for the variant constructor. Ordinary owned payloads stay independent even beside a view payload.
    pub(super) fn enum_argument_view_sources(&mut self, arguments: &[CallArg]) -> Result<Vec<bool>, Unavailable> {
        let mut sources = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                _ => {
                    sources.push(false);
                    continue;
                }
            };
            let typed_view = self
                .checker
                .type_info
                .expr_type(value.span)
                .is_some_and(|ty| self.checker.type_holds_borrow(ty));
            let borrowed_place = self.optional_place(value)?.is_some_and(|path| match &path.place.root {
                super::Root::Binding(root) => {
                    self.borrowed_inputs.contains(&path.place.root) || self.graph.dependent_locals.contains_key(root)
                }
                super::Root::Temporary { .. } => false,
            });
            sources.push(typed_view || borrowed_place);
        }
        Ok(sources)
    }

    /// Require each stored borrow in a constructed view to resolve to the producing callable's one declared source.
    /// Direct places and previously retained view locals are compared by canonical graph place, so diagnostics can
    /// name the two independent source bindings without inferring per-field lifetimes.
    fn validate_view_field_source(&mut self, value: &Spanned<Expr>) -> Result<(), Unavailable> {
        let Some(expected) = self.owned_source.clone() else {
            return Ok(());
        };
        let Some(path) = self.optional_place(value)? else {
            return Ok(());
        };
        let sources = match &path.place.root {
            super::Root::Binding(root) => self
                .graph
                .dependent_locals
                .get(root)
                .map(|loans| {
                    loans
                        .iter()
                        .filter_map(|loan| self.graph.loans.get(loan.0).map(|loan| loan.source.clone()))
                        .collect::<Vec<_>>()
                })
                .filter(|sources| !sources.is_empty())
                .unwrap_or_else(|| vec![path.place]),
            super::Root::Temporary { .. } => vec![path.place],
        };
        for actual in sources {
            if actual != expected {
                self.view_errors.push(super::view_error(
                    &format!(
                        "A view value must hold borrows from one source; '{}' differs from declared source '{}'",
                        Self::view_source_name(&actual),
                        Self::view_source_name(&expected)
                    ),
                    value.span,
                ));
            }
        }
        Ok(())
    }

    /// Render the root binding of one checked source place for a stable, source-oriented diagnostic.
    fn view_source_name(place: &super::Place) -> &str {
        match &place.root {
            super::Root::Binding(identity) => identity.declaration_name.as_str(),
            super::Root::Temporary { .. } => "temporary value",
        }
    }

    /// Copy values need no transfer; literal, formatted, list-literal, explicitly cloned and nested constructed
    /// values own their fresh storage (a list literal's elements are Copy scalars, which its own evaluation checks).
    /// A source place with a non-Copy type is never reclassified as fresh merely because its last use is here.
    pub(super) fn fresh_or_copy_field(&self, value: &Spanned<Expr>) -> bool {
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
            Expr::Field(_, _) => self
                .checker
                .type_info
                .resolved_identity(value.span)
                .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Variant),
            Expr::Paren(inner) => self.fresh_or_copy_field(inner),
            Expr::Call(callee, _, _) => {
                self.checker.type_info.constructor_field_binding(value.span).is_some()
                    || self
                        .checker
                        .type_info
                        .resolved_identity(callee.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Variant)
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
            Expr::MethodCall(..) => {
                self.checked_clone_call(value.span)
                    || self
                        .checker
                        .type_info
                        .resolved_identity(value.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Variant)
            }
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
