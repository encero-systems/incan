//! Emit RFC-008 consts safely and predictably.
//!
//! This module contains:
//!
//! - RFC-008 const representability checks (rejecting non-emittable consts with actionable errors)
//! - const-friendly helpers for `&'static str` concatenation chains (emitting `concat!(...)`)
//!
//! ## Notes
//!
//! - Const evaluation happens in the frontend; this module focuses on *emission constraints* and ergonomics in Rust
//!   `const` contexts.
//! - Const string folding is intentionally conservative; if a value cannot be proven to be a `'static` literal chain,
//!   emission falls back to regular expression emission.
//!
//! ## See also
//!
//! - `docs/RFCs/done/008_const_bindings.md`
//! - [`crate::backend::ir::emit::program`]: where const string folding is initialized

use proc_macro2::TokenStream;
use quote::quote;

use super::super::expr::{BinOp, IrDictEntry, IrExprKind, IrListEntry, Literal as IrLiteral, TypedExpr};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};
use incan_core::lang::types::collections::{self, CollectionTypeId};

impl<'a> IrEmitter<'a> {
    /// RFC 008 const representability check.
    ///
    /// Allowed (phase 1):
    /// - Int, exact-width numeric types, Float, Bool
    /// - StaticStr (e.g., "literal")
    /// - StaticBytes (e.g., b"...")
    /// - Tuples of allowed consts
    /// - FrozenList/Set/Dict with allowed element types (emitted via `FrozenX::new(&[..])`)
    ///
    /// Everything else is rejected with an actionable error.
    pub(super) fn validate_const_emittable(&self, name: &str, ty: &IrType, value: &TypedExpr) -> Result<(), EmitError> {
        /// Return whether an IR type can appear in a Rust `const` initializer emitted by RFC 008.
        fn ok_ty(ty: &IrType) -> bool {
            match ty {
                IrType::Int
                | IrType::Numeric(_)
                | IrType::Float
                | IrType::Bool
                | IrType::StaticStr
                | IrType::StaticBytes
                | IrType::FrozenStr
                | IrType::FrozenBytes => true,
                IrType::Tuple(items) => items.iter().all(ok_ty),
                IrType::NamedGeneric(name, args) if name == collections::as_str(CollectionTypeId::FrozenList) => {
                    args.first().map(ok_ty).unwrap_or(false)
                }
                IrType::NamedGeneric(name, args) if name == collections::as_str(CollectionTypeId::FrozenSet) => {
                    args.first().map(ok_ty).unwrap_or(false)
                }
                IrType::NamedGeneric(name, args) if name == collections::as_str(CollectionTypeId::FrozenDict) => {
                    args.first().map(ok_ty).unwrap_or(false) && args.get(1).map(ok_ty).unwrap_or(false)
                }
                _ => false,
            }
        }

        if !ok_ty(ty) {
            let ty_name = ty.rust_name();
            return Err(EmitError::Unsupported(format!(
                "const '{}' of type '{}' is not representable as a Rust const.\n\
                 Allowed: int/exact-width numeric/float/bool/&'static str/&'static [u8]/tuples, FrozenList/Set/Dict with allowed element types.\n\
                 Consider computing at runtime or simplifying the const.",
                name, ty_name
            )));
        }

        Self::validate_const_expr_kind(&value.kind)
    }

