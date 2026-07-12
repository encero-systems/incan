//! Emit descriptor-registered method fast paths.
//!
//! This module is the generic bridge between source-level Incan methods and generated Rust helper methods that exist
//! for performance-sensitive stdlib surfaces. It deliberately does not know about `OrdinalMap`, or any other concrete
//! collection, by name. Fast paths are declared in `incan_core::lang::generated_support`; this emitter only checks that
//! the lowered receiver and argument shapes match a descriptor and then emits the corresponding helper call.
//!
//! Returning `None` is part of the contract: if no descriptor matches, ordinary method-call emission continues. That
//! keeps these accelerators optional implementation details rather than semantic requirements of the language surface.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{IrCallArg, IrExprKind, TypedExpr};
use crate::backend::ir::types::IrType;
use incan_core::lang::generated_support::{self, MethodFastPath, MethodFastPathArgShape};

/// Emit a descriptor-backed helper call for a known fast path, if the call shape matches.
///
/// Descriptors match on the concrete receiver family, receiver type argument, method name, and a narrow argument
/// borrowing policy. The emitted method keeps the original Incan API value-shaped while allowing generated Rust to pass
/// borrowed views into stdlib-adjacent helper code where that is the efficient representation.
pub(super) fn emit_registered_method_fast_path(
    emitter: &IrEmitter,
    receiver: &TypedExpr,
    method: &str,
    args: &[IrCallArg],
    receiver_tokens: &TokenStream,
) -> Result<Option<TokenStream>, EmitError> {
    let Some(arg) = args.first() else {
        return Ok(None);
    };
    if args.len() != 1 {
        return Ok(None);
    }

    for fast_path in generated_support::method_fast_paths() {
        if method == fast_path.method && receiver_matches_fast_path(emitter, &receiver.ty, fast_path) {
            let target = format_ident!("{}", fast_path.target_method);
            let arg_tokens = emit_fast_path_arg(emitter, fast_path.arg_shape, &arg.expr)?;
            return Ok(Some(quote! { #receiver_tokens.#target(#arg_tokens) }));
        }
    }

    Ok(None)
}

/// Return whether a receiver can use a method fast path.
fn receiver_matches_fast_path(emitter: &IrEmitter, receiver_ty: &IrType, fast_path: &MethodFastPath) -> bool {
    let Some((name, args)) = named_generic_receiver(receiver_ty) else {
        return false;
    };
    type_name_matches(name, fast_path.receiver_type)
        && args
            .first()
            .is_some_and(|arg| concrete_type_arg_matches(arg, fast_path.receiver_arg_type))
        && type_module_matches(emitter, name, fast_path)
}

/// Return the named generic receiver, if present.
fn named_generic_receiver(ty: &IrType) -> Option<(&str, &[IrType])> {
    match peel_refs(ty) {
        IrType::NamedGeneric(name, args) => Some((name.as_str(), args.as_slice())),
        _ => None,
    }
}

/// Remove transparent reference wrappers from an IR type.
fn peel_refs(ty: &IrType) -> &IrType {
    let mut ty = ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = ty {
        ty = inner.as_ref();
    }
    ty
}

/// Return whether a type name matches an expected Rust path.
fn type_name_matches(actual: &str, expected: &str) -> bool {
    actual == expected || terminal_type_name(actual) == expected
}

/// Return the final nominal segment from either source-qualified (`std.collections.Type`) or Rust-qualified paths.
fn terminal_type_name(type_name: &str) -> &str {
    type_name
        .rsplit([':', '.'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(type_name)
}

/// Return whether a concrete type argument matches an expected Rust path.
fn concrete_type_arg_matches(actual: &IrType, expected: &str) -> bool {
    match expected {
        "str" => matches!(
            peel_refs(actual),
            IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
        ),
        "bytes" => matches!(
            peel_refs(actual),
            IrType::Bytes | IrType::StaticBytes | IrType::FrozenBytes
        ),
        _ => peel_refs(actual).incan_name() == expected,
    }
}

/// Return whether a type module path matches an expected Rust module.
fn type_module_matches(emitter: &IrEmitter, type_name: &str, fast_path: &MethodFastPath) -> bool {
    let short_name = terminal_type_name(type_name);
    type_path_matches(type_name, fast_path.source_module, fast_path.receiver_type)
        || type_path_matches(type_name, fast_path.generated_module, fast_path.receiver_type)
        || emitter.is_builtin_stdlib_artifact_type_in_module(short_name, fast_path.source_module)
        || [type_name, short_name].iter().any(|name| {
            emitter.type_module_paths.get(*name).is_some_and(|module| {
                module_matches(module, fast_path.source_module)
                    || module_matches(module, fast_path.generated_module)
                    || compiled_artifact_module_matches(emitter, module, fast_path.source_module)
            })
        })
}

/// Return whether an artifact-owned provider module is the direct crate-root projection of a stdlib source module.
fn compiled_artifact_module_matches(emitter: &IrEmitter, module: &[String], source_module: &str) -> bool {
    let Some(artifact_module) = source_module.strip_prefix("std.") else {
        return false;
    };
    emitter.is_builtin_stdlib_artifact_module_path(module) && module_matches(module, artifact_module)
}

/// Return whether a type path matches an expected Rust path.
fn type_path_matches(type_name: &str, module: &str, receiver_type: &str) -> bool {
    let module_path = module.replace('.', "::");
    type_name == format!("{module_path}::{receiver_type}") || type_name == format!("{module}.{receiver_type}")
}

/// Return whether a module path matches an expected Rust module.
fn module_matches(actual: &[String], expected: &str) -> bool {
    actual.iter().map(String::as_str).eq(expected.split('.'))
}

/// Emit one argument for a method fast path.
fn emit_fast_path_arg(
    emitter: &IrEmitter,
    shape: MethodFastPathArgShape,
    arg: &TypedExpr,
) -> Result<TokenStream, EmitError> {
    match shape {
        MethodFastPathArgShape::BorrowedStr => emit_borrowed_str_arg(emitter, arg),
        MethodFastPathArgShape::BorrowedStringList => {
            let emitted = emitter.emit_expr(arg)?;
            Ok(borrow_expr_for_call(&arg.ty, emitted))
        }
    }
}

/// Emit an argument borrowed as `str` for a method fast path.
fn emit_borrowed_str_arg(emitter: &IrEmitter, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
    if let IrExprKind::Index { object, index } = &arg.kind
        && list_element_type(&object.ty).is_some_and(is_owned_string_type)
    {
        let object_tokens = emitter.emit_expr(object)?;
        let index_tokens = emitter.emit_expr(index)?;
        let list_tokens = borrow_expr_for_call(&object.ty, object_tokens);
        return Ok(quote! { incan_stdlib::collections::list_get(#list_tokens, (#index_tokens) as i64).as_str() });
    }

    let emitted = emitter.emit_expr(arg)?;
    Ok(borrowed_str_tokens(&arg.ty, emitted))
}

/// Return the element type for a list IR type.
fn list_element_type(ty: &IrType) -> Option<&IrType> {
    match peel_refs(ty) {
        IrType::List(elem) => Some(elem.as_ref()),
        _ => None,
    }
}

/// Return whether an IR type is owned string storage.
fn is_owned_string_type(ty: &IrType) -> bool {
    matches!(peel_refs(ty), IrType::String)
}

/// Emit tokens that borrow an expression as `str`.
fn borrowed_str_tokens(ty: &IrType, emitted: TokenStream) -> TokenStream {
    match ty {
        IrType::StaticStr | IrType::StrRef => emitted,
        IrType::FrozenStr => quote! { <_ as AsRef<str>>::as_ref(&#emitted) },
        IrType::Ref(inner) | IrType::RefMut(inner) => match peel_refs(inner) {
            IrType::StaticStr | IrType::StrRef => emitted,
            IrType::FrozenStr => quote! { <_ as AsRef<str>>::as_ref(#emitted) },
            _ => quote! { <_ as AsRef<str>>::as_ref(#emitted) },
        },
        _ => quote! { <_ as AsRef<str>>::as_ref(&#emitted) },
    }
}

/// Emit an expression borrowed for a Rust call boundary.
fn borrow_expr_for_call(ty: &IrType, emitted: TokenStream) -> TokenStream {
    match ty {
        IrType::Ref(_) | IrType::RefMut(_) => emitted,
        _ => quote! { &#emitted },
    }
}
