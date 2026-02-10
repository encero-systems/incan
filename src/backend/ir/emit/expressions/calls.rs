//! Emit Rust code for function calls and binary operations.
//!
//! This module handles emission of regular function calls (user-defined functions) and binary operator expressions.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::conversions::{BinOpEmitKind, ConversionContext, determine_binop_plan, determine_conversion};
use super::super::super::expr::{BinOp, IrCallArg, IrExprKind, TypedExpr, VarAccess, VarRefKind};
use super::super::super::types::{IrType, Mutability};
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit a function call expression.
    ///
    /// Handles regular function calls (user-defined functions).
    /// Built-in functions are handled by `emit_builtin_call` or `try_emit_builtin_call`.
    pub(in super::super) fn emit_call_expr(
        &self,
        func: &TypedExpr,
        args: &[IrCallArg],
    ) -> Result<TokenStream, EmitError> {
        // Handle builtin functions specially (legacy string-based path)
        if let IrExprKind::Var { name, .. } = &func.kind {
            let positional: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
            if let Some(result) = self.try_emit_builtin_call(name, &positional)? {
                return Ok(result);
            }
        }

        let f = self.emit_expr(func)?;

        // Look up function signature
        let function_sig = if let IrExprKind::Var { name, .. } = &func.kind {
            self.function_registry.get(name)
        } else {
            None
        };

        // Order arguments only when keyword args are present (positional-only calls preserve previous behavior,
        // which is important for snapshots + for default-arg lowering work that happens elsewhere).
        let has_named_args = args.iter().any(|a| a.name.is_some());
        let ordered_args: Vec<&TypedExpr> = if has_named_args {
            if let Some(sig) = function_sig {
                let mut positional: Vec<&TypedExpr> = Vec::new();
                let mut named: std::collections::HashMap<&str, &TypedExpr> = std::collections::HashMap::new();
                for a in args {
                    if let Some(name) = a.name.as_deref() {
                        named.insert(name, &a.expr);
                    } else {
                        positional.push(&a.expr);
                    }
                }

                let mut pos_idx = 0usize;
                let mut out: Vec<&TypedExpr> = Vec::new();
                for p in &sig.params {
                    if let Some(v) = named.get(p.name.as_str()) {
                        out.push(*v);
                    } else if pos_idx < positional.len() {
                        out.push(positional[pos_idx]);
                        pos_idx += 1;
                    }
                }
                out
            } else {
                args.iter().map(|a| &a.expr).collect()
            }
        } else {
            args.iter().map(|a| &a.expr).collect()
        };

        // Handle argument passing with signature-based borrow insertion
        let arg_tokens: Vec<TokenStream> = ordered_args
            .iter()
            .enumerate()
            .map(|(idx, a)| {
                let emitted = self.emit_expr(a)?;

                // Check VarAccess for explicit borrow requirements
                if let IrExprKind::Var { access, .. } = &a.kind {
                    match access {
                        VarAccess::BorrowMut => return Ok(quote! { &mut #emitted }),
                        VarAccess::Borrow => return Ok(quote! { &#emitted }),
                        _ => {}
                    }
                }

                // If we have a function signature, use it to determine borrows
                if let Some(param) = function_sig.and_then(|sig| sig.params.get(idx)) {
                    if param.mutability == Mutability::Mutable {
                        match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => return Ok(quote! { &mut #emitted }),
                        }
                    }
                    if matches!(&param.ty, IrType::Ref(_)) {
                        match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => {
                                if !a.ty.is_copy() {
                                    return Ok(quote! { &#emitted });
                                }
                            }
                        }
                    }
                }

                // Determine conversion context based on whether this is an Incan or Rust function
                let in_return = *self.in_return_context.borrow();
                let context = if let IrExprKind::Var { name, ref_kind, .. } = &func.kind {
                    // External Rust functions: explicit rust imports or collected Rust-from imports
                    if matches!(ref_kind, VarRefKind::ExternalName) || self.external_rust_functions.contains(name) {
                        ConversionContext::ExternalFunctionArg
                    } else if in_return {
                        ConversionContext::IncanFunctionArgInReturn
                    } else {
                        ConversionContext::IncanFunctionArg
                    }
                } else if in_return {
                    ConversionContext::IncanFunctionArgInReturn
                } else {
                    ConversionContext::IncanFunctionArg
                };

                let target_ty = function_sig.and_then(|sig| sig.params.get(idx)).map(|param| &param.ty);

                let conversion = determine_conversion(a, target_ty, context);
                Ok(conversion.apply(emitted))
            })
            .collect::<Result<_, _>>()?;

        Ok(quote! { #f(#(#arg_tokens),*) })
    }

    /// Emit a binary operation expression.
    pub(in super::super) fn emit_binop_expr(
        &self,
        op: &BinOp,
        left: &TypedExpr,
        right: &TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        // Special-case: const-fold string additions using literals/known consts
        if matches!(op, BinOp::Add) {
            if let Some(tokens) = self.try_emit_static_str_add(left, right)? {
                return Ok(tokens);
            }
        }

        let l_raw = self.emit_expr(left)?;
        let r_raw = self.emit_expr(right)?;

        // Determine binop plan (conversions + emit strategy)
        let plan = determine_binop_plan(op, left, right);
        let l = plan.lhs_conv.apply(l_raw);
        let r = plan.rhs_conv.apply(r_raw);

        match plan.emit {
            BinOpEmitKind::StdlibCall { path, borrow_args } => {
                if borrow_args {
                    Ok(quote! { #path(&#l, &#r) })
                } else {
                    Ok(quote! { #path(#l, #r) })
                }
            }
            BinOpEmitKind::Pow { result_is_int } => {
                if result_is_int {
                    Ok(quote! { #l.pow(#r as u32) })
                } else {
                    Ok(quote! { #l.powf(#r) })
                }
            }
            BinOpEmitKind::Infix { token } => {
                let op_tokens = token;

                // Handle reference vs value comparisons
                let is_comparison = matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                if is_comparison {
                    let left_is_ref = matches!(&left.ty, IrType::Ref(_) | IrType::RefMut(_));
                    let right_is_value = !matches!(&right.ty, IrType::Ref(_) | IrType::RefMut(_));

                    if left_is_ref && right_is_value {
                        return Ok(quote! { *#l #op_tokens #r });
                    }
                }

                Ok(quote! { #l #op_tokens #r })
            }
        }
    }
}
