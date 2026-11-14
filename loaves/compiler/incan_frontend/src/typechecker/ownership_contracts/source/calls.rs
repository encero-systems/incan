//! Selected source-call access projection. Ordinary unqualified fixed calls evaluate in declaration order;
//! methods preserve written order. Original written positions remain provenance in both cases. Returned/native
//! dependencies and unresolved adaptations refuse this profile until their own facts are available.

use super::super::flow::{InputEvaluation, Invocation, InvocationArgument};
use super::call_results::{conditional_result, plain_result, primitive_display};
use super::{
    Access, Builder, Event, Loan, LoanId, LoanKind, OwnershipMode, Path, Permission, Place, Projection, ResolvedType,
    Root, SourceSlot, Unavailable, ViewRelation, ViewSource,
};
use crate::ast::{CallArg, Expr, ParamKind, Receiver, Span, Spanned};
use crate::symbols::CallableParam;
use crate::typechecker::TypeChecker;
use incan_lang::lang::builtins::{BuiltinFnId, BuiltinMethodId};
use incan_lang::lang::surface::{
    clone_methods::CloneMethodId, list_methods::ListMethodId, option_methods::OptionMethodId,
};
use incan_lang::lang::trait_bounds;
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::IncanInputAccess;

impl Builder<'_> {
    /// Apply a selected free-function contract without inspecting its body. Borrowed arguments remain live until
    /// invocation, and owned arguments transfer at their source evaluation position even for a read-only callee.
    /// Omitted declaration-owned defaults evaluate in the callee frame and therefore add no caller-side source
    /// event. Missing rest/capture/returned-dependency facts never become a successful partial call graph.
    pub(super) fn call(
        &mut self,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if let Some(builtin) = self.checker.type_info.resolved_builtin_call(span) {
            return if matches!(builtin, BuiltinFnId::Print | BuiltinFnId::Len) {
                self.observer_call(builtin, arguments, span, incoming)
            } else {
                self.builtin_call(builtin, arguments, span, incoming)
            };
        }
        if self.carrier_constructor(callee) {
            return self.carrier_construction(arguments, span, incoming);
        }
        if self.checker.type_info.constructor_field_binding(span).is_some() {
            return self.model_construction(callee, arguments, span, incoming);
        }
        if let Some(view_fields) = self.enum_view_constructor_fields(callee, span) {
            if view_fields.iter().any(|field| *field) {
                return self.enum_view_construction(arguments, &view_fields, span, incoming);
            }
            let view_fields = self.enum_argument_view_sources(arguments)?;
            if view_fields.iter().any(|field| *field) {
                return self.enum_view_construction(arguments, &view_fields, span, incoming);
            }
        }
        if self
            .checker
            .type_info
            .resolved_identity(span)
            .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Variant)
            && self
                .checker
                .type_info
                .expr_type(span)
                .is_some_and(|ty| self.checker.type_holds_borrow(ty))
        {
            let view_fields = self.enum_argument_view_sources(arguments)?;
            if view_fields.iter().any(|field| *field) {
                return self.enum_view_construction(arguments, &view_fields, span, incoming);
            }
        }
        if self
            .checker
            .type_info
            .resolved_identity(span)
            .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Variant)
        {
            return self.variant_construction(arguments, span, incoming);
        }
        if self.checker.type_info.resolved_collection_constructor(span).is_some() {
            return self.collection_construction(arguments, incoming);
        }
        if !matches!(callee.node, Expr::Ident(_)) {
            return Err(Unavailable {
                span,
                reason: "callee evaluation needs checked access facts".into(),
            });
        }
        if let Some(closure) = self.closure_local(callee) {
            return self.closure_call(&closure, arguments, span, incoming);
        }
        if self.uses_ordinary_iteration_access(span) {
            return self.ordinary_iteration_call(None, arguments, incoming);
        }
        self.apply_call(None, arguments, span, incoming)
    }

    /// Evaluate a checker-selected builtin collection constructor as fresh independent storage. The constructor
    /// identity, argument types and result type were fixed by expression checking; source flow only preserves
    /// written evaluation order and any dependencies inside its arguments.
    fn collection_construction(
        &mut self,
        arguments: &[CallArg],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for argument in arguments {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                CallArg::PositionalUnpack(value) | CallArg::KeywordUnpack(value) => {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "collection construction needs fixed checked arguments".into(),
                    });
                }
            };
            incoming = self.expression(value, incoming, true)?;
        }
        Ok(incoming)
    }

    /// Whether an iteration-only graph must project this missing or inferred ordinary call without claiming an
    /// explicit or native parameter contract. Ordinary calls still evaluate all source arguments below.
    pub(super) fn uses_ordinary_iteration_access(&self, span: Span) -> bool {
        self.allow_fresh_aggregate_elements
            && self
                .checker
                .type_info
                .protocols
                .explicit_input_calls
                .get(&span)
                .is_none_or(|call| call.kind == super::super::InputCallKind::Ordinary)
    }

    /// Project an ordinary call while building the iteration-only ownership profile. Every argument is still
    /// evaluated in source order and observed as a read; a retained loop item is transferred because the historical
    /// loop-item contract hands non-cloneable items on to ordinary calls. Selected explicit inputs continue through
    /// [`Self::apply_call`], where each parameter's canonical graph permission owns the decision.
    pub(super) fn ordinary_iteration_call(
        &mut self,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if let Some(receiver) = receiver {
            incoming = self.expression(receiver, incoming, false)?;
        }
        for argument in arguments {
            let value = match argument {
                CallArg::Positional(value)
                | CallArg::Named(_, value)
                | CallArg::PositionalUnpack(value)
                | CallArg::KeywordUnpack(value) => value,
            };
            incoming = self.expression(value, incoming, self.is_loop_item_value(value))?;
        }
        Ok(incoming)
    }

    /// A supported builtin collection or carrier method reads or changes its receiver place after evaluating its
    /// arguments. A collection element is fresh or Copy until values moved into a collection gain checked transfer
    /// materialization.
    pub(super) fn builtin_place_method(
        &mut self,
        receiver: &Spanned<Expr>,
        writes: bool,
        arguments: &[CallArg],
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let (place, mut incoming) = if let Some(place) = self.optional_place(receiver)? {
            let incoming = self.evaluate_place(receiver, incoming)?;
            (place, incoming)
        } else {
            self.borrowed_input_source(receiver, span, incoming)?
        };
        for argument in arguments {
            let CallArg::Positional(value) = argument else {
                return Err(Unavailable {
                    span,
                    reason: "builtin place method needs positional arguments".into(),
                });
            };
            if !self.fresh_or_copy_field(value) && !matches!(value.node, Expr::Call(..) | Expr::MethodCall(..)) {
                return Err(Unavailable {
                    span: value.span,
                    reason: "a non-Copy builtin method argument needs checked ownership transfer materialization"
                        .into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        let access = if writes { Access::Write } else { Access::Read };
        Ok(self.event(
            incoming,
            Event::Access {
                place: place.place,
                access,
                through: place.through,
            },
            span,
        ))
    }

    /// Primitive observers read their arguments through a checker-selected builtin identity. Nominal display and
    /// length hooks, native adapters and source functions with the same name require their own callable facts.
    fn observer_call(
        &mut self,
        builtin: BuiltinFnId,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if builtin == BuiltinFnId::Len && arguments.len() != 1 {
            return Err(Unavailable {
                span,
                reason: "length observer needs one checked argument".into(),
            });
        }
        let mut operands = Vec::new();
        for argument in arguments {
            let expression = match argument {
                CallArg::Positional(expression) | CallArg::Named(_, expression) => expression,
                _ => {
                    return Err(Unavailable {
                        span,
                        reason: "observer argument expansion is not projected".into(),
                    });
                }
            };
            let ty = self.checker.type_info.expr_type(expression.span);
            let supported = match builtin {
                BuiltinFnId::Len => {
                    matches!(
                        ty,
                        Some(
                            ResolvedType::Str
                                | ResolvedType::FrozenStr
                                | ResolvedType::Bytes
                                | ResolvedType::FrozenBytes
                        )
                    ) || matches!(ty, Some(ResolvedType::Generic(name, arguments))
                        if collections::from_str(name) == Some(CollectionTypeId::List) && arguments.len() == 1)
                }
                BuiltinFnId::Print => ty.is_some_and(primitive_display),
                _ => false,
            };
            if !supported {
                return Err(Unavailable {
                    span: expression.span,
                    reason: "observer may invoke an unprojected display or length operation".into(),
                });
            }
            incoming = self.expression(expression, incoming, false)?;
            let (evaluated, dependency) = self.retain_operation_operand(expression, incoming)?;
            incoming = evaluated;
            operands.extend(dependency);
        }
        self.finish_operation(span, operands, incoming)
    }

    /// Evaluate a checker-selected core builtin as a bounded operation. Builtins do not retain caller storage after
    /// completion; argument reads still keep an existing iterator or returned view live through the call. The
    /// `isinstance` target is a checked type expression rather than a runtime value and therefore has no access event.
    fn builtin_call(
        &mut self,
        builtin: BuiltinFnId,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let mut operands = Vec::new();
        for (index, argument) in arguments.iter().enumerate() {
            let expression = match argument {
                CallArg::Positional(expression) | CallArg::Named(_, expression) => expression,
                _ => {
                    return Err(Unavailable {
                        span,
                        reason: "builtin argument expansion needs checked access facts".into(),
                    });
                }
            };
            if builtin == BuiltinFnId::IsInstance && index == 1 {
                continue;
            }
            incoming = self.expression(expression, incoming, false)?;
            let (evaluated, dependency) = self.retain_operation_operand(expression, incoming)?;
            incoming = evaluated;
            operands.extend(dependency);
        }
        self.finish_operation(span, operands, incoming)
    }

    /// Require an async call to be awaited at its source position so its retained loans have a bounded lifetime.
    fn require_awaited(&self, span: Span) -> Result<(), Unavailable> {
        if self
            .checker
            .type_info
            .calls
            .body_ir_async_call_targets
            .contains_key(&span)
            && !self.awaited.contains(&span)
        {
            return Err(Unavailable {
                span,
                reason: "a call to an async callable needs `await` where it is made".into(),
            });
        }
        Ok(())
    }

    /// Apply one canonical call, evaluating its receiver before written arguments. The receiver's declared access
    /// remains live through invocation; no method-body analysis is needed to strengthen or weaken that contract.
    /// A call to an async callable makes a future that holds the call's loans until it completes; only an `await` on
    /// the call itself ends them where the call is made.
    pub(super) fn apply_call(
        &mut self,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        self.require_awaited(span)?;
        // ---- Selected declaration and supported dependency shape ----
        let call = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&span)
            .ok_or(Unavailable {
                span,
                reason: "call has no selected explicit input access facts".into(),
            })?;
        let contract = self
            .checker
            .type_info
            .selected_input_contract(call)
            .map_err(|reason| Unavailable {
                span,
                reason: reason.into(),
            })?;
        if call.owner.as_ref() != Some(&self.owner) {
            return Err(Unavailable {
                span,
                reason: "call facts belong to a different callable".into(),
            });
        }
        // A static or native associated function is written on its type, but its selected callable contract has no
        // receiver value. The checked call fact, rather than the surface qualifier's spelling, owns that distinction.
        let receiver = if call.receiver.is_none() { None } else { receiver };
        if call.receiver.is_some() != receiver.is_some() {
            return Err(Unavailable {
                span,
                reason: "selected receiver contract does not match call syntax".into(),
            });
        }
        // A direct returned view is resolved by `view_relation`; aggregate dependencies refuse here.
        let view = crate::ownership_metadata::returned_view(contract).map_err(|reason| Unavailable {
            span,
            reason: reason.into(),
        })?;
        if contract.is_some_and(|contract| {
            contract.modes.iter().any(|mode| {
                mode.component.parameter.is_some()
                    && (!mode.component.children.is_empty() || mode.mode == OwnershipMode::Reference)
            })
        }) || call
            .parameters
            .iter()
            .any(|parameter| parameter.kind != ParamKind::Normal || parameter.is_partial_preset)
        {
            return Err(Unavailable {
                span,
                reason: "call needs returned or packed-argument dependency facts".into(),
            });
        }
        let result = self.checker.type_info.expr_type(span).ok_or(Unavailable {
            span,
            reason: "missing call result type".into(),
        })?;
        let native_dependencies = if call.kind == super::super::InputCallKind::Native {
            self.checker
                .native_result_dependencies(span)
                .map_err(|reason| Unavailable { span, reason })?
        } else {
            None
        };
        let native_independent_result =
            call.kind == super::super::InputCallKind::Native && self.checker.native_result_is_independent(span);
        let owned_independent_result = crate::ownership_metadata::owned_result(contract)
            .map_err(|reason| Unavailable {
                span,
                reason: reason.into(),
            })?
            .is_some()
            && contract.is_some_and(|contract| contract.result_source.is_none());
        let owned_dependent_result = crate::ownership_metadata::owned_result_source(contract)
            .map_err(|reason| Unavailable {
                span,
                reason: reason.into(),
            })?
            .is_some();
        let clone_result = self.checked_clone_call(span);
        // A native call that lends nothing returns a value that cannot keep a borrow of the caller's storage.
        let lends_nothing = call.kind == super::super::InputCallKind::Native
            && (call.receiver.is_none() || self.checker.type_info.native_value_receiver(span))
            && contract.is_some_and(|contract| {
                contract
                    .modes
                    .iter()
                    .all(|mode| !matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut))
            });
        let result_requirement = (call.kind == super::super::InputCallKind::Explicit
            && contract.is_some_and(|contract| {
                contract.result_source.is_none() && contract.modes.iter().all(|mode| mode.component.parameter.is_some())
            })
            && conditional_result(result))
        .then(|| result.clone());
        if view.is_none()
            && native_dependencies.is_none()
            && !native_independent_result
            && !owned_independent_result
            && !owned_dependent_result
            && !clone_result
            && !plain_result(result)
            && result_requirement.is_none()
            && !lends_nothing
            && !(call.kind == super::super::InputCallKind::Ordinary && call.parameters.is_empty())
        {
            return Err(Unavailable {
                span,
                reason: "call result needs retained dependency facts".into(),
            });
        }
        let mut paired = TypeChecker::arguments_with_parameter_slots(&call.parameters, arguments)
            .into_iter()
            .enumerate()
            .collect::<Vec<_>>();
        if paired.len() != arguments.len() {
            return Err(Unavailable {
                span,
                reason: "unpacked call requires checked argument expansion".into(),
            });
        }
        // ---- Existing call evaluation policy, separate from written provenance ----
        // `call` admits only unqualified fixed calls; the language reference defines declaration-order evaluation
        // for that surface. A method retains written order. Do not change this when adding ownership requests.
        if receiver.is_none() && call.kind != super::super::InputCallKind::Indirect {
            paired.sort_by_key(|(_, (_, slot))| *slot);
        }
        let mut loans = Vec::new();
        let mut invocation = Invocation {
            callee: call.callee.clone(),
            result_requirement,
            receiver: None,
            receiver_owned: false,
            receiver_source: None,
            receiver_dependencies: Vec::new(),
            arguments: Vec::with_capacity(paired.len()),
            receiver_evaluation: None,
            completion_node: 0,
            final_accesses: Vec::new(),
        };
        if let Some(receiver) = receiver {
            let start_node = self.graph.nodes.len();
            incoming = self.event(incoming, Event::Nothing, receiver.span);
            if self.checker.type_info.native_value_receiver(span) {
                let source = self.optional_place(receiver)?.map(|path| path.place);
                if let Some(source) = &source {
                    self.constrained.insert(source.root.clone());
                }
                let dependencies = if let Some(relation) = self.native_result_relation(receiver)? {
                    let (evaluated, dependencies) = self.evaluate_native_result(receiver, &relation, incoming)?;
                    incoming = evaluated;
                    dependencies
                } else {
                    let dependencies = source
                        .as_ref()
                        .and_then(|source| match &source.root {
                            Root::Binding(identity) => self.graph.dependent_locals.get(identity).cloned(),
                            Root::Temporary { .. } => None,
                        })
                        .unwrap_or_default();
                    incoming = self.expression(receiver, incoming, true)?;
                    dependencies
                };
                let source_node = source.as_ref().map(|_| self.graph.nodes.len() - 1);
                let end_node = self.graph.nodes.len();
                incoming = self.event(incoming, Event::Nothing, receiver.span);
                invocation.receiver_owned = true;
                invocation.receiver_source = source;
                invocation.receiver_dependencies = dependencies.clone();
                loans.extend(dependencies);
                invocation.receiver_evaluation = Some(InputEvaluation {
                    start_node,
                    end_node,
                    source_node,
                });
            } else {
                // A chained direct view continues on the place its inner call substituted. An aggregate temporary
                // owns distinct storage while retaining the loans carried by that value, so the outer call borrows
                // the temporary and keeps every carried dependency live through invocation.
                let aggregate = self
                    .call_relation(receiver)?
                    .filter(|relation| !relation.component.is_empty() && relation.carried.is_none());
                let source = if let Some(relation) = aggregate {
                    let (evaluated, dependencies) = self.evaluate_aggregate_view_call(receiver, relation, incoming)?;
                    incoming = evaluated;
                    let root = Root::Temporary {
                        owner: self.owner.clone(),
                        expression: receiver.span,
                        invocation: span,
                    };
                    self.graph.permissions.insert(
                        root.clone(),
                        Permission {
                            exclusive: true,
                            writable: true,
                            transferable: true,
                        },
                    );
                    invocation.receiver_dependencies = dependencies.clone();
                    loans.extend(dependencies);
                    Path {
                        place: Place {
                            root,
                            projection: Vec::new(),
                        },
                        through: None,
                    }
                } else {
                    let (source, evaluated) = self.borrowed_input_source(receiver, span, incoming)?;
                    incoming = evaluated;
                    source
                };
                let mutable = call.receiver == Some(Receiver::Mutable);
                let loan = self.call_loan(source, mutable, mutable, receiver.span);
                let source_node = self.graph.nodes.len();
                incoming = self.event(incoming, Event::Borrow(loan), receiver.span);
                let end_node = self.graph.nodes.len();
                incoming = self.event(incoming, Event::Nothing, receiver.span);
                loans.push(loan);
                invocation.receiver = Some(loan);
                invocation.receiver_evaluation = Some(InputEvaluation {
                    start_node,
                    end_node,
                    source_node: Some(source_node),
                });
            }
        }
        for (written_position, (expression, slot)) in paired {
            let slot = slot.ok_or(Unavailable {
                span: expression.span,
                reason: "missing selected argument slot".into(),
            })?;
            let mode = contract
                .and_then(|contract| {
                    contract
                        .modes
                        .iter()
                        .find(|mode| mode.component.parameter == Some(slot))
                })
                .map(|mode| mode.mode);
            // A closure local passed on is read where it is passed, through the borrow it holds.
            if let Some(closure) = self.closure_local(expression) {
                let (outgoing, argument) =
                    self.closure_argument(&closure, &call.callee, expression, (slot, written_position), incoming)?;
                incoming = outgoing;
                invocation.arguments.push(argument);
                continue;
            }
            let source = self.optional_place(expression)?.map(|path| path.place);
            let start_node = self.graph.nodes.len();
            incoming = self.event(incoming, Event::Nothing, expression.span);
            let (outgoing, loan) = self.call_argument(
                expression,
                &call.parameters[slot],
                mode,
                call.kind == super::super::InputCallKind::Ordinary,
                matches!(
                    call.kind,
                    super::super::InputCallKind::Explicit | super::super::InputCallKind::Native
                ) || (call.kind == super::super::InputCallKind::Ordinary
                    && self
                        .checker
                        .type_info
                        .constructor_field_binding(expression.span)
                        .is_some()),
                span,
                incoming,
            )?;
            incoming = outgoing;
            let source_node = if source.is_some() || loan.is_some() {
                Some(*incoming.first().filter(|_| incoming.len() == 1).ok_or(Unavailable {
                    span: expression.span,
                    reason: "argument source requires one final evaluated event".into(),
                })?)
            } else {
                None
            };
            let end_node = self.graph.nodes.len();
            incoming = self.event(incoming, Event::Nothing, expression.span);
            if let Some(loan) = loan {
                loans.push(loan);
            }
            invocation.arguments.push(InvocationArgument {
                span: expression.span,
                slot,
                written_position,
                access: match mode {
                    Some(OwnershipMode::Borrowed) => IncanInputAccess::Shared,
                    Some(OwnershipMode::BorrowedMut) => IncanInputAccess::Exclusive,
                    Some(OwnershipMode::Owned) => IncanInputAccess::Owned,
                    None => IncanInputAccess::Inferred,
                    Some(OwnershipMode::Reference) => {
                        return Err(Unavailable {
                            span,
                            reason: "returned input reference is not an invocation mode".into(),
                        });
                    }
                },
                loan,
                source: loan.map(|loan| self.graph.loans[loan.0].source.clone()).or(source),
                evaluation: InputEvaluation {
                    start_node,
                    end_node,
                    source_node,
                },
            });
        }
        invocation.completion_node = self.graph.nodes.len();
        incoming = self.event(incoming, Event::Nothing, span);
        // ---- Invocation retains every loan, independently of callee body use ----
        for id in loans {
            let loan = &self.graph.loans[id.0];
            invocation.final_accesses.push(self.graph.nodes.len());
            incoming = self.event(
                incoming,
                Event::Access {
                    place: loan.source.clone(),
                    access: Access::Read,
                    through: Some(id),
                },
                span,
            );
        }
        if self.graph.invocations.insert(span, invocation).is_some() {
            return Err(Unavailable {
                span,
                reason: "invocation has more than one checked argument binding".into(),
            });
        }
        Ok(incoming)
    }

    /// Return whether semantic method resolution selected the standard `Clone.clone` slot for this call. A source
    /// method that merely uses the spelling `clone` does not gain independent-result semantics.
    pub(super) fn checked_clone_call(&self, span: Span) -> bool {
        let builtin = crate::symbols::canonical_builtin_member_identity(
            core_traits::as_str(TraitId::Clone),
            trait_bounds::rust::CLONE_METHOD,
        );
        if matches!(
            self.checker.type_info.resolved_builtin_method_call(span),
            Some(
                BuiltinMethodId::Clone(CloneMethodId::Clone)
                    | BuiltinMethodId::List(ListMethodId::Clone)
                    | BuiltinMethodId::Option(OptionMethodId::Clone)
            )
        ) || self.checker.type_info.traits.inferred_method_identities.get(&span) == Some(&builtin)
        {
            return true;
        }
        self.checker.type_info.resolved_method_call(span).is_some_and(|call| {
            call.method == trait_bounds::rust::CLONE_METHOD
                && matches!(
                    &call.dispatch,
                    crate::typechecker::ResolvedMethodDispatch::Trait { trait_name, .. }
                        if core_traits::from_str(trait_name) == Some(TraitId::Clone)
                )
        })
    }

    /// Evaluate one argument according to its selected slot. A borrowed place is evaluated once before taking its
    /// loan; an explicit ownership request constrains even an otherwise ordinary source binding. Unresolved
    /// duckborrow or ordinary mutable adapters refuse rather than cloning or guessing an access mode.
    fn call_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        mode: Option<OwnershipMode>,
        ordinary_callee: bool,
        accepts_fresh_inferred: bool,
        invocation: Span,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Option<LoanId>), Unavailable> {
        match mode {
            Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) => {
                let (source, incoming) = self.borrowed_input_source(expression, invocation, incoming)?;
                let loan = self.call_loan(
                    source,
                    mode == Some(OwnershipMode::BorrowedMut),
                    parameter.is_mut,
                    expression.span,
                );
                Ok((self.event(incoming, Event::Borrow(loan), expression.span), Some(loan)))
            }
            Some(OwnershipMode::Owned) => {
                if let Some(source) = self.optional_place(expression)? {
                    self.constrained.insert(source.place.root);
                }
                self.expression(expression, incoming, true)
                    .map(|outgoing| (outgoing, None))
            }
            None if self.copy_value(&parameter.ty) && !parameter.is_mut => self
                .expression(expression, incoming, false)
                .map(|outgoing| (outgoing, None)),
            // A fresh closed value has no caller-owned source to conflict with. Explicit boundaries and an ordinary
            // model constructor argument retain that checked independence without inventing a copy or loan.
            None if accepts_fresh_inferred
                && !parameter.is_mut
                && self.optional_place(expression)?.is_none()
                && (self
                    .checker
                    .type_info
                    .expr_type(expression.span)
                    .is_some_and(plain_result)
                    || (ordinary_callee
                        && self
                            .checker
                            .type_info
                            .constructor_field_binding(expression.span)
                            .is_some())) =>
            {
                self.expression(expression, incoming, true)
                    .map(|outgoing| (outgoing, None))
            }
            // A whole local may be observed by an ordinary helper while its body retains later storage. Parameters,
            // projections and borrowed aliases still need an explicit access fact rather than inferred widening.
            None if ordinary_callee
                && !parameter.is_mut
                && self.optional_place(expression)?.is_some_and(|path| {
                    path.through.is_none()
                        && path.place.projection.is_empty()
                        && matches!(
                            path.place.root,
                            Root::Binding(ref identity)
                                if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
                        )
                }) =>
            {
                self.expression(expression, incoming, false)
                    .map(|outgoing| (outgoing, None))
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "ordinary argument requires inferred access or mutable binding facts".into(),
            }),
        }
    }

    /// A fresh scalar or owned byte-string expression, or a list literal of Copy scalars, may be borrowed for this
    /// call. Existing places keep their original permissions even through parentheses; evaluating a temporary
    /// preserves nested moves and loans.
    fn borrowed_input_source(
        &mut self,
        expression: &Spanned<Expr>,
        invocation: Span,
        incoming: Vec<usize>,
    ) -> Result<(Path, Vec<usize>), Unavailable> {
        if let Some(view) = self.view_source(expression)? {
            let (outgoing, source) = self.evaluate_view_source(expression, view, incoming)?;
            return Ok((source, outgoing));
        }
        if let Some(relation) = self.native_result_relation(expression)?
            && relation.sources.len() == 1
        {
            let (outgoing, loans) = self.evaluate_native_result(expression, &relation, incoming)?;
            let loan = loans
                .first()
                .and_then(|loan| self.graph.loans.get(loan.0))
                .ok_or(Unavailable {
                    span: expression.span,
                    reason: "native chained borrow has no retained source loan".into(),
                })?;
            return Ok((
                Path {
                    place: loan.source.clone(),
                    through: loans.first().copied(),
                },
                outgoing,
            ));
        }
        if let Some(source) = self.optional_place(expression)? {
            return Ok((source, self.evaluate_place(expression, incoming)?));
        }
        let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
            span: expression.span,
            reason: "temporary argument has no checked type".into(),
        })?;
        // A list literal's own evaluation admits only Copy scalar elements, so it retains no dependency either. A
        // complete model or class constructor likewise produces fresh storage after its checked fields evaluate.
        let fresh_constructor = self
            .checker
            .type_info
            .constructor_field_binding(expression.span)
            .is_some();
        let independent_source_result = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&expression.span)
            .is_some_and(|call| {
                (call.kind == super::super::InputCallKind::Ordinary && call.parameters.is_empty())
                    || (call.kind == super::super::InputCallKind::Explicit
                        && self
                            .checker
                            .type_info
                            .selected_input_contract(call)
                            .ok()
                            .and_then(|contract| crate::ownership_metadata::owned_result(contract).ok())
                            .flatten()
                            .is_some()
                        && self
                            .checker
                            .type_info
                            .selected_input_contract(call)
                            .is_ok_and(|contract| contract.is_some_and(|contract| contract.result_source.is_none())))
            });
        let independent_native_result = self.checker.native_result_is_independent(expression.span);
        if !plain_result(ty)
            && !matches!(expression.node, Expr::List(_))
            && !fresh_constructor
            && !independent_source_result
            && !independent_native_result
        {
            return Err(Unavailable {
                span: expression.span,
                reason: "temporary argument needs retained dependency facts".into(),
            });
        }
        let outgoing = self.expression(expression, incoming, true)?;
        let root = Root::Temporary {
            owner: self.owner.clone(),
            expression: expression.span,
            invocation,
        };
        self.graph.permissions.insert(
            root.clone(),
            Permission {
                exclusive: true,
                writable: true,
                transferable: true,
            },
        );
        Ok((
            Path {
                place: Place {
                    root,
                    projection: Vec::new(),
                },
                through: None,
            },
            outgoing,
        ))
    }

    /// A borrowed argument derives its permissions from the evaluated source. Exclusive access and permission to
    /// write remain separate, and reborrowing preserves the parent's dependency rather than upgrading the owner.
    pub(super) fn call_loan(&mut self, source: Path, exclusive: bool, writable: bool, span: Span) -> LoanId {
        let id = LoanId(self.graph.loans.len());
        self.graph.loans.push(Loan {
            source: source.place,
            kind: if exclusive {
                LoanKind::Exclusive
            } else {
                LoanKind::Shared
            },
            writable: exclusive && writable,
            parent: source.through,
            span,
        });
        id
    }

    /// The place view an expression produces: a selected call returning a direct view, or `?` taking a carried view
    /// out of its `Result` or `Option` success payload. A call returning a carried view is itself a carrier value, not
    /// a place. `?` on a call whose error carries the view would pass that view on out of this body, and `?` in a body
    /// whose own error carries its view would return another error; neither executes yet.
    pub(super) fn view_relation(&self, expression: &Spanned<Expr>) -> Result<Option<ViewRelation>, Unavailable> {
        match &expression.node {
            Expr::Paren(inner) => self.view_relation(inner),
            Expr::Try(inner) => match self.call_relation(inner)? {
                Some(relation) if relation.carried == Some(1) => Err(Unavailable {
                    span: expression.span,
                    reason: "`?` passing on an error that carries a view".into(),
                }),
                _ if self.view_carried == Some(1) => Err(super::error_view_try(expression.span)),
                Some(relation) if relation.carried == Some(0) => Ok(Some(ViewRelation {
                    carried: None,
                    ..relation
                })),
                _ => Ok(None),
            },
            _ => Ok(self
                .call_relation(expression)?
                .filter(|relation| relation.component.is_empty())),
        }
    }

    /// The place view an expression produces, from a call as `view_relation` selects it or from `?` on a local still
    /// holding a carried call. `?` on such a local takes the view out as `?` on the call does.
    pub(super) fn view_source(&self, expression: &Spanned<Expr>) -> Result<Option<ViewSource>, Unavailable> {
        if let Some(relation) = self.view_relation(expression)? {
            return Ok(Some(ViewSource::Call(relation)));
        }
        match &expression.node {
            Expr::Paren(inner) => self.view_source(inner),
            Expr::Try(inner) => {
                let Some((identity, payload)) = self.bound_carrier(inner) else {
                    return Ok(None);
                };
                if payload != 0 {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "`?` passing on an error that carries a view".into(),
                    });
                }
                if self.view_carried == Some(1) {
                    return Err(super::error_view_try(expression.span));
                }
                Ok(Some(ViewSource::Carrier(identity)))
            }
            _ => Ok(None),
        }
    }

    /// Evaluate the place view `view_source` selected, then borrow the caller's place under its result loan.
    pub(super) fn evaluate_view_source(
        &mut self,
        expression: &Spanned<Expr>,
        source: ViewSource,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        match (source, &expression.node) {
            (ViewSource::Call(relation), _) => self.evaluate_view_call(expression, relation, incoming),
            (source @ ViewSource::Carrier(_), Expr::Paren(inner)) => self.evaluate_view_source(inner, source, incoming),
            (ViewSource::Carrier(identity), Expr::Try(inner)) => self.take_bound_view(inner, identity, incoming),
            (ViewSource::Carrier(_), _) => Err(Unavailable {
                span: expression.span,
                reason: "bound carried view needs `?` or a match to take the view out".into(),
            }),
        }
    }

    /// The returned view relation of a selected call itself, direct or carried, read from its callee's contract
    /// without its body. Await and grouping preserve the selected call's relationship.
    pub(super) fn call_relation(&self, expression: &Spanned<Expr>) -> Result<Option<ViewRelation>, Unavailable> {
        let expression = super::super::builtins::view_call(expression);
        match &expression.node {
            Expr::Call(..) | Expr::MethodCall(..) => {}
            _ => return Ok(None),
        }
        let span = expression.span;
        let Some(call) = self.checker.type_info.protocols.explicit_input_calls.get(&span) else {
            return Ok(None);
        };
        let refusal = |reason: &'static str| Unavailable {
            span,
            reason: reason.into(),
        };
        let contract = self.checker.type_info.selected_input_contract(call).map_err(refusal)?;
        Ok(crate::ownership_metadata::returned_view(contract)
            .map_err(refusal)?
            .map(|view| ViewRelation {
                exclusive: view.access == IncanInputAccess::Exclusive,
                slot: view.source.slot.clone(),
                fields: view.source.fields.clone(),
                component: view.component.to_vec(),
                carried: view.carried,
            }))
    }

    /// The retained dependency declared by a selected top-level `Owned[T] from source` result. Its source input
    /// determines whether the result keeps shared or exclusive access; the owned result itself is ordinary value
    /// storage, not an alias of that source.
    pub(super) fn owned_relation(&self, expression: &Spanned<Expr>) -> Result<Option<ViewRelation>, Unavailable> {
        let expression = super::super::builtins::view_call(expression);
        if !matches!(expression.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return Ok(None);
        }
        let span = expression.span;
        let Some(call) = self.checker.type_info.protocols.explicit_input_calls.get(&span) else {
            return Ok(None);
        };
        let refusal = |reason: &'static str| Unavailable {
            span,
            reason: reason.into(),
        };
        let contract = self.checker.type_info.selected_input_contract(call).map_err(refusal)?;
        let Some((_, source)) = crate::ownership_metadata::owned_result_source(contract).map_err(refusal)? else {
            return Ok(None);
        };
        let exclusive = match source.slot {
            SourceSlot::Receiver => call.receiver == Some(Receiver::Mutable),
            SourceSlot::Parameter(slot) => contract
                .and_then(|contract| {
                    contract
                        .modes
                        .iter()
                        .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty())
                })
                .is_some_and(|mode| mode.mode == OwnershipMode::BorrowedMut),
        };
        Ok(Some(ViewRelation {
            exclusive,
            slot: source.slot.clone(),
            fields: source.fields.clone(),
            component: Vec::new(),
            carried: None,
        }))
    }

    /// The exact retained owners of a native call result, including a builtin carrier payload when one holds the
    /// borrowed value. Static results return an empty source set and need no caller-owned loan.
    pub(super) fn native_result_relation(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<super::super::native::NativeResultDependencies>, Unavailable> {
        let expression = super::super::builtins::view_call(expression);
        if !matches!(expression.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return Ok(None);
        }
        let is_native = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&expression.span)
            .is_some_and(|call| call.kind == super::super::InputCallKind::Native);
        if !is_native {
            return Ok(None);
        }
        self.checker
            .native_result_dependencies(expression.span)
            .map_err(|reason| Unavailable {
                span: expression.span,
                reason,
            })
    }

    /// Evaluate a native call once, then retain one result loan for every rustc-proven caller-owned input. The
    /// invocation's temporary loans end with the call; each result loan starts from the same admitted source place.
    pub(super) fn evaluate_native_result(
        &mut self,
        expression: &Spanned<Expr>,
        relation: &super::super::native::NativeResultDependencies,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Vec<super::LoanId>), Unavailable> {
        let expression = super::super::builtins::view_call(expression);
        let span = expression.span;
        let incoming = match &expression.node {
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.apply_call(Some(receiver), arguments, span, incoming)?
            }
            Expr::Call(callee, _, arguments) if matches!(callee.node, Expr::Ident(_)) => {
                self.apply_call(None, arguments, span, incoming)?
            }
            _ => {
                return Err(Unavailable {
                    span,
                    reason: "native borrowed result needs a checked call".into(),
                });
            }
        };
        let invocation = self.graph.invocations.get(&span).cloned().ok_or(Unavailable {
            span,
            reason: "native borrowed result has no checked invocation".into(),
        })?;
        let mut incoming = incoming;
        let mut retained = Vec::with_capacity(relation.sources.len());
        for source in &relation.sources {
            let invocation_loans = match source {
                SourceSlot::Receiver => invocation
                    .receiver
                    .into_iter()
                    .chain(invocation.receiver_dependencies.iter().copied())
                    .collect::<Vec<_>>(),
                SourceSlot::Parameter(slot) => invocation
                    .arguments
                    .iter()
                    .find(|argument| argument.slot == *slot)
                    .and_then(|argument| argument.loan)
                    .into_iter()
                    .collect(),
            };
            if invocation_loans.is_empty() {
                return Err(Unavailable {
                    span,
                    reason: "native borrowed result source is not an admitted borrowed input".into(),
                });
            }
            for invocation_loan in invocation_loans {
                let base = self.graph.loans.get(invocation_loan.0).cloned().ok_or(Unavailable {
                    span,
                    reason: "native borrowed result source is not an admitted borrowed input".into(),
                })?;
                if !matches!(base.source.root, Root::Binding(_)) {
                    return Err(Unavailable {
                        span,
                        reason: format!("native call `{}` returns a borrow of temporary storage", relation.call),
                    });
                }
                let exclusive = base.kind == super::LoanKind::Exclusive;
                let loan = self.call_loan(
                    Path {
                        place: base.source,
                        through: base.parent,
                    },
                    exclusive,
                    base.writable,
                    span,
                );
                incoming = self.event(incoming, Event::Borrow(loan), span);
                retained.push(loan);
            }
        }
        Ok((incoming, retained))
    }

    /// Evaluate a call that returns a view, then borrow the substituted place: the invocation's receiver or
    /// argument source with the declared fields. The invocation's own loans cover the whole input during the call
    /// and end with it; afterwards only the narrower result loan stays live, under the source's parent loan.
    pub(super) fn evaluate_view_call(
        &mut self,
        expression: &Spanned<Expr>,
        relation: ViewRelation,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        let span = expression.span;
        let aggregate = !relation.component.is_empty();
        let incoming = match &expression.node {
            // `?` continues on the carried view's place; its error path leaves the body carrying no loan.
            Expr::Paren(inner) | Expr::Try(inner) => return self.evaluate_view_call(inner, relation, incoming),
            Expr::Surface(surface) if super::super::builtins::awaited(surface).is_some() => {
                let awaited = super::super::builtins::awaited(surface).ok_or(Unavailable {
                    span,
                    reason: "await has no returned-view expression".into(),
                })?;
                let call = super::super::builtins::view_call(awaited);
                self.awaited.insert(call.span);
                return self.evaluate_view_call(awaited, relation, incoming);
            }
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.apply_call(Some(receiver), arguments, span, incoming)?
            }
            Expr::Call(callee, _, arguments) if matches!(callee.node, Expr::Ident(_)) => {
                self.apply_call(None, arguments, span, incoming)?
            }
            _ => {
                return Err(Unavailable {
                    span,
                    reason: "returned view call needs a checked callee".into(),
                });
            }
        };
        let invocation = self.graph.invocations.get(&span).ok_or(Unavailable {
            span,
            reason: "returned view call has no checked invocation".into(),
        })?;
        let base = match relation.slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .cloned()
        .ok_or(Unavailable {
            span,
            reason: "returned view source was not borrowed by its invocation".into(),
        })?;
        if matches!(base.source.root, Root::Temporary { .. }) && !aggregate {
            return Err(Unavailable {
                span,
                reason: "a returned view of a temporary cannot outlive its invocation".into(),
            });
        }
        let mut place = base.source;
        place
            .projection
            .extend(relation.fields.into_iter().map(Projection::Field));
        let loan = self.call_loan(
            Path {
                place: place.clone(),
                through: base.parent,
            },
            relation.exclusive,
            relation.exclusive,
            span,
        );
        let incoming = self.event(incoming, Event::Borrow(loan), span);
        Ok((
            incoming,
            Path {
                place,
                through: Some(loan),
            },
        ))
    }

    /// Evaluate an aggregate returned view and retain both its immediate source storage and every dependency stored
    /// by that source value. A cursor borrowing a selection therefore keeps the selection local alive and carries
    /// the selection's graph loan forward; neither dependency may be reconstructed from the nominal field shape.
    pub(super) fn evaluate_aggregate_view_call(
        &mut self,
        expression: &Spanned<Expr>,
        relation: ViewRelation,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Vec<LoanId>), Unavailable> {
        let source_slot = relation.slot.clone();
        let (mut incoming, path) = self.evaluate_view_call(expression, relation, incoming)?;
        let direct = path.through.ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view has no retained source loan".into(),
        })?;
        let call = super::super::builtins::view_call(expression);
        let invocation = self.graph.invocations.get(&call.span).ok_or(Unavailable {
            span: call.span,
            reason: "aggregate view call has no checked invocation".into(),
        })?;
        let source = match &source_slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == *slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .ok_or(Unavailable {
            span: call.span,
            reason: "aggregate view source was not borrowed by its invocation".into(),
        })?;
        let dependencies = match &source.source.root {
            Root::Binding(root) => self.graph.dependent_locals.get(root).cloned().unwrap_or_default(),
            Root::Temporary { .. } if source_slot == SourceSlot::Receiver => invocation.receiver_dependencies.clone(),
            Root::Temporary { .. } => Vec::new(),
        };
        let mut loans = vec![direct];
        for dependency in dependencies {
            let (next, _, loan) = self.carry_dependency(dependency, call.span, incoming)?;
            incoming = next;
            loans.push(loan);
        }
        Ok((incoming, loans))
    }

    /// Recover the substituted source place of an already evaluated view call without creating another result loan.
    /// Aggregate-return validation uses this after ordinary value evaluation so its graph matches the stored value's
    /// execution while still proving the declaration's exact `from` relationship.
    pub(super) fn evaluated_view_call_place(
        &self,
        expression: &Spanned<Expr>,
        relation: &ViewRelation,
    ) -> Result<Place, Unavailable> {
        let expression = super::super::builtins::view_call(expression);
        let invocation = self.graph.invocations.get(&expression.span).ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view call has no checked invocation".into(),
        })?;
        let base = match relation.slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view source was not borrowed by its invocation".into(),
        })?;
        let mut place = base.source.clone();
        place
            .projection
            .extend(relation.fields.iter().cloned().map(Projection::Field));
        Ok(place)
    }
}
