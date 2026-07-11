//! Emit Rust code for RFC 088 iterator adapter and terminal methods.
//!
//! The typechecker and stdlib define the user-facing protocol surface, and this module keeps codegen aligned with that
//! surface by constructing the adapter models from `std.derives.collection`.
//!
//! Terminal methods still lower here because generated Rust needs concrete loops over the Incan `Iterator.__next__`
//! trait method. Checked-newtype `sum` also stays compiler-supported so the backend can unwrap to the primitive
//! carrier, accumulate from the primitive zero value, and reconstruct through the selected checked constructor.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{IrExprKind, IteratorMethodKind, TypedExpr};
use crate::backend::ir::ownership::plan_owned_iterator_source;
use crate::backend::ir::types::IrType;
use incan_core::lang::traits::{self as core_traits, TraitId};

use super::ReceiverInfo;

/// Emit iterator-related known methods through the Incan stdlib adapter models.
///
/// The lowering layer has already classified these calls through `MethodKind::Iterator`, so this emitter only maps
/// structured method kinds to model constructors or terminal `__next__` loops.
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
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { std::convert::identity });
            Ok(quote! { crate::__incan_std::derives::collection::MapIterator { source: (#r), f: #callback } })
        }
        IteratorMethodKind::Filter => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| true });
            Ok(quote! { crate::__incan_std::derives::collection::FilterIterator { source: (#r), f: #callback } })
        }
        IteratorMethodKind::Enumerate => Ok(quote! {
            crate::__incan_std::derives::collection::EnumerateIterator {
                source: (#r),
                index: 0i64,
                marker: None,
            }
        }),
        IteratorMethodKind::Zip => {
            let other = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { std::iter::empty() });
            Ok(quote! {
                crate::__incan_std::derives::collection::ZipIterator {
                    left: (#r),
                    right: (#other),
                    left_marker: None,
                    right_marker: None,
                }
            })
        }
        IteratorMethodKind::Take => {
            let count = emit_count_arg(emitter, args)?;
            Ok(quote! {
                crate::__incan_std::derives::collection::TakeIterator {
                    source: (#r),
                    remaining: #count,
                    marker: None,
                }
            })
        }
        IteratorMethodKind::Skip => {
            let count = emit_count_arg(emitter, args)?;
            Ok(quote! {
                crate::__incan_std::derives::collection::SkipIterator {
                    source: (#r),
                    remaining: #count,
                    marker: None,
                }
            })
        }
        IteratorMethodKind::TakeWhile => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| true });
            Ok(quote! {
                crate::__incan_std::derives::collection::TakeWhileIterator {
                    source: (#r),
                    f: #callback,
                    done: false,
                }
            })
        }
        IteratorMethodKind::SkipWhile => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| false });
            Ok(quote! {
                crate::__incan_std::derives::collection::SkipWhileIterator {
                    source: (#r),
                    f: #callback,
                    skipping: true,
                }
            })
        }
        IteratorMethodKind::Chain => {
            let other = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { std::iter::empty() });
            Ok(quote! {
                crate::__incan_std::derives::collection::ChainIterator {
                    first: (#r),
                    second: (#other),
                    in_second: false,
                    marker: None,
                }
            })
        }
        IteratorMethodKind::FlatMap => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { std::convert::identity });
            Ok(quote! {
                crate::__incan_std::derives::collection::FlatMapIterator {
                    source: (#r),
                    f: #callback,
                    current: Vec::new(),
                    index: 0i64,
                }
            })
        }
        IteratorMethodKind::Batch => {
            let size = emit_count_arg(emitter, args)?;
            Ok(quote! {
                crate::__incan_std::derives::collection::BatchIterator {
                    source: (#r),
                    size: #size,
                    marker: None,
                }
            })
        }
        IteratorMethodKind::Collect => Ok(emit_collect(r)),
        IteratorMethodKind::Count => Ok(emit_count(r)),
        IteratorMethodKind::Reduce => emit_reduce(emitter, r, args),
        IteratorMethodKind::Fold => emit_fold(emitter, r, args),
        IteratorMethodKind::Any => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| true });
            Ok(emit_any(r, callback))
        }
        IteratorMethodKind::All => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| true });
            Ok(emit_all(r, callback))
        }
        IteratorMethodKind::Find => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { |_| true });
            Ok(emit_find(r, callback))
        }
        IteratorMethodKind::ForEach => {
            let callback = emit_arg(emitter, args, 0)?.unwrap_or_else(|| quote! { drop });
            Ok(emit_for_each(r, callback))
        }
        IteratorMethodKind::Sum => Ok(emit_sum(emitter, receiver, r)),
    }
}

