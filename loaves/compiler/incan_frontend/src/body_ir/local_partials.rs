//! Checked targets and explicit construction-time snapshots for local partial callables.

use super::args::fixed_elements;
use super::*;

/// Own the checked signature while local-frame construction mutates the builder; identities are never reconstructed.
pub(super) struct PartialTarget<'ast> {
    /// Diagnostic spelling, normalized by the checker for a method alias.
    name: String,
    /// Checked declaration slots, excluding a method receiver.
    params: Vec<CallableParam>,
    /// The target's checked result before contextual partial specialization.
    result: IncanType,
    /// Canonical target with its complete resolved positional slot binding.
    callee: bir::Callee,
    /// A value-place receiver evaluated before presets, if this is a checked method partial.
    receiver: Option<&'ast ast::Spanned<ast::Expr>>,
    /// Declaration-owned source defaults available for top-level function targets.
    defaults: Option<Vec<FunctionDefaultSource>>,
}

/// Construction-time values and private deferred-frame locals for one partial.
struct PartialFrame {
    /// Scope shared by the deferred parameters and captures.
    scope: bir::ScopeId,
    /// Evaluated in source order before introducing any synthetic bindings.
    operands: Vec<bir::Operand>,
    /// Same-order deferred-frame capture locals.
    captures: Vec<bir::LocalId>,
    /// Captured receiver, separate from the callable parameter surface.
    receiver: Option<bir::LocalId>,
    /// Preset names select their overrideable default capture slots.
    presets: HashMap<String, bir::LocalId>,
    /// Restore all shadowed bindings in reverse order after constructing the deferred frame.
    saved: Vec<(String, Option<bir::LocalId>)>,
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Select only checked function bindings or explicitly checked method-partial targets, before mutating locals.
    pub(super) fn partial_target<'ast>(
        &self,
        partial: &'ast ast::PartialExpr,
        expression_span: ast::Span,
    ) -> Result<PartialTarget<'ast>, String> {
        let target = match &partial.target.node {
            ast::Expr::Ident(name) => self.partial_function_target(name, partial, expression_span)?,
            ast::Expr::Field(receiver, _) => {
                if !partial.type_args.is_empty() {
                    return Err("partial method with explicit type arguments".to_owned());
                }
                let span = partial.target.span;
                let name = self
                    .type_info
                    .expressions
                    .local_partial_method_targets
                    .get(&span)
                    .ok_or("partial method target without checked value-receiver facts")?;
                let canonical = self
                    .type_info
                    .resolved_identity(span)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Method)
                    .ok_or("partial method target without a checked method identity")?;
                let Some(ResolvedType::Function(params, result)) = self.type_info.expr_type(span) else {
                    return Err("partial method target without a checked callable signature".to_string());
                };
                PartialTarget {
                    name: name.clone(),
                    params: params.clone(),
                    result: self.checked_type(result),
                    callee: bir::Callee::Method(bir::MethodTarget {
                        native_trait_delegation: None,
                        name: name.clone(),
                        intrinsic_carrier: None,
                        reflection: None,
                        loaf_trait: None,
                        trait_arguments: None,
                        canonical: Some(canonical.clone()),
                        type_args: Vec::new(),
                        binding: bir::ArgumentBinding::resolved_positional(params.len()),
                    }),
                    receiver: Some(receiver),
                    defaults: None,
                }
            }
            _ => return Err("partial callable with a non-function-name target".to_string()),
        };
        if target.params.iter().any(|param| {
            param.name.is_none()
                || (param.kind != ast::ParamKind::Normal && (target.receiver.is_some() || !partial.args.is_empty()))
        }) {
            return Err("partial callable target with an unnamed or rest parameter".to_string());
        }
        Ok(target)
    }

    /// Read the checker's ordered instance arguments at this exact partial; written syntax alone grants no instance.
    pub(super) fn partial_instance_arguments(
        &self,
        partial: &ast::PartialExpr,
        span: ast::Span,
    ) -> Result<Vec<IncanType>, String> {
        let arguments = self.type_info.calls.call_site_monomorph_type_args.get(&span);
        if !partial.type_args.is_empty() && arguments.is_none_or(|arguments| arguments.len() != partial.type_args.len())
        {
            return Err("partial function without checked instance arguments".to_owned());
        }
        Ok(arguments
            .into_iter()
            .flatten()
            .map(|argument| self.checked_type(argument))
            .collect())
    }

    /// Retain the checked function instance, original defaults and exact declaration identity without reopening
    /// binders.
    fn partial_function_target(
        &self,
        name: &str,
        partial: &ast::PartialExpr,
        expression_span: ast::Span,
    ) -> Result<PartialTarget<'static>, String> {
        let binding = self
            .type_info
            .declarations
            .function_bindings
            .get(name)
            .ok_or("partial callable target with no resolvable top-level function signature")?;
        let selected = self
            .type_info
            .resolved_identity(partial.target.span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Function)
            .ok_or("partial function without a checked target identity")?;
        if binding.identity.as_ref() != Some(selected) {
            return Err("partial function differs from its checked declaration".to_owned());
        }
        let (params, result) = match self.type_info.expr_type(partial.target.span) {
            Some(ResolvedType::Function(params, result)) => (params.as_slice(), result.as_ref()),
            None if partial.type_args.is_empty()
                && partial.args.is_empty()
                && self.local_nongeneric_functions.contains(selected)
                && !binding
                    .params
                    .iter()
                    .any(|parameter| self.leaves_type_open(&self.checked_type(&parameter.ty)))
                && !self.leaves_type_open(&self.checked_type(&binding.return_type)) =>
            {
                (binding.params.as_slice(), &binding.return_type)
            }
            _ => return Err("partial function without a checked target signature".to_owned()),
        };
        let type_args = self.partial_instance_arguments(partial, expression_span)?;
        let direct_call_id =
            self.local_function_declarations
                .get(name)
                .and_then(|candidates| match candidates.as_slice() {
                    [span] => Some(CompilerNodeId::declaration_at(self.module_identity, *span)),
                    _ => None,
                });
        Ok(PartialTarget {
            name: name.to_string(),
            params: params.to_vec(),
            result: self.checked_type(result),
            callee: bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: name.to_string(),
                direct_call_id,
                builtin: None,
                canonical: binding.identity.clone(),
                type_args,
                binding: bir::ArgumentBinding::resolved_positional(params.len()),
            })),
            receiver: None,
            defaults: self.function_default_sources.get(name).cloned(),
        })
    }

    /// Build the same forwarding closure for functions and methods, keeping the receiver outside declared slots.
    pub(super) fn lower_partial_target(
        &mut self,
        partial: &ast::PartialExpr,
        target: PartialTarget<'_>,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        // ---- Construction-time captures and deferred parameters ----
        let span = hir_span(expr_span);
        let mut frame = self.partial_captures(partial, &target, span, scope, out);
        let (checked_params, result) = match self.resolve_ty(expr_span) {
            IncanType::Function { params, return_type } => (params, *return_type),
            _ => (Vec::new(), target.result.clone()),
        };
        let params = self.partial_parameters(&target, &checked_params, &mut frame, span);

        // ---- Forward through the checked target and borrowed receiver ----
        let mut arguments = Vec::with_capacity(params.len() + usize::from(frame.receiver.is_some()));
        if let Some(receiver) = frame.receiver {
            arguments.push(bir::Operand::place(
                bir::Place::from_local(receiver),
                bir::OwnershipFact::Borrow,
                false,
            ));
        }
        for param in &params {
            let place = bir::Place::from_local(param.local);
            let (fact, last_use) = self.ownership_fact_for_place(&place, &param.ty);
            let value = bir::Operand::place(place, fact, last_use);
            arguments.push(if param.mutable {
                self.borrow_for_mut_parameter(value)
            } else {
                value
            });
        }
        let mut statements = Vec::new();
        let result = self.push_call_temp(
            target.callee,
            fixed_elements(arguments),
            result,
            frame.scope,
            span,
            false,
            &mut statements,
        );

        // ---- Restore enclosing bindings before assigning the completed closure ----
        for (name, previous) in frame.saved.into_iter().rev() {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }
        work::nested_body(self.module_identity, span, "local_partial");
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params,
                captured_operands: frame.operands,
                body: Box::new(bir::ClosureBody {
                    native_storage: None,
                    original_callable: None,
                    consumes_environment: false,
                    function_item: false,
                    capture_locals: frame.captures,
                    stmts: statements,
                    result,
                }),
            },
            self.resolve_ty(expr_span),
            scope,
            span,
            out,
        )
    }

    /// Evaluate receiver and presets before synthetic bindings can shadow an argument's source name.
    fn partial_captures(
        &mut self,
        partial: &ast::PartialExpr,
        target: &PartialTarget<'_>,
        span: HirSourceSpan,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> PartialFrame {
        // ---- Snapshot values before later expressions can mutate their source places ----
        let mut values = Vec::with_capacity(partial.args.len() + usize::from(target.receiver.is_some()));
        if let Some(receiver) = target.receiver {
            let (ty, operand) = self.snapshot_partial_value(receiver, scope, out);
            values.push((None, ty, operand));
        }
        for arg in &partial.args {
            let (ty, operand) = self.snapshot_partial_value(&arg.value, scope, out);
            values.push((Some(arg.name.clone()), ty, operand));
        }

        // ---- Bind deferred captures after every source expression has been evaluated ----
        let mut frame = PartialFrame {
            scope: self.new_scope(Some(scope), span),
            operands: Vec::with_capacity(values.len()),
            captures: Vec::with_capacity(values.len()),
            receiver: None,
            presets: HashMap::new(),
            saved: Vec::new(),
        };
        for (preset, ty, operand) in values {
            let name = preset.as_ref().map_or_else(
                || "__partial_receiver".to_string(),
                |name| format!("__partial_preset_{name}"),
            );
            let previous = self.bindings.get(&name).copied();
            let local = self.declare_new_local_with_reads(name.clone(), ty, frame.scope, span, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Captured;
            frame.operands.push(operand);
            frame.captures.push(local);
            frame.saved.push((name, previous));
            match preset {
                Some(name) => {
                    frame.presets.insert(name, local);
                }
                None => frame.receiver = Some(local),
            }
        }
        frame
    }

    /// Materialize a checked value immediately, so later presets cannot change a retained place before it is read.
    ///
    /// The enclosing scope owns this temporary until the closure consumes it. This preserves the source read's
    /// checked Clone/Move/Borrow contract and introduces no synthetic name visible to subsequent source expressions.
    pub(super) fn snapshot_partial_value(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> (IncanType, bir::Operand) {
        let ty = self.resolve_ty(expression.span);
        let value = self.lower_expr_to_operand(expression, scope, out);
        let snapshot = self.push_assign_temp(
            bir::Rvalue::Use(value),
            ty.clone(),
            scope,
            hir_span(expression.span),
            out,
        );
        (ty, snapshot)
    }

    /// Retain full declared slots with overrideable preset defaults and refuse unavailable method source defaults.
    fn partial_parameters(
        &mut self,
        target: &PartialTarget<'_>,
        checked: &[IncanCallableParam],
        frame: &mut PartialFrame,
        span: HirSourceSpan,
    ) -> Vec<bir::CallableParam> {
        let mut params = Vec::with_capacity(target.params.len());
        for (index, param) in target.params.iter().enumerate() {
            let Some(name) = &param.name else {
                continue;
            };
            let ty = checked
                .get(index)
                .map(|param| param.ty.clone())
                .unwrap_or_else(|| self.checked_type(&param.ty));
            let ty = super::variadics::parameter_storage_type(ty, param.kind);
            let previous = self.bindings.get(name).copied();
            let local = self.declare_new_local_with_reads(name.clone(), ty.clone(), frame.scope, span, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            let source = target.defaults.as_ref().and_then(|defaults| defaults.get(index));
            let default = match frame.presets.get(name).copied() {
                Some(capture) => bir::CallableParamDefault::PartialPreset { capture },
                None => match source {
                    Some(source) => self.lower_callable_default(source.default.as_ref(), frame.scope),
                    None if param.has_default => bir::CallableParamDefault::Unsupported {
                        span,
                        description: format!(
                            "partial target {} declares a default Body IR could not source",
                            target.name
                        ),
                    },
                    None => bir::CallableParamDefault::Required,
                },
            };
            params.push(bir::CallableParam {
                local,
                kind: parameter_types::source_parameter_kind(param.kind),
                name: name.clone(),
                ty,
                span: source.map_or(span, |source| hir_span(source.param_span)),
                default,
                mutable: param.is_mut,
                native_owned: parameter_types::native_collection_parameter(&param.ty, self.type_info),
            });
            if params.last().is_some_and(|parameter| parameter.native_owned) {
                self.owned_parameters.insert(local);
            } else if param.is_mut {
                self.borrowed_parameters.insert(local);
            }
            frame.saved.push((name.clone(), previous));
        }
        params
    }
}
