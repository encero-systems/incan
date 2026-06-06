//! Function declaration lowering.

use super::super::super::Mutability;
use super::super::super::decl::{FunctionParam, IrFunction, IrTraitBound, IrTraitBoundOrigin};
use super::super::super::expr::{IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry};
use super::super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, DecoratorArg, DecoratorArgValue, Expr, ImportPath, Spanned};
use incan_core::lang::types::collections::{self, CollectionTypeId};

/// Return whether a lowered callable return type is the canonical `Generator[...]` wrapper.
fn return_type_is_generator(ty: &IrType) -> bool {
    matches!(ty, IrType::NamedGeneric(name, _)
        if collections::from_str(name.as_str()) == Some(CollectionTypeId::Generator))
}

/// Return whether a function body contains a source `yield` expression.
fn body_contains_yield(body: &[ast::Spanned<ast::Statement>]) -> bool {
    body.iter().any(|stmt| match &stmt.node {
        ast::Statement::Expr(expr) | ast::Statement::Return(Some(expr)) => matches!(expr.node, ast::Expr::Yield(_)),
        ast::Statement::If(stmt) => {
            body_contains_yield(&stmt.then_body)
                || stmt.elif_branches.iter().any(|(_, body)| body_contains_yield(body))
                || stmt.else_body.as_ref().is_some_and(|body| body_contains_yield(body))
        }
        ast::Statement::Loop(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::While(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::For(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::VocabBlock(stmt) => body_contains_yield(&stmt.body),
        _ => false,
    })
}

/// Extract the leading function docstring expression, using the same source convention as API metadata extraction.
pub(in crate::backend::ir::lower) fn callable_docstring(body: &[Spanned<ast::Statement>]) -> Option<String> {
    let first = body.first()?;
    let ast::Statement::Expr(expr) = &first.node else {
        return None;
    };
    let Expr::Literal(ast::Literal::String(docstring)) = &expr.node else {
        return None;
    };
    Some(docstring.clone())
}

/// Collect generic callable-name type parameters referenced by an expression.
fn collect_generic_callable_name_type_params_from_expr(expr: &super::super::super::IrExpr, out: &mut Vec<String>) {
    match &expr.kind {
        IrExprKind::Field { object, field } => {
            if field == "__name__"
                && let IrType::Generic(name) = &object.ty
                && !out.contains(name)
            {
                out.push(name.clone());
            }
            collect_generic_callable_name_type_params_from_expr(object, out);
        }
        IrExprKind::BinOp { left, right, .. } => {
            collect_generic_callable_name_type_params_from_expr(left, out);
            collect_generic_callable_name_type_params_from_expr(right, out);
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::NumericResize { expr: operand, .. }
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => {
            collect_generic_callable_name_type_params_from_expr(operand, out);
        }
        IrExprKind::Call { func, args, .. } => {
            collect_generic_callable_name_type_params_from_expr(func, out);
            for arg in args {
                collect_generic_callable_name_type_params_from_expr(&arg.expr, out);
            }
        }
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                collect_generic_callable_name_type_params_from_expr(arg, out);
            }
        }
        IrExprKind::KnownMethodCall { args, .. } => {
            for arg in args {
                collect_generic_callable_name_type_params_from_expr(&arg.expr, out);
            }
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            collect_generic_callable_name_type_params_from_expr(receiver, out);
            for arg in args {
                collect_generic_callable_name_type_params_from_expr(&arg.expr, out);
            }
        }
        IrExprKind::Index { object, index } => {
            collect_generic_callable_name_type_params_from_expr(object, out);
            collect_generic_callable_name_type_params_from_expr(index, out);
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            collect_generic_callable_name_type_params_from_expr(target, out);
            for expr in [start, end, step].into_iter().flatten() {
                collect_generic_callable_name_type_params_from_expr(expr, out);
            }
        }
        IrExprKind::ListComp {
            element,
            iterable,
            filter,
            ..
        } => {
            collect_generic_callable_name_type_params_from_expr(element, out);
            collect_generic_callable_name_type_params_from_expr(iterable, out);
            if let Some(filter) = filter {
                collect_generic_callable_name_type_params_from_expr(filter, out);
            }
        }
        IrExprKind::DictComp {
            key,
            value,
            iterable,
            filter,
            ..
        } => {
            collect_generic_callable_name_type_params_from_expr(key, out);
            collect_generic_callable_name_type_params_from_expr(value, out);
            collect_generic_callable_name_type_params_from_expr(iterable, out);
            if let Some(filter) = filter {
                collect_generic_callable_name_type_params_from_expr(filter, out);
            }
        }
        IrExprKind::Generator { element, clauses } => {
            collect_generic_callable_name_type_params_from_expr(element, out);
            for clause in clauses {
                match clause {
                    IrGeneratorClause::For { iterable, .. } => {
                        collect_generic_callable_name_type_params_from_expr(iterable, out);
                    }
                    IrGeneratorClause::If(condition) => {
                        collect_generic_callable_name_type_params_from_expr(condition, out);
                    }
                }
            }
        }
        IrExprKind::List(items) => {
            for item in items {
                match item {
                    IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                        collect_generic_callable_name_type_params_from_expr(value, out);
                    }
                }
            }
        }
        IrExprKind::Dict(items) => {
            for item in items {
                match item {
                    IrDictEntry::Pair(key, value) => {
                        collect_generic_callable_name_type_params_from_expr(key, out);
                        collect_generic_callable_name_type_params_from_expr(value, out);
                    }
                    IrDictEntry::Spread(value) => {
                        collect_generic_callable_name_type_params_from_expr(value, out);
                    }
                }
            }
        }
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
            for item in items {
                collect_generic_callable_name_type_params_from_expr(item, out);
            }
        }
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                collect_generic_callable_name_type_params_from_expr(value, out);
            }
        }
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_generic_callable_name_type_params_from_expr(condition, out);
            collect_generic_callable_name_type_params_from_expr(then_branch, out);
            if let Some(else_branch) = else_branch {
                collect_generic_callable_name_type_params_from_expr(else_branch, out);
            }
        }
        IrExprKind::Match { scrutinee, arms } => {
            collect_generic_callable_name_type_params_from_expr(scrutinee, out);
            for arm in arms {
                for binding in &arm.bindings {
                    collect_generic_callable_name_type_params_from_expr(&binding.value, out);
                    if let Some(guard_value) = &binding.guard_value {
                        collect_generic_callable_name_type_params_from_expr(guard_value, out);
                    }
                }
                if let Some(guard) = &arm.guard {
                    collect_generic_callable_name_type_params_from_expr(guard, out);
                }
                collect_generic_callable_name_type_params_from_expr(&arm.body, out);
            }
        }
        IrExprKind::Closure { body, .. } => {
            collect_generic_callable_name_type_params_from_expr(body, out);
        }
        IrExprKind::Block { stmts, value } => {
            collect_generic_callable_name_type_params_from_stmts(stmts, out);
            if let Some(value) = value {
                collect_generic_callable_name_type_params_from_expr(value, out);
            }
        }
        IrExprKind::Loop { body } => collect_generic_callable_name_type_params_from_stmts(body, out),
        IrExprKind::Race { arms, .. } => {
            for arm in arms {
                collect_generic_callable_name_type_params_from_expr(&arm.awaitable, out);
                collect_generic_callable_name_type_params_from_expr(&arm.body, out);
            }
        }
        IrExprKind::Range { start, end, .. } => {
            for expr in [start, end].into_iter().flatten() {
                collect_generic_callable_name_type_params_from_expr(expr, out);
            }
        }
        IrExprKind::Format { parts } => {
            for part in parts {
                if let super::super::super::expr::FormatPart::Expr { expr, .. } = part {
                    collect_generic_callable_name_type_params_from_expr(expr, out);
                }
            }
        }
        IrExprKind::RegisterCallableName { callable, .. } => {
            collect_generic_callable_name_type_params_from_expr(callable, out);
        }
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
            collect_generic_callable_name_type_params_from_expr(value, out);
        }
        IrExprKind::Var { .. }
        | IrExprKind::StaticRead { .. }
        | IrExprKind::StaticBinding { .. }
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => {}
    }
}

