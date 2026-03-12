//! Emit Rust expressions from Incan IR.
//!
//! This module converts IR expressions ([`TypedExpr`]/[`IrExprKind`]) into Rust expression
//! fragments ([`TokenStream`]).
//!
//! It is used by [`IrEmitter`] to implement the "IR → Rust" portion of the backend at the
//! expression level (literals, operators, calls, method calls, comprehensions, indexing/slicing,
//! and control flow).
//!
//! ## Module organization
//!
//! The expression emitter is split into focused submodules:
//!
//! - [`builtins`]: Built-in function calls (`print`, `len`, `range`, etc.)
//! - [`methods`]: Method calls (both known methods via `MethodKind` and string-based fallback)
//! - [`calls`]: Regular function calls and binary operations
//! - [`indexing`]: Index, slice, and field access expressions
//! - [`comprehensions`]: List and dict comprehensions
//! - [`structs_enums`]: Struct constructor expressions
//! - [`format`]: Format strings and range expressions
//! - [`lvalue`]: Assignment target expressions
//!
//! ## Notes
//!
//! - **Not lexer tokens**: [`TokenStream`] here is `proc_macro2::TokenStream` used for Rust codegen. Lexer output is a
//!   separate token type in the frontend.
//! - **Conversions are centralized**: Ownership/borrow/copy/string adjustments should go through
//!   [`determine_conversion`] using a [`ConversionContext`] instead of being hand-coded inline.
//! - **Side-effect free**: Emission is pure codegen; it does not touch the filesystem.
//!
//! ## Examples
//!
//! ```rust,ignore
//! // Pseudocode: IrEmitter is constructed by the backend codegen pipeline.
//! let tokens: proc_macro2::TokenStream = emitter.emit_expr(&typed_expr)?;
//! ```
//!
//! ## See also
//!
//! - `src/backend/ir/conversions.rs`: conversion policy and ownership rules
//! - `src/backend/ir/emit/mod.rs`: higher-level emission (items/statements) that calls into this module

mod builtins;
mod calls;
mod comprehensions;
mod format;
mod indexing;
mod lvalue;
mod methods;
mod structs_enums;

use proc_macro2::{Literal, TokenStream};
use quote::{ToTokens, format_ident, quote};

