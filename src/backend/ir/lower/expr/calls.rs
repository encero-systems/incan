//! Call expression lowering: struct constructors, builtin dispatch, newtype checked construction, and regular function
//! calls.

use super::super::super::decl::FunctionParam;
use super::super::super::expr::{
    BuiltinFn, IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrInteropCoercionKind, IrListEntry,
    Literal as IrLiteral, MatchArm, MethodCallArgPolicy, Pattern, VarAccess, VarRefKind,
};
use super::super::super::types::IrType;
use super::super::super::{FunctionSignature, TypedExpr};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use crate::frontend::symbols::{CallableParam, ResolvedType};
use crate::frontend::typechecker::{FixedUnpackPlan, RustArgCoercionKind};
use crate::frontend::typechecker::{IdentKind, ResolvedOperatorKind};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::stdlib;
use incan_core::lang::surface::constructors::{self, ConstructorId};

pub(crate) const INTERNAL_PANIC_FN: &str = "__incan_internal_panic";

impl AstLowering {
    /// Rebuild a callable signature from frontend metadata for rest-aware IR emission.
    fn callable_signature_from_params(&self, params: &[CallableParam], ret: &ResolvedType) -> FunctionSignature {
        FunctionSignature {
            params: params
                .iter()
                .enumerate()
                .map(|(idx, param)| {
                    let base_ty = self.lower_resolved_type(&param.ty);
                    let ty = Self::lower_param_container_type(param.kind, base_ty);
                    FunctionParam {
                        name: param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                        ty,
                        mutability: super::super::super::types::Mutability::Immutable,
                        is_self: false,
                        kind: param.kind,
                        default: None,
                    }
                })
                .collect(),
            return_type: self.lower_resolved_type(ret),
        }
    }

    /// Rebuild a callable signature directly from a stdlib method declaration so default expressions survive import
    /// metadata boundaries.
    fn callable_signature_from_stdlib_method_decl(&mut self, method: &ast::MethodDecl) -> FunctionSignature {
        FunctionSignature {
            params: method
                .params
                .iter()
                .map(|param| {
                    let base_ty = self.lower_type(&param.node.ty.node);
                    let ty = Self::lower_param_container_type(param.node.kind, base_ty);
                    FunctionParam {
                        name: param.node.name.clone(),
                        ty,
                        mutability: super::super::super::types::Mutability::Immutable,
                        is_self: false,
                        kind: param.node.kind,
                        default: param
                            .node
                            .default
                            .as_ref()
                            .and_then(|default_expr| self.lower_expr_spanned(default_expr).ok()),
                    }
                })
                .collect(),
            return_type: self.lower_type(&method.return_type.node),
        }
    }

    /// Resolve an imported stdlib type method signature by loading the owning stdlib stub AST.
    ///
    /// Function metadata already has a direct stdlib lookup path, but type-member calls such as `App.run()` arrive as
    /// method calls. The lightweight frontend import metadata only records `has_default`, so this path rehydrates the
    /// actual default expressions from the stdlib source declaration before IR emission fills omitted arguments.
    pub(in crate::backend::ir::lower) fn callable_signature_for_imported_stdlib_type_method_path(
        &mut self,
        path: &[String],
        method_name: &str,
    ) -> Option<FunctionSignature> {
        if path.len() < 3 || path.first().map(String::as_str) != Some(incan_core::lang::stdlib::STDLIB_ROOT) {
            return None;
        }
        let type_name = path.last()?;
        let module_path = &path[..path.len() - 1];
        let mut cache = crate::frontend::typechecker::stdlib_loader::StdlibAstCache::new();
        let method = cache.lookup_type_method_decl(module_path, type_name, method_name)?;
        Some(self.callable_signature_from_stdlib_method_decl(&method))
    }

    /// Resolve the signature for an imported stdlib function by its canonical import path.
    ///
    /// Lowered stdlib modules may import private helpers from sibling stdlib modules. Those helpers are not in the
    /// current module's IR function registry, but their `.incn` declarations are still available through the stdlib AST
    /// cache. Attaching the exact module-qualified signature here lets codegen apply normal Incan argument conversion
    /// rules without merging same-named helpers from unrelated stdlib modules.
    fn callable_signature_for_imported_stdlib_path(&self, path: &[String]) -> Option<FunctionSignature> {
        if path.len() < 2 || path.first().map(String::as_str) != Some(incan_core::lang::stdlib::STDLIB_ROOT) {
            return None;
        }
        let function_name = path.last()?;
        let module_path = &path[..path.len() - 1];
        let mut cache = crate::frontend::typechecker::stdlib_loader::StdlibAstCache::new();
        let info = cache.lookup_function(module_path, function_name)?;
        Some(self.callable_signature_from_params(&info.params, &info.return_type))
    }

