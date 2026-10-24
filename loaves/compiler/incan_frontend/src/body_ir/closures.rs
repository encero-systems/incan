//! Lowering for closures and partial applications, including how each computes and represents its captures.

use super::free_vars::*;
use super::reads::*;
use super::*;
use incan_lang::lang::builtins::BuiltinFnId;

/// Follow straight-line whole-local moves to a checked canonical zip producer, refusing intervening control flow.
fn canonical_zip_local(mut local: bir::LocalId, mut statements: &[bir::Statement]) -> bool {
    loop {
        let mut predecessor = None;
        for (index, statement) in statements.iter().enumerate().rev() {
            match &statement.kind {
                bir::StatementKind::Call {
                    destination: Some(destination),
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if destination == &bir::Place::from_local(local) => return target.builtin == Some(BuiltinFnId::Zip),
                bir::StatementKind::Call {
                    destination: Some(destination),
                    ..
                } if destination.local_id() == Some(local) => return false,
                bir::StatementKind::Assign { place, rvalue } if place == &bir::Place::from_local(local) => {
                    if let bir::Rvalue::Use(bir::Operand::Place(read)) = rvalue
                        && read.place.projection.is_empty()
                        && let Some(source) = read.place.local_id()
                    {
                        predecessor = Some((source, index));
                    }
                    break;
                }
                bir::StatementKind::Assign { place, .. } if place.local_id() == Some(local) => return false,
                bir::StatementKind::If { .. }
                | bir::StatementKind::Loop { .. }
                | bir::StatementKind::UnsafeRegion { .. }
                | bir::StatementKind::Race { .. } => return false,
                _ => {}
            }
        }
        let Some((source, index)) = predecessor else {
            return false;
        };
        local = source;
        statements = &statements[..index];
    }
}

/// Retain consuming authority only when a captured canonical zip moves into a deferred generator source.
fn consumes_zip_environment(
    captures: &[bir::Operand],
    capture_locals: &[bir::LocalId],
    body: &[bir::Statement],
    prefix: &[bir::Statement],
) -> bool {
    body.iter().any(|statement| {
        let bir::StatementKind::Assign {
            rvalue:
                bir::Rvalue::Generator {
                    source: bir::Operand::Place(source),
                    ..
                },
            ..
        } = &statement.kind
        else {
            return false;
        };
        if source.fact != bir::OwnershipFact::Move || !source.place.projection.is_empty() {
            return false;
        }
        let Some(local) = source.place.local_id() else {
            return false;
        };
        let Some(index) = capture_locals.iter().position(|capture| *capture == local) else {
            return false;
        };
        let Some(bir::Operand::Place(outer)) = captures.get(index) else {
            return false;
        };
        if !outer.place.projection.is_empty() {
            return false;
        }
        outer
            .place
            .local_id()
            .is_some_and(|outer| canonical_zip_local(outer, prefix))
    })
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Mark only a resolver-proven named function read, preserving its checked target rather than identifying it by
    /// the shape of an arbitrary source closure. The forwarding body remains available to older Body IR consumers.
    pub(super) fn retain_function_item(&self, statements: &mut [bir::Statement]) {
        let Some(bir::Statement {
            kind:
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Closure { body, .. },
                    ..
                },
            ..
        }) = statements.last_mut()
        else {
            return;
        };
        if let [
            bir::Statement {
                kind:
                    bir::StatementKind::Call {
                        callee: bir::Callee::Function(bir::CallableTarget::Named(_target)),
                        ..
                    },
                ..
            },
        ] = body.stmts.as_slice()
        {
            body.function_item = true;
        }
    }

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
                kind: parameter_types::source_parameter_kind(param.node.kind),
                name: param.node.name.clone(),
                ty,
                span: hir_span(param.span),
                default: self.lower_callable_default(param.node.default.as_ref(), closure_scope),
                mutable: param.node.is_mut,
                native_owned: self
                    .type_info
                    .expr_type(param.span)
                    .is_some_and(|ty| parameter_types::native_collection_parameter(ty, self.type_info)),
            });
        }
        self.owned_parameters.extend(
            closure_params
                .iter()
                .filter(|parameter| parameter.native_owned)
                .map(|parameter| parameter.local),
        );

        // ---- Lower the body under the closure's own bindings, then restore the enclosing scope's ----
        // The closure body is deferred. Its assignments must not update the enclosing body's proof that a local
        // currently has the source-local range aggregate layout; its captured/writeable locals are a distinct
        // invocation frame.
        let saved_materialized_range_locals = self.materialized_range_locals.clone();
        // An `Ok(...)` or `Err(...)` the body returns is built with the result type the checker settled for the
        // closure, which fixes the side the constructor leaves open.
        if let IncanType::Function { return_type, .. } = self.resolve_ty(expr_span) {
            self.settle_open_result_value(body_expr, &return_type);
        }
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
            native_storage: self
                .type_info
                .rust
                .closure_storage
                .get(&(expr_span.start, expr_span.end))
                .cloned(),
            original_callable: None,
            function_item: false,
            consumes_environment: consumes_zip_environment(&captured_operands, &capture_locals, &body_stmts, out),
            capture_locals,
            stmts: body_stmts,
            result,
        };
        let ty = self.retain_closure_result_channels(expr_span, &closure_body.result);
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
    /// (`loaves/compiler/incan_ir/src/lower/expr/mod.rs`), minus that backend's additional Rust-display-exact override,
    /// which is meaningful only for concrete Rust closure syntax, not this target-agnostic model.
    pub(super) fn closure_param_types(
        &self,
        params: &[ast::Spanned<ast::Param>],
        expr_span: ast::Span,
    ) -> Vec<IncanType> {
        let resolved = match self.resolve_ty(expr_span) {
            IncanType::Function {
                params: callable_params,
                ..
            } => Some(callable_params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>()),
            _ => None,
        };
        match resolved {
            Some(types) if types.len() == params.len() => types,
            _ => vec![IncanType::Unknown; params.len()],
        }
    }

    /// Lower a local partial using the checker's target identity and full parameter surface.
    ///
    /// A retained same-module constructor uses its checked field layout; functions and methods keep their existing
    /// forwarding frames. Checked newtype ingress remains refused until its deferred validation operations are
    /// retained.
    ///
    /// A method receiver and each preset are evaluated once before any deferred-frame binding is introduced.
    /// Forwarding borrows the captured receiver, retains named overrideable presets, and preserves source defaults
    /// only when their declaration-owned computations are available.
    /// Independently checked module/function identities select the existing capture-free module forwarding path,
    /// without treating a namespace as a value receiver.
    pub(super) fn lower_partial(
        &mut self,
        partial: &ast::PartialExpr,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let module_function = match &partial.target.node {
            ast::Expr::Field(receiver, _) => {
                self.type_info
                    .resolved_identity(receiver.span)
                    .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
                    && self
                        .type_info
                        .resolved_identity(partial.target.span)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function)
            }
            _ => false,
        };
        if module_function
            || self
                .type_info
                .resolved_identity(partial.target.span)
                .is_some_and(|identity| {
                    matches!(
                        identity.kind,
                        SemanticSourceTargetKind::Model
                            | SemanticSourceTargetKind::Class
                            | SemanticSourceTargetKind::Newtype
                    )
                })
        {
            return self.lower_local_partial(partial, expr_span, scope, out);
        }
        match self.partial_target(partial, expr_span) {
            Ok(target) => self.lower_partial_target(partial, target, expr_span, scope, out),
            Err(description) => self.unsupported_operand(description, scope, hir_span(expr_span), out),
        }
    }
}
