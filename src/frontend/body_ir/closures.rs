//! Lowering for closures and partial applications, including how each computes and represents its captures.

use super::args::*;
use super::free_vars::*;
use super::reads::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a closure literal `(params) => expr` into a [`bir::Rvalue::Closure`].
    ///
    /// Body IR must represent captures explicitly rather than deferring to a consuming backend's own closure syntax
    /// to auto-capture (see this module's docs and #1101's B4 pre-intake), so this: (1) statically determines every
    /// free variable the closure body reads via [`free_vars_in_closure_body`]; (2) reads each one exactly once, at
    /// this closure-creation site, through the same [`Self::ownership_fact_for_place`] path any other read in this
    /// body uses, recording the result as this closure's `captured_operands`; (3) declares a fresh
    /// [`bir::LocalOrigin::Captured`] local per capture plus one [`bir::LocalOrigin::Parameter`] local per declared
    /// parameter, shadowing (and restoring afterward) any outer binding of the same name, so the closure body's own
    /// reads resolve to its own bound copy rather than silently reading through to the enclosing scope; then (4)
    /// lowers the body expression under those bindings. The restore step is what makes this different from every
    /// other nested block this file lowers -- ordinary nested blocks (`if`/`loop` bodies) let a shadowing binding
    /// leak forward in `self.bindings` with no restore, which is harmless for straight-line control flow but would
    /// be wrong here: code lexically after the closure literal must keep resolving the shadowed name to the
    /// *enclosing* variable, not to the closure's own captured copy.
    pub(super) fn lower_closure(
        &mut self,
        params: &[ast::Spanned<ast::Param>],
        body_expr: &ast::Spanned<ast::Expr>,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        let closure_scope = self.new_scope(Some(scope), hir_span_value);

        // ---- Capture every free variable exactly once, at this closure-creation site ----
        let free_names = free_vars_in_closure_body(params, body_expr);
        let mut captured_operands = Vec::with_capacity(free_names.len());
        let mut capture_locals = Vec::with_capacity(free_names.len());
        let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
        let enclosing_identity_bindings = self.identity_bindings.clone();
        for name in &free_names {
            // A free name that does not resolve to a tracked outer local (for example module storage) is not
            // captured. The closure body resolves that source reference independently from its canonical identity,
            // yielding a global place for a `const`/`static` or an explicit unsupported operand for an identity
            // Body IR cannot represent.
            let Some(&outer_local) = self.bindings.get(name) else {
                continue;
            };
            let outer_ty = self.locals[outer_local.index()].ty.clone();
            let outer_place = bir::Place::from_local(outer_local);
            let (fact, last_use) = self.ownership_fact_for_place(&outer_place, &outer_ty);
            captured_operands.push(bir::Operand::place(outer_place, fact, last_use));

            let total_reads = count_reads_in_expr(name, &body_expr.node);
            let capture_local =
                self.declare_new_local_with_reads(name.clone(), outer_ty, closure_scope, hir_span_value, total_reads);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            if let Some(identity) = self.locals[outer_local.index()].identity.clone() {
                self.locals[capture_local.index()].identity = Some(identity.clone());
                self.identity_bindings.insert(identity, capture_local);
            }
            capture_locals.push(capture_local);
            saved_bindings.push((name.clone(), Some(outer_local)));
        }

        // ---- Bind the closure's own parameters, shadowing any outer binding of the same name ----
        let param_types = self.closure_param_types(params, expr_span);
        let mut closure_param_locals = Vec::with_capacity(params.len());
        for (param, ty) in params.iter().zip(param_types) {
            let previous = self.bindings.get(&param.node.name).copied();
            let total_reads = count_reads_in_expr(&param.node.name, &body_expr.node);
            let local = self.declare_new_local_with_reads(
                param.node.name.clone(),
                ty.clone(),
                closure_scope,
                hir_span(param.span),
                total_reads,
            );
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            closure_param_locals.push(local);
            saved_bindings.push((param.node.name.clone(), previous));
        }

        let mut closure_params = Vec::with_capacity(params.len());
        for (param, local) in params.iter().zip(closure_param_locals) {
            let ty = self.locals[local.index()].ty.clone();
            closure_params.push(bir::CallableParam {
                local,
                name: param.node.name.clone(),
                ty,
                span: hir_span(param.span),
                default: self.lower_callable_default(param.node.default.as_ref(), closure_scope),
            });
        }

        // ---- Lower the body under the closure's own bindings, then restore the enclosing scope's ----
        // The closure body is deferred. Its assignments must not update the enclosing body's proof that a local
        // currently has the source-local range aggregate layout; its captured/writeable locals are a distinct
        // invocation frame.
        let saved_materialized_range_locals = self.materialized_range_locals.clone();
        let mut body_stmts = Vec::new();
        let result = self.lower_expr_to_operand(body_expr, closure_scope, &mut body_stmts);
        for (name, previous) in saved_bindings {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }
        self.identity_bindings = enclosing_identity_bindings;
        self.materialized_range_locals = saved_materialized_range_locals;

        let closure_body = bir::ClosureBody {
            capture_locals,
            stmts: body_stmts,
            result,
        };
        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params: closure_params,
                captured_operands,
                body: Box::new(closure_body),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Resolve each of a closure literal's parameter types from the typechecker's resolved callable type at the
    /// closure's own span, falling back to [`IncanType::Unknown`] per parameter when unavailable or of mismatched
    /// length. Mirrors the existing Rust-emission backend's own `recorded_param_types` fallback
    /// (`src/backend/ir/lower/expr/mod.rs`), minus that backend's additional Rust-display-exact override, which is
    /// meaningful only for concrete Rust closure syntax, not this target-agnostic model.
    pub(super) fn closure_param_types(
        &self,
        params: &[ast::Spanned<ast::Param>],
        expr_span: ast::Span,
    ) -> Vec<IncanType> {
        let resolved = self.type_info.expr_type(expr_span).and_then(|ty| match ty {
            ResolvedType::Function(callable_params, _) => Some(
                callable_params
                    .iter()
                    .map(|p| semantic_type_from_resolved(&p.ty))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
        match resolved {
            Some(types) if types.len() == params.len() => types,
            _ => vec![IncanType::Unknown; params.len()],
        }
    }

    /// Lower a partial callable preset expression (`partial Target(name=value, ...)`) into the same
    /// [`bir::Rvalue::Closure`] shape a closure literal produces, mirroring how the existing Rust-emission backend
    /// already desugars a partial application into a synthesized closure that forwards the still-missing arguments
    /// into a call (`src/backend/ir/lower/expr/mod.rs`'s `ast::Expr::Partial` arm) -- see #1101's B4 pre-intake.
    /// Partial construction currently supports only a bare top-level function-name `target` whose full parameter list
    /// the typechecker resolved. General Body IR calls still distinguish named functions from local callable values
    /// and record local supplied-parameter slots (see [`Self::lower_call`]). A method-shaped partial target from
    /// `partial recv.method(...)`, explicit type arguments, or a target with an unnamed parameter lowers to an
    /// explicit unsupported placeholder instead.
    ///
    /// Preset values (`partial.args`) are lowered once each, at the partial-creation site -- exactly like an
    /// ordinary call argument, not deduplicated per free-variable name the way [`Self::lower_closure`]'s captures
    /// are -- and folded into the synthesized closure's own `captured_operands`. Every declared target parameter
    /// remains a closure parameter in declaration order. A preset parameter records
    /// [`bir::CallableParamDefault::PartialPreset`], while an unpresetted target default retains its distinct
    /// source-default contract: a deferred [`bir::CallableParamDefault::Source`] computation only when it has
    /// usable type facts, otherwise an original-span refusal. Positional local calls skip only preset parameters;
    /// [`Self::lower_call`] records the supplied declaration slots rather than pretending the complete callable
    /// surface is a residual function type.
    ///
    /// `Expr::Partial` uses this same full callable surface through `local_partial_params`; module-level partial
    /// declarations intentionally keep their existing full-signature-plus-preset-metadata projection for backend
    /// and export consumers. A compound-assignment-style mutation of a captured preset from inside a nested closure
    /// is out of scope here in the same way [`Self::lower_closure`]'s own docs note for ordinary closures.
    pub(super) fn lower_partial(
        &mut self,
        partial: &ast::PartialExpr,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        let ast::Expr::Ident(target_name) = &partial.target.node else {
            return self.unsupported_operand(
                "partial callable with a non-function-name target".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if !partial.type_args.is_empty() {
            return self.unsupported_operand(
                "partial callable with explicit type arguments".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let Some(binding) = self.type_info.declarations.function_bindings.get(target_name).cloned() else {
            return self.unsupported_operand(
                "partial callable target with no resolvable top-level function signature".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if binding
            .params
            .iter()
            .any(|param| param.name.is_none() || param.kind != ast::ParamKind::Normal)
        {
            return self.unsupported_operand(
                "partial callable target with an unnamed or rest parameter".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let target_name = target_name.clone();
        let target_canonical = binding.identity.clone();
        let direct_call_id = self
            .local_function_declarations
            .get(&target_name)
            .and_then(|candidates| match candidates.as_slice() {
                [target_span] => Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    target_span.start,
                    target_span.end,
                )),
                _ => None,
            });
        let target_default_sources = self.function_default_sources.get(&target_name).cloned();
        let closure_scope = self.new_scope(Some(scope), hir_span_value);

        // ---- Lower each preset value once, at the partial-creation site, as a captured operand ----
        let mut captured_operands = Vec::with_capacity(partial.args.len());
        let mut capture_locals = Vec::with_capacity(partial.args.len());
        let mut preset_lookup: HashMap<String, bir::LocalId> = HashMap::with_capacity(partial.args.len());
        let mut saved_bindings = Vec::with_capacity(binding.params.len() + partial.args.len());
        for arg in &partial.args {
            let value_ty = self.resolve_ty(arg.value.span);
            let operand = self.lower_expr_to_operand(&arg.value, scope, out);
            captured_operands.push(operand);
            let capture_name = format!("__partial_preset_{}", arg.name);
            let previous = self.bindings.get(&capture_name).copied();
            let capture_local =
                self.declare_new_local_with_reads(capture_name.clone(), value_ty, closure_scope, hir_span_value, 1);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            capture_locals.push(capture_local);
            preset_lookup.insert(arg.name.clone(), capture_local);
            saved_bindings.push((capture_name, previous));
        }

        // ---- Every target parameter stays on the closure surface; presets become overrideable defaults ----
        let mut closure_params = Vec::new();
        let mut call_arg_locals = Vec::with_capacity(binding.params.len());
        for (index, param) in binding.params.iter().enumerate() {
            let Some(param_name) = &param.name else {
                return self.unsupported_operand(
                    "partial callable target with an unnamed parameter".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let ty = semantic_type_from_resolved(&param.ty);
            let previous = self.bindings.get(param_name).copied();
            let local =
                self.declare_new_local_with_reads(param_name.clone(), ty.clone(), closure_scope, hir_span_value, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            let source_param = target_default_sources.as_ref().and_then(|params| params.get(index));
            let default = match preset_lookup.get(param_name).copied() {
                Some(capture) => bir::CallableParamDefault::PartialPreset { capture },
                None => match source_param {
                    Some(source_param) => self.lower_callable_default(source_param.default.as_ref(), closure_scope),
                    None if param.has_default => bir::CallableParamDefault::Unsupported {
                        span: hir_span_value,
                        description: format!(
                            "partial target {target_name} declares a default Body IR could not source"
                        ),
                    },
                    None => bir::CallableParamDefault::Required,
                },
            };
            closure_params.push(bir::CallableParam {
                local,
                name: param_name.clone(),
                ty,
                span: source_param.map_or(hir_span_value, |param| hir_span(param.param_span)),
                default,
            });
            call_arg_locals.push(local);
            saved_bindings.push((param_name.clone(), previous));
        }

        // ---- Synthesize the forwarding call as the closure's single-statement body ----
        let mut body_stmts = Vec::new();
        let call_args: Vec<bir::Operand> = call_arg_locals
            .iter()
            .zip(&binding.params)
            .map(|(&local, param)| {
                let ty = semantic_type_from_resolved(&param.ty);
                let place = bir::Place::from_local(local);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            })
            .collect();
        let ret_ty = semantic_type_from_resolved(&binding.return_type);
        // The synthesized forwarding call supplies every declared parameter of the target, in declaration order:
        // preset slots are filled from the captured locals and residual slots from the closure's own parameters.
        let forwarding_binding = bir::ArgumentBinding::resolved_positional(call_args.len());
        let result = self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name: target_name,
                direct_call_id,
                builtin: None,
                // The forwarding call has no source call site of its own, but the partial target was checked as a
                // declaration before synthesis. Carry that declaration identity forward; `direct_call_id` is only
                // its physical same-module representation and cannot replace semantic authority.
                canonical: target_canonical,
                type_args: Vec::new(),
                binding: forwarding_binding,
            })),
            fixed_elements(call_args),
            ret_ty,
            closure_scope,
            hir_span_value,
            false,
            &mut body_stmts,
        );

        let closure_body = bir::ClosureBody {
            capture_locals,
            stmts: body_stmts,
            result,
        };

        // ---- The synthesized closure's bindings are lexically private to it, not new outer bindings ----
        for (name, previous) in saved_bindings.into_iter().rev() {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }

        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params: closure_params,
                captured_operands,
                body: Box::new(closure_body),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }
}