    /// Resolve a callable signature from the callee expression's type information.
    ///
    /// This covers values whose type is already known as `Function(...)`, which is separate from call-site metadata
    /// gathered for defaults, named arguments, and other invocation-specific details.
    fn callable_signature_for_callee_span(&self, span: ast::Span) -> Option<FunctionSignature> {
        let info = self.type_info.as_ref()?;
        match info.expr_type(span)? {
            ResolvedType::Function(params, ret) => Some(self.callable_signature_from_params(params, ret)),
            _ => None,
        }
    }

    /// Return the typechecker-proven callable signature for a full call expression span.
    pub(in crate::backend::ir::lower) fn callable_signature_for_call_span(
        &self,
        span: ast::Span,
    ) -> Option<FunctionSignature> {
        let info = self.type_info.as_ref()?;
        let params = info.call_site_callable_params(span)?;
        Some(FunctionSignature {
            params: self
                .callable_signature_from_params(params, &ResolvedType::Unknown)
                .params,
            return_type: IrType::Unknown,
        })
    }

    /// Prefer monomorphized call-site type args from the typechecker (RFC 054); otherwise lower AST types.
    pub(super) fn lower_call_site_type_args(
        &self,
        call_span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Vec<IrType> {
        if let Some(info) = self.type_info.as_ref()
            && let Some(resolved) = info
                .call_site_monomorph_type_args
                .get(&(call_span.start, call_span.end))
        {
            return resolved.iter().map(|t| self.lower_resolved_type(t)).collect();
        }
        type_args.iter().map(|ty| self.lower_type(&ty.node)).collect()
    }

    fn call_arg_expr(arg: &ast::CallArg) -> &ast::Spanned<ast::Expr> {
        match arg {
            ast::CallArg::Positional(e)
            | ast::CallArg::Named(_, e)
            | ast::CallArg::PositionalUnpack(e)
            | ast::CallArg::KeywordUnpack(e) => e,
        }
    }

    fn lower_adapter_kind(adapter_kind: ast::InteropAdapterKind) -> super::super::super::decl::IrInteropAdapterKind {
        match adapter_kind {
            ast::InteropAdapterKind::Via => super::super::super::decl::IrInteropAdapterKind::Via,
            ast::InteropAdapterKind::Try => super::super::super::decl::IrInteropAdapterKind::Try,
        }
    }

    fn lower_rusttype_interop_adapter(
        &mut self,
        arg_ty: &IrType,
        target_ty: &IrType,
    ) -> Result<Option<(TypedExpr, super::super::super::decl::IrInteropAdapterKind)>, LoweringError> {
        if let Some(type_name) = arg_ty.nominal_type_name()
            && let Some(edges) = self.rusttype_interop_edges.get(type_name).cloned()
        {
            for edge in edges {
                if !matches!(edge.direction, ast::InteropDirection::Into) {
                    continue;
                }
                let edge_ty = self.lower_type(&edge.ty.node);
                if edge_ty != *target_ty {
                    continue;
                }
                let adapter_expr = self.lower_expr_spanned(&edge.adapter)?;
                return Ok(Some((adapter_expr, Self::lower_adapter_kind(edge.adapter_kind))));
            }
        }

        if let Some(type_name) = target_ty.nominal_type_name()
            && let Some(edges) = self.rusttype_interop_edges.get(type_name).cloned()
        {
            for edge in edges {
                if !matches!(edge.direction, ast::InteropDirection::From) {
                    continue;
                }
                let edge_ty = self.lower_type(&edge.ty.node);
                if edge_ty != *arg_ty {
                    continue;
                }
                let adapter_expr = self.lower_expr_spanned(&edge.adapter)?;
                return Ok(Some((adapter_expr, Self::lower_adapter_kind(edge.adapter_kind))));
            }
        }

        Ok(None)
    }

    /// Wrap the result of a method call in an `InteropCoerce` node when the typechecker recorded a return coercion for
    /// the call expression span.
    ///
    /// This handles the case where a `rusttype` method is declared in Incan with return type `str` but the actual Rust
    /// method returns `&str`. The metadata-driven coercion detection records the mismatch; this function inserts
    /// `.to_string()` (or equivalent) in the IR so the generated Rust code compiles without type errors.
    pub(in crate::backend::ir::lower) fn wrap_with_rust_return_coercion(
        &mut self,
        expr: TypedExpr,
        span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let coercion = self
            .type_info
            .as_ref()
            .and_then(|info| info.rust_return_coercion(span).cloned());
        let Some(coercion) = coercion else {
            return Ok(expr);
        };
        // Return coercions are always Builtin; RustTypeUnwrap / RustTypeInterop do not apply here.
        let RustArgCoercionKind::Builtin(policy) = coercion.kind else {
            return Ok(expr);
        };
        let target_ty = self.lower_resolved_type(&coercion.target_type);
        let from_ty = expr.ty.clone();
        Ok(TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(expr),
                from_ty,
                to_ty: target_ty.clone(),
                kind: IrInteropCoercionKind::Builtin {
                    policy,
                    rust_target: coercion.rust_target_type,
                },
            },
            target_ty,
        ))
    }

    /// Wrap one call argument in `InteropCoerce` when typechecking recorded a Rust boundary coercion.
    ///
    /// For `RustTypeInterop`, lowering first attempts to resolve a declared `interop:` adapter. If no
    /// adapter edge matches, lowering falls back to `RustTypeUnwrap` so the generated Rust call still
    /// receives the underlying Rust value.
    pub(in crate::backend::ir::lower) fn wrap_with_rust_arg_coercion(
        &mut self,
        arg_expr: TypedExpr,
        span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let coercion = self
            .type_info
            .as_ref()
            .and_then(|info| info.rust_arg_coercion(span).cloned());
        let Some(coercion) = coercion else {
            return Ok(arg_expr);
        };
        let target_ty = self.lower_resolved_type(&coercion.target_type);
        let from_ty = arg_expr.ty.clone();
        let kind = match coercion.kind {
            RustArgCoercionKind::Builtin(policy) => IrInteropCoercionKind::Builtin {
                policy,
                rust_target: coercion.rust_target_type,
            },
            RustArgCoercionKind::RustTypeUnwrap => IrInteropCoercionKind::RustTypeUnwrap,
            RustArgCoercionKind::RustTypeInterop => {
                if let Some((adapter, adapter_kind)) = self.lower_rusttype_interop_adapter(&from_ty, &target_ty)? {
                    IrInteropCoercionKind::AdapterCall {
                        adapter: Box::new(adapter),
                        adapter_kind,
                    }
                } else {
                    IrInteropCoercionKind::RustTypeUnwrap
                }
            }
        };
        Ok(TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(arg_expr),
                from_ty,
                to_ty: target_ty.clone(),
                kind,
            },
            target_ty,
        ))
    }

    /// Lower a function/constructor call expression.
    ///
    /// Handles struct constructors, builtin functions, newtype checked construction, and regular function calls.
    pub(in crate::backend::ir::lower) fn lower_call_expr(
        &mut self,
        f: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        call_span: ast::Span,
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        // Check if this is a struct/model/class constructor call
        if let ast::Expr::Ident(name) = &f.node {
            let constructor_name = self.symbol_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
            if stdlib::is_graph_constructor_type(&constructor_name) && args.is_empty() {
                let lowered_type_args = self.lower_call_site_type_args(call_span, type_args);
                let receiver_ty = if lowered_type_args.is_empty() {
                    IrType::Struct(constructor_name.clone())
                } else {
                    IrType::NamedGeneric(constructor_name.clone(), lowered_type_args.clone())
                };
                return Ok((
                    IrExprKind::MethodCall {
                        receiver: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: constructor_name,
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::TypeName,
                            },
                            receiver_ty.clone(),
                        )),
                        method: "__incan_new".to_string(),
                        type_args: Vec::new(),
                        args: Vec::new(),
                        dispatch: None,
                        callable_signature: None,
                        arg_policy: MethodCallArgPolicy::Default,
                    },
                    receiver_ty,
                ));
            }
            if keywords::from_str(name.as_str()) == Some(KeywordId::Cls)
                && matches!(self.lookup_var(name), IrType::Unknown)
                && let Some(owner_name) = self.current_classmethod_constructor.clone()
            {
                return self.lower_constructor_call(&owner_name, args);
            }

            // Constructor lowering must follow typechecker resolution, not identifier casing. Local declarations are
            // still available through `struct_names`; imported constructors are marked as `TypeName` on the callee
            // span by the typechecker.
            let is_known_struct = self.struct_names.contains_key(&constructor_name);
            let is_resolved_type_name = self
                .type_info
                .as_ref()
                .is_some_and(|info| matches!(info.ident_kind(f.span), Some(IdentKind::TypeName)));

            if is_known_struct || is_resolved_type_name {
                return self.lower_constructor_call(&constructor_name, args);
            }
        }

        let imported_callee_path = match &f.node {
            ast::Expr::Ident(name) => self.import_aliases.get(name).cloned(),
            _ => None,
        };
        let func = self.lower_expr_spanned(f)?;
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(call_span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Len
        {
            let Some(first_arg) = args.first() else {
                return Ok((
                    IrExprKind::BuiltinCall {
                        func: BuiltinFn::Len,
                        args: Vec::new(),
                    },
                    IrType::Int,
                ));
            };
            let receiver = self.lower_expr_spanned(Self::call_arg_expr(first_arg))?;
            return Ok((
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: resolved_operator.method,
                    dispatch: None,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: self.callable_signature_for_call_span(call_span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Int,
            ));
        }
        if let ast::Expr::Ident(name) = &f.node
            && let Some(builtin) = BuiltinFn::from_name(name)
            && imported_callee_path.is_none()
            && !matches!(func.ty, IrType::Function { .. })
        {
            let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
            return Ok((
                IrExprKind::BuiltinCall {
                    func: builtin,
                    args: args_ir,
                },
                IrType::Unknown, // Return type depends on the builtin
            ));
        }

        // Regular function call (user-defined or unknown)
        let mut args_ir = self.lower_call_args(args)?;
        let lowered_type_args = self.lower_call_site_type_args(call_span, type_args);
        for (arg_ir, arg_ast) in args_ir.iter_mut().zip(args.iter()) {
            let arg_span = Self::call_arg_expr(arg_ast).span;
            arg_ir.expr = self.wrap_with_rust_arg_coercion(arg_ir.expr.clone(), arg_span)?;
        }
        if imported_callee_path.as_ref().is_some_and(|path| {
            path.len() == 3 && path[0] == "std" && path[1] == "testing" && path[2] == "assert_raises"
        }) && args_ir
            .get(1)
            .is_none_or(|arg| !matches!(arg.expr.kind, IrExprKind::Literal(IrLiteral::StaticStr(_))))
        {
            let Some(error_type) = type_args.first() else {
                return Err(LoweringError {
                    message: "std.testing.assert_raises requires an error type argument".to_string(),
                    span: call_span.into(),
                });
            };
            args_ir.insert(
                1,
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Literal(IrLiteral::StaticStr(error_type.node.to_string())),
                        IrType::StaticStr,
                    ),
                },
            );
        }
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(call_span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Call
        {
            let ret_ty = self
                .type_info
                .as_ref()
                .and_then(|info| info.expr_type(call_span))
                .map(|ty| self.lower_resolved_type(ty))
                .unwrap_or(IrType::Unknown);
            return Ok((
                IrExprKind::MethodCall {
                    receiver: Box::new(func),
                    method: resolved_operator.method,
                    dispatch: None,
                    type_args: Vec::new(),
                    args: args_ir,
                    callable_signature: self.callable_signature_for_call_span(call_span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                ret_ty,
            ));
        }
        let ret_ty = if let IrType::Function { ret, .. } = &func.ty {
            (**ret).clone()
        } else {
            IrType::Unknown
        };
        Ok((
            IrExprKind::Call {
                func: Box::new(func),
                type_args: lowered_type_args,
                args: args_ir,
                callable_signature: imported_callee_path
                    .as_deref()
                    .and_then(|path| self.callable_signature_for_imported_stdlib_path(path))
                    .or_else(|| self.callable_signature_for_call_span(call_span))
                    .or_else(|| self.callable_signature_for_callee_span(f.span)),
                canonical_path: imported_callee_path,
            },
            ret_ty,
        ))
    }

    /// Lower a struct/model/class/newtype constructor call.
    fn lower_constructor_call(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        // Get type if known, otherwise Unknown (will be inferred at emit time)
        let struct_ty = self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown);

        // ----------------------------------------------------------------
        // Newtype checked construction (v0.1 hardening for #44, RFC runway)
        // ----------------------------------------------------------------
        if self.newtype_checked_ctor.contains_key(name)
            && args.len() == 1
            && matches!(args[0], ast::CallArg::Positional(_))
            && self.current_impl_type.as_deref() != Some(name)
        {
            let ast::CallArg::Positional(value) = &args[0] else {
                unreachable!("checked by matches! above")
            };
            let lowered_value = self.lower_expr_spanned(value)?;
            let ctor = self
                .newtype_checked_ctor
                .get(name)
                .cloned()
                .unwrap_or_else(|| "from_underlying".to_string());

            // Use the actual newtype struct type for the receiver (not Unknown)
            let receiver = TypedExpr::new(
                IrExprKind::Var {
                    name: name.to_string(),
                    access: VarAccess::Copy,
                    ref_kind: VarRefKind::TypeName,
                },
                struct_ty.clone(),
            );
            let from_underlying_call = TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: ctor.clone(),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: vec![IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: lowered_value,
                    }],
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Result(Box::new(struct_ty.clone()), Box::new(IrType::Unknown)),
            );
            let value_name = "__incan_newtype_value".to_string();
            // Keep the failure path local to generated code: the Err branch still panics, but we no longer emit an
            // `.expect()` extraction in the generated Rust.
            let ok_arm = MatchArm {
                pattern: Pattern::Enum {
                    name: "Result".to_string(),
                    variant: constructors::as_str(ConstructorId::Ok).to_string(),
                    fields: vec![Pattern::Var(value_name.clone())],
                },
                guard: None,
                body: TypedExpr::new(
                    IrExprKind::Var {
                        name: value_name,
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::Value,
                    },
                    struct_ty.clone(),
                ),
            };
            let panic_message = TypedExpr::new(
                IrExprKind::Literal(super::super::super::expr::Literal::StaticStr(format!(
                    "validated newtype construction failed: {name}::{ctor}"
                ))),
                IrType::StaticStr,
            );
            let err_arm = MatchArm {
                pattern: Pattern::Enum {
                    name: "Result".to_string(),
                    variant: constructors::as_str(ConstructorId::Err).to_string(),
                    fields: vec![Pattern::Wildcard],
                },
                guard: None,
                body: TypedExpr::new(
                    IrExprKind::Call {
                        func: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: INTERNAL_PANIC_FN.to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Unknown,
                        )),
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: panic_message,
                        }],
                        callable_signature: None,
                        canonical_path: None,
                    },
                    struct_ty.clone(),
                ),
            };
            return Ok((
                IrExprKind::Match {
                    scrutinee: Box::new(from_underlying_call),
                    arms: vec![ok_arm, err_arm],
                },
                struct_ty,
            ));
        }

        // This is a constructor call - lower as struct instantiation
        // RFC 021: resolve field aliases to canonical names
        let struct_name = name.to_string();
        let fields: Vec<(String, TypedExpr)> = args
            .iter()
            .map(|arg| match arg {
                ast::CallArg::Named(field_name, value) => {
                    let lowered_value = self.lower_expr_spanned(value)?;
                    // RFC 021: map alias → canonical field name
                    let canonical = self.resolve_field_alias(&struct_name, field_name);
                    Ok((canonical, lowered_value))
                }
                ast::CallArg::Positional(value) => {
                    // Positional args - use empty string for field name
                    // (emitter will detect this and use tuple-style construction)
                    let lowered_value = self.lower_expr_spanned(value)?;
                    Ok((String::new(), lowered_value))
                }
                ast::CallArg::PositionalUnpack(value) | ast::CallArg::KeywordUnpack(value) => {
                    let lowered_value = self.lower_expr_spanned(value)?;
                    Ok((String::new(), lowered_value))
                }
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;
        Ok((
            IrExprKind::Struct {
                name: name.to_string(),
                fields,
            },
            struct_ty,
        ))
    }

    /// Lower call arguments to IR expressions.
    ///
    /// Handles positional, named, and unpack arguments.
    pub(in crate::backend::ir::lower) fn lower_call_args(
        &mut self,
        args: &[ast::CallArg],
    ) -> Result<Vec<IrCallArg>, LoweringError> {
        let mut lowered = Vec::new();
        for arg in args {
            match arg {
                ast::CallArg::Positional(e) => lowered.push(IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: self.lower_expr_spanned(e)?,
                }),
                ast::CallArg::Named(name, e) => lowered.push(IrCallArg {
                    name: Some(name.clone()),
                    kind: IrCallArgKind::Named,
                    expr: self.lower_expr_spanned(e)?,
                }),
                ast::CallArg::PositionalUnpack(e) => {
                    let expr = self.lower_expr_spanned(e)?;
                    if let Some(FixedUnpackPlan::Positional(item_types)) =
                        self.type_info.as_ref().and_then(|info| info.fixed_unpack_plan(e.span))
                    {
                        lowered.extend(self.lower_fixed_positional_unpack_args(&expr, item_types));
                    } else {
                        lowered.push(IrCallArg {
                            name: None,
                            kind: IrCallArgKind::PositionalUnpack,
                            expr,
                        });
                    }
                }
                ast::CallArg::KeywordUnpack(e) => {
                    let expr = self.lower_expr_spanned(e)?;
                    if let Some(FixedUnpackPlan::Keyword(keys)) =
                        self.type_info.as_ref().and_then(|info| info.fixed_unpack_plan(e.span))
                    {
                        lowered.extend(self.lower_fixed_keyword_unpack_args(&expr, keys));
                    } else {
                        lowered.push(IrCallArg {
                            name: None,
                            kind: IrCallArgKind::KeywordUnpack,
                            expr,
                        });
                    }
                }
            }
        }
        Ok(lowered)
    }

    /// Expand a typechecker-proven `*expr` shape into ordinary positional IR arguments.
    fn lower_fixed_positional_unpack_args(&self, expr: &TypedExpr, item_types: &[ResolvedType]) -> Vec<IrCallArg> {
        let items = match &expr.kind {
            IrExprKind::Tuple(items) => items.clone(),
            IrExprKind::List(items) => items
                .iter()
                .filter_map(|item| match item {
                    IrListEntry::Element(value) => Some(value.clone()),
                    IrListEntry::Spread(_) => None,
                })
                .collect(),
            _ => item_types
                .iter()
                .enumerate()
                .map(|(idx, ty)| {
                    TypedExpr::new(
                        IrExprKind::Field {
                            object: Box::new(expr.clone()),
                            field: idx.to_string(),
                        },
                        self.lower_resolved_type(ty),
                    )
                    .with_span(expr.span)
                })
                .collect(),
        };

        items
            .into_iter()
            .map(|expr| IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr,
            })
            .collect()
    }

    /// Expand a typechecker-proven `**expr` key set into ordinary named IR arguments.
    fn lower_fixed_keyword_unpack_args(&self, expr: &TypedExpr, keys: &[String]) -> Vec<IrCallArg> {
        let IrExprKind::Dict(entries) = &expr.kind else {
            return vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::KeywordUnpack,
                expr: expr.clone(),
            }];
        };

        entries
            .iter()
            .zip(keys.iter())
            .filter_map(|(entry, name)| match entry {
                IrDictEntry::Pair(_, value) => Some(IrCallArg {
                    name: Some(name.clone()),
                    kind: IrCallArgKind::Named,
                    expr: value.as_ref().clone(),
                }),
                IrDictEntry::Spread(_) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::AstLowering;
    use crate::backend::ir::decl::IrDeclKind;
    use crate::backend::ir::expr::{IrExprKind, MethodCallArgPolicy, VarRefKind};
    use crate::backend::ir::stmt::IrStmtKind;
    use crate::backend::ir::types::IrType;
    use crate::frontend::ast::{
        CallArg, Expr, InteropAdapterKind, InteropDirection, InteropEdgeDecl, Literal, Span, Spanned, Type,
    };
    use crate::frontend::symbols::ResolvedType;
    use crate::frontend::typechecker::{RustArgCoercionInfo, RustArgCoercionKind, TypeCheckInfo};
    use incan_core::interop::CoercionPolicy;

    fn mk_edge(
        direction: InteropDirection,
        ty: Type,
        adapter_kind: InteropAdapterKind,
        adapter_name: &str,
    ) -> InteropEdgeDecl {
        InteropEdgeDecl {
            direction,
            ty: Spanned::new(ty, Span::new(0, 0)),
            adapter_kind,
            adapter: Spanned::new(Expr::Ident(adapter_name.to_string()), Span::new(0, 0)),
        }
    }

    #[test]
    fn lower_rusttype_interop_adapter_uses_into_edge_for_rusttype_argument() -> Result<(), String> {
        let mut lowering = AstLowering::new();
        lowering.rusttype_interop_edges.insert(
            "Email".to_string(),
            vec![mk_edge(
                InteropDirection::Into,
                Type::Simple("str".to_string()),
                InteropAdapterKind::Via,
                "email_into_str",
            )],
        );

        let adapter = lowering
            .lower_rusttype_interop_adapter(&IrType::Struct("Email".to_string()), &IrType::String)
            .map_err(|err| format!("expected successful adapter lowering, got {err:?}"))?;

        assert!(adapter.is_some(), "expected into edge adapter to resolve");
        Ok(())
    }

    #[test]
    fn lower_rusttype_interop_adapter_uses_from_edge_for_rusttype_target() -> Result<(), String> {
        let mut lowering = AstLowering::new();
        lowering.rusttype_interop_edges.insert(
            "Email".to_string(),
            vec![mk_edge(
                InteropDirection::From,
                Type::Simple("str".to_string()),
                InteropAdapterKind::Try,
                "email_parse",
            )],
        );

        let adapter = lowering
            .lower_rusttype_interop_adapter(&IrType::String, &IrType::Struct("Email".to_string()))
            .map_err(|err| format!("expected successful adapter lowering, got {err:?}"))?;

        assert!(adapter.is_some(), "expected from edge adapter to resolve");
        Ok(())
    }

    #[test]
    fn lower_method_call_wraps_args_with_rust_arg_coercion() -> Result<(), String> {
        let arg_span = Span::new(10, 20);
        let mut type_info = TypeCheckInfo::default();
        type_info.rust_arg_coercions.insert(
            (arg_span.start, arg_span.end),
            RustArgCoercionInfo {
                rust_target_type: "&str".to_string(),
                target_type: ResolvedType::Str,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Borrow),
            },
        );

        let mut lowering = AstLowering::new_with_type_info(type_info);
        let expr = Expr::MethodCall(
            Box::new(Spanned::new(Expr::Ident("value".to_string()), Span::new(0, 5))),
            "coerce_me".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::String("hello".to_string())),
                arg_span,
            ))],
        );

        let lowered = lowering
            .lower_expr(&expr, Span::new(0, 100))
            .map_err(|err| format!("expected successful lowering, got {err:?}"))?;

        match lowered.kind {
            IrExprKind::MethodCall { args, .. } => {
                assert!(
                    matches!(
                        args.first().map(|arg| &arg.expr.kind),
                        Some(IrExprKind::InteropCoerce { .. })
                    ),
                    "expected first method arg to be wrapped in InteropCoerce, got {args:?}"
                );
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn lower_method_call_threads_arg_shape_hint_from_typechecker() -> Result<(), String> {
        let receiver_span = Span::new(0, 5);
        let arg_span = Span::new(10, 17);
        let mut type_info = TypeCheckInfo::default();
        type_info.record_regular_method_arg_shape(receiver_span, "get");
        type_info.rust_arg_coercions.insert(
            (arg_span.start, arg_span.end),
            RustArgCoercionInfo {
                rust_target_type: "&Q".to_string(),
                target_type: ResolvedType::Ref(Box::new(ResolvedType::RustPath("Q".to_string()))),
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Borrow),
            },
        );

        let mut lowering = AstLowering::new_with_type_info(type_info);
        let expr = Expr::MethodCall(
            Box::new(Spanned::new(Expr::Ident("value".to_string()), receiver_span)),
            "get".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::String("hello".to_string())),
                arg_span,
            ))],
        );

        let lowered = lowering
            .lower_expr(&expr, Span::new(0, 100))
            .map_err(|err| format!("expected successful lowering, got {err:?}"))?;

        match lowered.kind {
            IrExprKind::MethodCall { arg_policy, args, .. } => {
                assert_eq!(arg_policy, MethodCallArgPolicy::PreserveShape);
                assert!(
                    !matches!(
                        args.first().map(|arg| &arg.expr.kind),
                        Some(IrExprKind::InteropCoerce { .. })
                    ),
                    "expected preserved lookup method args to skip rust arg coercion wrapping, got {args:?}"
                );
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn lower_rust_import_associated_method_keeps_type_like_receiver() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::datafusion::dataframe import DataFrameWriteOptions

def f() -> None:
  _ = DataFrameWriteOptions.new()
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.first() else {
            return Err("expected expression statement body".to_string());
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected expression statement body, got {:?}", function.body));
        };

        match &expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "new");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "DataFrameWriteOptions");
                        assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_nested_rust_associated_method_arg_keeps_type_like_receiver() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::dataframe import DataFrameWriteOptions

