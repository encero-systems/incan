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
use super::super::expr::{IrCallArg, IrExpr, IrExprKind, MethodKind, UnaryOp, VarAccess, VarRefKind};
use super::super::types::IrType;
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::typechecker::IdentKind;
use incan_semantics_core::SurfaceExprLoweringAction;

impl AstLowering {
    /// Lower an expression using the available typechecker output (if present).
    ///
    /// This wraps [`lower_expr`] and then overrides the inferred IR type using the typechecker  span-to-type map.
    /// This is a stepping stone toward fully typed lowering.
    pub fn lower_expr_spanned(&mut self, expr: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        let mut lowered = self.lower_expr(&expr.node)?;
        if let Some(info) = &self.type_info {
            if let Some(res_ty) = info.expr_type(expr.span) {
                // Preserve reference wrappers introduced by lowering (e.g. mutable parameters are tracked as
                // `RefMut(T)` in IR), while still benefiting from the typechecker's inner type information.
                //
                // The frontend type system does not model references, so `expr_type` typically returns `T` where
                // lowering may have already marked the same binding as `Ref(T)`/`RefMut(T)`.
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
                    IdentKind::RustImport => VarRefKind::ExternalRustName,
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
    pub fn lower_expr(&mut self, expr: &ast::Expr) -> Result<TypedExpr, LoweringError> {
        let (kind, ty) = match expr {
            // ---- Identifiers ----
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

            // ---- Literals ----
            ast::Expr::Literal(lit) => match lit {
                ast::Literal::Int(n) => (IrExprKind::Int(*n), IrType::Int),
                ast::Literal::Float(n) => (IrExprKind::Float(*n), IrType::Float),
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

            // ---- Unary operations ----
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

            // ---- Function / constructor calls (delegated to calls submodule) ----
            ast::Expr::Call(f, args) => return self.lower_call_expr(f, args).map(|(k, t)| TypedExpr::new(k, t)),

            // ---- Method calls ----
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

            // ---- Index access ----
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

            // ---- Field access ----
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
                        let lowered_inner = self.lower_expr(&inner.node)?;
                        super::super::surface_semantics::lower_await_expression(lowered_inner)
                    }
                }
            }

            // ---- Try (?) ----
            ast::Expr::Try(e) => {
                let inner = self.lower_expr(&e.node)?;
                let ty = match &inner.ty {
                    IrType::Result(ok, _) => (**ok).clone(),
                    _ => inner.ty.clone(),
                };
                (IrExprKind::Try(Box::new(inner)), ty)
            }

            // ---- Match expressions (delegated to patterns submodule) ----
            ast::Expr::Match(s, arms) => {
                let scrutinee = self.lower_expr(&s.node)?;
                let arms_ir = self.lower_match_arms(arms)?;
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
                (
                    IrExprKind::If {
                        condition: Box::new(cond),
                        then_branch: Box::new(then_expr),
                        else_branch: else_expr.map(Box::new),
                    },
                    IrType::Unit,
                )
            }

            // ---- Closures ----
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

            // ---- Collection literals ----
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

            // ---- Parenthesized expression (transparent) ----
            ast::Expr::Paren(e) => return self.lower_expr(&e.node),

            // ---- Constructor (variant / struct literal) ----
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

            // ---- Range expressions ----
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

            // ---- F-strings ----
            ast::Expr::FString(parts) => {
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

            // ---- Slice expressions ----
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

            // ---- Comprehensions (delegated to comprehensions submodule) ----
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp)?,
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp)?,

            // ---- Yield (placeholder) ----
            ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown),
        };
        Ok(TypedExpr::new(kind, ty))
    }
}
