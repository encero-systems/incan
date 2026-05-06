//! Emit Rust code for RFC 088 iterator adapter and terminal methods.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{IrExprKind, IteratorMethodKind, TypedExpr};
use crate::backend::ir::types::IrType;
use incan_core::lang::traits::{self as core_traits, TraitId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};

use super::ReceiverInfo;

/// Emit iterator-related known methods as Rust iterator chains.
///
/// The lowering layer has already classified these calls through `MethodKind::Iterator`, so this emitter keeps the
/// backend surface structured instead of rediscovering behavior from method-name strings.
pub(super) fn emit_iterator_method(
    emitter: &IrEmitter<'_>,
    receiver: &TypedExpr,
    info: &ReceiverInfo,
    kind: &IteratorMethodKind,
    args: &[TypedExpr],
) -> Result<TokenStream, EmitError> {
    let r = &info.r;
    match kind {
        IteratorMethodKind::Iter => Ok(emit_iter_receiver(receiver, r)),
        IteratorMethodKind::Map => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).map(std::convert::identity) });
            };
            Ok(quote! { (#r).map(#callback) })
        }
        IteratorMethodKind::Filter => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).filter(|_| true) });
            };
            Ok(quote! { (#r).filter(|__incan_item| (#callback)((*__incan_item).clone())) })
        }
        IteratorMethodKind::Enumerate => Ok(quote! { (#r).enumerate().map(|(idx, value)| (idx as i64, value)) }),
        IteratorMethodKind::Zip => {
            let Some(other) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).zip(std::iter::empty()) });
            };
            Ok(quote! { (#r).zip((#other).into_iter()) })
        }
        IteratorMethodKind::Take => {
            let count = emit_count_arg(emitter, args)?;
            Ok(quote! { (#r).take(incan_stdlib::iter::nonnegative_count(#count)) })
        }
        IteratorMethodKind::Skip => {
            let count = emit_count_arg(emitter, args)?;
            Ok(quote! { (#r).skip(incan_stdlib::iter::nonnegative_count(#count)) })
        }
        IteratorMethodKind::TakeWhile => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).take_while(|_| true) });
            };
            Ok(quote! { (#r).take_while(|__incan_item| (#callback)((*__incan_item).clone())) })
        }
        IteratorMethodKind::SkipWhile => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).skip_while(|_| false) });
            };
            Ok(quote! { (#r).skip_while(|__incan_item| (#callback)((*__incan_item).clone())) })
        }
        IteratorMethodKind::Chain => {
            let Some(other) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).chain(std::iter::empty()) });
            };
            Ok(quote! { (#r).chain((#other).into_iter()) })
        }
        IteratorMethodKind::FlatMap => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).flat_map(std::convert::identity) });
            };
            Ok(quote! { (#r).flat_map(#callback) })
        }
        IteratorMethodKind::Batch => {
            let size = emit_count_arg(emitter, args)?;
            Ok(quote! { incan_stdlib::iter::batch((#r), #size) })
        }
        IteratorMethodKind::Collect => Ok(quote! { (#r).collect::<Vec<_>>() }),
        IteratorMethodKind::Count => Ok(quote! { ::std::convert::identity((#r).count() as i64) }),
        IteratorMethodKind::Reduce => emit_reduce(emitter, r, args),
        IteratorMethodKind::Fold => emit_fold(emitter, r, args),
        IteratorMethodKind::Any => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).any(|_| true) });
            };
            Ok(quote! { (#r).any(|__incan_item| (#callback)(__incan_item)) })
        }
        IteratorMethodKind::All => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).all(|_| true) });
            };
            Ok(quote! { (#r).all(|__incan_item| (#callback)(__incan_item)) })
        }
        IteratorMethodKind::Find => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).find(|_| true) });
            };
            Ok(quote! { (#r).find(|__incan_item| (#callback)((*__incan_item).clone())) })
        }
        IteratorMethodKind::ForEach => {
            let Some(callback) = emit_arg(emitter, args, 0)? else {
                return Ok(quote! { (#r).for_each(drop) });
            };
            Ok(quote! { (#r).for_each(#callback) })
        }
        IteratorMethodKind::Sum => Ok(emit_sum(emitter, receiver, r)),
    }
}

fn emit_iter_receiver(receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    match receiver_type_for_iterator_dispatch(&receiver.ty) {
        IrType::List(_) | IrType::Set(_) => quote! { (#r).iter().cloned() },
        IrType::NamedGeneric(name, _)
            if matches!(
                collection_types::from_str(name),
                Some(CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet)
            ) =>
        {
            quote! { (#r).into_iter().cloned() }
        }
        IrType::NamedGeneric(name, _) | IrType::Struct(name) if is_iterator_protocol_type_name(name) => quote! { (#r) },
        _ => quote! { (#r).into_iter() },
    }
}

fn receiver_type_for_iterator_dispatch(receiver_ty: &IrType) -> &IrType {
    let mut receiver_ty = receiver_ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
        receiver_ty = inner.as_ref();
    }
    receiver_ty
}

fn is_iterator_protocol_type_name(name: &str) -> bool {
    name.rsplit("::").next() == Some(core_traits::as_str(TraitId::Iterator))
}

fn emit_reduce(emitter: &IrEmitter<'_>, receiver: &TokenStream, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    match (args.first(), args.get(1)) {
        (Some(init), Some(callback)) => {
            let init = emitter.emit_expr(init)?;
            let callback = emitter.emit_expr(callback)?;
            Ok(quote! { (#receiver).fold(#init, #callback) })
        }
        _ => Ok(quote! { (#receiver).fold((), |__incan_acc, _| __incan_acc) }),
    }
}

/// Emit `.fold(init, f)` as Rust's explicit-accumulator iterator fold.
fn emit_fold(emitter: &IrEmitter<'_>, receiver: &TokenStream, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    match (args.first(), args.get(1)) {
        (Some(init), Some(callback)) => {
            let init = emitter.emit_expr(init)?;
            let callback = emitter.emit_expr(callback)?;
            Ok(quote! { (#receiver).fold(#init, #callback) })
        }
        _ => Ok(quote! { (#receiver).fold((), |__incan_acc, _| __incan_acc) }),
    }
}

fn emit_arg(emitter: &IrEmitter<'_>, args: &[TypedExpr], index: usize) -> Result<Option<TokenStream>, EmitError> {
    args.get(index).map(|arg| emitter.emit_expr(arg)).transpose()
}

fn emit_count_arg(emitter: &IrEmitter<'_>, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    let Some(arg) = args.first() else {
        return Ok(quote! { 0i64 });
    };
    let emitted = emitter.emit_expr(arg)?;
    Ok(match &arg.kind {
        IrExprKind::Int(value) => quote! { #value },
        _ => quote! { (#emitted) as i64 },
    })
}

fn emit_sum(emitter: &IrEmitter<'_>, receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    let Some(item_ty) = iterator_element_type(&receiver.ty) else {
        return quote! { (#r).sum::<i64>() };
    };
    if let Some((newtype_name, underlying_ty)) = newtype_sum_underlying(emitter, item_ty) {
        let sum_ty = emitter.emit_type(underlying_ty);
        let underlying_sum = quote! { (#r).map(|__incan_item| __incan_item.0).sum::<#sum_ty>() };
        return emit_newtype_from_underlying_sum(emitter, newtype_name, underlying_sum);
    }
    let sum_ty = emitter.emit_type(item_ty);
    quote! { (#r).sum::<#sum_ty>() }
}

fn newtype_sum_underlying<'a>(emitter: &'a IrEmitter<'_>, item_ty: &'a IrType) -> Option<(&'a str, &'a IrType)> {
    let name = match item_ty {
        IrType::Struct(name) | IrType::NamedGeneric(name, _) => name.as_str(),
        _ => return None,
    };
    let underlying = emitter.struct_field_types.get(&(name.to_string(), "0".to_string()))?;
    if matches!(underlying, IrType::Int | IrType::Float) {
        Some((name, underlying))
    } else {
        None
    }
}

fn emit_newtype_from_underlying_sum(
    emitter: &IrEmitter<'_>,
    newtype_name: &str,
    underlying_sum: TokenStream,
) -> TokenStream {
    let newtype_path = emit_path_ident(newtype_name);
    if let Some(ctor) = emitter.newtype_checked_ctor.get(newtype_name) {
        let ctor_ident = format_ident!("{}", ctor);
        let message = format!("validated newtype construction failed: {newtype_name}::{ctor}");
        return quote! {
            match #newtype_path :: #ctor_ident(#underlying_sum) {
                Ok(__incan_newtype_value) => __incan_newtype_value,
                Err(_) => panic!(#message),
            }
        };
    }
    quote! { #newtype_path(#underlying_sum) }
}

fn emit_path_ident(path: &str) -> TokenStream {
    if !path.contains("::") {
        let ident = format_ident!("{}", path);
        return quote! { #ident };
    }
    let mut segments = path.split("::").filter(|segment| !segment.is_empty()).map(|segment| {
        let ident = format_ident!("{}", segment);
        quote! { #ident }
    });
    let Some(first) = segments.next() else {
        return quote! { _ };
    };
    segments.fold(first, |acc, segment| quote! { #acc :: #segment })
}

fn iterator_element_type(receiver_ty: &IrType) -> Option<&IrType> {
    let receiver_ty = receiver_type_for_iterator_dispatch(receiver_ty);
    match receiver_ty {
        IrType::NamedGeneric(name, args) if is_iterator_protocol_type_name(name) => args.first(),
        _ => None,
    }
}