/// Collect generic callable-name type parameters referenced by statements.
fn collect_generic_callable_name_type_params_from_stmts(stmts: &[IrStmt], out: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Expr(expr)
            | IrStmtKind::Yield(expr)
            | IrStmtKind::Let { value: expr, .. }
            | IrStmtKind::CompoundAssign { value: expr, .. } => {
                collect_generic_callable_name_type_params_from_expr(expr, out);
            }
            IrStmtKind::Assign { target, value } => {
                collect_generic_callable_name_type_params_from_assign_target(target, out);
                collect_generic_callable_name_type_params_from_expr(value, out);
            }
            IrStmtKind::Return(Some(expr)) => collect_generic_callable_name_type_params_from_expr(expr, out),
            IrStmtKind::Break { value: Some(expr), .. } => {
                collect_generic_callable_name_type_params_from_expr(expr, out);
            }
            IrStmtKind::While { condition, body, .. } => {
                collect_generic_callable_name_type_params_from_expr(condition, out);
                collect_generic_callable_name_type_params_from_stmts(body, out);
            }
            IrStmtKind::For { iterable, body, .. } => {
                collect_generic_callable_name_type_params_from_expr(iterable, out);
                collect_generic_callable_name_type_params_from_stmts(body, out);
            }
            IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => {
                collect_generic_callable_name_type_params_from_stmts(body, out);
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                collect_generic_callable_name_type_params_from_expr(condition, out);
                collect_generic_callable_name_type_params_from_stmts(then_branch, out);
                if let Some(else_branch) = else_branch {
                    collect_generic_callable_name_type_params_from_stmts(else_branch, out);
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                collect_generic_callable_name_type_params_from_expr(scrutinee, out);
                for arm in arms {
                    for binding in &arm.bindings {
                        collect_generic_callable_name_type_params_from_expr(&binding.value, out);
                        if let Some(guard_value) = &binding.guard_value {
                            collect_generic_callable_name_type_params_from_expr(guard_value, out);
                        }
                    }
                    if let Some(guard) = &arm.guard {
                        collect_generic_callable_name_type_params_from_expr(guard, out);
                    }
                    collect_generic_callable_name_type_params_from_expr(&arm.body, out);
                }
            }
            IrStmtKind::Return(None) | IrStmtKind::Break { value: None, .. } | IrStmtKind::Continue(_) => {}
        }
    }
}