use super::super::expr::{IrExprKind, Literal as IrLiteral, TypedExpr, UnaryOp, VarRefKind};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Check whether an expression is a type-like identifier that should use Rust path syntax.
    ///
    /// This covers Incan type names, enum variants, module placeholders, and external Rust imports.
    pub(super) fn expr_is_type_like(expr: &TypedExpr) -> bool {
        match &expr.kind {
            IrExprKind::Var { ref_kind, .. } => !matches!(ref_kind, VarRefKind::Value),
            _ => false,
        }
    }

    /// Emit an IR expression as a Rust `TokenStream`.
    ///
    /// ## Parameters
    /// - `expr`: The typed IR expression to emit.
    ///
    /// ## Returns
    /// - A Rust `TokenStream` representing an expression.
    ///
    /// ## Errors
    /// - `EmitError`: if the IR contains an unsupported construct or emission of a sub-expression fails.
    ///
    /// ## Notes
    /// - This is the main entry point for expression emission; it delegates to specialized helpers in submodules for
    ///   complex expression kinds.
    pub(super) fn emit_expr(&self, expr: &TypedExpr) -> Result<TokenStream, EmitError> {
        match &expr.kind {
            IrExprKind::Unit => Ok(quote! { () }),
            IrExprKind::None => match &expr.ty {
                IrType::Option(inner) => {
                    let inner_ty = self.emit_type(inner);
                    Ok(quote! { None::<#inner_ty> })
                }
                _ => Ok(quote! { None }),
            },
            IrExprKind::Bool(b) => Ok(if *b {
                quote! { true }
            } else {
                quote! { false }
            }),
            IrExprKind::Int(n) => {
                // Emit integers without suffix to let Rust infer the type
                let lit = if *n >= 0 {
                    Literal::u64_unsuffixed(*n as u64)
                } else {
                    Literal::i64_unsuffixed(*n)
                };
                Ok(lit.to_token_stream())
            }
            IrExprKind::Float(n) => Ok(quote! { #n }),
            IrExprKind::String(s) => Ok(quote! { #s }),
            IrExprKind::Bytes(bytes) => {
                let lit = Literal::byte_string(bytes);
                Ok(lit.to_token_stream())
            }

            IrExprKind::Var { name, access: _, .. } => {
                let n = Self::rust_ident(name);
                Ok(quote! { #n })
            }

            IrExprKind::BinOp { op, left, right } => self.emit_binop_expr(op, left, right),

            IrExprKind::UnaryOp { op, operand } => {
                let o = self.emit_expr(operand)?;
                match op {
                    UnaryOp::Neg => Ok(quote! { -#o }),
                    UnaryOp::Not => Ok(quote! { !#o }),
                    UnaryOp::Deref => Ok(quote! { *#o }),
                    UnaryOp::Ref => Ok(quote! { (&#o) }),
                    UnaryOp::RefMut => Ok(quote! { (&mut #o) }),
                }
            }

            IrExprKind::Call {
                func,
                args,
                canonical_path,
            } => self.emit_call_expr(func, args, canonical_path.as_deref()),
            IrExprKind::BuiltinCall { func, args } => self.emit_builtin_call(func, args),
            IrExprKind::MethodCall { receiver, method, args } => self.emit_method_call_expr(receiver, method, args),
            IrExprKind::KnownMethodCall { receiver, kind, args } => self.emit_known_method_call(receiver, kind, args),

            IrExprKind::Field { object, field } => self.emit_field_expr(object, field),
            IrExprKind::Index { object, index } => self.emit_index_expr(object, index),
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => self.emit_slice_expr(target, start, end, step),

            IrExprKind::ListComp {
                element,
                variable,
                iterable,
                filter,
            } => self.emit_list_comp(element, variable, iterable, filter.as_deref()),
            IrExprKind::DictComp {
                key,
                value,
                variable,
                iterable,
                filter,
            } => self.emit_dict_comp(key, value, variable, iterable, filter.as_deref()),

            IrExprKind::List(items) => {
                let item_tokens: Vec<TokenStream> =
                    items.iter().map(|i| self.emit_expr(i)).collect::<Result<_, _>>()?;
                Ok(quote! { vec![#(#item_tokens),*] })
            }

            IrExprKind::Dict(pairs) => {
                if pairs.is_empty() {
                    Ok(quote! { HashMap::new() })
                } else {
                    let pair_tokens: Vec<TokenStream> = pairs
                        .iter()
                        .map(|(k, v)| {
                            let kk = self.emit_expr(k)?;
                            let vv = self.emit_expr(v)?;
                            Ok(quote! { (#kk, #vv) })
                        })
                        .collect::<Result<_, EmitError>>()?;
                    Ok(quote! { [#(#pair_tokens),*].into_iter().collect::<HashMap<_, _>>() })
                }
            }

            IrExprKind::Set(items) => {
                if items.is_empty() {
                    Ok(quote! { HashSet::new() })
                } else {
                    let item_tokens: Vec<TokenStream> =
                        items.iter().map(|i| self.emit_expr(i)).collect::<Result<_, _>>()?;
                    Ok(quote! { [#(#item_tokens),*].into_iter().collect::<HashSet<_>>() })
                }
            }

            IrExprKind::Tuple(items) => {
                let item_tokens: Vec<TokenStream> =
                    items.iter().map(|i| self.emit_expr(i)).collect::<Result<_, _>>()?;
                Ok(quote! { (#(#item_tokens),*) })
            }

            IrExprKind::Struct { name, fields } => self.emit_struct_expr(name, fields),

            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let c = self.emit_expr(condition)?;
                let t = self.emit_expr(then_branch)?;
                if let Some(e) = else_branch {
                    let ee = self.emit_expr(e)?;
                    Ok(quote! { if #c { #t } else { #ee } })
                } else {
                    Ok(quote! { if #c { #t } })
                }
            }

            IrExprKind::Match { scrutinee, arms } => {
                let s = self.emit_expr(scrutinee)?;
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
                    match #s {
                        #(#arm_tokens),*
                    }
                })
            }

            IrExprKind::Closure {
                params,
                body,
                captures: _,
            } => {
                let param_tokens: Vec<TokenStream> = params
                    .iter()
                    .map(|(pname, _pty)| {
                        let n = Self::rust_ident(pname);
                        quote! { #n }
                    })
                    .collect();
                let b = self.emit_expr(body)?;
                Ok(quote! { |#(#param_tokens),*| #b })
            }

            IrExprKind::Block { stmts, value } => {
                let stmt_tokens: Vec<TokenStream> =
                    stmts.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
                if let Some(v) = value {
                    let vv = self.emit_expr(v)?;
                    Ok(quote! {
                        {
                            #(#stmt_tokens)*
                            #vv
                        }
                    })
                } else {
                    Ok(quote! {
                        {
                            #(#stmt_tokens)*
                        }
                    })
                }
            }

            IrExprKind::Await(inner) => {
                let i = self.emit_expr(inner)?;
                Ok(quote! { #i.await })
            }

            IrExprKind::Try(inner) => {
                let i = self.emit_expr(inner)?;
                Ok(quote! { #i? })
            }

            IrExprKind::Range { start, end, inclusive } => {
                self.emit_range_expr(start.as_deref(), end.as_deref(), *inclusive)
            }

            IrExprKind::Cast { expr, to_type } => {
                let e = self.emit_expr(expr)?;
                let t = self.emit_type(to_type);
                Ok(quote! { #e as #t })
            }

            IrExprKind::Format { parts } => self.emit_format_expr(parts),

            IrExprKind::Literal(lit) => match lit {
                IrLiteral::StaticStr(s) => Ok(quote! { #s }),
            },

            IrExprKind::FieldsList(fields) => Ok(quote! { vec![#(#fields),*] }),

            IrExprKind::SerdeToJson => Ok(quote! {
                serde_json::to_string(self).unwrap_or_else(|_| {
                    incan_stdlib::errors::raise_json_serialization_error(std::any::type_name::<Self>())
                })
            }),

            IrExprKind::SerdeFromJson(type_name) => {
                let type_ident = format_ident!("{}", type_name);
                Ok(quote! {
                    serde_json::from_str::<#type_ident>(&s)
                        .map_err(|e| incan_stdlib::errors::json_decode_error_string(e))
                })
            }
        }
    }
}
