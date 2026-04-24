use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::conversions::{ConversionContext, determine_conversion};
use crate::backend::ir::emit::expressions::methods::ReceiverInfo;
use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{CollectionMethodKind, TypedExpr};
use crate::backend::ir::types::IrType;

/// Borrow a list-like receiver immutably unless emission is already operating on a reference-typed value.
///
/// Known collection helpers such as `list_count` and `list_index` take shared borrows; this keeps emitted Rust
/// aligned with the receiver's existing reference shape instead of manufacturing `&&T`.
fn emit_list_shared_receiver(receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    match &receiver.ty {
        IrType::Ref(_) | IrType::RefMut(_) => quote! { #r },
        _ => quote! { &#r },
    }
}

/// Borrow a list-like receiver mutably unless emission is already operating on a mutable reference.
///
/// This is used for strict mutating helpers such as `list_remove` and `list_swap` so known-method emission can route
/// through stdlib helpers without double-borrowing the receiver.
fn emit_list_mut_receiver(receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    match &receiver.ty {
        IrType::RefMut(_) => quote! { #r },
        _ => quote! { &mut #r },
    }
}

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
pub(super) fn emit_dict_lookup_key(receiver: &TypedExpr, arg: &TypedExpr, emitted: TokenStream) -> TokenStream {
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
                let list_mut = emit_list_mut_receiver(receiver, r);
                return Ok(quote! { incan_stdlib::collections::list_remove(#list_mut, (#a) as i64) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Append => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let elem_ty = match &receiver.ty {
                    IrType::List(elem) => Some(elem.as_ref()),
                    IrType::Ref(inner) | IrType::RefMut(inner) => match inner.as_ref() {
                        IrType::List(elem) => Some(elem.as_ref()),
                        _ => None,
                    },
                    _ => None,
                };
                let conversion = determine_conversion(arg, elem_ty, ConversionContext::CollectionElement);
                let converted = conversion.apply(a);
                return Ok(quote! { #r.push(#converted) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Extend => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list_mut = emit_list_mut_receiver(receiver, r);
                return Ok(quote! { incan_stdlib::collections::list_extend(#list_mut, &#a) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Pop => {
            // Incan types `pop()` as `T`, not `Option<T>`. Route through the runtime helper so generated Rust does not
            // encode the empty-list fallback itself while preserving the canonical Python-like error message.
            let list_mut = emit_list_mut_receiver(receiver, r);
            Ok(quote! { incan_stdlib::collections::list_pop(#list_mut) })
        }
        CollectionMethodKind::Swap => {
            if args.len() >= 2 {
                let a1 = emitter.emit_expr(&args[0])?;
                let a2 = emitter.emit_expr(&args[1])?;
                let list_mut = emit_list_mut_receiver(receiver, r);
                return Ok(quote! { incan_stdlib::collections::list_swap(#list_mut, (#a1) as i64, (#a2) as i64) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Count => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list = emit_list_shared_receiver(receiver, r);
                return Ok(quote! { incan_stdlib::collections::list_count(#list, &#a) });
            }
            Ok(quote! { 0i64 })
        }
        CollectionMethodKind::Index => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list = emit_list_shared_receiver(receiver, r);
                return Ok(quote! { incan_stdlib::collections::list_index(#list, &#a) });
            }
            Ok(quote! { incan_stdlib::errors::raise_list_value_not_found() })
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