/// Collect generic callable-name type parameters referenced by an assignment target.
fn collect_generic_callable_name_type_params_from_assign_target(target: &AssignTarget, out: &mut Vec<String>) {
    match target {
        AssignTarget::Field { object, .. } => collect_generic_callable_name_type_params_from_expr(object, out),
        AssignTarget::Index { object, index } => {
            collect_generic_callable_name_type_params_from_expr(object, out);
            collect_generic_callable_name_type_params_from_expr(index, out);
        }
        AssignTarget::Var(_) | AssignTarget::StaticBinding(_) | AssignTarget::Static(_) => {}
    }
}

impl AstLowering {
    /// Lower a function declaration.
    ///
    /// # Parameters
    ///
    /// * `f` - The AST function declaration
    ///
    /// # Returns
    ///
    /// The corresponding IR function.
    pub(in crate::backend::ir::lower) fn lower_function(
        &mut self,
        f: &ast::FunctionDecl,
    ) -> Result<IrFunction, LoweringError> {
        self.lower_function_named(f, f.name.clone(), self.map_callable_visibility(f.visibility))
    }

    /// Lower a function declaration using an explicit emitted name and visibility.
    pub(in crate::backend::ir::lower) fn lower_function_named(
        &mut self,
        f: &ast::FunctionDecl,
        name: String,
        visibility: super::super::super::decl::Visibility,
    ) -> Result<IrFunction, LoweringError> {
        self.push_scope();

        let type_param_names: std::collections::HashSet<&str> =
            f.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let mut hidden_type_params = Vec::new();
        let mut hidden_counter = 0usize;

        let mut params: Vec<FunctionParam> = f
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_callable_param_type(
                    &p.node.ty.node,
                    Some(&type_param_names),
                    &mut hidden_type_params,
                    &mut hidden_counter,
                );
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                // For mutable parameters, wrap in RefMut to track that it's a &mut reference
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(param_ty.clone()))
                } else {
                    param_ty.clone()
                };
                self.define_local_binding(p.node.name.clone(), ty.clone(), false);
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty, // Store the emitted parameter type (emit will add &mut for mutable params)
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
                    kind: p.node.kind,
                    default: match &p.node.default {
                        Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                        None => None,
                    },
                }
            })
            .collect();

        let return_type = self.lower_callable_return_type(&f.return_type.node, Some(&type_param_names));
        let is_generator = return_type_is_generator(&return_type) && body_contains_yield(&f.body);
        self.push_callable_param_scope(&params);
        let body_result = self.lower_statements(&f.body);
        if body_result.is_ok() {
            for param in &mut params {
                if matches!(param.ty, IrType::Function { .. }) {
                    let refined_ty = self.lookup_var(&param.name);
                    if matches!(refined_ty, IrType::Function { .. }) {
                        param.ty = refined_ty;
                    }
                }
            }
        }
        self.pop_callable_param_scope();
        let body = match body_result {
            Ok(body) => body,
            Err(err) => {
                self.pop_scope();
                return Err(err);
            }
        };
        self.pop_scope();

        // RFC 023: detect @rust.extern decorator to mark this function as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&f.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&f.decorators);
        let lint_allows = self.extract_rust_lint_allows(&f.decorators);

        let mut all_type_params = Self::lower_type_params(&f.type_params);
        all_type_params.extend(hidden_type_params);
        let mut callable_name_type_params = Vec::new();
        collect_generic_callable_name_type_params_from_stmts(&body, &mut callable_name_type_params);
        for type_param_name in callable_name_type_params {
            if let Some(type_param) = all_type_params
                .iter_mut()
                .find(|type_param| type_param.name == type_param_name)
                && !type_param.bounds.iter().any(|bound| {
                    bound.trait_path == "__IncanCallableName"
                        && bound.type_args.is_empty()
                        && bound.assoc_types.is_empty()
                })
            {
                type_param.bounds.push(IrTraitBound::simple("__IncanCallableName"));
            }
        }
        if is_generator {
            for type_param in &mut all_type_params {
                for trait_path in ["Send", "Static"] {
                    if !type_param.bounds.iter().any(|bound| {
                        bound.origin == IrTraitBoundOrigin::RustCapability && bound.trait_path == trait_path
                    }) {
                        type_param.bounds.push(IrTraitBound {
                            trait_path: trait_path.to_string(),
                            type_args: Vec::new(),
                            assoc_types: Vec::new(),
                            origin: IrTraitBoundOrigin::RustCapability,
                        });
                    }
                }
            }
        }

        Ok(IrFunction {
            name,
            docstring: callable_docstring(&f.body),
            params,
            return_type,
            body,
            is_async: f.is_async(),
            is_generator,
            visibility,
            type_params: all_type_params,
            is_extern,
            rust_attributes,
            lint_allows,
        })
    }

    /// Return the private emitted function name that stores an undecorated original.
    pub(in crate::backend::ir::lower) fn decorator_original_function_name(name: &str) -> String {
        format!("__incan_original_{name}")
    }

    /// Return the private emitted static name that stores the decorated callable binding.
    pub(in crate::backend::ir::lower) fn decorator_static_binding_name(name: &str) -> String {
        format!("__incan_decorated_{name}")
    }

    /// Return the span used for synthetic decorator callee nodes.
    ///
    /// The full decorator factory call keeps the source decorator span for typechecker handoff. Nested synthetic
    /// callees must not reuse that span because expression metadata is span-keyed and the factory result type would
    /// otherwise overwrite the callee's callable signature during lowering.
    pub(in crate::backend::ir::lower) fn decorator_synthetic_callee_span() -> ast::Span {
        ast::Span::default()
    }

    /// Build an expression that resolves a decorator's path through ordinary expression lowering.
    pub(in crate::backend::ir::lower) fn decorator_path_expr(
        decorator: &ast::Decorator,
        span: ast::Span,
    ) -> Spanned<Expr> {
        Self::decorator_path_expr_from_import_path(&decorator.path, span)
    }

    /// Convert an import-like decorator path into chained identifier/field expressions.
    pub(in crate::backend::ir::lower) fn decorator_path_expr_from_import_path(
        path: &ImportPath,
        span: ast::Span,
    ) -> Spanned<Expr> {
        let mut segments = path.segments.iter();
        let Some(first) = segments.next() else {
            return Spanned::new(Expr::Ident(String::new()), span);
        };
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in segments {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        expr
    }

    /// Build the bottom-up decorator application expression for a function declaration.
    pub(in crate::backend::ir::lower) fn decorator_application_expr(
        &self,
        function_name: &str,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Result<Spanned<Expr>, LoweringError> {
        let original_name = Self::decorator_original_function_name(function_name);
        let mut current = Spanned::new(Expr::Ident(original_name), ast::Span::default());
        for decorator in decorators.iter().rev() {
            if !self.is_user_defined_decorator_candidate(&decorator.node) {
                continue;
            }
            let callable = Self::decorator_callable_expr(decorator)?;
            current = Spanned::new(
                Expr::Call(Box::new(callable), Vec::new(), vec![ast::CallArg::Positional(current)]),
                Self::decorator_synthetic_callee_span(),
            );
        }
        Ok(current)
    }

    /// Build the callable expression for one decorator before it is applied to the decorated function value.
    pub(in crate::backend::ir::lower) fn decorator_callable_expr(
        decorator: &Spanned<ast::Decorator>,
    ) -> Result<Spanned<Expr>, LoweringError> {
        if decorator.node.is_call {
            let args = Self::decorator_call_args(decorator)?;
            let path = &decorator.node.path.segments;
            if path.len() >= 2 {
                let base_path = ImportPath {
                    parent_levels: decorator.node.path.parent_levels,
                    is_absolute: decorator.node.path.is_absolute,
                    segments: path[..path.len() - 1].to_vec(),
                };
                let base =
                    Self::decorator_path_expr_from_import_path(&base_path, Self::decorator_synthetic_callee_span());
                let method = path.last().cloned().unwrap_or_default();
                Ok(Spanned::new(
                    Expr::MethodCall(Box::new(base), method, decorator.node.type_args.clone(), args),
                    decorator.span,
                ))
            } else {
                let callee = Self::decorator_path_expr(&decorator.node, Self::decorator_synthetic_callee_span());
                Ok(Spanned::new(
                    Expr::Call(Box::new(callee), decorator.node.type_args.clone(), args),
                    decorator.span,
                ))
            }
        } else {
            Ok(Self::decorator_path_expr(&decorator.node, decorator.span))
        }
    }

    /// Convert parsed decorator arguments into ordinary call arguments for lowering.
    pub(in crate::backend::ir::lower) fn decorator_call_args(
        decorator: &Spanned<ast::Decorator>,
    ) -> Result<Vec<ast::CallArg>, LoweringError> {
        decorator
            .node
            .args
            .iter()
            .map(|arg| match arg {
                DecoratorArg::Positional(expr) => Ok(ast::CallArg::Positional(expr.clone())),
                DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) => {
                    Ok(ast::CallArg::Named(name.clone(), expr.clone()))
                }
                DecoratorArg::Named(_, DecoratorArgValue::Type(ty)) => Err(LoweringError {
                    message: "type-valued user-defined decorator arguments cannot be lowered".to_string(),
                    span: ty.span.into(),
                }),
            })
            .collect()
    }
}
