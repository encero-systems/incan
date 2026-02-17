//! Call expression lowering: struct constructors, builtin dispatch, newtype checked construction, and regular function
//! calls.

use super::super::super::TypedExpr;
use super::super::super::expr::{BuiltinFn, IrCallArg, IrExprKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;

impl AstLowering {
    /// Lower a function/constructor call expression.
    ///
    /// Handles struct constructors, builtin functions, newtype checked construction, and regular function calls.
    pub(in crate::backend::ir::lower) fn lower_call_expr(
        &mut self,
        f: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
    ) -> Result<(IrExprKind, IrType), LoweringError> {
        // Check if this is a struct/model/class constructor call
        if let ast::Expr::Ident(name) = &f.node {
            // Use two strategies for constructor detection:
            // 1. Known struct from current file (in struct_names map)
            // 2. Uppercase identifier heuristic (works cross-file like old codegen)
            let is_known_struct = self.struct_names.contains_key(name);
            let is_uppercase = name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

            if is_known_struct || is_uppercase {
                return self.lower_constructor_call(name, args);
            }

            // Check for known builtins (enum-based dispatch)
            if let Some(builtin) = BuiltinFn::from_name(name) {
                let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
                return Ok((
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
        Ok((
            IrExprKind::Call {
                func: Box::new(func),
                args: args_ir,
                canonical_path: None,
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
            let lowered_value = self.lower_expr(&value.node)?;
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
                IrExprKind::Literal(super::super::super::expr::Literal::StaticStr(format!(
                    "validated newtype construction failed: {name}::{ctor}"
                ))),
                IrType::StaticStr,
            );
            return Ok((
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
        let struct_name = name.to_string();
        let fields: Vec<(String, TypedExpr)> = args
            .iter()
            .map(|arg| match arg {
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
    /// Handles both positional and named arguments.
    pub(in crate::backend::ir::lower) fn lower_call_args(
        &mut self,
        args: &[ast::CallArg],
    ) -> Result<Vec<IrCallArg>, LoweringError> {
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
}
