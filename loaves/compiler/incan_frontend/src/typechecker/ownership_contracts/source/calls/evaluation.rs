//! The steps of one canonical call's evaluation: admitting its result shape, evaluating a by-value or borrowed
//! receiver and each written argument, and completing the invocation with every loan it holds.

use super::super::super::flow::{InputEvaluation, Invocation, InvocationArgument, ValueUseKind};
use super::super::super::{InputCall, InputCallKind};
use super::super::call_results::{conditional_result, plain_result};
use super::super::{
    Access, Builder, Event, LoanId, OwnershipMode, Path, Permission, Place, ResolvedType, Root, Unavailable,
};
use super::two_phase::{Reservation, ReservedSlot};
use crate::ast::{CallArg, Expr, ParamKind, Receiver, Span, Spanned};
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_semantics_core::IncanInputAccess;

impl Builder<'_> {
    /// Activate the call's reserved loans, record its completion, and keep every loan the invocation holds live through
    /// it with a final read of each source, independently of how the callee body uses it. One invocation per call.
    pub(super) fn complete_invocation(
        &mut self,
        span: Span,
        mut invocation: Invocation,
        mut loans: Vec<LoanId>,
        reservations: &[Reservation],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        incoming = self.activate_reservations(reservations, &mut invocation, &mut loans, incoming)?;
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

    /// Refuse a selected callable whose contract names a nested or returned-reference parameter component, or whose
    /// parameters include packed or partially preset slots: such calls need dependency facts this profile lacks.
    pub(super) fn require_supported_call_shape(
        call: &InputCall,
        contract: Option<&crate::ownership_metadata::CheckedOwnershipContract>,
        span: Span,
    ) -> Result<(), Unavailable> {
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
        Ok(())
    }

    /// Classify the call's result: a returned or carried view, a native or owned dependency, an independent result
    /// the checker proved, or a plain value. Any other result needs retained dependency facts and is refused.
    pub(super) fn call_result_shape(
        &mut self,
        call: &InputCall,
        contract: Option<&crate::ownership_metadata::CheckedOwnershipContract>,
        has_view: bool,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        span: Span,
    ) -> Result<CallResultShape, Unavailable> {
        let result = self.checker.type_info.expr_type(span).ok_or(Unavailable {
            span,
            reason: "missing call result type".into(),
        })?;
        let native_dependencies = if call.kind == InputCallKind::Native || self.checker.ordinary_borrowed_result(span) {
            self.checker
                .native_result_dependencies(span)
                .map_err(|reason| Unavailable { span, reason })?
        } else {
            None
        };
        let native_independent_result =
            call.kind == InputCallKind::Native && self.checker.native_result_is_independent(span);
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
        let path_join_independent_result =
            self.checked_standard_path_join_result(receiver, arguments, span, call, result);
        let selected_result_map_err = call.callee
            == crate::symbols::canonical_builtin_member_identity("Result", "map_err")
            && call.receiver == Some(Receiver::Immutable)
            && self.checker.type_info.resolved_builtin_method_call(span)
                == Some(BuiltinMethodId::Result(
                    incan_lang::lang::surface::result_methods::ResultMethodId::MapErr,
                ));
        // A native call that lends nothing returns a value that cannot keep a borrow of the caller's storage.
        let lends_nothing = call.kind == InputCallKind::Native
            && (call.receiver.is_none() || self.checker.type_info.native_value_receiver(span))
            && contract.is_some_and(|contract| {
                contract
                    .modes
                    .iter()
                    .all(|mode| !matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut))
            });
        let result_requirement = (call.kind == InputCallKind::Explicit
            && contract.is_some_and(|contract| {
                contract.result_source.is_none() && contract.modes.iter().all(|mode| mode.component.parameter.is_some())
            })
            && conditional_result(result))
        .then(|| result.clone());
        let closed_carrier_result = self.consumed_carrier_calls.contains(&span)
            && call.kind == InputCallKind::Ordinary
            && !has_view
            && native_dependencies.is_none()
            && !native_independent_result
            && !owned_independent_result
            && !owned_dependent_result
            && !clone_result
            && !path_join_independent_result
            && result_requirement.is_none()
            && (result.is_result() || result.is_option())
            && !self.checker.type_holds_borrow(result)
            && !self.checker.type_holds_native_loan(result);
        if !has_view
            && native_dependencies.is_none()
            && !native_independent_result
            && !owned_independent_result
            && !owned_dependent_result
            && !clone_result
            && !path_join_independent_result
            && !plain_result(result)
            && result_requirement.is_none()
            && !closed_carrier_result
            && !lends_nothing
            && !self.independent_call_results.contains(&span)
            && !selected_result_map_err
            && !(call.kind == InputCallKind::Ordinary && call.parameters.is_empty())
        {
            return Err(Unavailable {
                span,
                reason: "call result needs retained dependency facts".into(),
            });
        }
        Ok(CallResultShape {
            result_requirement,
            selected_result_map_err,
            path_join_independent_result,
        })
    }

    /// Classify the receiver of a compiler-selected `Result.map_err` as the call receiver it feeds.
    pub(super) fn map_err_receiver_use(
        &mut self,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        span: Span,
    ) -> Result<super::super::value_uses::ValueUsePlan, Unavailable> {
        let receiver = receiver.ok_or(Unavailable {
            span,
            reason: "call has no selected explicit input access facts".into(),
        })?;
        self.classify_value_use(
            receiver,
            super::super::value_uses::ValueUseContext::CallReceiver {
                invocation: span,
                arguments,
            },
        )?
        .ok_or(Unavailable {
            span,
            reason: "call has no selected explicit input access facts".into(),
        })
    }

    /// Evaluate a receiver the native callee takes by value: it is consumed, and the result keeps the loans its value
    /// depended on.
    pub(super) fn evaluate_native_value_receiver(
        &mut self,
        receiver: &Spanned<Expr>,
        span: Span,
        start_node: usize,
        mut incoming: Vec<usize>,
        invocation: &mut Invocation,
        loans: &mut Vec<LoanId>,
    ) -> Result<Vec<usize>, Unavailable> {
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
        Ok(incoming)
    }

    /// Evaluate a borrowed receiver: a chained view, an aggregate temporary, a classified `map_err` receiver or a
    /// stored place, lent to the call under one receiver loan. A `mut self` receiver outside the whole-body proof is
    /// reserved as a shared loan until the call.
    pub(super) fn evaluate_borrowed_receiver(
        &mut self,
        call: &InputCall,
        receiver: &Spanned<Expr>,
        span: Span,
        start_node: usize,
        receiver_value_use: Option<super::super::value_uses::ValueUsePlan>,
        mut incoming: Vec<usize>,
        (invocation, loans, reservations): CallTracking<'_>,
    ) -> Result<Vec<usize>, Unavailable> {
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
            if let Some(plan) = receiver_value_use {
                incoming = self.apply_value_use(receiver, plan, incoming)?;
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
            }
        };
        // A `mut self` receiver is Rust's two-phase autoref: outside the whole-body proof it is reserved, as a
        // shared loan, until the call.
        let mutable = call.receiver == Some(Receiver::Mutable);
        let reserved = mutable && self.checker.native_borrow_pass;
        let loan = self.call_loan(source, mutable && !reserved, mutable && !reserved, receiver.span);
        if reserved {
            reservations.push(Reservation {
                slot: ReservedSlot::Receiver,
                loan,
                writable: true,
                span: receiver.span,
            });
        }
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
        Ok(incoming)
    }

    /// Evaluate one written argument in its selected slot and record its access, loan, source and evaluation nodes on
    /// the invocation. An exclusive reborrow of a `&mut` binding is reserved as a shared loan until the call.
    pub(super) fn evaluate_call_argument(
        &mut self,
        site: &CallSite<'_>,
        written_position: usize,
        expression: &Spanned<Expr>,
        slot: Option<usize>,
        mut incoming: Vec<usize>,
        (invocation, loans, reservations): CallTracking<'_>,
    ) -> Result<Vec<usize>, Unavailable> {
        let CallSite {
            call,
            contract,
            span,
            selected_result_map_err,
            path_join_independent_result,
        } = *site;
        let slot = slot.ok_or(Unavailable {
            span: expression.span,
            reason: "missing selected argument slot".into(),
        })?;
        let access = self
            .checker
            .type_info
            .selected_input_access(call, contract, slot)
            .map_err(|reason| Unavailable {
                span: expression.span,
                reason: reason.into(),
            })?;
        // A closure local passed on is read where it is passed, through the borrow it holds.
        if let Some(closure) = self.closure_local(expression) {
            let (outgoing, argument) =
                self.closure_argument(&closure, &call.callee, expression, (slot, written_position), incoming)?;
            invocation.arguments.push(argument);
            return Ok(outgoing);
        }
        let call_result_field =
            super::super::call_results::direct_call_field_projection(&self.checker.type_info, expression);
        let source = if call_result_field.is_some() {
            None
        } else {
            self.optional_place(expression)?.map(|path| path.place)
        };
        let closed_enum_parameter_observer = if call_result_field.is_some() {
            false
        } else {
            self.closed_enum_parameter_argument(
                expression,
                &call.parameters[slot],
                call.kind == InputCallKind::Ordinary,
            )?
        };
        // An exclusive reborrow of a `&mut` binding is reserved, as a shared loan, until the call.
        let reserved = access == IncanInputAccess::Exclusive && self.reborrowed_exclusive_argument(expression)?;
        let start_node = self.graph.nodes.len();
        incoming = self.event(incoming, Event::Nothing, expression.span);
        let (outgoing, loan) = self.call_argument(
            expression,
            &call.parameters[slot],
            argument_mode(access, reserved),
            call.kind == InputCallKind::Ordinary,
            closed_enum_parameter_observer,
            selected_result_map_err,
            matches!(call.kind, InputCallKind::Explicit | InputCallKind::Native)
                || (call.kind == InputCallKind::Ordinary
                    && self
                        .checker
                        .type_info
                        .constructor_field_binding(expression.span)
                        .is_some())
                || path_join_independent_result,
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
            if reserved {
                reservations.push(Reservation {
                    slot: ReservedSlot::Argument(invocation.arguments.len()),
                    loan,
                    writable: call.parameters[slot].is_mut,
                    span: expression.span,
                });
            }
        }
        let classified_move = self
            .graph
            .value_uses
            .get(&expression.span)
            .is_some_and(|value_use| value_use.kind == ValueUseKind::Move);
        invocation.arguments.push(InvocationArgument {
            span: expression.span,
            slot,
            written_position,
            access: match (access, classified_move) {
                (IncanInputAccess::Inferred, true) => IncanInputAccess::Owned,
                (access, _) => access,
            },
            loan,
            source: loan.map(|loan| self.graph.loans[loan.0].source.clone()).or(source),
            closed_enum_parameter_observer,
            evaluation: InputEvaluation {
                start_node,
                end_node,
                source_node,
            },
        });
        Ok(incoming)
    }

    /// Require a compiler-selected `Result.map_err` to retain the exact receiver, callback and invocation route its
    /// checked flow recorded.
    pub(super) fn require_retained_map_err(
        &self,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        span: Span,
    ) -> Result<(), Unavailable> {
        let receiver = receiver.ok_or(Unavailable {
            span,
            reason: "call has no selected explicit input access facts".into(),
        })?;
        if super::super::call_results::retained_result_map_err(
            &self.checker.type_info,
            &self.owner,
            receiver,
            span,
            arguments,
            &self.graph,
        )
        .is_none()
        {
            return Err(Unavailable {
                span,
                reason: "call has no selected explicit input access facts".into(),
            });
        }
        Ok(())
    }
}