/// Emit the value returned by `.iter()` for builtin lists and values that are already Incan iterators.
fn emit_iter_receiver(receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    match receiver_type_for_iterator_dispatch(&receiver.ty) {
        IrType::List(_) => {
            let items = plan_owned_iterator_source(receiver).apply(r.clone());
            quote! { crate::__incan_std::derives::collection::ListIterator { items: #items, index: 0i64 } }
        }
        IrType::NamedGeneric(name, _) | IrType::Struct(name) if is_iterator_protocol_type_name(name) => quote! { (#r) },
        _ => quote! { (#r) },
    }
}

/// Return the receiver type after removing transparent borrow wrappers.
///
/// Method classification and `.iter()` emission care about the underlying surface collection or protocol type, not
/// whether lowering happened to borrow it for a particular use site.
fn receiver_type_for_iterator_dispatch(receiver_ty: &IrType) -> &IrType {
    let mut receiver_ty = receiver_ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
        receiver_ty = inner.as_ref();
    }
    receiver_ty
}

/// Return whether a nominal IR type name denotes the standard `Iterator` protocol.
///
/// Lowering may preserve a short stdlib name or a qualified path. Only the final path segment is semantically relevant
/// for routing RFC 088 known methods through this emitter.
fn is_iterator_protocol_type_name(name: &str) -> bool {
    name.rsplit("::").next() == Some(core_traits::as_str(TraitId::Iterator))
}

/// Emit a fully-qualified call to the Incan iterator protocol's `__next__` method.
fn next_call(iter: &TokenStream) -> TokenStream {
    quote! { crate::__incan_std::derives::collection::Iterator::__next__(&mut #iter) }
}

/// Emit `.collect()` as a loop that appends every remaining protocol item into a `Vec`.
fn emit_collect(receiver: &TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        let mut __incan_items = Vec::new();
        loop {
            match #next {
                Some(__incan_item) => __incan_items.push(__incan_item),
                None => break __incan_items,
            }
        }
    }}
}

/// Emit `.count()` as a loop over `__next__`, preserving Incan's signed `int` result.
fn emit_count(receiver: &TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        let mut __incan_total = 0i64;
        loop {
            match #next {
                Some(_) => __incan_total += 1,
                None => break __incan_total,
            }
        }
    }}
}

/// Emit `.reduce(init, f)` as Rust's explicit-accumulator iterator fold.
///
/// RFC 088 keeps `reduce` and `fold` aligned for now: both require an initial accumulator and both consume the
/// receiver. Keeping this as a separate helper makes it straightforward to diverge later if a no-initial-value
/// reduction is standardized.
fn emit_reduce(emitter: &IrEmitter<'_>, receiver: &TokenStream, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    match (args.first(), args.get(1)) {
        (Some(init), Some(callback)) => {
            let init = emitter.emit_expr(init)?;
            let callback = emitter.emit_expr(callback)?;
            Ok(emit_fold_loop(receiver, init, callback))
        }
        _ => Ok(quote! { () }),
    }
}

/// Emit `.fold(init, f)` as Rust's explicit-accumulator iterator fold.
fn emit_fold(emitter: &IrEmitter<'_>, receiver: &TokenStream, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    match (args.first(), args.get(1)) {
        (Some(init), Some(callback)) => {
            let init = emitter.emit_expr(init)?;
            let callback = emitter.emit_expr(callback)?;
            Ok(emit_fold_loop(receiver, init, callback))
        }
        _ => Ok(quote! { () }),
    }
}

/// Emit the shared terminal loop for `.fold(init, f)` and `.reduce(init, f)`.
fn emit_fold_loop(receiver: &TokenStream, init: TokenStream, callback: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        let mut __incan_acc = #init;
        loop {
            match #next {
                Some(__incan_item) => __incan_acc = (#callback)(__incan_acc, __incan_item),
                None => break __incan_acc,
            }
        }
    }}
}

/// Emit `.any(f)` with short-circuiting over the Incan iterator protocol.
fn emit_any(receiver: &TokenStream, callback: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        loop {
            match #next {
                Some(__incan_item) => {
                    if (#callback)(__incan_item) {
                        break true;
                    }
                }
                None => break false,
            }
        }
    }}
}

/// Emit `.all(f)` with short-circuiting over the Incan iterator protocol.
fn emit_all(receiver: &TokenStream, callback: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        loop {
            match #next {
                Some(__incan_item) => {
                    if !(#callback)(__incan_item) {
                        break false;
                    }
                }
                None => break true,
            }
        }
    }}
}

/// Emit `.find(f)`, returning the first item whose cloned value satisfies the predicate.
fn emit_find(receiver: &TokenStream, callback: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        loop {
            match #next {
                Some(__incan_item) => {
                    if (#callback)(__incan_item.clone()) {
                        break Some(__incan_item);
                    }
                }
                None => break None,
            }
        }
    }}
}

