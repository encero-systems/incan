//! One current-source classification for stored reads, moves, materializations and independent call results.

use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::super::flow::{ResultMapErrUse, ValueUse, ValueUseAccess, ValueUseKind, ValueUseRole};
use super::{Builder, Path, ResolvedType, Root, Unavailable};
use crate::ast::{AssignmentStmt, BindingKind, CallArg, Expr, MatchArm, MatchBody, Pattern, PatternArg, Span, Spanned};
use crate::symbols::{CallableParam, TypeInfo};

mod call_results;

/// The source context that requests one value classification.
pub(super) enum ValueUseContext<'a> {
    /// A first unannotated local declaration may own one independently materialized value.
    LocalInitializer {
        assignment: &'a AssignmentStmt,
        destination: &'a CanonicalSymbolId,
        source: &'a Path,
    },
    /// One immutable ordinary parameter observes a value without taking its owner.
    OrdinaryArgument(&'a CallableParam),
    /// One exact constructor field stores the classified value.
    ConstructorField(&'a ResolvedType),
    /// One builtin aggregate element owns an exact fresh call result or field-only stored projection.
    AggregateElement(&'a ResolvedType),
    /// One selected builtin call observes a direct independent source-call result as its receiver.
    CallReceiver { invocation: Span, arguments: &'a [CallArg] },
    /// One inferred assignment extracts a single independent enum payload.
    MatchAssignment(&'a [Spanned<MatchArm>]),
}

/// A classification plan whose graph event is retained only after source evaluation succeeds.
pub(super) struct ValueUsePlan {
    kind: ValueUseKind,
    /// Exact invocation evaluated for a source-less result. It equals the classified span for whole calls and names
    /// the inner direct call for the one authenticated field projection.
    call_result: Option<Span>,
    independent_local: Option<CanonicalSymbolId>,
    match_payload: Option<CanonicalSymbolId>,
    result_map_err: Option<ResultMapErrUse>,
}

impl Builder<'_> {
    fn same_source_value_type(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        super::super::projection::same_checked_value_type_in(&self.nominal_scope, actual, expected)
    }

    fn source_nominal_identity(&self, name: &str) -> Option<&CanonicalSymbolId> {
        super::super::projection::checked_nominal_identity_in(&self.nominal_scope, name)
    }

    /// Classify one value from current checked source facts. The result is not persisted and grants no authority to a
    /// restored body; every supported context consumes this same read/move/call-result vocabulary.
    pub(super) fn classify_value_use(
        &mut self,
        value: &Spanned<Expr>,
        context: ValueUseContext<'_>,
    ) -> Result<Option<ValueUsePlan>, Unavailable> {
        match context {
            ValueUseContext::LocalInitializer {
                assignment,
                destination,
                source,
            } => Ok(self
                .independent_parameter_index(assignment, destination, source)
                .then(|| ValueUsePlan {
                    kind: ValueUseKind::Read,
                    call_result: None,
                    independent_local: Some(destination.clone()),
                    match_payload: None,
                    result_map_err: None,
                })),
            ValueUseContext::OrdinaryArgument(parameter) => {
                let kind = if self.ordinary_read(value, &parameter.ty) {
                    Some(ValueUseKind::Read)
                } else if self.closed_call_result(value, &parameter.ty) {
                    Some(ValueUseKind::CallResult)
                } else if self.ordinary_move(value, parameter)? {
                    Some(ValueUseKind::Move)
                } else {
                    None
                };
                Ok(kind.map(|kind| ValueUsePlan {
                    kind,
                    call_result: (kind == ValueUseKind::CallResult).then_some(value.span),
                    independent_local: None,
                    match_payload: None,
                    result_map_err: None,
                }))
            }
            ValueUseContext::ConstructorField(expected) => {
                let kind = if self.constructor_move(value, expected)? {
                    Some(ValueUseKind::Move)
                } else if self.constructor_read(value, expected)? {
                    Some(ValueUseKind::Read)
                } else if self.closed_call_result(value, expected) {
                    Some(ValueUseKind::CallResult)
                } else {
                    None
                };
                Ok(kind.map(|kind| ValueUsePlan {
                    kind,
                    call_result: (kind == ValueUseKind::CallResult).then_some(value.span),
                    independent_local: None,
                    match_payload: None,
                    result_map_err: None,
                }))
            }
            ValueUseContext::AggregateElement(expected) => {
                let kind = if self.closed_call_result(value, expected) {
                    Some(ValueUseKind::CallResult)
                } else if self.aggregate_move(value, expected)? {
                    Some(ValueUseKind::Move)
                } else {
                    None
                };
                Ok(kind.map(|kind| ValueUsePlan {
                    kind,
                    call_result: (kind == ValueUseKind::CallResult).then_some(value.span),
                    independent_local: None,
                    match_payload: None,
                    result_map_err: None,
                }))
            }
            ValueUseContext::CallReceiver { invocation, arguments } => Ok(self
                .closed_source_result_receiver(value, invocation, arguments)
                .map(|callback| ValueUsePlan {
                    kind: ValueUseKind::CallResult,
                    call_result: Some(value.span),
                    independent_local: None,
                    match_payload: None,
                    result_map_err: Some(ResultMapErrUse {
                        invocation,
                        receiver: value.span,
                        callback,
                    }),
                })),
            ValueUseContext::MatchAssignment(arms) => {
                Ok(self.match_payload(value, arms)?.map(|payload| ValueUsePlan {
                    kind: ValueUseKind::Move,
                    call_result: None,
                    independent_local: None,
                    match_payload: Some(payload),
                    result_map_err: None,
                }))
            }
        }
    }

    /// Classify one ordinary immutable argument that takes a canonical field from a fresh, dependency-free direct
    /// call result. The persisted authority belongs to the outer value while evaluation executes the inner call once.
    pub(super) fn classify_call_result_field_projection(
        &self,
        value: &Spanned<Expr>,
        expected: &ResolvedType,
    ) -> Option<ValueUsePlan> {
        let base = super::call_results::direct_call_field_projection(&self.checker.type_info, value)?;
        let outer = self.checker.type_info.expr_type(value.span)?;
        let base_type = self.checker.type_info.expr_type(base.span)?;
        if !self.same_source_value_type(outer, expected)
            || self.checker.type_holds_borrow(outer)
            || self.checker.type_holds_native_loan(outer)
            || self.checker.type_holds_borrow(base_type)
            || self.checker.type_holds_native_loan(base_type)
            || !self.closed_call_result(base, base_type)
        {
            return None;
        }
        Some(ValueUsePlan {
            kind: ValueUseKind::CallResult,
            call_result: Some(base.span),
            independent_local: None,
            match_payload: None,
            result_map_err: None,
        })
    }

    /// Evaluate and retain one classification. Stored routes require one exact final event; a selected call result
    /// retains no stored access and is replayed only while this non-persisted classification exists.
    pub(super) fn apply_value_use(
        &mut self,
        value: &Spanned<Expr>,
        plan: ValueUsePlan,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let outgoing = match plan.kind {
            ValueUseKind::Read => self.expression(value, incoming, false)?,
            ValueUseKind::Move => {
                let path = self.optional_place(value)?.ok_or(Unavailable {
                    span: value.span,
                    reason: "classified move has no checked source place".into(),
                })?;
                self.constrained.insert(path.place.root);
                self.expression(value, incoming, true)?
            }
            ValueUseKind::CallResult => {
                let call_span = plan.call_result.ok_or(Unavailable {
                    span: value.span,
                    reason: "call-result classification has no exact invocation".into(),
                })?;
                let evaluated = if call_span == value.span {
                    value
                } else {
                    super::call_results::direct_call_field_projection(&self.checker.type_info, value)
                        .filter(|base| base.span == call_span)
                        .ok_or(Unavailable {
                            span: value.span,
                            reason: "call-result field projection changed after classification".into(),
                        })?
                };
                self.independent_call_results.insert(call_span);
                let result = self.expression(evaluated, incoming, false);
                self.independent_call_results.remove(&call_span);
                result?
            }
        };
        let accesses = if plan.kind == ValueUseKind::CallResult {
            Vec::new()
        } else {
            let [node] = outgoing.as_slice() else {
                return Err(Unavailable {
                    span: value.span,
                    reason: "classified stored value needs one final access".into(),
                });
            };
            vec![self.retained_value_access(*node, ValueUseRole::Stored)?]
        };
        let value_type = self
            .checker
            .type_info
            .expr_type(value.span)
            .cloned()
            .ok_or(Unavailable {
                span: value.span,
                reason: "classified value has no checked type".into(),
            })?;
        if self
            .graph
            .value_uses
            .insert(
                value.span,
                ValueUse {
                    value_type,
                    kind: plan.kind,
                    accesses,
                    result_map_err: plan.result_map_err,
                },
            )
            .is_some()
        {
            return Err(Unavailable {
                span: value.span,
                reason: "value has more than one source classification".into(),
            });
        }
        if let Some(local) = plan.independent_local {
            self.independent_locals.insert(local);
        }
        Ok(outgoing)
    }

    /// Evaluate one classified match extraction and retain its exact subject and payload moves under the outer
    /// expression span. The shared `Move` classification, rather than a second replay classifier, owns both events.
    pub(super) fn apply_match_value_use(
        &mut self,
        value: &Spanned<Expr>,
        scrutinee: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        plan: ValueUsePlan,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let payload = plan.match_payload.ok_or(Unavailable {
            span: value.span,
            reason: "match extraction has no classified payload".into(),
        })?;
        let subject = self.optional_place(scrutinee)?.ok_or(Unavailable {
            span: scrutinee.span,
            reason: "match extraction has no stored subject".into(),
        })?;
        self.constrained.insert(subject.place.root.clone());
        let first = self.graph.nodes.len();
        let outgoing = self.match_statement(scrutinee, arms, Some(&payload), incoming)?;
        let payload_place = super::Place {
            root: Root::Binding(payload),
            projection: Vec::new(),
        };
        let mut subject_access = None;
        let mut payload_access = None;
        for (index, node) in self.graph.nodes.iter().enumerate().skip(first) {
            let super::Event::Access {
                place,
                access: super::Access::Move,
                ..
            } = &node.event
            else {
                continue;
            };
            if place == &subject.place {
                subject_access = Some(index);
            } else if place == &payload_place {
                payload_access = Some(index);
            }
        }
        let (Some(subject_access), Some(payload_access)) = (subject_access, payload_access) else {
            return Err(Unavailable {
                span: value.span,
                reason: "match extraction needs exact subject and payload moves".into(),
            });
        };
        let accesses = vec![
            self.retained_value_access(subject_access, ValueUseRole::MatchSubject)?,
            self.retained_value_access(payload_access, ValueUseRole::MatchPayload)?,
        ];
        let value_type = self
            .checker
            .type_info
            .expr_type(value.span)
            .cloned()
            .ok_or(Unavailable {
                span: value.span,
                reason: "match extraction has no checked result type".into(),
            })?;
        if self
            .graph
            .value_uses
            .insert(
                value.span,
                ValueUse {
                    value_type,
                    kind: ValueUseKind::Move,
                    accesses,
                    result_map_err: None,
                },
            )
            .is_some()
        {
            return Err(Unavailable {
                span: value.span,
                reason: "match extraction has more than one source classification".into(),
            });
        }
        Ok(outgoing)
    }

    /// Snapshot the exact access event that one current-source classification owns.
    fn retained_value_access(&self, node: usize, role: ValueUseRole) -> Result<ValueUseAccess, Unavailable> {
        let retained = self.graph.nodes.get(node).ok_or(Unavailable {
            span: self.owner.declaration_span,
            reason: "classified access is outside callable".into(),
        })?;
        let super::Event::Access { place, access, through } = &retained.event else {
            return Err(Unavailable {
                span: retained.span,
                reason: "classified value has no retained access".into(),
            });
        };
        Ok(ValueUseAccess {
            node,
            place: place.clone(),
            access: *access,
            through: *through,
            role,
        })
    }

    /// Whether one first immutable local owns the builtin index result cloned from a plain transferable parameter.
    fn independent_parameter_index(
        &self,
        assignment: &AssignmentStmt,
        destination: &CanonicalSymbolId,
        source: &Path,
    ) -> bool {
        let Expr::Index(base, _) = &assignment.value.node else {
            return false;
        };
        let Some(result) = self.checker.type_info.expr_type(assignment.value.span) else {
            return false;
        };
        destination.kind == SemanticSourceTargetKind::Local
            && destination.declaration_span == assignment.name_span
            && !matches!(assignment.binding, BindingKind::Mutable | BindingKind::Reassign)
            && self
                .locals
                .get(destination)
                .is_some_and(|permission| !permission.exclusive && !permission.writable && permission.transferable)
            && self
                .checker
                .type_info
                .resolved_operator_call(assignment.value.span)
                .is_none()
            && self
                .checker
                .type_info
                .resolved_write_type(assignment.name_span, &assignment.name)
                == Some(result)
            && self.checker.is_clone_type(result)
            && !self.checker.type_holds_borrow(result)
            && !self.checker.type_holds_native_loan(result)
            && matches!(self.checker.type_info.expr_type(base.span),
                Some(ResolvedType::Generic(name, arguments))
                    if collections::from_str(name) == Some(CollectionTypeId::List)
                        && arguments.as_slice() == std::slice::from_ref(result))
            && source.through.is_none()
            && matches!(&source.place.root,
                Root::Binding(identity) if identity.kind == SemanticSourceTargetKind::Parameter)
            && source.place.projection.len() == 1
            && self
                .graph
                .permissions
                .get(&source.place.root)
                .is_some_and(|permission| !permission.exclusive && !permission.writable && permission.transferable)
            && !self.borrowed_inputs.contains(&source.place.root)
            && !self.constrained.contains(&source.place.root)
    }

    /// Whether one immutable ordinary slot may read the exact checked stored projection.
    fn ordinary_read(&mut self, value: &Spanned<Expr>, expected: &ResolvedType) -> bool {
        if !self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|actual| self.same_source_value_type(actual, expected))
            || !self.checker.is_clone_type(expected)
            || self.checker.type_holds_borrow(expected)
            || self.checker.type_holds_native_loan(expected)
        {
            return false;
        }
        let Ok(Some(path)) = self.optional_place(value) else {
            return false;
        };
        let Root::Binding(identity) = &path.place.root else {
            return false;
        };
        if path.through.is_some()
            || self.borrowed_inputs.contains(&path.place.root)
            || self.graph.local_loans.contains_key(identity)
            || self.graph.dependent_locals.contains_key(identity)
            || self.graph.owned_locals.contains_key(identity)
            || !self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| !permission.exclusive && !permission.writable && permission.transferable)
        {
            return false;
        }
        match path.place.projection.as_slice() {
            [super::Projection::Element(_)] => self.independent_locals.contains(identity),
            [super::Projection::Element(_), super::Projection::Field(field)] => {
                identity.kind == SemanticSourceTargetKind::Local
                    && field.kind == SemanticSourceTargetKind::Field
                    && self
                        .checker
                        .type_info
                        .expr_type(value.span)
                        .is_some_and(|actual| self.same_source_value_type(actual, expected))
            }
            _ => false,
        }
    }

    /// Whether one immutable ordinary slot consumes an exact whole owned Parameter once. Borrowed roots, Copy
    /// values, projections, aliases and previously constrained parameters retain their existing routes.
    fn ordinary_move(&mut self, value: &Spanned<Expr>, parameter: &CallableParam) -> Result<bool, Unavailable> {
        if parameter.is_mut
            || !self
                .checker
                .type_info
                .expr_type(value.span)
                .is_some_and(|actual| self.same_source_value_type(actual, &parameter.ty))
            || self.copy_value(&parameter.ty)
            || self.checker.type_holds_borrow(&parameter.ty)
            || self.checker.type_holds_native_loan(&parameter.ty)
        {
            return Ok(false);
        }
        let Some(path) = self.optional_place(value)? else {
            return Ok(false);
        };
        if path.through.is_some()
            || !path.place.projection.is_empty()
            || self.borrowed_inputs.contains(&path.place.root)
            || self.constrained.contains(&path.place.root)
        {
            return Ok(false);
        }
        Ok(matches!(&path.place.root, Root::Binding(identity)
        if identity.kind == SemanticSourceTargetKind::Parameter
            && self.graph.permissions.get(&path.place.root).is_some_and(|permission| {
                !permission.exclusive && !permission.writable && permission.transferable
            })))
    }

    /// Whether one constructor field consumes a whole plain enum Parameter or one exact unaliased Local. Local
    /// mutability does not prevent final transfer; the persisted Move constrains the root against later reuse.
    fn constructor_move(&mut self, value: &Spanned<Expr>, expected: &ResolvedType) -> Result<bool, Unavailable> {
        if !self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|actual| self.same_source_value_type(actual, expected))
            || self.checker.type_holds_borrow(expected)
            || self.checker.type_holds_native_loan(expected)
        {
            return Ok(false);
        }
        let Some(path) = self.optional_place(value)? else {
            return Ok(false);
        };
        if path.through.is_some()
            || !path.place.projection.is_empty()
            || self.borrowed_inputs.contains(&path.place.root)
        {
            return Ok(false);
        }
        let Root::Binding(identity) = &path.place.root else {
            return Ok(false);
        };
        let transferable = self
            .graph
            .permissions
            .get(&path.place.root)
            .is_some_and(|permission| permission.transferable);
        let plain_enum_parameter = identity.kind == SemanticSourceTargetKind::Parameter
            && !self.declared_constrained_inputs.contains(&path.place.root)
            && matches!(expected,
                ResolvedType::Named(owner) | ResolvedType::Generic(owner, _)
                    if matches!(self.checker.lookup_type_info(owner), Some(TypeInfo::Enum(_)))
                        || self.source_nominal_identity(owner)
                            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Enum));
        let whole_local = identity.kind == SemanticSourceTargetKind::Local
            && !self.aliases.contains_key(identity)
            && !self.graph.local_loans.contains_key(identity)
            && !self.graph.dependent_locals.contains_key(identity)
            && !self.graph.owned_locals.contains_key(identity)
            && !self.constrained.contains(&path.place.root)
            && self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| permission.transferable);
        Ok(transferable && (plain_enum_parameter || whole_local))
    }

    /// Whether one constructor field reads a scalar element from an authenticated independent local snapshot.
    fn constructor_read(&mut self, value: &Spanned<Expr>, expected: &ResolvedType) -> Result<bool, Unavailable> {
        if !self.ordinary_read(value, expected) {
            return Ok(false);
        }
        Ok(matches!(value.node, Expr::Index(..)))
    }

    /// Whether one aggregate element consumes an exact field-only stored projection. This preserves the existing
    /// list-element transfer semantics: it is a Move, not an inferred Clone-like read.
    fn aggregate_move(&mut self, value: &Spanned<Expr>, expected: &ResolvedType) -> Result<bool, Unavailable> {
        if !self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|actual| self.same_source_value_type(actual, expected))
            || self.checker.type_holds_borrow(expected)
            || self.checker.type_holds_native_loan(expected)
        {
            return Ok(false);
        }
        let Some(path) = self.optional_place(value)? else {
            return Ok(false);
        };
        Ok(path.through.is_none()
            && !self.borrowed_inputs.contains(&path.place.root)
            && matches!(&path.place.root,
                Root::Binding(identity)
                    if matches!(identity.kind,
                        SemanticSourceTargetKind::Local | SemanticSourceTargetKind::Parameter))
            && !path.place.projection.is_empty()
            && path
                .place
                .projection
                .iter()
                .all(|projection| matches!(projection, super::Projection::Field(_)))
            && self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| permission.transferable))
    }

    /// Whether one exact selected call, optionally unwrapped by `?`, has a dependency-free result equal to the
    /// ordinary slot. This classifies the fresh result itself; stored results retain the ordinary-place refusal.
    fn closed_call_result(&self, value: &Spanned<Expr>, expected: &ResolvedType) -> bool {
        let (call_expression, tried) = match &value.node {
            Expr::Call(..) | Expr::MethodCall(..) => (value, false),
            Expr::Try(inner) if matches!(inner.node, Expr::Call(..) | Expr::MethodCall(..)) => (inner.as_ref(), true),
            _ => return false,
        };
        if !self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|actual| self.same_source_value_type(actual, expected))
            || self.checker.type_holds_borrow(expected)
            || self.checker.type_holds_native_loan(expected)
        {
            return false;
        }
        let Some(call) = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&call_expression.span)
        else {
            return false;
        };
        let Ok(contract) = self.checker.type_info.selected_input_contract(call) else {
            return false;
        };
        matches!(
            call.kind,
            super::super::InputCallKind::Ordinary | super::super::InputCallKind::Explicit
        ) && call.owner.as_ref() == Some(&self.owner)
            && crate::ownership_metadata::returned_view(contract).is_ok_and(|view| view.is_none())
            && crate::ownership_metadata::owned_result(contract).is_ok_and(|owned| owned.is_none())
            && if tried {
                matches!(self.checker.type_info.expr_type(call_expression.span),
                    Some(ResolvedType::Generic(name, arguments))
                        if collections::from_str(name) == Some(CollectionTypeId::Result)
                            && arguments
                                .first()
                                .is_some_and(|actual| self.same_source_value_type(actual, expected))
                            && arguments.len() == 2)
            } else {
                self.checker
                    .type_info
                    .expr_type(call_expression.span)
                    .is_some_and(|actual| self.same_source_value_type(actual, expected))
            }
            && self
                .checker
                .native_result_dependencies(call_expression.span)
                .is_ok_and(|dependencies| dependencies.is_none())
            && !self.checker.native_result_is_independent(call_expression.span)
            && !self.checked_clone_call(call_expression.span)
    }

    /// Select one continuing canonical enum-payload arm; every other arm binds no payload and terminates.
    fn match_payload(
        &mut self,
        value: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
    ) -> Result<Option<CanonicalSymbolId>, Unavailable> {
        let Expr::Match(scrutinee, _) = &value.node else {
            return Ok(None);
        };
        let Some(path) = self.optional_place(scrutinee)? else {
            return Ok(None);
        };
        let Root::Binding(subject) = &path.place.root else {
            return Ok(None);
        };
        let enum_subject = matches!(self.checker.type_info.expr_type(scrutinee.span),
            Some(ResolvedType::Named(owner) | ResolvedType::Generic(owner, _))
                if matches!(self.checker.lookup_type_info(owner), Some(TypeInfo::Enum(_)))
                    || self.checker.type_info.declarations.enum_derives.contains_key(owner)
                    || self.source_nominal_identity(owner)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Enum));
        if path.through.is_some()
            || subject.kind != SemanticSourceTargetKind::Local
            || self.graph.local_loans.contains_key(subject)
            || self.graph.dependent_locals.contains_key(subject)
            || !enum_subject
            || self
                .checker
                .type_info
                .expr_type(scrutinee.span)
                .is_some_and(|ty| self.checker.type_holds_borrow(ty) || self.checker.type_holds_native_loan(ty))
            || !self.checker.type_info.scrutinee_holds_no_loan(scrutinee.span)
        {
            return Ok(None);
        }
        let mut payload = None;
        for arm in arms {
            if arm.node.guard.is_some() {
                return Ok(None);
            }
            match (&arm.node.pattern.node, &arm.node.body) {
                (Pattern::Constructor(name, arguments), MatchBody::Expr(result))
                    if self
                        .checker
                        .type_info
                        .resolved_identity(name.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Variant)
                        && arguments.len() == 1 =>
                {
                    let [PatternArg::Positional(binding)] = arguments.as_slice() else {
                        return Ok(None);
                    };
                    let Pattern::Binding(name) = &binding.node else {
                        return Ok(None);
                    };
                    let Some(identity) = self
                        .checker
                        .type_info
                        .resolved_write_identity(binding.span, name)
                        .cloned()
                    else {
                        return Ok(None);
                    };
                    if payload.is_some()
                        || self.checker.type_info.resolved_identity(result.span) != Some(&identity)
                        || self.checker.type_info.expr_type(result.span)
                            != self.checker.type_info.resolved_write_type(binding.span, name)
                    {
                        return Ok(None);
                    }
                    payload = Some(identity);
                }
                (Pattern::Wildcard | Pattern::Literal(_), MatchBody::Block(_)) => {}
                _ => return Ok(None),
            }
        }
        Ok(payload)
    }
}