    /// RFC 008 const expression shape check (defensive backend guard).
    ///
    /// Frontend const-eval should already reject non-const expressions, but this
    /// backend guard prevents emitting invalid consts when typechecker info is
    /// missing and lowering falls back to heuristic typing.
    fn validate_const_expr_kind(kind: &IrExprKind) -> Result<(), EmitError> {
        use IrExprKind as K;

        match kind {
            // Primitives and static literals
            K::Unit | K::None | K::Bool(_) | K::Int(_) | K::Float(_) | K::String(_) | K::Bytes(_) => Ok(()),
            K::Literal(IrLiteral::StaticStr(_)) => Ok(()),

            // Const-to-const references
            K::Var { .. } => Ok(()),

            // Unary / binary operations (operands must be const-safe)
            K::UnaryOp { operand, .. } => Self::validate_const_expr_kind(&operand.kind),
            K::BinOp { left, right, .. } => {
                Self::validate_const_expr_kind(&left.kind)?;
                Self::validate_const_expr_kind(&right.kind)
            }

            // Collections: validate elements (and key/value pairs)
            K::Tuple(items) | K::Set(items) => {
                for item in items {
                    Self::validate_const_expr_kind(&item.kind)?;
                }
                Ok(())
            }
            K::List(items) => {
                for item in items {
                    match item {
                        IrListEntry::Element(value) => Self::validate_const_expr_kind(&value.kind)?,
                        IrListEntry::Spread(_) => {
                            return Err(EmitError::Unsupported(
                                "List spread is not supported in const initializers".to_string(),
                            ));
                        }
                    }
                }
                Ok(())
            }
            K::Dict(pairs) => {
                for entry in pairs {
                    match entry {
                        IrDictEntry::Pair(k, v) => {
                            Self::validate_const_expr_kind(&k.kind)?;
                            Self::validate_const_expr_kind(&v.kind)?;
                        }
                        IrDictEntry::Spread(_) => {
                            return Err(EmitError::Unsupported(
                                "Dict spread is not supported in const initializers".to_string(),
                            ));
                        }
                    }
                }
                Ok(())
            }

            // Disallowed constructs in const initializers
            K::Call { .. } => Err(EmitError::Unsupported(
                "Function calls are not allowed in const initializers".to_string(),
            )),
            K::BuiltinCall { .. } => Err(EmitError::Unsupported(
                "Builtin calls are not allowed in const initializers".to_string(),
            )),
            K::MethodCall { .. } | K::KnownMethodCall { .. } => Err(EmitError::Unsupported(
                "Method calls are not allowed in const initializers".to_string(),
            )),
            K::ListComp { .. } | K::DictComp { .. } | K::Generator { .. } => Err(EmitError::Unsupported(
                "Comprehensions are not allowed in const initializers".to_string(),
            )),
            K::Closure { .. } => Err(EmitError::Unsupported(
                "Closures are not allowed in const initializers".to_string(),
            )),
            K::Await(_) => Err(EmitError::Unsupported(
                "Await expressions are not allowed in const initializers".to_string(),
            )),
            K::If { .. }
            | K::Match { .. }
            | K::Block { .. }
            | K::Field { .. }
            | K::Index { .. }
            | K::Slice { .. }
            | K::Struct { .. }
            | K::Range { .. }
            | K::Cast { .. }
            | K::Format { .. }
            | K::Try(_) => Err(EmitError::Unsupported(
                "Expression is not allowed in const initializers (phase 1)".to_string(),
            )),

            // Catch-all for future/unknown kinds
            other => Err(EmitError::Unsupported(format!(
                "Expression kind '{other:?}' is not allowed in const initializers"
            ))),
        }
    }

    /// Evaluate a TypedExpr as a compile-time `'static` string value, if possible.
    fn eval_static_str_expr(
        expr: &TypedExpr,
        const_exprs: &std::collections::HashMap<String, TypedExpr>,
        visiting: &mut std::collections::HashSet<String>,
        cache: &mut std::collections::HashMap<String, String>,
    ) -> Option<String> {
        match &expr.kind {
            IrExprKind::String(s) => Some(s.clone()),
            IrExprKind::Literal(IrLiteral::StaticStr(s)) => Some(s.clone()),
            IrExprKind::Var { name, .. } => Self::resolve_static_str_const(name, const_exprs, visiting, cache),
            IrExprKind::BinOp {
                op: BinOp::Add,
                left,
                right,
            } => {
                let l = Self::eval_static_str_expr(left, const_exprs, visiting, cache)?;
                let r = Self::eval_static_str_expr(right, const_exprs, visiting, cache)?;
                Some(format!("{l}{r}"))
            }
            _ => None,
        }
    }

    /// Resolve a const name to its full `'static` string literal value (if representable).
    ///
    /// Uses a small DFS with cycle protection and memoization.
    pub(super) fn resolve_static_str_const(
        name: &str,
        const_exprs: &std::collections::HashMap<String, TypedExpr>,
        visiting: &mut std::collections::HashSet<String>,
        cache: &mut std::collections::HashMap<String, String>,
    ) -> Option<String> {
        if let Some(v) = cache.get(name) {
            return Some(v.clone());
        }
        if visiting.contains(name) {
            // Defensive: frontend should reject const cycles, but avoid infinite recursion here.
            return None;
        }

        let expr = const_exprs.get(name)?;
        visiting.insert(name.to_string());
        let out = Self::eval_static_str_expr(expr, const_exprs, visiting, cache);
        visiting.remove(name);

        if let Some(v) = out.clone() {
            cache.insert(name.to_string(), v);
        }
        out
    }

    /// Try to emit a const-friendly concatenation for `&'static str` additions.
    ///
    /// Supports cases where both sides are string literals or const `&'static str`
    /// bindings (recorded during program scan). Emits `concat!(.., ..)` which
    /// is valid in const contexts.
    pub(super) fn try_emit_static_str_add(
        &self,
        left: &TypedExpr,
        right: &TypedExpr,
    ) -> Result<Option<TokenStream>, EmitError> {
        // Helper to convert an expr into a string-literal token if possible
        let to_lit_tokens = |e: &TypedExpr| -> Option<TokenStream> {
            match &e.kind {
                IrExprKind::String(s) => Some(quote! { #s }),
                IrExprKind::Literal(IrLiteral::StaticStr(s)) => Some(quote! { #s }),
                IrExprKind::Var { name, .. } => self.const_string_literals.get(name).map(|lit| {
                    let l = lit.clone();
                    quote! { #l }
                }),
                _ => None,
            }
        };

        let l_tok = to_lit_tokens(left);
        let r_tok = to_lit_tokens(right);
        if let (Some(lt), Some(rt)) = (l_tok, r_tok) {
            return Ok(Some(quote! { concat!(#lt, #rt) }));
        }
        Ok(None)
    }
}
