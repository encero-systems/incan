//! Selected source-call access projection. Ordinary unqualified fixed calls evaluate in declaration order;
//! methods preserve written order. Original written positions remain provenance in both cases. Returned/native
//! dependencies and unresolved adaptations refuse this profile until their own facts are available.

use super::super::flow::{InputEvaluation, Invocation, InvocationArgument};
use super::{
    Access, Builder, Event, Loan, LoanId, LoanKind, OwnershipMode, Path, Permission, Place, ResolvedType, Root,
    Unavailable,
};
use crate::ast::{CallArg, Expr, ParamKind, Receiver, Span, Spanned};
use crate::symbols::CallableParam;
use crate::typechecker::TypeChecker;
use incan_lang::lang::builtins::BuiltinFnId;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::IncanInputAccess;

impl Builder<'_> {
    /// Apply a selected free-function contract without inspecting its body. Borrowed arguments remain live until
    /// invocation, and owned arguments transfer at their source evaluation position even for a read-only callee.
    /// Missing default/rest/capture/returned-dependency facts never become a successful partial call graph.
    pub(super) fn call(
        &mut self,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        if let Some(builtin) = self
            .checker
            .type_info
            .resolved_builtin_call(span)
            .filter(|builtin| matches!(builtin, BuiltinFnId::Print | BuiltinFnId::Len))
        {
            return self.observer_call(builtin, arguments, span, incoming);
        }
        if self.checker.type_info.constructor_field_binding(span).is_some() {
            return self.model_construction(callee, arguments, span, incoming);
        }
        if !matches!(callee.node, Expr::Ident(_)) {
            return Err(Unavailable {
                span,
                reason: "callee evaluation needs checked access facts".into(),
            });
        }
        self.apply_call(None, arguments, span, incoming)
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
                BuiltinFnId::Print => matches!(
                    ty,
                    Some(
                        ResolvedType::Str
                            | ResolvedType::FrozenStr
                            | ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Unit
                    )
                ),
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

    /// Apply one canonical call, evaluating its receiver before written arguments. The receiver's declared access
    /// remains live through invocation; no method-body analysis is needed to strengthen or weaken that contract.
    pub(super) fn apply_call(
        &mut self,
        receiver: Option<&Spanned<Expr>>,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
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
            .declarations
            .ownership_contracts
            .get(&call.callee)
            .ok_or(Unavailable {
                span,
                reason: "selected callable contract is missing".into(),
            })?;
        if call.owner.as_ref() != Some(&self.owner) {
            return Err(Unavailable {
                span,
                reason: "call facts belong to a different callable".into(),
            });
        }
        if call.receiver.is_some() != receiver.is_some() {
            return Err(Unavailable {
                span,
                reason: "selected receiver contract does not match call syntax".into(),
            });
        }
        if contract.result_source.is_some()
            || contract.modes.iter().any(|mode| {
                mode.component.parameter.is_none()
                    || !mode.component.children.is_empty()
                    || mode.mode == OwnershipMode::Reference
            })
            || call
                .parameters
                .iter()
                .any(|parameter| parameter.kind != ParamKind::Normal || parameter.is_partial_preset)
            || arguments.len() != call.parameters.len()
        {
            return Err(Unavailable {
                span,
                reason: "call needs returned, default or packed-argument dependency facts".into(),
            });
        }
        let result = self.checker.type_info.expr_type(span).ok_or(Unavailable {
            span,
            reason: "missing call result type".into(),
        })?;
        if !plain_result(result) {
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
        if receiver.is_none() {
            paired.sort_by_key(|(_, (_, slot))| *slot);
        }
        let mut loans = Vec::new();
        let mut invocation = Invocation {
            callee: call.callee.clone(),
            receiver: None,
            arguments: Vec::with_capacity(paired.len()),
            receiver_evaluation: None,
            completion_node: 0,
            final_accesses: Vec::new(),
        };
        if let Some(receiver) = receiver {
            let start_node = self.graph.nodes.len();
            incoming = self.event(incoming, Event::Nothing, receiver.span);
            let source = self.place(receiver)?;
            incoming = self.evaluate_place(receiver, incoming)?;
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
        for (written_position, (expression, slot)) in paired {
            let slot = slot.ok_or(Unavailable {
                span: expression.span,
                reason: "missing selected argument slot".into(),
            })?;
            let mode = contract
                .modes
                .iter()
                .find(|mode| mode.component.parameter == Some(slot))
                .map(|mode| mode.mode);
            let source = self.optional_place(expression)?.map(|path| path.place);
            let start_node = self.graph.nodes.len();
            incoming = self.event(incoming, Event::Nothing, expression.span);
            let (outgoing, loan) = self.call_argument(expression, &call.parameters[slot], mode, span, incoming)?;
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

    /// Evaluate one argument according to its selected slot. A borrowed place is evaluated once before taking its
    /// loan; an explicit ownership request constrains even an otherwise ordinary source binding. Unresolved
    /// duckborrow or ordinary mutable adapters refuse rather than cloning or guessing an access mode.
    fn call_argument(
        &mut self,
        expression: &Spanned<Expr>,
        parameter: &CallableParam,
        mode: Option<OwnershipMode>,
        invocation: Span,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Option<LoanId>), Unavailable> {
        match mode {
            Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) => {
                let (source, incoming) = self.borrowed_argument_source(expression, invocation, incoming)?;
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
            _ => Err(Unavailable {
                span: expression.span,
                reason: "ordinary argument requires inferred access or mutable binding facts".into(),
            }),
        }
    }

    /// A fresh scalar or owned byte-string expression may be borrowed for this call. Existing places keep their
    /// original permissions even through parentheses; evaluating a temporary preserves nested moves and loans.
    fn borrowed_argument_source(
        &mut self,
        expression: &Spanned<Expr>,
        invocation: Span,
        incoming: Vec<usize>,
    ) -> Result<(Path, Vec<usize>), Unavailable> {
        if let Some(source) = self.optional_place(expression)? {
            return Ok((source, self.evaluate_place(expression, incoming)?));
        }
        let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
            span: expression.span,
            reason: "temporary argument has no checked type".into(),
        })?;
        if !plain_result(ty) {
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
    fn call_loan(&mut self, source: Path, exclusive: bool, writable: bool, span: Span) -> LoanId {
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
}

/// This initial call profile admits only closed scalar/owned byte-string results, with no possible retained source
/// dependency. Aggregates, references, native and generic results await their checked dependency representation.
fn plain_result(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Unit
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
    )
}
