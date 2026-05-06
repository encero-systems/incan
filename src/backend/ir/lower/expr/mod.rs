//! Expression lowering for AST to IR conversion.
//!
//! This module handles lowering of all expression types: literals, identifiers, binary/unary operations, function
//! calls, method calls, comprehensions, etc.
//!
//! Large helpers (calls, patterns, comprehensions, pow helpers) are split into submodules; all methods live on `impl
//! AstLowering`.

mod calls;
mod comprehensions;
mod helpers;
mod patterns;

use super::super::TypedExpr;
use super::super::expr::{
    CollectionMethodKind, IrCallArg, IrCallArgKind, IrDictEntry, IrExpr, IrExprKind, IrListEntry, MethodCallArgPolicy,
    MethodKind, UnaryOp, VarAccess, VarRefKind,
};
use super::super::types::IrType;
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::typechecker::{IdentKind, ResolvedOperatorKind};
use incan_core::interop::RustCollectionFamily;
use incan_core::lang::magic_methods::{self, MagicMethodId};
use incan_semantics_core::SurfaceExprLoweringAction;

impl AstLowering {
    /// Lower a control-flow condition, rewriting validated `__bool__` hooks into direct method calls.
    pub(in crate::backend::ir::lower) fn lower_condition_expr(
        &mut self,
        expr: &Spanned<ast::Expr>,
    ) -> Result<TypedExpr, LoweringError> {
        let receiver = self.lower_expr_spanned(expr)?;
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(expr.span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Truthiness
        {
            return Ok(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: resolved_operator.method,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: self.callable_signature_for_call_span(expr.span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Bool,
            ));
        }
        Ok(receiver)
    }

    /// Return the element type carried by a lowered list spread operand.
    fn lowered_list_spread_element_type(ty: &IrType) -> Option<IrType> {
        match ty {
            IrType::List(elem) => Some((**elem).clone()),
            _ => None,
        }
    }

    /// Return the key/value types carried by a lowered dict spread operand.
    fn lowered_dict_spread_entry_types(ty: &IrType) -> Option<(IrType, IrType)> {
        match ty {
            IrType::Dict(key, value) => Some(((**key).clone(), (**value).clone())),
            _ => None,
        }
    }

    fn rust_collection_family_for_ir_type(ty: &IrType) -> Option<RustCollectionFamily> {
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                RustCollectionFamily::for_canonical_path(name).or(RustCollectionFamily::for_type_name(name))
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::rust_collection_family_for_ir_type(inner),
            _ => None,
        }
    }

