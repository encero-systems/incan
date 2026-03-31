use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::emit::expressions::methods::ReceiverInfo;
use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{MethodKind, TypedExpr};
use crate::backend::ir::types::IrType;

/// Emit collection-related known methods (list/dict/set).
pub(super) fn emit_collection_method(
    emitter: &IrEmitter,
    receiver: &TypedExpr,
    info: &ReceiverInfo,
    kind: &MethodKind,
    args: &[TypedExpr],
) -> Option<Result<TokenStream, EmitError>> {
    let r = &info.r;

    match kind {
        MethodKind::Get => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.get(#a) }));
            }
            Some(Ok(quote! { None }))
        }
        MethodKind::Insert => {
            if args.len() >= 2 {
                let k = match emitter.emit_expr(&args[0]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let v = match emitter.emit_expr(&args[1]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.insert(#k, #v) }));
            }
            Some(Ok(quote! { () }))
        }
        MethodKind::Remove => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.remove(#a) }));
            }
            Some(Ok(quote! { None }))
        }
        MethodKind::Append => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                // Incan has value-like semantics: appending an item should not necessarily invalidate the local
                // variable binding. In Rust, `Vec::push` moves non-Copy values, so we conservatively clone here.
                if arg.ty.is_copy() {
                    return Some(Ok(quote! { #r.push(#a) }));
                }
                return Some(Ok(quote! { #r.push(#a.clone()) }));
            }
            Some(Ok(quote! { () }))
        }
        MethodKind::Pop => {
            // Incan types `pop()` as `T`, but `Vec::pop` is `Option<T>`. Avoid `unwrap_or_default()` so `T` need not
            // implement `Default` (e.g. Clone-only models, #194). Empty list uses canonical `IndexError: pop from
            // empty list` via stdlib (Python-compatible).
            Some(Ok(
                quote! { #r.pop().unwrap_or_else(|| incan_stdlib::errors::raise_list_pop_empty()) },
            ))
        }
        MethodKind::Swap => {
            if args.len() >= 2 {
                let a1 = match emitter.emit_expr(&args[0]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let a2 = match emitter.emit_expr(&args[1]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.swap((#a1) as usize, (#a2) as usize) }));
            }
            Some(Ok(quote! { () }))
        }
        MethodKind::Reserve => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.reserve((#a) as usize) }));
            }
            Some(Ok(quote! { () }))
        }
        MethodKind::ReserveExact => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                return Some(Ok(quote! { #r.reserve_exact((#a) as usize) }));
            }
            Some(Ok(quote! { () }))
        }
        MethodKind::Contains => {
            if let Some(arg) = args.first() {
                let a = match emitter.emit_expr(arg) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                match &receiver.ty {
                    IrType::List(_) | IrType::Set(_) => {
                        return Some(Ok(quote! { #r.contains(&#a) }));
                    }
                    IrType::Dict(_, _) => {
                        return Some(Ok(quote! { #r.contains_key(&#a) }));
                    }
                    _ => return None,
                }
            }
            Some(Ok(quote! { false }))
        }
        MethodKind::Slice => None,
        _ => None,
    }
}