/// The facts a call's result admission settles for the rest of its evaluation.
pub(super) struct CallResultShape {
    /// The checked conditional result an Explicit producer must keep, when it has one.
    pub(super) result_requirement: Option<ResolvedType>,
    /// Whether the call is the compiler-selected `Result.map_err` combinator.
    pub(super) selected_result_map_err: bool,
    /// Whether the canonical `Path.joinpath` returns an independent result at this call.
    pub(super) path_join_independent_result: bool,
}

/// One call's selected facts, shared by the evaluation of each of its written arguments.
#[derive(Clone, Copy)]
pub(super) struct CallSite<'a> {
    pub(super) call: &'a InputCall,
    pub(super) contract: Option<&'a crate::ownership_metadata::CheckedOwnershipContract>,
    pub(super) span: Span,
    pub(super) selected_result_map_err: bool,
    pub(super) path_join_independent_result: bool,
}

/// The invocation being built for a call, the loans it holds, and its reserved two-phase loans.
pub(super) type CallTracking<'a> = (&'a mut Invocation, &'a mut Vec<LoanId>, &'a mut Vec<Reservation>);

/// Translate authenticated input access into local evaluation mode, reserving an exclusive reborrow as shared.
fn argument_mode(access: IncanInputAccess, reserved: bool) -> Option<OwnershipMode> {
    match access {
        IncanInputAccess::Shared | IncanInputAccess::InferredShared => Some(OwnershipMode::Borrowed),
        IncanInputAccess::Exclusive if reserved => Some(OwnershipMode::Borrowed),
        IncanInputAccess::Exclusive => Some(OwnershipMode::BorrowedMut),
        IncanInputAccess::Owned => Some(OwnershipMode::Owned),
        IncanInputAccess::Inferred => None,
    }
}

/// An invocation of `callee` with no evaluated receiver, arguments or nodes yet.
pub(super) fn empty_invocation(
    callee: incan_semantics_core::CanonicalSymbolId,
    result_requirement: Option<ResolvedType>,
    arguments: usize,
) -> Invocation {
    Invocation {
        callee,
        result_requirement,
        receiver: None,
        receiver_owned: false,
        receiver_source: None,
        receiver_dependencies: Vec::new(),
        arguments: Vec::with_capacity(arguments),
        receiver_evaluation: None,
        completion_node: 0,
        final_accesses: Vec::new(),
    }
}
