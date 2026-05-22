use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::backend::ir::expr::{IrExprKind, Literal as IrLiteral, TypedExpr};
use crate::backend::ir::types::IrType;

/// Emit a typechecker-selected Rust borrow coercion without re-planning ownership at the call site.
pub(super) fn emit_builtin_borrow_coercion(
    inner_expr: &TypedExpr,
    inner_tokens: TokenStream,
    target_ty: &IrType,
) -> TokenStream {
    if let Some(emitted) = emit_structural_borrow_coercion(inner_tokens.clone(), target_ty) {
        return emitted;
    }
    match target_ty {
        IrType::StrRef => match &inner_expr.ty {
            IrType::StaticStr | IrType::StrRef | IrType::FrozenStr | IrType::Ref(_) | IrType::RefMut(_) => {
                quote! { #inner_tokens }
            }
            _ => quote! { &#inner_tokens },
        },
        IrType::Ref(inner) if matches!(inner.as_ref(), IrType::Bytes) => match &inner_expr.ty {
            IrType::StaticBytes | IrType::FrozenBytes | IrType::Ref(_) | IrType::RefMut(_) => {
                quote! { #inner_tokens }
            }
            _ => quote! { &#inner_tokens },
        },
        IrType::Ref(inner) | IrType::RefMut(inner) if is_owned_rust_string_target(inner) => {
            if expr_already_materializes_owned_string(inner_expr) {
                quote! { &#inner_tokens }
            } else {
                quote! { &(#inner_tokens).to_string() }
            }
        }
        IrType::Ref(inner) | IrType::RefMut(inner) if is_owned_rust_bytes_target(inner) => {
            if expr_already_materializes_owned_bytes(inner_expr) {
                quote! { &#inner_tokens }
            } else {
                quote! { &(#inner_tokens).to_vec() }
            }
        }
        IrType::Ref(_) | IrType::RefMut(_) => quote! { &#inner_tokens },
        _ => quote! { #inner_tokens },
    }
}

/// Return whether an expression already emits an owned Rust `String` value.
fn expr_already_materializes_owned_string(expr: &TypedExpr) -> bool {
    matches!(expr.ty, IrType::String)
        && !matches!(
            expr.kind,
            IrExprKind::String(_) | IrExprKind::Literal(IrLiteral::StaticStr(_)) | IrExprKind::StaticRead { .. }
        )
}

/// Return whether an expression already emits an owned Rust `Vec<u8>` value.
fn expr_already_materializes_owned_bytes(expr: &TypedExpr) -> bool {
    matches!(expr.ty, IrType::Bytes) && !matches!(expr.kind, IrExprKind::Bytes(_) | IrExprKind::StaticRead { .. })
}

/// Return whether a Rust boundary target is an owned Rust string value.
fn is_owned_rust_string_target(ty: &IrType) -> bool {
    matches!(ty, IrType::String)
        || matches!(
            ty,
            IrType::Struct(name) if matches!(
                name.as_str(),
                "String" | "std::string::String" | "alloc::string::String"
            )
        )
}

/// Return whether a Rust boundary target is an owned Rust byte vector.
fn is_owned_rust_bytes_target(ty: &IrType) -> bool {
    matches!(ty, IrType::Bytes)
        || matches!(
            ty,
            IrType::Struct(name) if matches!(
                name.as_str(),
                "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>"
            )
        )
}

/// Emit a projection from a referenced source item into a Rust-boundary target item.
///
/// Structural borrow coercions iterate source containers so the element expression is usually `&T`. Exact scalar leaves
/// can be copied or cloned from that reference, while borrowed leaves project to the Rust borrow shape the frontend
/// recorded from metadata.
fn emit_structural_borrow_projection(source_tokens: TokenStream, target_ty: &IrType) -> TokenStream {
    match target_ty {
        IrType::StrRef => quote! { #source_tokens.as_str() },
        IrType::Ref(inner) if matches!(inner.as_ref(), IrType::Bytes) => {
            quote! { #source_tokens.as_slice() }
        }
        IrType::Ref(_) | IrType::RefMut(_) => quote! { #source_tokens },
        IrType::List(inner) => {
            let item_ident = format_ident!("__incan_item");
            let item_tokens = emit_structural_borrow_projection(quote! { #item_ident }, inner);
            quote! { #source_tokens.iter().map(|#item_ident| #item_tokens).collect::<Vec<_>>() }
        }
        IrType::Set(inner) => {
            let item_ident = format_ident!("__incan_item");
            let item_tokens = emit_structural_borrow_projection(quote! { #item_ident }, inner);
            quote! {
                #source_tokens
                    .iter()
                    .map(|#item_ident| #item_tokens)
                    .collect::<std::collections::HashSet<_>>()
            }
        }
        IrType::Dict(key_ty, value_ty) => {
            let key_ident = format_ident!("__incan_key");
            let value_ident = format_ident!("__incan_value");
            let key_tokens = emit_structural_borrow_projection(quote! { #key_ident }, key_ty);
            let value_tokens = emit_structural_borrow_projection(quote! { #value_ident }, value_ty);
            quote! {
                #source_tokens
                    .iter()
                    .map(|(#key_ident, #value_ident)| (#key_tokens, #value_tokens))
                    .collect::<std::collections::HashMap<_, _>>()
            }
        }
        IrType::Option(inner) => {
            let item_ident = format_ident!("__incan_item");
            let item_tokens = emit_structural_borrow_projection(quote! { #item_ident }, inner);
            quote! { #source_tokens.as_ref().map(|#item_ident| #item_tokens) }
        }
        IrType::Result(ok_ty, err_ty) => {
            let ok_ident = format_ident!("__incan_ok");
            let err_ident = format_ident!("__incan_err");
            let ok_tokens = emit_structural_borrow_projection(quote! { #ok_ident }, ok_ty);
            let err_tokens = emit_structural_borrow_projection(quote! { #err_ident }, err_ty);
            quote! {
                #source_tokens
                    .as_ref()
                    .map(|#ok_ident| #ok_tokens)
                    .map_err(|#err_ident| #err_tokens)
            }
        }
        IrType::Bool | IrType::Int | IrType::Float | IrType::Numeric(_) | IrType::Unit => {
            quote! { *#source_tokens }
        }
        _ => quote! { (*#source_tokens).clone() },
    }
}

fn emit_structural_borrow_coercion(inner_tokens: TokenStream, target_ty: &IrType) -> Option<TokenStream> {
    match target_ty {
        IrType::List(_) | IrType::Set(_) | IrType::Dict(_, _) | IrType::Option(_) | IrType::Result(_, _) => {
            Some(emit_structural_borrow_projection(inner_tokens, target_ty))
        }
        _ => None,
    }
}
