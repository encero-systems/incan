use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::emit::expressions::methods::ReceiverInfo;
use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{StringMethodKind, TypedExpr};

/// Emit known string methods for string-like receivers.
pub(super) fn emit_string_method(
    emitter: &IrEmitter,
    info: &ReceiverInfo,
    kind: &StringMethodKind,
    args: &[TypedExpr],
) -> Result<TokenStream, EmitError> {
    let r_borrow = &info.r_borrow;

    match kind {
        StringMethodKind::Upper => Ok(quote! { incan_stdlib::strings::str_upper(#r_borrow) }),
        StringMethodKind::Lower => Ok(quote! { incan_stdlib::strings::str_lower(#r_borrow) }),
        StringMethodKind::Strip => Ok(quote! { incan_stdlib::strings::str_strip(#r_borrow) }),
        StringMethodKind::Split => {
            let sep = if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                quote! { Some(&#a) }
            } else {
                quote! { None }
            };
            Ok(quote! { incan_stdlib::strings::str_split(#r_borrow, #sep) })
        }
        StringMethodKind::Replace => {
            if args.len() >= 2 {
                let pattern = emitter.emit_expr(&args[0])?;
                let replacement = emitter.emit_expr(&args[1])?;
                Ok(quote! { incan_stdlib::strings::str_replace(#r_borrow, &#pattern, &#replacement) })
            } else {
                Ok(quote! { (*#r_borrow).to_string() })
            }
        }
        StringMethodKind::Join => {
            if let Some(arg) = args.first() {
                let items = emitter.emit_expr(arg)?;
                Ok(quote! { incan_stdlib::strings::str_join(#r_borrow, &#items) })
            } else {
                Ok(quote! { String::new() })
            }
        }
        StringMethodKind::StartsWith => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_stdlib::strings::str_starts_with(#r_borrow, &#a) })
            } else {
                Ok(quote! { true })
            }
        }
        StringMethodKind::EndsWith => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_stdlib::strings::str_ends_with(#r_borrow, &#a) })
            } else {
                Ok(quote! { true })
            }
        }
        StringMethodKind::Contains => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                Ok(quote! { incan_stdlib::strings::str_contains(#r_borrow, &#a) })
            } else {
                Ok(quote! { false })
            }
        }
    }
}
