use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::emit::expressions::methods::ReceiverInfo;
use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{CollectionMethodKind, TypedExpr};
use crate::backend::ir::ownership::{ValueUseSite, plan_collection_receiver, plan_dict_lookup_key};
use crate::backend::ir::types::IrType;

/// Emit a dictionary lookup key, normalizing string-key probes to `AsRef<str>` when appropriate.
///
/// For `Dict[str, V]`-like receivers, owned string probes are lowered through `AsRef<str>` so emitted Rust matches the
/// standard library's borrowed lookup surface without adding an extra `&str` layer. Other key families use the shared
/// ownership planner's borrowed probe rules.
pub(super) fn emit_dict_lookup_key(receiver: &TypedExpr, arg: &TypedExpr, emitted: TokenStream) -> TokenStream {
    plan_dict_lookup_key(&receiver.ty, &arg.ty).apply(emitted)
}

fn collection_element_type(ty: &IrType) -> Option<&IrType> {
    match ty {
        IrType::List(elem) | IrType::Set(elem) => Some(elem.as_ref()),
        IrType::Ref(inner) | IrType::RefMut(inner) => collection_element_type(inner),
        _ => None,
    }
}

fn is_string_storage_type(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
    )
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
                let (key_target_ty, value_target_ty) = match &receiver.ty {
                    IrType::Dict(key_ty, value_ty) => (Some(key_ty.as_ref()), Some(value_ty.as_ref())),
                    IrType::Ref(inner) | IrType::RefMut(inner) => match inner.as_ref() {
                        IrType::Dict(key_ty, value_ty) => (Some(key_ty.as_ref()), Some(value_ty.as_ref())),
                        _ => (None, None),
                    },
                    _ => (None, None),
                };
                let k = emitter.emit_expr_for_use(
                    &args[0],
                    ValueUseSite::CollectionElement {
                        target_ty: key_target_ty,
                    },
                )?;
                let v = emitter.emit_expr_for_use(
                    &args[1],
                    ValueUseSite::CollectionElement {
                        target_ty: value_target_ty,
                    },
                )?;
                return Ok(quote! { #r.insert(#k, #v) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Remove => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list_mut = plan_collection_receiver(&receiver.ty, true).apply(r.clone());
                return Ok(quote! { incan_stdlib::collections::list_remove(#list_mut, (#a) as i64) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Append => {
            if let Some(arg) = args.first() {
                let elem_ty = match &receiver.ty {
                    IrType::List(elem) => Some(elem.as_ref()),
                    IrType::Ref(inner) | IrType::RefMut(inner) => match inner.as_ref() {
                        IrType::List(elem) => Some(elem.as_ref()),
                        _ => None,
                    },
                    _ => None,
                };
                let converted =
                    emitter.emit_expr_for_use(arg, ValueUseSite::CollectionElement { target_ty: elem_ty })?;
                return Ok(quote! { #r.push(#converted) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Extend => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list_mut = plan_collection_receiver(&receiver.ty, true).apply(r.clone());
                return Ok(quote! { incan_stdlib::collections::list_extend(#list_mut, &#a) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Pop => {
            // Incan types `pop()` as `T`, not `Option<T>`. Route through the runtime helper so generated Rust does not
            // encode the empty-list fallback itself while preserving the canonical Python-like error message.
            let list_mut = plan_collection_receiver(&receiver.ty, true).apply(r.clone());
            Ok(quote! { incan_stdlib::collections::__private::list_pop(#list_mut) })
        }
        CollectionMethodKind::Swap => {
            if args.len() >= 2 {
                let a1 = emitter.emit_expr(&args[0])?;
                let a2 = emitter.emit_expr(&args[1])?;
                let list_mut = plan_collection_receiver(&receiver.ty, true).apply(r.clone());
                return Ok(quote! { incan_stdlib::collections::list_swap(#list_mut, (#a1) as i64, (#a2) as i64) });
            }
            Ok(quote! { () })
        }
        CollectionMethodKind::Count => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list = plan_collection_receiver(&receiver.ty, false).apply(r.clone());
                return Ok(quote! { incan_stdlib::collections::list_count(#list, &#a) });
            }
            Ok(quote! { 0i64 })
        }
        CollectionMethodKind::Index => {
            if let Some(arg) = args.first() {
                let a = emitter.emit_expr(arg)?;
                let list = plan_collection_receiver(&receiver.ty, false).apply(r.clone());
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
                    IrType::List(_) | IrType::Ref(_) | IrType::RefMut(_)
                        if collection_element_type(&receiver.ty).is_some_and(is_string_storage_type) =>
                    {
                        return Ok(quote! {{
                            let __incan_probe = #a;
                            let __incan_probe = <_ as AsRef<str>>::as_ref(&__incan_probe);
                            #r.iter().any(|__incan_item| <_ as AsRef<str>>::as_ref(__incan_item) == __incan_probe)
                        }});
                    }
                    IrType::Set(_) if collection_element_type(&receiver.ty).is_some_and(is_string_storage_type) => {
                        return Ok(quote! {{
                            let __incan_probe = #a;
                            let __incan_probe = <_ as AsRef<str>>::as_ref(&__incan_probe);
                            #r.contains(__incan_probe)
                        }});
                    }
                    IrType::List(_) | IrType::Set(_) | IrType::Ref(_) | IrType::RefMut(_) => {
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
