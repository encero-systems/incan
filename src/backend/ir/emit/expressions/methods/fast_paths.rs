use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{IrCallArg, IrExprKind, TypedExpr};
use crate::backend::ir::types::IrType;
use incan_core::lang::generated_support::{self, MethodFastPath, MethodFastPathArgShape};

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

fn named_generic_receiver(ty: &IrType) -> Option<(&str, &[IrType])> {
    match peel_refs(ty) {
        IrType::NamedGeneric(name, args) => Some((name.as_str(), args.as_slice())),
        _ => None,
    }
}

fn peel_refs(ty: &IrType) -> &IrType {
    let mut ty = ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = ty {
        ty = inner.as_ref();
    }
    ty
}

fn type_name_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.rsplit("::").next() == Some(expected)
}

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

fn type_module_matches(emitter: &IrEmitter, type_name: &str, fast_path: &MethodFastPath) -> bool {
    let short_name = type_name.rsplit("::").next().unwrap_or(type_name);
    type_path_matches(type_name, fast_path.source_module, fast_path.receiver_type)
        || type_path_matches(type_name, fast_path.generated_module, fast_path.receiver_type)
        || [type_name, short_name].iter().any(|name| {
            emitter.type_module_paths.get(*name).is_some_and(|module| {
                module_matches(module, fast_path.source_module) || module_matches(module, fast_path.generated_module)
            })
        })
}

fn type_path_matches(type_name: &str, module: &str, receiver_type: &str) -> bool {
    let module_path = module.replace('.', "::");
    type_name == format!("{module_path}::{receiver_type}")
}

fn module_matches(actual: &[String], expected: &str) -> bool {
    actual.iter().map(String::as_str).eq(expected.split('.'))
}

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

fn list_element_type(ty: &IrType) -> Option<&IrType> {
    match peel_refs(ty) {
        IrType::List(elem) => Some(elem.as_ref()),
        _ => None,
    }
}

fn is_owned_string_type(ty: &IrType) -> bool {
    matches!(peel_refs(ty), IrType::String)
}

fn borrowed_str_tokens(ty: &IrType, emitted: TokenStream) -> TokenStream {
    match ty {
        IrType::StaticStr | IrType::StrRef => emitted,
        IrType::FrozenStr => quote! { #emitted.as_str() },
        IrType::Ref(inner) | IrType::RefMut(inner) => match peel_refs(inner) {
            IrType::StaticStr | IrType::StrRef => emitted,
            IrType::FrozenStr => quote! { #emitted.as_str() },
            _ => quote! { <_ as AsRef<str>>::as_ref(#emitted) },
        },
        _ => quote! { <_ as AsRef<str>>::as_ref(&#emitted) },
    }
}

fn borrow_expr_for_call(ty: &IrType, emitted: TokenStream) -> TokenStream {
    match ty {
        IrType::Ref(_) | IrType::RefMut(_) => emitted,
        _ => quote! { &#emitted },
    }
}