/// Emit `.for_each(f)` as a side-effecting drain of the receiver.
fn emit_for_each(receiver: &TokenStream, callback: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        loop {
            match #next {
                Some(__incan_item) => (#callback)(__incan_item),
                None => break (),
            }
        }
    }}
}

/// Emit the positional argument at `index`, returning `None` for malformed already-diagnosed calls.
fn emit_arg(emitter: &IrEmitter<'_>, args: &[TypedExpr], index: usize) -> Result<Option<TokenStream>, EmitError> {
    args.get(index).map(|arg| emitter.emit_expr(arg)).transpose()
}

/// Emit a count argument for `take`, `skip`, and `batch`.
///
/// The frontend typechecks these arguments as Incan `int`. Emission keeps integer literals direct so generated code
/// stays readable, and casts non-literals to `i64` before delegating final boundary behavior to `incan_stdlib::iter`.
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

/// Emit `.sum()` for primitive numeric iterators and newtypes over primitive numeric values.
///
/// The loop consumes the Incan iterator protocol directly. For newtypes, generated code unwraps to the underlying
/// carrier, sums that carrier, then reconstructs the newtype. Checked newtypes intentionally route through the selected
/// checked constructor so an invalid empty-iterator zero or invalid accumulated value fails at runtime.
fn emit_sum(emitter: &IrEmitter<'_>, receiver: &TypedExpr, r: &TokenStream) -> TokenStream {
    let Some(item_ty) = iterator_element_type(&receiver.ty) else {
        return emit_primitive_sum_loop(r, quote! { i64 }, quote! { 0i64 });
    };
    if let Some((newtype_name, underlying_ty)) = newtype_sum_underlying(emitter, item_ty) {
        let sum_ty = emitter.emit_type(underlying_ty);
        let zero = sum_zero_for_type(underlying_ty);
        return emit_newtype_sum_loop(emitter, r, newtype_name, sum_ty, zero);
    }
    let sum_ty = emitter.emit_type(item_ty);
    let zero = sum_zero_for_type(item_ty);
    emit_primitive_sum_loop(r, sum_ty, zero)
}

/// Return the primitive zero literal used to seed an RFC 088 `.sum()` loop.
fn sum_zero_for_type(ty: &IrType) -> TokenStream {
    match ty {
        IrType::Float => quote! { 0.0f64 },
        _ => quote! { 0i64 },
    }
}

/// Emit `.sum()` for primitive numeric item types.
fn emit_primitive_sum_loop(receiver: &TokenStream, sum_ty: TokenStream, zero: TokenStream) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    quote! {{
        let mut __incan_iter = #receiver;
        let mut __incan_sum: #sum_ty = #zero;
        loop {
            match #next {
                Some(__incan_item) => __incan_sum += __incan_item,
                None => break __incan_sum,
            }
        }
    }}
}

/// Emit `.sum()` for newtype item types by summing the carrier and reconstructing the wrapper.
fn emit_newtype_sum_loop(
    emitter: &IrEmitter<'_>,
    receiver: &TokenStream,
    newtype_name: &str,
    sum_ty: TokenStream,
    zero: TokenStream,
) -> TokenStream {
    let next = next_call(&quote! { __incan_iter });
    let reconstructed = emit_newtype_from_underlying_sum(emitter, newtype_name, quote! { __incan_sum });
    quote! {{
        let mut __incan_iter = #receiver;
        let mut __incan_sum: #sum_ty = #zero;
        loop {
            match #next {
                Some(__incan_item) => __incan_sum += __incan_item.0,
                None => break #reconstructed,
            }
        }
    }}
}

/// Return a newtype's primitive summation carrier when the item type is supported by RFC 088 `.sum()`.
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

/// Reconstruct a newtype after summing its primitive carrier.
///
/// Unchecked tuple newtypes are emitted as direct tuple-struct construction. Checked newtypes go through the
/// constructor chosen during lowering so runtime validation remains the single source of truth.
fn emit_newtype_from_underlying_sum(
    emitter: &IrEmitter<'_>,
    newtype_name: &str,
    underlying_sum: TokenStream,
) -> TokenStream {
    let newtype_path = emit_path_ident(newtype_name);
    if let Some(ctor) = emitter
        .newtype_construction
        .get(newtype_name)
        .and_then(|plan| plan.checked_constructor.as_ref())
    {
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

/// Emit a possibly qualified Rust path as identifiers instead of a string.
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

/// Return the element type for a receiver that is statically typed as `Iterator[T]`.
fn iterator_element_type(receiver_ty: &IrType) -> Option<&IrType> {
    let receiver_ty = receiver_type_for_iterator_dispatch(receiver_ty);
    match receiver_ty {
        IrType::NamedGeneric(name, args) if is_iterator_protocol_type_name(name) => args.first(),
        _ => None,
    }
}
