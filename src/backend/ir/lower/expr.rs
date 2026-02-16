//! Expression lowering for AST to IR conversion.
//!
//! This module handles lowering of all expression types: literals, identifiers,
//! binary/unary operations, function calls, method calls, comprehensions, etc.

use super::super::TypedExpr;
use super::super::expr::{
    BuiltinFn, IrCallArg, IrExpr, IrExprKind, MatchArm, MethodKind, Pattern, UnaryOp, VarAccess, VarRefKind,
};
use super::super::types::IrType;
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::typechecker::IdentKind;
use incan_core::PowExponentKind;

impl AstLowering {
    /// Lower an expression using the available typechecker output (if present).
    ///
    /// This wraps [`lower_expr`] and then overrides the inferred IR type using the typechecker
    /// span->type map. This is a stepping stone toward fully typed lowering.
    pub fn lower_expr_spanned(&mut self, expr: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        let mut lowered = self.lower_expr(&expr.node)?;
        if let Some(info) = &self.type_info {
            if let Some(res_ty) = info.expr_type(expr.span) {
                // Preserve reference wrappers introduced by lowering (e.g. mutable parameters are tracked as
                // `RefMut(T)` in IR), while still benefiting from the typechecker’s inner type information.
                //
                // The frontend type system does not model references, so `expr_type` typically returns `T`
                // where lowering may have already marked the same binding as `Ref(T)`/`RefMut(T)`.
                let inner = self.lower_resolved_type(res_ty);
                lowered.ty = match &lowered.ty {
                    IrType::Ref(_) => IrType::Ref(Box::new(inner)),
                    IrType::RefMut(_) => IrType::RefMut(Box::new(inner)),
                    _ => inner,
                };
            }
            if let Some(kind) = info.ident_kind(expr.span)
                && let IrExprKind::Var { ref mut ref_kind, .. } = lowered.kind
            {
                *ref_kind = match kind {
                    IdentKind::Value => VarRefKind::Value,
                    IdentKind::TypeName => VarRefKind::TypeName,
                    IdentKind::Variant => VarRefKind::TypeName,
                    IdentKind::Module => VarRefKind::ExternalName,
                    IdentKind::RustImport => VarRefKind::ExternalName,
                    IdentKind::Trait => VarRefKind::TypeName,
                };
            }
        }
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
    /// # Parameters
    ///
    /// * `expr` - The AST expression to lower
    ///
    /// # Returns
    ///
    /// A typed IR expression.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if the expression cannot be lowered.
    pub fn lower_expr(&mut self, expr: &ast::Expr) -> Result<TypedExpr, LoweringError> {
        let (kind, ty) = match expr {
            ast::Expr::Ident(name) => {
                let ty = self.lookup_var(name);
                let access = self.select_var_access_for_ident(name, &ty);
                (
                    IrExprKind::Var {
                        name: name.clone(),
                        access,
                        ref_kind: VarRefKind::Value,
                    },
                    ty,
                )
            }

            ast::Expr::Literal(lit) => match lit {
                ast::Literal::Int(n) => (IrExprKind::Int(*n), IrType::Int),
                ast::Literal::Float(n) => (IrExprKind::Float(*n), IrType::Float),
                ast::Literal::String(s) => (IrExprKind::String(s.clone()), IrType::String),
                ast::Literal::Bytes(bytes) => (IrExprKind::Bytes(bytes.clone()), IrType::Unknown),
                ast::Literal::Bool(b) => (IrExprKind::Bool(*b), IrType::Bool),
                ast::Literal::None => (IrExprKind::None, IrType::Option(Box::new(IrType::Unknown))),
            },

            ast::Expr::SelfExpr => (
                IrExprKind::Var {
                    name: "self".to_string(),
                    access: VarAccess::Borrow,
                    ref_kind: VarRefKind::Value,
                },
                IrType::Unknown,
            ),

            ast::Expr::Binary(l, op, r) => {
                // Special handling for `in` and `not in` operators
                // `x in collection` → `collection.contains(&x)`
                // `x not in collection` → `!collection.contains(&x)`
                match op {
                    ast::BinaryOp::In | ast::BinaryOp::NotIn => {
                        let item = self.lower_expr(&l.node)?;
                        let collection = self.lower_expr(&r.node)?;

                        // Generate collection.contains(&item)
                        let contains_call = IrExprKind::MethodCall {
                            receiver: Box::new(collection),
                            method: "contains".to_string(),
                            args: vec![IrCallArg { name: None, expr: item }],
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
                                op: self.lower_binop(op),
                                left: Box::new(left),
                                right: Box::new(right),
                            },
                            result_ty,
                        )
                    }
                }
            }

            ast::Expr::Unary(op, e) => {
                let operand = self.lower_expr(&e.node)?;
                let ty = operand.ty.clone();
                (
                    IrExprKind::UnaryOp {
                        op: match op {
                            ast::UnaryOp::Neg => UnaryOp::Neg,
                            ast::UnaryOp::Not => UnaryOp::Not,
                        },
                        operand: Box::new(operand),
                    },
                    ty,
                )
            }

            ast::Expr::Call(f, args) => {
                // Check if this is a struct/model/class constructor call
                if let ast::Expr::Ident(name) = &f.node {
                    // Use two strategies for constructor detection:
                    // 1. Known struct from current file (in struct_names map)
                    // 2. Uppercase identifier heuristic (works cross-file like old codegen)
                    let is_known_struct = self.struct_names.contains_key(name);
                    let is_uppercase = name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

                    if is_known_struct || is_uppercase {
                        // Get type if known, otherwise Unknown (will be inferred at emit time)
                        let struct_ty = self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown);

                        // ----------------------------------------------------------------
                        // Newtype checked construction (v0.1 hardening for #44, RFC runway)
                        // ----------------------------------------------------------------
                        // When T(x) is called for a newtype with a validated constructor (from_underlying or single
                        // matching from_*):
                        //
                        //   1. Skip rewriting if we're inside `impl T` to avoid infinite recursion when the ctor itself
                        //  calls T(x).
                        //   2. Otherwise rewrite to: T::from_underlying(x).expect("...")  (fail-fast with context)
                        //
                        // This ensures newtype invariants are enforced at construction.
                        // ----------------------------------------------------------------
                        if self.newtype_checked_ctor.contains_key(name)
                            && args.len() == 1
                            && matches!(args[0], ast::CallArg::Positional(_))
                            && self.current_impl_type.as_deref() != Some(name.as_str())
                        {
                            let ast::CallArg::Positional(value) = &args[0] else {
                                unreachable!("checked by matches! above")
                            };
                            let lowered_value = self.lower_expr(&value.node)?;
                            let ctor = self
                                .newtype_checked_ctor
                                .get(name)
                                .cloned()
                                .unwrap_or_else(|| "from_underlying".to_string());

                            // Use the actual newtype struct type for the receiver (not Unknown)
                            let receiver = TypedExpr::new(
                                IrExprKind::Var {
                                    name: name.clone(),
                                    access: VarAccess::Copy,
                                    ref_kind: VarRefKind::TypeName,
                                },
                                struct_ty.clone(),
                            );
                            let from_underlying_call = TypedExpr::new(
                                IrExprKind::MethodCall {
                                    receiver: Box::new(receiver),
                                    method: ctor.clone(),
                                    args: vec![IrCallArg {
                                        name: None,
                                        expr: lowered_value,
                                    }],
                                },
                                IrType::Result(Box::new(struct_ty.clone()), Box::new(IrType::Unknown)),
                            );
                            // Prefer `expect` over `unwrap` so panics carry context. Note: for `Result`,
                            // Rust's `expect` includes the `Err(...)` payload via Debug formatting.
                            let msg = TypedExpr::new(
                                IrExprKind::Literal(super::super::expr::Literal::StaticStr(format!(
                                    "validated newtype construction failed: {name}::{ctor}"
                                ))),
                                IrType::StaticStr,
                            );
                            return Ok(TypedExpr::new(
                                IrExprKind::MethodCall {
                                    receiver: Box::new(from_underlying_call),
                                    method: "expect".to_string(),
                                    args: vec![IrCallArg { name: None, expr: msg }],
                                },
                                struct_ty,
                            ));
                        }

                        // This is a constructor call - lower as struct instantiation
                        // RFC 021: resolve field aliases to canonical names
                        let struct_name = name.clone();
                        let fields: Vec<(String, TypedExpr)> = args
                            .iter()
                            .map(|arg| {
                                match arg {
                                    ast::CallArg::Named(field_name, value) => {
                                        let lowered_value = self.lower_expr(&value.node)?;
                                        // RFC 021: map alias → canonical field name
                                        let canonical = self.resolve_field_alias(&struct_name, field_name);
                                        Ok((canonical, lowered_value))
                                    }
                                    ast::CallArg::Positional(value) => {
                                        // Positional args - use empty string for field name
                                        // (emitter will detect this and use tuple-style construction)
                                        let lowered_value = self.lower_expr(&value.node)?;
                                        Ok((String::new(), lowered_value))
                                    }
                                }
                            })
                            .collect::<Result<Vec<_>, LoweringError>>()?;
                        return Ok(TypedExpr::new(
                            IrExprKind::Struct {
                                name: name.clone(),
                                fields,
                            },
                            struct_ty,
                        ));
                    }

                    // Check for known builtins (enum-based dispatch)
                    if let Some(builtin) = BuiltinFn::from_name(name) {
                        let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
                        return Ok(TypedExpr::new(
                            IrExprKind::BuiltinCall {
                                func: builtin,
                                args: args_ir,
                            },
                            IrType::Unknown, // Return type depends on the builtin
                        ));
                    }
                }

                // Regular function call (user-defined or unknown)
                let func = self.lower_expr_spanned(f)?;
                let args_ir = self.lower_call_args(args)?;
                let ret_ty = if let IrType::Function { ret, .. } = &func.ty {
                    (**ret).clone()
                } else {
                    IrType::Unknown
                };
                (
                    IrExprKind::Call {
                        func: Box::new(func),
                        args: args_ir,
                    },
                    ret_ty,
                )
            }

            ast::Expr::MethodCall(o, m, args) => {
                let receiver = self.lower_expr_spanned(o)?;
                let args_ir = self.lower_call_args(args)?;

                // Check for known methods (enum-based dispatch)
                if let Some(kind) = MethodKind::from_name(m) {
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
                            method: m.clone(),
                            args: args_ir,
                        },
                        IrType::Unknown,
                    )
                }
            }

            ast::Expr::Index(o, i) => {
                let obj = self.lower_expr(&o.node)?;
                let idx = self.lower_expr(&i.node)?;
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

            ast::Expr::Field(o, f) => {
                // Prefer spanned lowering so typechecker output can drive the receiver type.
                // This is important for RFC 021 alias-aware field access, especially for `self.<alias>`.
                let obj = self.lower_expr_spanned(o)?;
                // RFC 021: resolve field alias to canonical name if object is a known struct type
                let struct_name = match &obj.ty {
                    IrType::Struct(struct_name) => Some(struct_name.as_str()),
                    _ => match &obj.kind {
                        IrExprKind::Var { name, .. } if name == "self" => self.current_impl_type.as_deref(),
                        _ => None,
                    },
                };
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

            ast::Expr::Await(e) => {
                let inner = self.lower_expr(&e.node)?;
                let ty = inner.ty.clone();
                (IrExprKind::Await(Box::new(inner)), ty)
            }

            ast::Expr::Try(e) => {
                let inner = self.lower_expr(&e.node)?;
                let ty = match &inner.ty {
                    IrType::Result(ok, _) => (**ok).clone(),
                    _ => inner.ty.clone(),
                };
                (IrExprKind::Try(Box::new(inner)), ty)
            }

            ast::Expr::Match(s, arms) => {
                let lowered_match = (|| -> Result<(IrExprKind, IrType), LoweringError> {
                    let scrutinee = self.lower_expr(&s.node)?;
                    let arms_ir = self.lower_match_arms(arms)?;
                    let ty = arms_ir.first().map(|a| a.body.ty.clone()).unwrap_or(IrType::Unknown);
                    Ok((
                        IrExprKind::Match {
                            scrutinee: Box::new(scrutinee),
                            arms: arms_ir,
                        },
                        ty,
                    ))
                })();
                lowered_match?
            }

            ast::Expr::If(i) => {
                let lowered_if = (|| -> Result<(IrExprKind, IrType), LoweringError> {
                    let cond = self.lower_expr(&i.condition.node)?;
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
                    Ok((
                        IrExprKind::If {
                            condition: Box::new(cond),
                            then_branch: Box::new(then_expr),
                            else_branch: else_expr.map(Box::new),
                        },
                        IrType::Unit,
                    ))
                })();
                lowered_if?
            }

            ast::Expr::Closure(params, body) => {
                let param_pairs: Vec<(String, IrType)> = params
                    .iter()
                    .map(|p| (p.node.name.clone(), self.lower_type(&p.node.ty.node)))
                    .collect();
                self.non_linear_context_depth += 1;
                let body_ir_result = self.lower_expr(&body.node);
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

            ast::Expr::Tuple(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr(&i.node))
                    .collect::<Result<_, _>>()?;
                let tys: Vec<IrType> = items_ir.iter().map(|i| i.ty.clone()).collect();
                (IrExprKind::Tuple(items_ir), IrType::Tuple(tys))
            }

            ast::Expr::List(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr(&i.node))
                    .collect::<Result<_, _>>()?;
                let elem = items_ir.first().map(|i| i.ty.clone()).unwrap_or(IrType::Unknown);
                (IrExprKind::List(items_ir), IrType::List(Box::new(elem)))
            }

            ast::Expr::Dict(pairs) => {
                let pairs_ir: Vec<(TypedExpr, TypedExpr)> = pairs
                    .iter()
                    .map(|(k, v)| Ok((self.lower_expr(&k.node)?, self.lower_expr(&v.node)?)))
                    .collect::<Result<_, LoweringError>>()?;
                let (k, v) = pairs_ir
                    .first()
                    .map(|(k, v)| (k.ty.clone(), v.ty.clone()))
                    .unwrap_or((IrType::Unknown, IrType::Unknown));
                (IrExprKind::Dict(pairs_ir), IrType::Dict(Box::new(k), Box::new(v)))
            }

            ast::Expr::Set(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr(&i.node))
                    .collect::<Result<_, _>>()?;
                let elem = items_ir.first().map(|i| i.ty.clone()).unwrap_or(IrType::Unknown);
                (IrExprKind::Set(items_ir), IrType::Set(Box::new(elem)))
            }

            ast::Expr::Paren(e) => return self.lower_expr(&e.node),

            ast::Expr::Constructor(name, args) => {
                let fields: Vec<(String, TypedExpr)> = args
                    .iter()
                    .map(|arg| match arg {
                        ast::CallArg::Named(n, e) => Ok((n.clone(), self.lower_expr(&e.node)?)),
                        ast::CallArg::Positional(e) => Ok((String::new(), self.lower_expr(&e.node)?)),
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

            ast::Expr::Range { start, end, inclusive } => {
                let s = self.lower_expr(&start.node)?;
                let e = self.lower_expr(&end.node)?;
                (
                    IrExprKind::Range {
                        start: Some(Box::new(s)),
                        end: Some(Box::new(e)),
                        inclusive: *inclusive,
                    },
                    IrType::Unknown,
                )
            }

            ast::Expr::FString(parts) => {
                // Lower f-string parts to Format IR
                let ir_parts: Vec<super::super::expr::FormatPart> = parts
                    .iter()
                    .map(|part| match part {
                        ast::FStringPart::Literal(s) => Ok(super::super::expr::FormatPart::Literal(s.clone())),
                        ast::FStringPart::Expr(e) => {
                            let lowered = self.lower_expr(&e.node)?;
                            Ok(super::super::expr::FormatPart::Expr(lowered))
                        }
                    })
                    .collect::<Result<Vec<_>, LoweringError>>()?;
                (IrExprKind::Format { parts: ir_parts }, IrType::String)
            }

            ast::Expr::Slice(target, slice) => {
                let target_expr = self.lower_expr(&target.node)?;
                let start = slice
                    .start
                    .as_ref()
                    .map(|s| Ok(Box::new(self.lower_expr(&s.node)?)))
                    .transpose()?;
                let end = slice
                    .end
                    .as_ref()
                    .map(|e| Ok(Box::new(self.lower_expr(&e.node)?)))
                    .transpose()?;
                let step = slice
                    .step
                    .as_ref()
                    .map(|st| Ok(Box::new(self.lower_expr(&st.node)?)))
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

            ast::Expr::ListComp(comp) => {
                // [expr for var in iter if cond]
                // → iter.iter().filter(|var| cond).map(|var| expr).collect()
                let iter_expr = self.lower_expr(&comp.iter.node)?;
                let var_name = comp.var.clone();

                // Build the filter predicate if present
                self.non_linear_context_depth += 1;
                let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> =
                    if let Some(filter) = &comp.filter {
                        Ok(Some(Box::new(self.lower_expr(&filter.node)?)))
                    } else {
                        Ok(None)
                    };

                // Build the map expression
                let map_expr_result = self.lower_expr(&comp.expr.node);
                self.non_linear_context_depth -= 1;
                let filter_tokens = filter_tokens_result?;
                let map_expr = map_expr_result?;

                // Determine element type from map expression
                let elem_ty = map_expr.ty.clone();

                (
                    IrExprKind::ListComp {
                        element: Box::new(map_expr),
                        variable: var_name,
                        iterable: Box::new(iter_expr),
                        filter: filter_tokens,
                    },
                    IrType::List(Box::new(elem_ty)),
                )
            }

            ast::Expr::DictComp(comp) => {
                // {key: value for var in iter if cond}
                let iter_expr = self.lower_expr(&comp.iter.node)?;
                let var_name = comp.var.clone();

                self.non_linear_context_depth += 1;
                let filter_tokens_result: Result<Option<Box<TypedExpr>>, LoweringError> =
                    if let Some(filter) = &comp.filter {
                        Ok(Some(Box::new(self.lower_expr(&filter.node)?)))
                    } else {
                        Ok(None)
                    };

                let key_expr_result = self.lower_expr(&comp.key.node);
                let value_expr_result = self.lower_expr(&comp.value.node);
                self.non_linear_context_depth -= 1;
                let filter_tokens = filter_tokens_result?;
                let key_expr = key_expr_result?;
                let value_expr = value_expr_result?;

                let key_ty = key_expr.ty.clone();
                let value_ty = value_expr.ty.clone();

                (
                    IrExprKind::DictComp {
                        key: Box::new(key_expr),
                        value: Box::new(value_expr),
                        variable: var_name,
                        iterable: Box::new(iter_expr),
                        filter: filter_tokens,
                    },
                    IrType::Dict(Box::new(key_ty), Box::new(value_ty)),
                )
            }

            // Expressions that need desugaring (emit placeholder for now)
            ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown),
        };
        Ok(TypedExpr::new(kind, ty))
    }

    /// Lower call arguments to IR expressions.
    ///
    /// Handles both positional and named arguments.
    ///
    /// # Parameters
    ///
    /// * `args` - The AST call arguments
    ///
    /// # Returns
    ///
    /// A vector of typed IR expressions.
    pub(super) fn lower_call_args(&mut self, args: &[ast::CallArg]) -> Result<Vec<IrCallArg>, LoweringError> {
        args.iter()
            .map(|a| match a {
                ast::CallArg::Positional(e) => Ok(IrCallArg {
                    name: None,
                    expr: self.lower_expr_spanned(e)?,
                }),
                ast::CallArg::Named(name, e) => Ok(IrCallArg {
                    name: Some(name.clone()),
                    expr: self.lower_expr_spanned(e)?,
                }),
            })
            .collect()
    }

    /// Lower match arms to IR.
    ///
    /// # Parameters
    ///
    /// * `arms` - The AST match arms
    ///
    /// # Returns
    ///
    /// A vector of IR match arms.
    pub(super) fn lower_match_arms(&mut self, arms: &[Spanned<ast::MatchArm>]) -> Result<Vec<MatchArm>, LoweringError> {
        arms.iter()
            .map(|a| {
                let pattern = self.lower_pattern(&a.node.pattern.node);
                let guard = a.node.guard.as_ref().map(|g| self.lower_expr(&g.node)).transpose()?;
                let body = match &a.node.body {
                    ast::MatchBody::Expr(e) => self.lower_expr(&e.node)?,
                    ast::MatchBody::Block(stmts) => {
                        let ir_stmts = self.lower_statements(stmts)?;
                        TypedExpr::new(
                            IrExprKind::Block {
                                stmts: ir_stmts,
                                value: None,
                            },
                            IrType::Unit,
                        )
                    }
                };
                Ok(MatchArm { pattern, guard, body })
            })
            .collect()
    }

    /// Lower a pattern to IR.
    ///
    /// Handles wildcard, binding, literal, constructor, and tuple patterns.
    ///
    /// # Parameters
    ///
    /// * `p` - The AST pattern
    ///
    /// # Returns
    ///
    /// The corresponding IR pattern.
    pub(super) fn lower_pattern(&mut self, p: &ast::Pattern) -> Pattern {
        match p {
            ast::Pattern::Wildcard => Pattern::Wildcard,
            ast::Pattern::Binding(name) => Pattern::Var(name.clone()),
            ast::Pattern::Literal(lit) => {
                // Lower the literal to an IR expression
                // If lowering fails (unlikely for literals), fall back to wildcard
                self.lower_expr(&ast::Expr::Literal(lit.clone()))
                    .map(Pattern::Literal)
                    .unwrap_or(Pattern::Wildcard)
            }
            ast::Pattern::Constructor(name, args) => {
                let mut named_fields = Vec::new();
                let mut positional_fields = Vec::new();
                let mut has_named = false;

                for arg in args {
                    match arg {
                        ast::PatternArg::Named(field, pat) => {
                            has_named = true;
                            // RFC 021: resolve field alias to canonical name for struct patterns
                            let canonical = self.resolve_field_alias(name, field);
                            named_fields.push((canonical, self.lower_pattern(&pat.node)));
                        }
                        ast::PatternArg::Positional(pat) => {
                            positional_fields.push(self.lower_pattern(&pat.node));
                        }
                    }
                }

                if has_named {
                    Pattern::Struct {
                        name: name.clone(),
                        fields: named_fields,
                    }
                } else {
                    let mut fields = positional_fields;
                    if has_named {
                        fields.extend(named_fields.into_iter().map(|(_, pat)| pat));
                    }
                    Pattern::Enum {
                        name: String::new(),
                        variant: name.clone(),
                        fields,
                    }
                }
            }
            ast::Pattern::Tuple(items) => Pattern::Tuple(items.iter().map(|i| self.lower_pattern(&i.node)).collect()),
        }
    }

    /// Determine PowExponentKind for a power expression's right operand.
    ///
    /// Used to implement Python-like `**` semantics where `int ** int` yields `Int`
    /// only for non-negative int literal exponents; otherwise `Float`.
    fn pow_exponent_kind(right_ast: &Spanned<ast::Expr>, right_ty: &IrType) -> PowExponentKind {
        let rhs_is_float = matches!(right_ty, IrType::Float);
        let rhs_int_literal = Self::extract_int_literal(right_ast);
        PowExponentKind::from_literal_info(rhs_is_float, rhs_int_literal)
    }

    /// Extract an integer literal value from an AST expression.
    fn extract_int_literal(expr: &Spanned<ast::Expr>) -> Option<i64> {
        match &expr.node {
            ast::Expr::Literal(ast::Literal::Int(n)) => Some(*n),
            ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
                if let ast::Expr::Literal(ast::Literal::Int(n)) = &inner.node {
                    Some(-n)
                } else {
                    None
                }
            }
            ast::Expr::Paren(inner) => Self::extract_int_literal(inner),
            _ => None,
        }
    }
}
