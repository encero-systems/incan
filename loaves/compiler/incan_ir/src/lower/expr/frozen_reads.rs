//! Reads of the frozen collections a `const` holds: `FrozenList[T]`, `FrozenSet[T]` and `FrozenDict[K, V]`.
//!
//! A frozen collection stores Incan `str` and `bytes` items as `'static` Rust views (`StaticStr`, `StaticBytes`),
//! while the checker types every item or value a read produces as the owned `str` or `bytes`. The helpers here give
//! each read the checked type at lowering, so no read of a frozen collection reaches emission carrying the storage
//! view: an item read is converted to its owned form, an iteration source iterates owned items, `len()` is the `int`
//! length `len(c)` produces, and a membership probe reaches the collection's own membership test.

use super::super::super::TypedExpr;
use super::super::super::expr::{BuiltinFn, IrExprKind, IrInteropCoercionKind, UnaryOp};
use super::super::super::types::IrType;
use incan_lang::interop::CoercionPolicy;
use incan_lang::lang::surface::{frozen_bytes_methods, frozen_dict_methods, frozen_list_methods, frozen_set_methods};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

/// Return the frozen collection family of a type and the item one iteration of it yields: the element of a
/// `FrozenList` or `FrozenSet`, the key of a `FrozenDict`.
pub(super) fn frozen_collection_item(ty: &IrType) -> Option<(CollectionTypeId, &IrType)> {
    let IrType::NamedGeneric(name, args) = ty else {
        return None;
    };
    match collections::from_str(name)? {
        id @ (CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet | CollectionTypeId::FrozenDict) => {
            args.first().map(|item| (id, item))
        }
        _ => None,
    }
}

/// Return the owned type a read of one frozen item produces: `'static` text reads as `str`, `'static` bytes as
/// `bytes`, and every other item as itself.
pub(super) fn owned_frozen_item_type(item: &IrType) -> IrType {
    match item {
        IrType::StaticStr | IrType::StrRef => IrType::String,
        IrType::StaticBytes => IrType::Bytes,
        other => other.clone(),
    }
}

/// Convert one value read out of a frozen collection to the owned type the checker gave the read.
///
/// The conversion is the exact `'static` view to owned value coercion a Rust boundary return takes (`&str` to `str`,
/// `&[u8]` to `bytes`), so every later use of the read sees an ordinary `str` or `bytes` value. A value that is not a
/// `'static` view is returned unchanged.
pub(super) fn owned_frozen_read(read: TypedExpr) -> TypedExpr {
    let (owned_ty, rust_target) = match &read.ty {
        IrType::StaticStr => (IrType::String, "String"),
        IrType::StaticBytes => (IrType::Bytes, "Vec<u8>"),
        _ => return read,
    };
    let span = read.span;
    let from_ty = read.ty.clone();
    TypedExpr::new(
        IrExprKind::InteropCoerce {
            expr: Box::new(read),
            from_ty,
            to_ty: owned_ty.clone(),
            kind: IrInteropCoercionKind::Builtin {
                policy: CoercionPolicy::Exact,
                rust_target: rust_target.to_string(),
            },
        },
        owned_ty,
    )
    .with_span(span)
}

/// Hand a comprehension, a generator clause or a `set(...)` conversion over a frozen collection the owned list of the
/// items the checker typed.
///
/// Iterating the frozen wrapper directly yields borrows of its `'static` storage, a `FrozenDict` yields its entries
/// rather than its keys, and text and bytes items are `'static` views; `list(source)` is the conversion that yields
/// exactly the owned items (the keys of a `FrozenDict`) the checker bound (#1757). Every other source is returned
/// unchanged.
pub(super) fn owned_frozen_iteration_source(source: TypedExpr) -> TypedExpr {
    let Some((_, item)) = frozen_collection_item(&source.ty) else {
        return source;
    };
    let owned_item = owned_frozen_item_type(item);
    let span = source.span;
    TypedExpr::new(
        IrExprKind::BuiltinCall {
            func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
            args: vec![source],
        },
        IrType::List(Box::new(owned_item)),
    )
    .with_span(span)
}

/// Whether a `set(...)` conversion over this source needs the owned item list [`owned_frozen_iteration_source`]
/// builds: a frozen collection whose items are `'static` views, or a `FrozenDict`, whose items are its keys.
pub(super) fn set_source_needs_owned_frozen_items(source_ty: &IrType) -> bool {
    frozen_collection_item(source_ty)
        .is_some_and(|(family, item)| family == CollectionTypeId::FrozenDict || owned_frozen_item_type(item) != *item)
}

/// Borrow a `FrozenList` or `FrozenSet` membership receiver, so the membership test reads the frozen storage the way
/// it reads a borrowed list or set; every other receiver is returned unchanged.
///
/// A `FrozenDict` keeps its own receiver: its membership is the keyed lookup a dict takes.
pub(super) fn frozen_membership_receiver(receiver: TypedExpr) -> TypedExpr {
    if !matches!(
        frozen_collection_item(&receiver.ty),
        Some((CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet, _))
    ) {
        return receiver;
    }
    let span = receiver.span;
    let borrowed_ty = IrType::Ref(Box::new(receiver.ty.clone()));
    TypedExpr::new(
        IrExprKind::UnaryOp {
            op: UnaryOp::Ref,
            operand: Box::new(receiver),
        },
        borrowed_ty,
    )
    .with_span(span)
}

/// Whether a no-argument method call on this receiver is the frozen `len()`, which reads as the `int` that `len(c)`
/// produces rather than the wrapper's own `usize` length.
pub(super) fn is_frozen_len_call(receiver_ty: &IrType, method: &str) -> bool {
    match receiver_ty {
        IrType::FrozenBytes | IrType::StaticBytes => {
            frozen_bytes_methods::from_str(method) == Some(frozen_bytes_methods::FrozenBytesMethodId::Len)
        }
        ty => match frozen_collection_item(ty) {
            Some((CollectionTypeId::FrozenList, _)) => {
                frozen_list_methods::from_str(method) == Some(frozen_list_methods::FrozenListMethodId::Len)
            }
            Some((CollectionTypeId::FrozenSet, _)) => {
                frozen_set_methods::from_str(method) == Some(frozen_set_methods::FrozenSetMethodId::Len)
            }
            Some((CollectionTypeId::FrozenDict, _)) => {
                frozen_dict_methods::from_str(method) == Some(frozen_dict_methods::FrozenDictMethodId::Len)
            }
            _ => false,
        },
    }
}
