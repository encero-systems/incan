use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::emit::expressions::methods::ReceiverInfo;
use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{CollectionMethodKind, TypedExpr};
use crate::backend::ir::types::IrType;

/// Emit a dictionary key argument with the borrow shape expected by Rust map APIs.
///
/// Borrowed/string-slice-like probes are passed through unchanged; owned probes are borrowed once so lookup-style
/// methods such as `get` and `contains_key` receive `&Q` rather than moving the key.
fn emit_dict_key_arg(arg: &TypedExpr, emitted: TokenStream) -> TokenStream {
    match &arg.ty {
        IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr => emitted,
        _ => quote! { &#emitted },
    }
}

/// Emit a dictionary lookup key, normalizing string-key probes to `AsRef<str>` when appropriate.
///
/// For `Dict[str, V]`-like receivers, owned string probes are lowered through `AsRef<str>` so emitted Rust matches the
/// standard library's borrowed lookup surface without adding an extra `&str` layer. Non-string-key dictionaries fall
/// back to [`emit_dict_key_arg`].
fn emit_dict_lookup_key(receiver: &TypedExpr, arg: &TypedExpr, emitted: TokenStream) -> TokenStream {
    match &receiver.ty {
        IrType::Dict(key_ty, _)
            if matches!(
                key_ty.as_ref(),
                IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
            ) =>
        {
            match &arg.ty {
                IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr => emitted,
                _ => quote! { <_ as AsRef<str>>::as_ref(&#emitted) },
            }
        }
        _ => emit_dict_key_arg(arg, emitted),
    }
}

/// Emit collection-related known methods (list/dict/set).
pub(super) fn emit_collection_method(
    emitter: &IrEmitter,
    receiver: &TypedExpr,
    info: &ReceiverInfo,
    kind: &CollectionMethodKind,
    args: &[TypedExpr],
) -> Result<TokenStream, EmitError> {
    let r = &info.r;

    match kind {
        CollectionMethodKind::Get => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                return match &receiver.ty {
                    IrType::Dict(_, _) => {
                        let key = emit_dict_lookup_key(receiver, arg, a);
                        Ok(quote! { #r.get(#key) })
                    }
                    _ => Ok(quote! { #r.get(#a) }),
                };
            }
            Ok(quote! { None })
        }
        CollectionMethodKind::Insert => {
            if args.len() >= 2 {
                let k = emitter.emit_expr(&args[0])?;
                let v = emitter.emit_expr(&args[1])?;
                return Ok(quote! { #r.insert(#k, #v) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Remove => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                return Ok(quote! { #r.remove(#a) });
            }
            Ok(quote! { None })
        }
        CollectionMethodKind::Append => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                // Incan has value-like semantics: appending an item should not necessarily invalidate the local
                // variable binding. In Rust, `Vec::push` moves non-Copy values, so we conservatively clone here.
                if arg.ty.is_copy() {
                    return Ok(quote! { #r.push(#a) });
                }
                return Ok(quote! { #r.push(#a.clone()) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Pop => {
            // Incan types `pop()` as `T`, but `Vec::pop` is `Option<T>`. Avoid `unwrap_or_default()` so `T` need not
            // implement `Default` (e.g. Clone-only models, #194). Empty list uses canonical `IndexError: pop from
            // empty list` via stdlib (Python-compatible).
            Ok(quote! { #r.pop().unwrap_or_else(|| incan_stdlib::errors::raise_list_pop_empty()) })
        }
        CollectionMethodKind::Swap => {
            if args.len() >= 2 {
                let a1 = emitter.emit_expr(&args[0])?;
                let a2 = emitter.emit_expr(&args[1])?;
                return Ok(quote! { #r.swap((#a1) as usize, (#a2) as usize) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Reserve => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                return Ok(quote! { #r.reserve((#a) as usize) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::ReserveExact => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                return Ok(quote! { #r.reserve_exact((#a) as usize) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Contains => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                match &receiver.ty {
                    IrType::List(_) | IrType::Set(_) => {
                        return Ok(quote! { #r.contains(&#a) });
                    }
                    IrType::Dict(_, _) => {
                        let key = emit_dict_lookup_key(receiver, arg, a);
                        return Ok(quote! { #r.contains_key(#key) });
                    }
                    _ => {}
                }
            }
            Ok(quote! { false })
        }
    }
}
