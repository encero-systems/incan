//! Statement emission for IR to Rust code generation
//!
//! This module handles emitting Rust statements from IR statements,
//! including let bindings, assignments, control flow, and blocks.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::conversions::{ConversionContext, determine_conversion};
use super::super::expr::{IrExprKind, Pattern};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::super::types::Mutability;
use super::{EmitError, IrEmitter};

/// Determine whether a `for` loop body requires mutable iteration of the loop variable.
///
/// We use this as a *codegen heuristic* to avoid emitting `.iter_mut()` when the loop body performs no mutation of the
/// loop item. Emitting `.iter_mut()`:
///
/// - requires mutable access to the source collection, and
/// - changes the loop item type from `&T` to `&mut T`.
fn for_body_needs_mut_iteration(pattern: &Pattern, body: &[IrStmt]) -> bool {
    let loop_var = match pattern {
        Pattern::Var(name) => name.as_str(),
        _ => return false,
    };

    /// Get the root variable name of an expression.
    fn root_var_name(expr: &super::super::expr::IrExpr) -> Option<&str> {
        match &expr.kind {
            IrExprKind::Var { name, .. } => Some(name.as_str()),
            IrExprKind::Field { object, .. } => root_var_name(object),
            IrExprKind::Index { object, .. } => root_var_name(object),
            _ => None,
        }
    }

    /// Check if an assignment target mutates a variable.
    fn target_mutates_var(target: &AssignTarget, var: &str) -> bool {
        match target {
            AssignTarget::Var(name) => name == var,
            AssignTarget::Field { object, .. } => root_var_name(object).is_some_and(|n| n == var),
            AssignTarget::Index { object, .. } => root_var_name(object).is_some_and(|n| n == var),
        }
    }

    /// Check if an expression contains a mutation of a variable.
    fn expr_contains_mutation(expr: &super::super::expr::IrExpr, var: &str) -> bool {
        match &expr.kind {
            IrExprKind::Block { stmts, value } => {
                stmts.iter().any(|s| stmt_mutates_var(s, var))
                    || value.as_ref().is_some_and(|v| expr_contains_mutation(v, var))
            }
            _ => false,
        }
    }

    /// Check if a statement mutates a variable.
    fn stmt_mutates_var(stmt: &IrStmt, var: &str) -> bool {
        match &stmt.kind {
            IrStmtKind::Assign { target, .. } => target_mutates_var(target, var),
            IrStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch.iter().any(|s| stmt_mutates_var(s, var))
                    || else_branch
                        .as_ref()
                        .is_some_and(|b| b.iter().any(|s| stmt_mutates_var(s, var)))
            }
            IrStmtKind::While { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::For { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Loop { body, .. } => body.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Block(stmts) => stmts.iter().any(|s| stmt_mutates_var(s, var)),
            IrStmtKind::Match { arms, .. } => arms.iter().any(|arm| expr_contains_mutation(&arm.body, var)),
            _ => false,
        }
    }

    body.iter().any(|s| stmt_mutates_var(s, loop_var))
}

impl<'a> IrEmitter<'a> {
    /// Emit a statement as Rust tokens.
    pub(super) fn emit_stmt(&self, stmt: &IrStmt) -> Result<TokenStream, EmitError> {
        match &stmt.kind {
            IrStmtKind::Expr(expr) => {
                // Lowering currently models tuple-unpack/chained-assignment expansion as a block
                // expression used in statement position. Emit those inner statements directly so
                // the introduced bindings remain visible to following statements.
                if let IrExprKind::Block { stmts, value: None } = &expr.kind {
                    let inner: Vec<TokenStream> = stmts.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                    return Ok(quote! { #(#inner)* });
                }
                let e = self.emit_expr(expr)?;
                Ok(quote! { #e; })
            }
            IrStmtKind::Let {
                name,
                ty,
                mutability,
                value,
            } => {
                let n = Self::rust_ident(name);
                let v = self.emit_expr(value)?;

                // Apply conversion if needed based on variable type
                let conversion = determine_conversion(value, Some(ty), ConversionContext::Assignment);
                let converted_v = conversion.apply(v);

                if matches!(mutability, Mutability::Mutable) {
                    Ok(quote! { let mut #n = #converted_v; })
                } else {
                    Ok(quote! { let #n = #converted_v; })
                }
            }
            IrStmtKind::Assign { target, value } => {
                // For Dict index assignment, use .insert() instead of []=
                // because HashMap's IndexMut doesn't work with owned keys
                if let AssignTarget::Index { object, index } = target
                    && matches!(&object.ty, IrType::Dict(_, _) | IrType::Unknown)
                {
                    let o = self.emit_expr(object)?;
                    let k = self.emit_expr(index)?;
                    let v = self.emit_expr(value)?;
                    return Ok(quote! { #o.insert(#k, #v); });
                }
                let t = self.emit_assign_target(target)?;
                let v = self.emit_expr(value)?;
                Ok(quote! { #t = #v; })
            }
            IrStmtKind::Return(Some(expr)) => {
                // Set return context so function calls inside can use move semantics
                *self.in_return_context.borrow_mut() = true;
                let e = self.emit_expr(expr)?;
                *self.in_return_context.borrow_mut() = false;

                // Apply conversion if needed based on function return type
                let converted = if let Some(return_type) = self.current_function_return_type.borrow().as_ref() {
                    let conversion = determine_conversion(expr, Some(return_type), ConversionContext::ReturnValue);
                    conversion.apply(e)
                } else {
                    e
                };

                Ok(quote! { return #converted; })
            }
            IrStmtKind::Return(None) => Ok(quote! { return; }),
            IrStmtKind::Break(label) => {
                if let Some(l) = label {
                    let label_lifetime = syn::Lifetime::new(&format!("'{}", l), proc_macro2::Span::call_site());
                    Ok(quote! { break #label_lifetime; })
                } else {
                    Ok(quote! { break; })
                }
            }
            IrStmtKind::Continue(label) => {
                if let Some(l) = label {
                    let label_lifetime = syn::Lifetime::new(&format!("'{}", l), proc_macro2::Span::call_site());
                    Ok(quote! { continue #label_lifetime; })
                } else {
                    Ok(quote! { continue; })
                }
            }
            IrStmtKind::While {
                label: _,
                condition,
                body,
            } => {
                let body_stmts: Vec<TokenStream> = body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                let is_infinite = matches!(condition.kind, IrExprKind::Bool(true));
                if is_infinite {
                    Ok(quote! {
                        loop {
                            #(#body_stmts)*
                        }
                    })
                } else {
                    let cond = self.emit_expr(condition)?;
                    Ok(quote! {
                        while #cond {
                            #(#body_stmts)*
                        }
                    })
                }
            }
            IrStmtKind::For {
                label: _,
                pattern,
                iterable,
                body,
            } => {
                let pat = self.emit_pattern(pattern);
                let iter = self.emit_expr(iterable)?;
                let body_stmts: Vec<TokenStream> = body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                // For non-copy collections, iterate by reference to avoid move
                // This handles the common case where a collection is used multiple times
                // For primitive element types, use .iter().copied() to get values instead of references
                let needs_mut_items = for_body_needs_mut_iteration(pattern, body);
                let iterable_is_borrowable_lvalue = matches!(
                    &iterable.kind,
                    IrExprKind::Var { .. } | IrExprKind::Field { .. } | IrExprKind::Index { .. }
                );
                let iter_expr = match &iterable.ty {
                    // If the iterable is a mutable reference to a collection, use .iter_mut() for non-Copy types
                    IrType::RefMut(inner) => {
                        match inner.as_ref() {
                            IrType::List(elem_ty) => {
                                // For primitive types, use .iter().copied() to get values
                                // For non-Copy types (structs), use .iter_mut() to allow mutation
                                match elem_ty.as_ref() {
                                    IrType::Int | IrType::Float | IrType::Bool => {
                                        quote! { #iter.iter().copied() }
                                    }
                                    _ => quote! { #iter.iter_mut() },
                                }
                            }
                            IrType::Set(_) | IrType::Dict(_, _) => {
                                quote! { #iter.iter_mut() }
                            }
                            _ => quote! { #iter },
                        }
                    }
                    // If the iterable is an immutable reference, use .iter()
                    IrType::Ref(inner) => match inner.as_ref() {
                        IrType::List(elem_ty) => match elem_ty.as_ref() {
                            IrType::Int | IrType::Float | IrType::Bool => {
                                quote! { #iter.iter().copied() }
                            }
                            _ => quote! { #iter.iter() },
                        },
                        IrType::Set(_) | IrType::Dict(_, _) => {
                            quote! { #iter.iter() }
                        }
                        _ => quote! { #iter },
                    },
                    IrType::List(elem_ty) => {
                        // If it's a borrowable lvalue (var/field/index), iterate by reference to avoid moving.
                        if iterable_is_borrowable_lvalue {
                            // For primitive types, use .iter().copied() to avoid reference issues
                            match elem_ty.as_ref() {
                                IrType::Int | IrType::Float | IrType::Bool => {
                                    quote! { #iter.iter().copied() }
                                }
                                // For non-Copy types (structs), use .iter_mut() to allow mutation
                                _ => {
                                    if needs_mut_items {
                                        quote! { #iter.iter_mut() }
                                    } else {
                                        quote! { #iter.iter() }
                                    }
                                }
                            }
                        } else {
                            quote! { #iter }
                        }
                    }
                    IrType::Set(_) | IrType::Dict(_, _) => {
                        if iterable_is_borrowable_lvalue {
                            quote! { &#iter }
                        } else {
                            quote! { #iter }
                        }
                    }
                    _ => quote! { #iter },
                };
                Ok(quote! {
                    for #pat in #iter_expr {
                        #(#body_stmts)*
                    }
                })
            }
            IrStmtKind::Loop { label: _, body } => {
                let body_stmts: Vec<TokenStream> = body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                Ok(quote! {
                    loop {
                        #(#body_stmts)*
                    }
                })
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond = self.emit_expr(condition)?;
                let then_stmts: Vec<TokenStream> = then_branch
                    .iter()
                    .map(|s| self.emit_stmt(s))
                    .collect::<Result<_, _>>()?;
                if let Some(else_stmts) = else_branch {
                    let else_tokens: Vec<TokenStream> =
                        else_stmts.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                    Ok(quote! {
                        if #cond {
                            #(#then_stmts)*
                        } else {
                            #(#else_tokens)*
                        }
                    })
                } else {
                    Ok(quote! {
                        if #cond {
                            #(#then_stmts)*
                        }
                    })
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                let scrut = self.emit_expr(scrutinee)?;
                let arm_tokens: Vec<TokenStream> = arms
                    .iter()
                    .map(|arm| {
                        let pat = self.emit_pattern(&arm.pattern);
                        let body = self.emit_expr(&arm.body)?;
                        if let Some(guard) = &arm.guard {
                            let g = self.emit_expr(guard)?;
                            Ok(quote! { #pat if #g => #body })
                        } else {
                            Ok(quote! { #pat => #body })
                        }
                    })
                    .collect::<Result<_, _>>()?;
                Ok(quote! {
                    match #scrut {
                        #(#arm_tokens),*
                    }
                })
            }
            IrStmtKind::Block(stmts) => {
                let inner: Vec<TokenStream> = stmts.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                Ok(quote! {
                    {
                        #(#inner)*
                    }
                })
            }
            IrStmtKind::CompoundAssign { .. } => Err(EmitError::Unsupported(
                "CompoundAssign should be lowered into a regular assignment before emission".to_string(),
            )),
        }
    }
}