def f(uri: str) -> None:
  ctx = SessionContext.new()
  _ = ctx.write_csv(uri, DataFrameWriteOptions.new(), None)
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.get(1) else {
            return Err(format!("expected nested write_csv statement, got {:?}", function.body));
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected let statement, got {:?}", function.body));
        };

        let IrExprKind::MethodCall { args, .. } = &expr.kind else {
            return Err(format!("expected outer MethodCall, got {:?}", expr.kind));
        };
        let nested = args
            .get(1)
            .ok_or_else(|| format!("expected second method arg, got {:?}", args))?;

        match &nested.expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "new");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "DataFrameWriteOptions");
                        assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            IrExprKind::InteropCoerce { expr, .. } => match &expr.kind {
                IrExprKind::MethodCall { receiver, method, .. } => {
                    assert_eq!(method, "new");
                    match &receiver.kind {
                        IrExprKind::Var { name, ref_kind, .. } => {
                            assert_eq!(name, "DataFrameWriteOptions");
                            assert_eq!(*ref_kind, VarRefKind::ExternalRustName);
                        }
                        other => return Err(format!("expected variable receiver, got {other:?}")),
                    }
                }
                other => return Err(format!("expected nested MethodCall in InteropCoerce, got {other:?}")),
            },
            other => return Err(format!("expected nested MethodCall arg, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_rust_constant_method_receiver_as_value_not_type_like() -> Result<(), String> {
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::std::time import Duration, UNIX_EPOCH

def f() -> None:
  duration = Duration.from_secs(1)
  _ = UNIX_EPOCH.saturating_add(duration)
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "f" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `f`".to_string())?;
        let Some(stmt) = function.body.get(1) else {
            return Err(format!("expected UNIX_EPOCH method statement, got {:?}", function.body));
        };
        let IrStmtKind::Let { value: expr, .. } = &stmt.kind else {
            return Err(format!("expected let statement, got {:?}", function.body));
        };

        match &expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "saturating_add");
                match &receiver.kind {
                    IrExprKind::Var { name, ref_kind, .. } => {
                        assert_eq!(name, "UNIX_EPOCH");
                        assert_eq!(*ref_kind, VarRefKind::Value);
                    }
                    other => return Err(format!("expected variable receiver, got {other:?}")),
                }
            }
            other => return Err(format!("expected MethodCall lowering, got {other:?}")),
        }

        Ok(())
    }

    #[test]
    fn lower_generic_box_as_ref_preserves_nominal_generic_receiver_args() -> Result<(), String> {
        use crate::backend::ir::decl::IrDeclKind;
        use crate::backend::ir::stmt::IrStmtKind;
        use crate::frontend::{lexer, parser, typechecker::TypeChecker};

        let source = r#"
from rust::std::boxed import Box

@derive(Clone)
class Node[T]:
  pub value: T

def take[T](node: Node[T]) -> T:
  return node.value

def from_box[T](child: Box[Node[T]]) -> T:
  return take(child.as_ref())
"#;
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;

        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let program = lowering
            .lower_program(&ast)
            .map_err(|err| format!("lowering failed: {err:?}"))?;

        let function = program
            .declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                IrDeclKind::Function(function) if function.name == "from_box" => Some(function),
                _ => None,
            })
            .ok_or_else(|| "expected lowered function `from_box`".to_string())?;
        let Some(stmt) = function.body.first() else {
            return Err("expected return statement body".to_string());
        };
        let IrStmtKind::Return(Some(expr)) = &stmt.kind else {
            return Err(format!("expected return statement body, got {:?}", function.body));
        };
        let IrExprKind::Call { args, .. } = &expr.kind else {
            return Err(format!("expected call expression, got {:?}", expr.kind));
        };
        let arg = args.first().ok_or_else(|| "expected call arg".to_string())?;

        match &arg.expr.kind {
            IrExprKind::MethodCall { receiver, method, .. } => {
                assert_eq!(method, "as_ref");
                assert_eq!(
                    receiver.ty,
                    IrType::NamedGeneric(
                        "Box".to_string(),
                        vec![IrType::NamedGeneric(
                            "Node".to_string(),
                            vec![IrType::Generic("T".to_string())]
                        )]
                    )
                );
            }
            other => return Err(format!("expected nested MethodCall arg, got {other:?}")),
        }

        Ok(())
    }
}