    fn regular_method_call_arg_policy(
        &self,
        receiver_span: crate::frontend::ast::Span,
        receiver: &TypedExpr,
        method: &str,
        args: &[IrCallArg],
    ) -> MethodCallArgPolicy {
        if self
            .type_info
            .as_ref()
            .is_some_and(|info| info.preserves_regular_method_arg_shape(receiver_span, method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        if Self::rust_collection_family_for_ir_type(&receiver.ty)
            .is_some_and(|family| family.preserves_lookup_arg_shape(method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        // Fallback for unresolved Rust-interop receivers when optional rust-inspect metadata is unavailable or local
        // type inference did not retain the receiver family. Keep lookup calls like `counts.get(word)` borrow-shaped
        // rather than forcing an extra `&`/`.into()` conversion on already string-like probe values.
        if matches!(receiver.ty, IrType::Unknown)
            && matches!(method, "get" | "contains" | "contains_key")
            && args.first().is_some_and(|arg| {
                matches!(
                    arg.expr.ty,
                    IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
                )
            })
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        MethodCallArgPolicy::Default
    }

    /// Lower an expression using the available typechecker output (if present).
    ///
    /// This wraps [`Self::lower_expr`] and then overrides the inferred IR type using the typechecker span-to-type map.
    /// This is a stepping stone toward fully typed lowering.
    pub fn lower_expr_spanned(&mut self, expr: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        let mut lowered = self.lower_expr(&expr.node, expr.span)?;
        if let Some(info) = &self.type_info {
            if let Some(res_ty) = info.expr_type(expr.span) {
                // Preserve reference wrappers introduced by lowering (e.g. mutable parameters are tracked as
                // `RefMut(T)` in IR), while still benefiting from the typechecker's inner type information.
                //
                // The frontend type system does not model references, so `expr_type` typically returns `T` where
                // lowering may have already marked the same binding as `Ref(T)`/`RefMut(T)`.
                //
                // Likewise, RFC-008 const lowering may have already refined `str`/`bytes` to their static IR forms.
                // Keep those backend-specific const representations intact so later emission can materialize owned
                // values only when required.
                let inferred = self.lower_resolved_type(res_ty);
                lowered.ty = match &lowered.ty {
                    IrType::Ref(existing_inner) => {
                        IrType::Ref(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                    }
                    IrType::RefMut(existing_inner) => {
                        IrType::RefMut(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                    }
                    IrType::StaticStr => IrType::StaticStr,
                    IrType::StaticBytes => IrType::StaticBytes,
                    existing => Self::merge_inferred_ir_type(existing, inferred),
                };
            }
            if let Some(kind) = info.ident_kind(expr.span) {
                match (&expr.node, &mut lowered.kind) {
                    (ast::Expr::Ident(name), _) if matches!(kind, IdentKind::Static) => {
                        lowered.kind = IrExprKind::StaticRead { name: name.clone() };
                    }
                    (_, IrExprKind::Var { ref_kind, .. }) => {
                        *ref_kind = match kind {
                            IdentKind::Value => *ref_kind,
                            IdentKind::Static => *ref_kind,
                            IdentKind::TypeName => VarRefKind::TypeName,
                            IdentKind::Variant => VarRefKind::TypeName,
                            IdentKind::Module => VarRefKind::ExternalName,
                            IdentKind::RustImport => VarRefKind::ExternalRustName,
                            IdentKind::Trait => VarRefKind::TypeName,
                        };
                    }
                    _ => {}
                }
            }
        }
        // Apply any rusttype method return coercion recorded by the typechecker (e.g. &str → String).
        lowered = self.wrap_with_rust_return_coercion(lowered, expr.span)?;
        Ok(lowered)
    }

    /// Lower an expression to IR.
    ///
    /// Handles all expression types including:
    /// - Literals (int, float, string, bool)
    /// - Identifiers (variable references)
    /// - Binary and unary operations
    /// - Function and method calls
    /// - Field and index access
    /// - Control flow expressions (if, match)
    /// - Collections (list, dict, set, tuple)
    /// - Comprehensions (list, dict)
    /// - Closures and async/await
    ///
    /// `expr_span` must be the span of the whole `expr` node (as in [`Self::lower_expr_spanned`]). It is required for
    /// [`Expr::Call`](ast::Expr::Call) and [`Expr::MethodCall`](ast::Expr::MethodCall) so lowering can align with the
    /// typechecker’s span-keyed metadata (RFC 054 monomorph snapshots).
    pub fn lower_expr(&mut self, expr: &ast::Expr, expr_span: ast::Span) -> Result<TypedExpr, LoweringError> {
        let (kind, ty) = match expr {
            // ---- Identifiers ----
            ast::Expr::Ident(name) => {
                let lowered_name = self.symbol_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
                let ty = self.lookup_var(&lowered_name);
                let access = self.select_var_access_for_ident(&lowered_name, &ty);
                (
                    IrExprKind::Var {
                        name: lowered_name.clone(),
                        access,
                        ref_kind: if self.is_static_binding(&lowered_name) {
                            VarRefKind::StaticBinding
                        } else {
                            VarRefKind::Value
                        },
                    },
                    ty,
                )
            }

            // ---- Literals ----
            ast::Expr::Literal(lit) => match lit {
                ast::Literal::Int(il) => (IrExprKind::Int(il.value), IrType::Int),
                ast::Literal::Float(fl) => (IrExprKind::Float(fl.value), IrType::Float),
                ast::Literal::String(s) => (IrExprKind::String(s.clone()), IrType::String),
                ast::Literal::Bytes(bytes) => (IrExprKind::Bytes(bytes.clone()), IrType::Unknown),
                ast::Literal::Bool(b) => (IrExprKind::Bool(*b), IrType::Bool),
                ast::Literal::None => (IrExprKind::None, IrType::Option(Box::new(IrType::Unknown))),
            },

            // ---- Self expression ----
            ast::Expr::SelfExpr => (
                IrExprKind::Var {
                    name: "self".to_string(),
                    access: VarAccess::Borrow,
                    ref_kind: VarRefKind::Value,
                },
                IrType::Unknown,
            ),

            // ---- Binary operations ----
            ast::Expr::Binary(l, op, r) => {
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Binary
                    // `__eq__` is represented in generated Rust as `PartialEq::eq`, not as an inherent method.
                    && resolved_operator.method != magic_methods::as_str(MagicMethodId::Eq)
                {
                    let receiver = self.lower_expr_spanned(l)?;
                    let arg_expr = self.lower_expr_spanned(r)?;
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method: resolved_operator.method,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: arg_expr,
                            }],
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Contains
                {
                    let item = self.lower_expr_spanned(l)?;
                    let receiver = self.lower_expr_spanned(r)?;
                    let contains_call = IrExprKind::MethodCall {
                        receiver: Box::new(receiver),
                        method: resolved_operator.method,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: item,
                        }],
                        callable_signature: self.callable_signature_for_call_span(expr_span),
                        arg_policy: MethodCallArgPolicy::Default,
                    };
                    if matches!(op, ast::BinaryOp::NotIn) {
                        (
                            IrExprKind::UnaryOp {
                                op: UnaryOp::Not,
                                operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                            },
                            IrType::Bool,
                        )
                    } else {
                        (contains_call, IrType::Bool)
                    }
                } else {
                    // Special handling for `in` and `not in` operators
                    // - `x in collection` → builtin-aware `collection.contains(x)`
                    // - `x not in collection` → `!collection.contains(x)`
                    match op {
                        ast::BinaryOp::In | ast::BinaryOp::NotIn => {
                            let item = self.lower_expr_spanned(l)?;
                            let collection = self.lower_expr_spanned(r)?;

                            // Generate `collection.contains(item)` using the same receiver-aware classification path as
                            // ordinary method syntax so containment keeps builtin semantics for strings, lists, sets,
                            // and dicts without emitter-side name guessing.
                            let contains_args = vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: item,
                            }];
                            let contains_kind = MethodKind::for_receiver(&collection.ty, "contains").or_else(|| {
                                let mut receiver_ty = &collection.ty;
                                while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
                                    receiver_ty = inner.as_ref();
                                }
                                matches!(receiver_ty, IrType::Dict(_, _))
                                    .then_some(MethodKind::Collection(CollectionMethodKind::Contains))
                            });
                            let contains_call = if let Some(kind) = contains_kind {
                                IrExprKind::KnownMethodCall {
                                    receiver: Box::new(collection),
                                    kind,
                                    args: contains_args,
                                }
                            } else {
                                let arg_policy = self.regular_method_call_arg_policy(
                                    r.span,
                                    &collection,
                                    "contains",
                                    &contains_args,
                                );
                                IrExprKind::MethodCall {
                                    receiver: Box::new(collection),
                                    method: "contains".to_string(),
                                    type_args: Vec::new(),
                                    args: contains_args,
                                    callable_signature: None,
                                    arg_policy,
                                }
                            };

                            if matches!(op, ast::BinaryOp::NotIn) {
                                // Wrap in negation for `not in`
                                (
                                    IrExprKind::UnaryOp {
                                        op: UnaryOp::Not,
                                        operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                                    },
                                    IrType::Bool,
                                )
                            } else {
                                (contains_call, IrType::Bool)
                            }
                        }
                        _ => {
                            let left = self.lower_expr_spanned(l)?;
                            let right = self.lower_expr_spanned(r)?;
                            // For Pow, compute exponent kind for policy-based result type
                            let pow_exp_kind = if matches!(op, ast::BinaryOp::Pow) {
                                Some(Self::pow_exponent_kind(r, &right.ty))
                            } else {
                                None
                            };
                            let result_ty = self.binary_result_type(&left.ty, &right.ty, op, pow_exp_kind);
                            (
                                IrExprKind::BinOp {
                                    op: self.lower_binop(op, expr_span)?,
                                    left: Box::new(left),
                                    right: Box::new(right),
                                },
                                result_ty,
                            )
                        }
                    }
                }
            }

            // ---- Unary operations ----
            ast::Expr::Unary(op, e) => {
                let operand = self.lower_expr_spanned(e)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Unary
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(operand),
                            method: resolved_operator.method,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    let ty = operand.ty.clone();
                    (
                        IrExprKind::UnaryOp {
                            op: match op {
                                ast::UnaryOp::Neg => UnaryOp::Neg,
                                ast::UnaryOp::Not => UnaryOp::Not,
                                ast::UnaryOp::Invert => UnaryOp::Not,
                            },
                            operand: Box::new(operand),
                        },
                        ty,
                    )
                }
            }

            // ---- Function / constructor calls (delegated to calls submodule) ----
            ast::Expr::Call(f, type_args, args) => {
                return self
                    .lower_call_expr(f, type_args, args, expr_span)
                    .map(|(k, t)| TypedExpr::new(k, t));
            }

            // ---- Method calls ----
            ast::Expr::MethodCall(o, m, type_args, args) => {
                let receiver = if let ast::Expr::Index(base, _) = &o.node
                    && let ast::Expr::Ident(name) = &base.node
                    && self.type_info.as_ref().is_some_and(|info| {
                        matches!(info.ident_kind(base.span), Some(IdentKind::TypeName | IdentKind::Trait))
                    }) {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(o.span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or_else(|| self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown));
                    TypedExpr::new(
                        IrExprKind::Var {
                            name: name.clone(),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::TypeName,
                        },
                        ty,
                    )
                } else {
                    self.lower_expr_spanned(o)?
                };
                let mut args_ir = self.lower_call_args(args)?;
                let lowered_type_args = self.lower_call_site_type_args(expr_span, type_args);
                let method_name = self.resolve_method_rebinding(&receiver.ty, m);
                let arg_policy = self.regular_method_call_arg_policy(o.span, &receiver, &method_name, &args_ir);
                if !matches!(arg_policy, MethodCallArgPolicy::PreserveShape) {
                    for (arg_ir, arg_ast) in args_ir.iter_mut().zip(args.iter()) {
                        let arg_span = match arg_ast {
                            ast::CallArg::Positional(expr)
                            | ast::CallArg::Named(_, expr)
                            | ast::CallArg::PositionalUnpack(expr)
                            | ast::CallArg::KeywordUnpack(expr) => expr.span,
                        };
                        arg_ir.expr = self.wrap_with_rust_arg_coercion(arg_ir.expr.clone(), arg_span)?;
                    }
                }

                // Check for known methods (enum-based dispatch)
                if let Some(kind) = MethodKind::for_receiver(&receiver.ty, &method_name) {
                    (
                        IrExprKind::KnownMethodCall {
                            receiver: Box::new(receiver),
                            kind,
                            args: args_ir,
                        },
                        IrType::Unknown,
                    )
                } else {
                    // Unknown method - keep as string-based call
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method: method_name,
                            type_args: lowered_type_args,
                            args: args_ir,
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy,
                        },
                        IrType::Unknown,
                    )
                }
            }

            // ---- Index access ----
            ast::Expr::Index(o, i) => {
                let obj = self.lower_expr_spanned(o)?;
                let idx = self.lower_expr_spanned(i)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Index
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method: resolved_operator.method,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: idx,
                            }],
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    let elem_ty = match &obj.ty {
                        IrType::List(e) => (**e).clone(),
                        IrType::Dict(_, v) => (**v).clone(),
                        IrType::String => IrType::String,
                        _ => IrType::Unknown,
                    };
                    (
                        IrExprKind::Index {
                            object: Box::new(obj),
                            index: Box::new(idx),
                        },
                        elem_ty,
                    )
                }
            }

            // ---- Field access ----
            ast::Expr::Field(o, f) => {
                // Prefer spanned lowering so typechecker output can drive the receiver type.
                // This is important for RFC 021 alias-aware field access, especially for `self.<alias>`.
                let obj = self.lower_expr_spanned(o)?;
                if let Some(access) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.computed_property_access(expr_span))
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method: access.property.clone(),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    // RFC 021: resolve field alias to canonical name if object is a known struct type
                    let struct_name = obj.ty.nominal_type_name().or_else(|| match &obj.kind {
                        IrExprKind::Var { name, .. } if name == "self" => self.current_impl_type.as_deref(),
                        _ => None,
                    });
                    let field = match struct_name {
                        Some(struct_name) => self.resolve_field_alias(struct_name, f),
                        None => f.clone(),
                    };
                    (
                        IrExprKind::Field {
                            object: Box::new(obj),
                            field,
                        },
                        IrType::Unknown,
                    )
                }
            }

            // ---- Surface expressions (routed through semantics registry) ----
            ast::Expr::Surface(surface_expr) => {
                use crate::semantics_registry::semantics_registry;

                let action = semantics_registry()
                    .lower_surface_expr_action(&surface_expr.key)
                    .ok_or_else(|| LoweringError {
                        message: format!(
                            "no lowering action registered for surface expression {:?}",
                            surface_expr.key
                        ),
                        span: super::super::IrSpan::default(),
                    })?;

                match (action, &surface_expr.payload) {
                    (SurfaceExprLoweringAction::Await, ast::SurfaceExprPayload::PrefixUnary(inner)) => {
                        // Preserve explicit grouping: `await (x?)` should keep the grouped `Try` operand shape
                        // instead of applying await/try normalization for the unparenthesized `await x()?` case.
                        let parenthesized_operand = matches!(&inner.node, ast::Expr::Paren(_));
                        let lowered_inner = self.lower_expr_spanned(inner)?;
                        if parenthesized_operand {
                            let ty = lowered_inner.ty.clone();
                            (IrExprKind::Await(Box::new(lowered_inner)), ty)
                        } else {
                            super::super::surface_semantics::lower_await_expression(lowered_inner)
                        }
                    }
                    _ => {
                        return Err(LoweringError {
                            message: format!(
                                "surface expression {:?} has an unsupported payload for lowering",
                                surface_expr.key
                            ),
                            span: super::super::IrSpan::default(),
                        });
                    }
                }
            }

            // ---- Try (?) ----
            ast::Expr::Try(e) => {
                let inner = self.lower_expr_spanned(e)?;
                let ty = match &inner.ty {
                    IrType::Result(ok, _) => (**ok).clone(),
                    _ => inner.ty.clone(),
                };
                (IrExprKind::Try(Box::new(inner)), ty)
            }

            // ---- Match expressions (delegated to patterns submodule) ----
            ast::Expr::Match(s, arms) => {
                let scrutinee = self.lower_expr_spanned(s)?;
                let arms_ir = self.lower_match_arms(arms, &scrutinee.ty)?;
                let ty = arms_ir.first().map(|a| a.body.ty.clone()).unwrap_or(IrType::Unknown);
                (
                    IrExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms: arms_ir,
                    },
                    ty,
                )
            }

            // ---- If expressions ----
            ast::Expr::If(i) => {
                let cond = self.lower_condition_expr(&i.condition)?;
                let then_stmts = self.lower_statements(&i.then_body)?;
                let then_expr = TypedExpr::new(
                    IrExprKind::Block {
                        stmts: then_stmts,
                        value: None,
                    },
                    IrType::Unit,
                );
                let else_expr = i
                    .else_body
                    .as_ref()
                    .map(|b| {
                        self.lower_statements(b)
                            .map(|stmts| TypedExpr::new(IrExprKind::Block { stmts, value: None }, IrType::Unit))
                    })
                    .transpose()?;
                (
                    IrExprKind::If {
                        condition: Box::new(cond),
                        then_branch: Box::new(then_expr),
                        else_branch: else_expr.map(Box::new),
                    },
                    IrType::Unit,
                )
            }

            ast::Expr::Loop(loop_expr) => {
                self.push_scope();
                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&loop_expr.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();
                (IrExprKind::Loop { body }, IrType::Unknown)
            }

            // ---- Closures ----
            ast::Expr::Closure(params, body) => {
                let param_pairs: Vec<(String, IrType)> = params
                    .iter()
                    .map(|p| (p.node.name.clone(), self.lower_type(&p.node.ty.node)))
                    .collect();
                self.non_linear_context_depth += 1;
                let body_ir_result = self.lower_expr_spanned(body);
                self.non_linear_context_depth -= 1;
                let body_ir = body_ir_result?;
                let ret_ty = body_ir.ty.clone();
                let param_tys: Vec<IrType> = param_pairs.iter().map(|(_, t)| t.clone()).collect();
                (
                    IrExprKind::Closure {
                        params: param_pairs,
                        body: Box::new(body_ir),
                        captures: vec![],
                    },
                    IrType::Function {
                        params: param_tys,
                        ret: Box::new(ret_ty),
                    },
                )
            }

            // ---- Collection literals ----
            ast::Expr::Tuple(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let tys: Vec<IrType> = items_ir.iter().map(|i| i.ty.clone()).collect();
                (IrExprKind::Tuple(items_ir), IrType::Tuple(tys))
            }

            ast::Expr::List(items) => {
                let items_ir: Vec<IrListEntry> = items
                    .iter()
                    .map(|i| match i {
                        ast::ListEntry::Element(value) => self.lower_expr_spanned(value).map(IrListEntry::Element),
                        ast::ListEntry::Spread(value) => self.lower_expr_spanned(value).map(IrListEntry::Spread),
                    })
                    .collect::<Result<_, _>>()?;
                let elem = items_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrListEntry::Element(value) => Some(value.ty.clone()),
                        IrListEntry::Spread(value) => Self::lowered_list_spread_element_type(&value.ty),
                    })
                    .unwrap_or(IrType::Unknown);
                (IrExprKind::List(items_ir), IrType::List(Box::new(elem)))
            }

            ast::Expr::Dict(pairs) => {
                let pairs_ir: Vec<IrDictEntry> = pairs
                    .iter()
                    .map(|entry| match entry {
                        ast::DictEntry::Pair(k, v) => Ok(IrDictEntry::Pair(
                            self.lower_expr_spanned(k)?,
                            Box::new(self.lower_expr_spanned(v)?),
                        )),
                        ast::DictEntry::Spread(value) => self.lower_expr_spanned(value).map(IrDictEntry::Spread),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                let (k, v) = pairs_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrDictEntry::Pair(key, value) => Some((key.ty.clone(), value.ty.clone())),
                        IrDictEntry::Spread(value) => Self::lowered_dict_spread_entry_types(&value.ty),
                    })
                    .unwrap_or((IrType::Unknown, IrType::Unknown));
                (IrExprKind::Dict(pairs_ir), IrType::Dict(Box::new(k), Box::new(v)))
            }

            ast::Expr::Set(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let elem = items_ir.first().map(|i| i.ty.clone()).unwrap_or(IrType::Unknown);
                (IrExprKind::Set(items_ir), IrType::Set(Box::new(elem)))
            }

            // ---- Parenthesized expression (transparent) ----
            ast::Expr::Paren(e) => return self.lower_expr_spanned(e),

            // ---- Constructor (variant / struct literal) ----
            ast::Expr::Constructor(name, args) => {
                let fields: Vec<(String, TypedExpr)> = args
                    .iter()
                    .map(|arg| match arg {
                        ast::CallArg::Named(n, e) => Ok((n.clone(), self.lower_expr_spanned(e)?)),
                        ast::CallArg::Positional(e)
                        | ast::CallArg::PositionalUnpack(e)
                        | ast::CallArg::KeywordUnpack(e) => Ok((String::new(), self.lower_expr_spanned(e)?)),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                (
                    IrExprKind::Struct {
                        name: name.clone(),
                        fields,
                    },
                    IrType::Struct(name.clone()),
                )
            }

            // ---- Range expressions ----
            ast::Expr::Range { start, end, inclusive } => {
                let s = self.lower_expr_spanned(start)?;
                let e = self.lower_expr_spanned(end)?;
                (
                    IrExprKind::Range {
                        start: Some(Box::new(s)),
                        end: Some(Box::new(e)),
                        inclusive: *inclusive,
                    },
                    IrType::Unknown,
                )
            }

            // ---- F-strings ----
            ast::Expr::FString(parts) => {
                let ir_parts: Vec<super::super::expr::FormatPart> = parts
                    .iter()
                    .map(|part| match part {
                        ast::FStringPart::Literal(s) => Ok(super::super::expr::FormatPart::Literal(s.clone())),
                        ast::FStringPart::Expr(e) => {
                            let lowered = self.lower_expr_spanned(e)?;
                            Ok(super::super::expr::FormatPart::Expr(lowered))
                        }
                    })
                    .collect::<Result<Vec<_>, LoweringError>>()?;
                (IrExprKind::Format { parts: ir_parts }, IrType::String)
            }

            // ---- Slice expressions ----
            ast::Expr::Slice(target, slice) => {
                let target_expr = self.lower_expr_spanned(target)?;
                let start = slice
                    .start
                    .as_ref()
                    .map(|s| Ok(Box::new(self.lower_expr_spanned(s)?)))
                    .transpose()?;
                let end = slice
                    .end
                    .as_ref()
                    .map(|e| Ok(Box::new(self.lower_expr_spanned(e)?)))
                    .transpose()?;
                let step = slice
                    .step
                    .as_ref()
                    .map(|st| Ok(Box::new(self.lower_expr_spanned(st)?)))
                    .transpose()?;

                let result_ty = match &target_expr.ty {
                    IrType::List(inner) => IrType::List(inner.clone()),
                    IrType::String => IrType::String,
                    _ => IrType::Unknown,
                };

                (
                    IrExprKind::Slice {
                        target: Box::new(target_expr),
                        start,
                        end,
                        step,
                    },
                    result_ty,
                )
            }

            // ---- Comprehensions (delegated to comprehensions submodule) ----
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp)?,
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp)?,

            // ---- Yield (placeholder) ----
            ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown),
        };
        Ok(TypedExpr::new(kind, ty))
    }
}
