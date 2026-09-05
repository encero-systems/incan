//! Emit Rust code for built-in function calls.
//!
//! This module handles emission of known built-in functions using enum-based dispatch
//! (`BuiltinFn`). It also contains the legacy string-based fallback for `Call` expressions
//! that haven't been lowered to `BuiltinCall`.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::conversions::exact_float_value_validation;
use super::super::super::expr::{BuiltinFn, IrExprKind, Pattern, TypedExpr};
use super::super::super::ownership::ValueUseSite;
use super::super::super::types::{
    IR_UNION_TYPE_NAME, IrType, SetConstructorIteration, isinstance_type_matches, isinstance_union_variant_indices,
};
use super::super::{EmitError, IrEmitter};
use super::methods::iterator_methods::emit_iter_receiver;
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::types::collections::{self, CollectionTypeId};

/// Get the element type of a list.
fn list_elem_type(ty: &IrType) -> &IrType {
    match ty {
        IrType::List(elem) => elem.as_ref(),
        IrType::NamedGeneric(name, args)
            if collections::from_str(name.as_str()) == Some(CollectionTypeId::FrozenList) =>
        {
            args.first().unwrap_or(ty)
        }
        IrType::Ref(inner) | IrType::RefMut(inner) => list_elem_type(inner),
        other => other,
    }
}

/// Return whether `enumerate()` can materialize source-level values with Rust `copied()`.
///
/// This is intentionally narrower than [`IrType::is_copy`]. `enumerate(xs)` yields owned `(int, T)` tuples in Incan,
/// so reference, tuple, option, result, and generic-placeholder items use the clone path unless the element family is a
/// scalar value with an unambiguous owned Rust representation.
fn enumerate_elem_can_copy(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Numeric(_)
            | IrType::Decimal { .. }
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
    )
}

/// Check if a type is a named generic.
fn is_named_generic(ty: &IrType, name: &str) -> bool {
    match ty {
        IrType::NamedGeneric(n, _) => n == name,
        IrType::Ref(inner) | IrType::RefMut(inner) => matches!(inner.as_ref(), IrType::NamedGeneric(n, _) if n == name),
        _ => false,
    }
}

fn is_frozen_collection_named_generic(ty: &IrType) -> bool {
    [
        CollectionTypeId::FrozenList,
        CollectionTypeId::FrozenSet,
        CollectionTypeId::FrozenDict,
    ]
    .iter()
    .any(|id| is_named_generic(ty, collections::as_str(*id)))
}

/// Convert a Rust filesystem result into the legacy builtin's declared owned-string error contract.
fn stringify_file_io_error(result: TokenStream) -> TokenStream {
    quote! { (#result).map_err(|error| error.to_string()) }
}

/// Emit scalar or trait-backed JSON serialization after evaluating the source operand exactly once.
fn emit_json_stringify(emitter: &IrEmitter<'_>, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    let [arg] = args else {
        return Err(EmitError::InternalInvariant(format!(
            "checked json_stringify call reached emission with {} operands instead of one",
            args.len()
        )));
    };
    // A source-level `None` has no payload type for Rust to infer here. JSON serializes every empty `Option<T>` as
    // `null`, so use the unit payload as the concrete, behavior-neutral representative at this builtin boundary.
    let value = if matches!(&arg.kind, IrExprKind::None) {
        quote! { None::<()> }
    } else {
        emitter.emit_expr(arg)?
    };
    let binding = if matches!(&arg.ty, IrType::Int) {
        // Integer literals are emitted without a suffix elsewhere so surrounding Rust can infer their type. The
        // generic serializer supplies no such context, so retain Incan's canonical signed 64-bit representation.
        quote! { let __incan_json_value: &i64 = &(#value); }
    } else {
        // Bind one borrow so an rvalue is evaluated once while a named owned value remains available to its source.
        quote! { let __incan_json_value = &(#value); }
    };
    Ok(quote! {{
        #binding
        incan_stdlib::json::__private::stringify_or_raise(
            __incan_json_value,
            std::any::type_name_of_val(__incan_json_value),
        )
    }})
}

/// Build the exact generated-union variant pattern for one checked member index.
fn isinstance_union_pattern(union_ty: &IrType, variant_index: usize) -> Result<Pattern, EmitError> {
    let variant = union_ty.union_variant_path(variant_index).ok_or_else(|| {
        EmitError::InternalInvariant("checked isinstance union lost its generated variant identity".to_string())
    })?;
    Ok(Pattern::Enum {
        name: union_ty
            .union_type_name()
            .unwrap_or_else(|| IR_UNION_TYPE_NAME.to_string()),
        variant,
        fields: vec![Pattern::Wildcard],
    })
}

/// Build every source-union variant pattern whose semantic identity satisfies one retained target.
fn isinstance_union_patterns(union_ty: &IrType, target_ty: &IrType) -> Result<Option<Vec<Pattern>>, EmitError> {
    let Some(indices) = isinstance_union_variant_indices(union_ty, target_ty) else {
        return Ok(None);
    };
    indices
        .into_iter()
        .map(|index| isinstance_union_pattern(union_ty, index))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Collapse one or more matching alternatives into a single IR pattern.
fn isinstance_alternatives(mut patterns: Vec<Pattern>) -> Pattern {
    if patterns.len() == 1 {
        patterns.remove(0)
    } else {
        Pattern::Or(patterns)
    }
}

/// Emit a checked `isinstance(value, Target)` expression without materializing `Target` at runtime.
fn emit_isinstance(emitter: &IrEmitter<'_>, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
    let [value, target] = args else {
        return Err(EmitError::InternalInvariant(format!(
            "checked isinstance reached emission with {} operands instead of two",
            args.len()
        )));
    };
    let IrExprKind::TypeToken { ty: target_ty } = &target.kind else {
        return Err(EmitError::InternalInvariant(
            "checked isinstance reached emission without its retained target token".to_string(),
        ));
    };
    let value_tokens = emitter.emit_expr(value)?;

    let pattern = if let Some(patterns) = isinstance_union_patterns(&value.ty, target_ty)? {
        Some(isinstance_alternatives(patterns))
    } else if let IrType::Option(inner) = &value.ty {
        if let Some(patterns) = isinstance_union_patterns(inner, target_ty)? {
            let patterns = patterns
                .into_iter()
                .map(|pattern| Pattern::Enum {
                    name: "Option".to_string(),
                    variant: constructors::as_str(ConstructorId::Some).to_string(),
                    fields: vec![pattern],
                })
                .collect();
            Some(isinstance_alternatives(patterns))
        } else if isinstance_type_matches(inner, target_ty) {
            Some(Pattern::Enum {
                name: "Option".to_string(),
                variant: constructors::as_str(ConstructorId::Some).to_string(),
                fields: vec![Pattern::Wildcard],
            })
        } else {
            None
        }
    } else {
        None
    };

    if let Some(pattern) = pattern {
        let pattern = emitter.emit_pattern(&pattern);
        Ok(quote! { matches!(#value_tokens, #pattern) })
    } else {
        let result = isinstance_type_matches(&value.ty, target_ty);
        Ok(quote! {{ let _ = #value_tokens; #result }})
    }
}

/// Emit the builtin `zip(left, right)` as the same source-owned iterator model used by `.zip()`.
fn emit_zip(emitter: &IrEmitter<'_>, args: &[TypedExpr]) -> Result<Option<TokenStream>, EmitError> {
    let [left, right, ..] = args else {
        return Ok(None);
    };
    let left_tokens = emitter.emit_expr(left)?;
    let right_tokens = emitter.emit_expr(right)?;
    let left_iter = emit_iter_receiver(left, &left_tokens);
    let right_iter = emit_iter_receiver(right, &right_tokens);
    Ok(Some(quote! {
        crate::__incan_std::derives::collection::ZipIterator {
            left: (#left_iter),
            right: (#right_iter),
            left_marker: None,
            right_marker: None,
        }
    }))
}

/// Return whether `ty` lowers to a Rust string-like value with `.chars()`.
fn is_string_iterable_type(ty: &IrType) -> bool {
    match ty {
        IrType::String | IrType::StaticStr | IrType::StrRef => true,
        IrType::Ref(inner) | IrType::RefMut(inner) => is_string_iterable_type(inner),
        _ => false,
    }
}

/// Return whether `ty` lowers to a `FrozenStr` wrapper that must be unwrapped before iteration.
fn is_frozen_string_iterable_type(ty: &IrType) -> bool {
    match ty {
        IrType::FrozenStr => true,
        IrType::Ref(inner) | IrType::RefMut(inner) => is_frozen_string_iterable_type(inner),
        _ => false,
    }
}

/// Emit canonical string length without consuming the argument; retain ordinary `.len()` for other operands.
fn emit_len(emitter: &IrEmitter<'_>, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
    let value = emitter.emit_expr(arg)?;
    if is_string_iterable_type(&arg.ty) || is_frozen_string_iterable_type(&arg.ty) {
        Ok(quote! { incan_stdlib::strings::str_len(&(#value)) })
    } else {
        Ok(quote! { ::std::convert::identity(#value.len() as i64) })
    }
}

/// Emit integer `abs` with one checked language behavior in every Rust build profile.
fn emit_abs(emitter: &IrEmitter<'_>, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
    let value = emitter.emit_expr(arg)?;
    Ok(quote! {
        ::std::convert::identity::<i64>(#value).checked_abs().unwrap_or_else(|| {
            incan_stdlib::errors::raise_value_error("integer overflow in builtin `abs`")
        })
    })
}

/// Emit builtin integer `sum` with checked accumulation independent of Rust overflow-check settings.
fn emit_sum(emitter: &IrEmitter<'_>, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
    let value = emitter.emit_expr(arg)?;
    let element = if matches!(list_elem_type(&arg.ty), IrType::Bool) {
        quote! { if *value { 1_i64 } else { 0_i64 } }
    } else {
        quote! { *value }
    };
    Ok(quote! {
        (#value)
            .iter()
            .try_fold(0_i64, |total, value| total.checked_add(#element))
            .unwrap_or_else(|| {
                incan_stdlib::errors::raise_value_error("integer overflow in builtin `sum`")
            })
    })
}

/// Return whether `ty` lowers to a byte vector or byte slice that yields Incan integer items.
fn is_bytes_iterable_type(ty: &IrType) -> bool {
    match ty {
        IrType::Bytes | IrType::StaticBytes => true,
        IrType::Ref(inner) | IrType::RefMut(inner) => is_bytes_iterable_type(inner),
        _ => false,
    }
}

/// Return whether `ty` lowers to a `FrozenBytes` wrapper that must be unwrapped before iteration.
fn is_frozen_bytes_iterable_type(ty: &IrType) -> bool {
    match ty {
        IrType::FrozenBytes => true,
        IrType::Ref(inner) | IrType::RefMut(inner) => is_frozen_bytes_iterable_type(inner),
        _ => false,
    }
}

impl<'a> IrEmitter<'a> {
    /// Emit a `print`/`println` call, rendering **every** argument space-separated.
    ///
    /// Both call paths funnel here because both previously emitted `args.first()` and discarded the rest: a program
    /// writing `println("count", 3)` typechecked, compiled, and printed `count`. Nothing reported the loss --
    /// `check_expr::calls::builtins` gives `Print` no arity check, unlike `Len` beside it -- so the missing output
    /// was invisible from source, from diagnostics, and from the generated Rust unless read line by line.
    ///
    /// The separator is a single space, matching Python's `print` and the replacement executor's own rendering, so
    /// the two backends agree on what a multi-argument print produces rather than one of them dropping arguments.
    fn emit_print_call(&self, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        if args.is_empty() {
            return Ok(quote! { println!() });
        }
        let rendered = args
            .iter()
            .map(|arg| {
                let emitted = self.emit_expr(arg)?;
                Ok(exact_float_value_validation(&arg.ty).apply(emitted))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // One `{}` per argument, joined by the separator. Built here rather than in `quote!` because the format
        // string must reach the macro as a literal, not as a runtime `&str`.
        let format = proc_macro2::Literal::string(&vec!["{}"; rendered.len()].join(" "));
        Ok(quote! { println!(#format, #(#rendered),*) })
    }

    /// Emit the iterator expression for `enumerate(arg)`.
    ///
    /// The frontend models `str` iteration as one-character `str` values and `bytes` iteration as Incan `int`
    /// values, so this helper materializes those language-level item types instead of leaking Rust `char`, `u8`,
    /// or reference items into generated code.
    pub(in super::super) fn emit_enumerate_iter(&self, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
        let a = self.emit_expr(arg)?;
        let tokens = if is_string_iterable_type(&arg.ty) {
            quote! { (#a).chars().enumerate().map(|(idx, value)| (idx as i64, value.to_string())) }
        } else if is_frozen_string_iterable_type(&arg.ty) {
            quote! { (#a).as_str().chars().enumerate().map(|(idx, value)| (idx as i64, value.to_string())) }
        } else if is_bytes_iterable_type(&arg.ty) {
            quote! { (#a).iter().enumerate().map(|(idx, value)| (idx as i64, (*value) as i64)) }
        } else if is_frozen_bytes_iterable_type(&arg.ty) {
            quote! { (#a).as_slice().iter().enumerate().map(|(idx, value)| (idx as i64, (*value) as i64)) }
        } else if enumerate_elem_can_copy(list_elem_type(&arg.ty)) {
            quote! { #a.iter().copied().enumerate().map(|(idx, value)| (idx as i64, value)) }
        } else {
            quote! { #a.iter().enumerate().map(|(idx, value)| (idx as i64, value.clone())) }
        };
        Ok(tokens)
    }

    /// Emit `enumerate(arg)` where the checked expression is retained as a `list[tuple[int, T]]` value.
    ///
    /// Direct `for` and comprehension consumers call [`Self::emit_enumerate_iter`] instead, preserving the lazy
    /// iterator path they consume immediately. A stored value must materialize before later list traversal.
    fn emit_enumerate_value(&self, arg: &TypedExpr) -> Result<TokenStream, EmitError> {
        let iter = self.emit_enumerate_iter(arg)?;
        Ok(quote! { (#iter).collect::<Vec<_>>() })
    }

    /// Emit a builtin function call using enum-based dispatch.
    ///
    /// This handles calls that have been lowered to `IrExprKind::BuiltinCall`.
    ///
    /// ## Parameters
    /// - `func`: The builtin function enum variant
    /// - `args`: The call arguments
    ///
    /// ## Returns
    /// - A Rust `TokenStream` for the builtin call
    pub(in super::super) fn emit_builtin_call(
        &self,
        func: &BuiltinFn,
        args: &[TypedExpr],
    ) -> Result<TokenStream, EmitError> {
        match func {
            BuiltinFn::IsInstance => emit_isinstance(self, args),
            BuiltinFn::Print => self.emit_print_call(args),
            BuiltinFn::Len => {
                if let Some(arg) = args.first() {
                    emit_len(self, arg)
                } else {
                    Ok(quote! { 0i64 })
                }
            }
            BuiltinFn::Sum => {
                if let Some(arg) = args.first() {
                    emit_sum(self, arg)
                } else {
                    Ok(quote! { 0i64 })
                }
            }
            BuiltinFn::Min => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let tokens = match elem_type {
                        IrType::Float => quote! { incan_stdlib::collections::__private::list_min_f64(&#a) },
                        IrType::String | IrType::FrozenStr => {
                            quote! { incan_stdlib::collections::__private::list_min_clone(&#a) }
                        }
                        _ => quote! { incan_stdlib::collections::__private::list_min_copy(&#a) },
                    };
                    Ok(tokens)
                } else {
                    Ok(quote! { incan_stdlib::errors::raise_value_error("min() missing argument") })
                }
            }
            BuiltinFn::Max => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let tokens = match elem_type {
                        IrType::Float => quote! { incan_stdlib::collections::__private::list_max_f64(&#a) },
                        IrType::String | IrType::FrozenStr => {
                            quote! { incan_stdlib::collections::__private::list_max_clone(&#a) }
                        }
                        _ => quote! { incan_stdlib::collections::__private::list_max_copy(&#a) },
                    };
                    Ok(tokens)
                } else {
                    Ok(quote! { incan_stdlib::errors::raise_value_error("max() missing argument") })
                }
            }
            BuiltinFn::Str => {
                if let Some(arg) = args.first() {
                    let a = exact_float_value_validation(&arg.ty).apply(self.emit_expr(arg)?);
                    Ok(quote! { #a.to_string() })
                } else {
                    Ok(quote! { String::new() })
                }
            }
            BuiltinFn::Int => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    match &arg.ty {
                        IrType::String | IrType::FrozenStr => {
                            Ok(quote! { incan_stdlib::conversions::int_from_str(&#a) })
                        }
                        IrType::Float => Ok(quote! { (#a) as i64 }),
                        IrType::Bool => Ok(quote! { if #a { 1 } else { 0 } }),
                        _ => Ok(quote! { (#a) as i64 }),
                    }
                } else {
                    Ok(quote! { 0i64 })
                }
            }
            BuiltinFn::Float => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    match &arg.ty {
                        IrType::String | IrType::FrozenStr => {
                            Ok(quote! { incan_stdlib::conversions::float_from_str(&#a) })
                        }
                        IrType::Int => Ok(quote! { (#a) as f64 }),
                        _ => Ok(quote! { (#a) as f64 }),
                    }
                } else {
                    Ok(quote! { 0.0f64 })
                }
            }
            BuiltinFn::Bool => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    match &arg.ty {
                        IrType::Bool => Ok(quote! { #a }),
                        IrType::Int => Ok(quote! { (#a) != 0 }),
                        IrType::Float => Ok(quote! { (#a) != 0.0 }),
                        IrType::String => Ok(quote! { !(#a).is_empty() }),
                        IrType::FrozenStr => Ok(quote! { !(#a).is_empty() }),
                        IrType::FrozenBytes => Ok(quote! { !(#a).is_empty() }),
                        IrType::List(_) => Ok(quote! { !(#a).is_empty() }),
                        IrType::Dict(_, _) => Ok(quote! { !(#a).is_empty() }),
                        IrType::Set(_) => Ok(quote! { !(#a).is_empty() }),
                        _ if is_frozen_collection_named_generic(&arg.ty) => Ok(quote! { !(#a).is_empty() }),
                        _ => Ok(quote! { true }),
                    }
                } else {
                    Ok(quote! { false })
                }
            }
            BuiltinFn::Abs => {
                if let Some(arg) = args.first() {
                    emit_abs(self, arg)
                } else {
                    Ok(quote! { 0 })
                }
            }
            BuiltinFn::Range => self
                .emit_range_call(args)
                .map(|opt| opt.unwrap_or_else(|| quote! { 0..0 })),
            BuiltinFn::Enumerate => {
                if let Some(arg) = args.first() {
                    self.emit_enumerate_value(arg)
                } else {
                    Ok(quote! { Vec::<(i64, ())>::new() })
                }
            }
            BuiltinFn::Zip => {
                emit_zip(self, args).map(|tokens| tokens.unwrap_or_else(|| quote! { std::iter::empty::<((), ())>() }))
            }
            BuiltinFn::Sorted => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let from_frozen_list = is_named_generic(&arg.ty, collections::as_str(CollectionTypeId::FrozenList));
                    let tokens = if from_frozen_list {
                        match elem_type {
                            IrType::Float => quote! {{
                                let mut __v = (#a).as_slice().to_vec();
                                __v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                __v
                            }},
                            _ => quote! {{
                                let mut __v = (#a).as_slice().to_vec();
                                __v.sort();
                                __v
                            }},
                        }
                    } else {
                        match elem_type {
                            IrType::Float => quote! {{
                                let mut __v = (#a).clone();
                                __v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                __v
                            }},
                            _ => quote! {{
                                let mut __v = (#a).clone();
                                __v.sort();
                                __v
                            }},
                        }
                    };
                    Ok(tokens)
                } else {
                    Ok(quote! { Vec::new() })
                }
            }
            BuiltinFn::ReadFile => {
                if let Some(arg) = args.first() {
                    let path = self.emit_expr(arg)?;
                    Ok(stringify_file_io_error(quote! { std::fs::read_to_string(#path) }))
                } else {
                    Ok(stringify_file_io_error(quote! {
                        Err::<String, _>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no path"))
                    }))
                }
            }
            BuiltinFn::WriteFile => {
                if args.len() >= 2 {
                    let path = self.emit_expr(&args[0])?;
                    let content = self.emit_expr(&args[1])?;
                    Ok(stringify_file_io_error(
                        quote! { std::fs::write(#path, #content).map(|_| ()) },
                    ))
                } else {
                    Ok(stringify_file_io_error(quote! {
                        Err::<(), _>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing args"))
                    }))
                }
            }
            BuiltinFn::JsonStringify => emit_json_stringify(self, args),
            BuiltinFn::CollectionConstructor(CollectionTypeId::Set) => {
                if args.len() > 1 {
                    return Err(EmitError::InternalInvariant(format!(
                        "Set collection constructor reached emission with {} arguments",
                        args.len()
                    )));
                }
                let Some(arg) = args.first() else {
                    return Ok(quote! { std::collections::HashSet::new() });
                };
                let values = self.emit_expr_for_use(
                    arg,
                    ValueUseSite::IncanCallArg {
                        target_ty: Some(&arg.ty),
                        callee_param: None,
                        in_return: false,
                    },
                )?;
                match arg
                    .ty
                    .set_constructor_source()
                    .map(|(_, iteration)| iteration)
                    .unwrap_or(SetConstructorIteration::IntoOwnedItems)
                {
                    SetConstructorIteration::CloneBorrowedItems => Ok(quote! {
                        (#values).iter().cloned().collect::<std::collections::HashSet<_>>()
                    }),
                    SetConstructorIteration::IntoOwnedItems => Ok(quote! {
                        (#values).into_iter().collect::<std::collections::HashSet<_>>()
                    }),
                }
            }
            BuiltinFn::CollectionConstructor(collection) => Err(EmitError::InternalInvariant(format!(
                "collection constructor `{}` reached emission without a lowering implementation",
                collections::as_str(*collection)
            ))),
            BuiltinFn::ListRepeat => {
                if args.len() >= 2 {
                    let value = self.emit_expr_for_use(
                        &args[0],
                        ValueUseSite::CollectionElement {
                            target_ty: Some(&args[0].ty),
                        },
                    )?;
                    let count = self.emit_expr(&args[1])?;
                    Ok(quote! { incan_stdlib::collections::list_repeat(#value, (#count) as i64) })
                } else {
                    Ok(quote! { incan_stdlib::collections::list_repeat((), 0i64) })
                }
            }
        }
    }

    /// Try to emit a builtin function call (legacy string-based dispatch).
    ///
    /// This is a fallback for `IrExprKind::Call` expressions where the function name
    /// matches a known builtin. Prefer using `emit_builtin_call` with enum dispatch.
    pub(in super::super) fn try_emit_builtin_call(
        &self,
        name: &str,
        args: &[TypedExpr],
    ) -> Result<Option<TokenStream>, EmitError> {
        let Some(id) = builtins::from_str(name) else {
            return Ok(None);
        };

        match id {
            BuiltinFnId::IsInstance => Ok(None),
            BuiltinFnId::Print => self.emit_print_call(args).map(Some),
            BuiltinFnId::Len => {
                if let Some(arg) = args.first() {
                    emit_len(self, arg).map(Some)
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Sum => {
                if let Some(arg) = args.first() {
                    emit_sum(self, arg).map(Some)
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Min => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let tokens = match elem_type {
                        IrType::Float => quote! { incan_stdlib::collections::__private::list_min_f64(&#a) },
                        IrType::String | IrType::FrozenStr => {
                            quote! { incan_stdlib::collections::__private::list_min_clone(&#a) }
                        }
                        _ => quote! { incan_stdlib::collections::__private::list_min_copy(&#a) },
                    };
                    Ok(Some(tokens))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Max => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let tokens = match elem_type {
                        IrType::Float => quote! { incan_stdlib::collections::__private::list_max_f64(&#a) },
                        IrType::String | IrType::FrozenStr => {
                            quote! { incan_stdlib::collections::__private::list_max_clone(&#a) }
                        }
                        _ => quote! { incan_stdlib::collections::__private::list_max_copy(&#a) },
                    };
                    Ok(Some(tokens))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Str => {
                if let Some(arg) = args.first() {
                    let a = exact_float_value_validation(&arg.ty).apply(self.emit_expr(arg)?);
                    Ok(Some(quote! { #a.to_string() }))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Int => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    match &arg.ty {
                        IrType::String | IrType::FrozenStr => {
                            Ok(Some(quote! { incan_stdlib::conversions::int_from_str(&#a) }))
                        }
                        IrType::Float => Ok(Some(quote! { (#a) as i64 })),
                        IrType::Bool => Ok(Some(quote! { if #a { 1 } else { 0 } })),
                        _ => Ok(Some(quote! { (#a) as i64 })),
                    }
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Float => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    match &arg.ty {
                        IrType::String | IrType::FrozenStr => {
                            Ok(Some(quote! { incan_stdlib::conversions::float_from_str(&#a) }))
                        }
                        IrType::Int => Ok(Some(quote! { (#a) as f64 })),
                        _ => Ok(Some(quote! { (#a) as f64 })),
                    }
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Bool => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let tokens = match &arg.ty {
                        IrType::Bool => quote! { #a },
                        IrType::Int => quote! { (#a) != 0 },
                        IrType::Float => quote! { (#a) != 0.0 },
                        IrType::String | IrType::FrozenStr => quote! { !(#a).is_empty() },
                        IrType::FrozenBytes => quote! { !(#a).is_empty() },
                        IrType::List(_) | IrType::Dict(_, _) | IrType::Set(_) => quote! { !(#a).is_empty() },
                        IrType::Option(_) => quote! { (#a).is_some() },
                        IrType::Result(_, _) => quote! { (#a).is_ok() },
                        _ if is_frozen_collection_named_generic(&arg.ty) => quote! { !(#a).is_empty() },
                        _ => quote! { true },
                    };
                    Ok(Some(tokens))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Abs => {
                if let Some(arg) = args.first() {
                    emit_abs(self, arg).map(Some)
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Range => self.emit_range_call(args),
            BuiltinFnId::Enumerate => {
                if let Some(arg) = args.first() {
                    self.emit_enumerate_value(arg).map(Some)
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Zip => emit_zip(self, args),
            BuiltinFnId::Sorted => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let from_frozen_list = is_named_generic(&arg.ty, collections::as_str(CollectionTypeId::FrozenList));
                    let tokens = if from_frozen_list {
                        match elem_type {
                            IrType::Float => quote! {{
                                let mut __v = (#a).as_slice().to_vec();
                                __v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                __v
                            }},
                            _ => quote! {{
                                let mut __v = (#a).as_slice().to_vec();
                                __v.sort();
                                __v
                            }},
                        }
                    } else {
                        match elem_type {
                            IrType::Float => quote! {{
                                let mut __v = (#a).clone();
                                __v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                __v
                            }},
                            _ => quote! {{
                                let mut __v = (#a).clone();
                                __v.sort();
                                __v
                            }},
                        }
                    };
                    Ok(Some(tokens))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::ReadFile => {
                if let Some(arg) = args.first() {
                    let path = self.emit_expr(arg)?;
                    Ok(Some(stringify_file_io_error(quote! { std::fs::read_to_string(#path) })))
                } else {
                    Ok(Some(stringify_file_io_error(quote! {
                        Err::<String, _>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no path"))
                    })))
                }
            }
            BuiltinFnId::WriteFile => {
                if args.len() >= 2 {
                    let path = self.emit_expr(&args[0])?;
                    let content = self.emit_expr(&args[1])?;
                    Ok(Some(stringify_file_io_error(
                        quote! { std::fs::write(#path, #content).map(|_| ()) },
                    )))
                } else {
                    Ok(Some(stringify_file_io_error(quote! {
                        Err::<(), _>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing args"))
                    })))
                }
            }
            BuiltinFnId::JsonStringify => emit_json_stringify(self, args).map(Some),
        }
    }

    /// Emit a range() function call.
    pub(in super::super) fn emit_range_call(&self, args: &[TypedExpr]) -> Result<Option<TokenStream>, EmitError> {
        if args.len() == 1 {
            if let IrExprKind::Range { start, end, inclusive } = &args[0].kind {
                match (start, end, inclusive) {
                    (Some(s), Some(e), false) => {
                        let ss = self.emit_expr(s)?;
                        let ee = self.emit_expr(e)?;
                        return Ok(Some(quote! { (#ss as i64)..(#ee as i64) }));
                    }
                    (Some(s), Some(e), true) => {
                        let ss = self.emit_expr(s)?;
                        let ee = self.emit_expr(e)?;
                        // Inclusive ranges are not a Python `range` feature; interpret as Rust-like convenience.
                        return Ok(Some(quote! { (#ss as i64)..=(#ee as i64) }));
                    }
                    (None, Some(e), _) => {
                        let ee = self.emit_expr(e)?;
                        if *inclusive {
                            return Ok(Some(quote! { 0_i64..=(#ee as i64) }));
                        }
                        return Ok(Some(quote! { 0_i64..(#ee as i64) }));
                    }
                    _ => {}
                }
            } else {
                let end = self.emit_expr(&args[0])?;
                return Ok(Some(quote! { 0_i64..(#end as i64) }));
            }
        }
        match args.len() {
            2 => {
                let start = self.emit_expr(&args[0])?;
                let end = self.emit_expr(&args[1])?;
                Ok(Some(quote! { (#start as i64)..(#end as i64) }))
            }
            3 => {
                let start = self.emit_expr(&args[0])?;
                let end = self.emit_expr(&args[1])?;
                if matches!(&args[2].kind, IrExprKind::Int(1)) {
                    return Ok(Some(quote! { (#start as i64)..(#end as i64) }));
                }
                let step = self.emit_expr(&args[2])?;
                Ok(Some(quote! { incan_stdlib::iter::range(#start, #end, (#step) as i64) }))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::FunctionRegistry;
    use crate::backend::ir::expr::{VarAccess, VarRefKind};

    #[test]
    fn legacy_file_result_stringifies_rust_io_errors_issue874() {
        let emitted = stringify_file_io_error(quote! { std::fs::read_to_string(path) }).to_string();
        assert!(emitted.contains("map_err"), "missing error conversion: {emitted}");
        assert!(
            emitted.contains("error . to_string"),
            "missing owned string conversion: {emitted}"
        );
    }

    #[test]
    fn enumerate_copy_policy_keeps_generic_tuple_items_owned() {
        assert!(enumerate_elem_can_copy(&IrType::Int));
        assert!(!enumerate_elem_can_copy(&IrType::Tuple(vec![
            IrType::Generic("T".to_string()),
            IrType::Int,
        ])));
        assert!(!enumerate_elem_can_copy(&IrType::Option(Box::new(IrType::Int))));
    }

    #[test]
    fn checked_isinstance_matches_string_storage_in_direct_union_and_option_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let frozen_union = IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), vec![IrType::FrozenStr, IrType::Int]);
        let mixed_string_union = IrType::NamedGeneric(
            IR_UNION_TYPE_NAME.to_string(),
            vec![IrType::FrozenStr, IrType::String, IrType::Int],
        );
        let cases = [
            ("direct_frozen", IrType::FrozenStr, false, 0),
            ("direct_static", IrType::StaticStr, false, 0),
            ("frozen_union", frozen_union.clone(), true, 1),
            ("optional_frozen", IrType::Option(Box::new(IrType::FrozenStr)), true, 0),
            ("optional_frozen_union", IrType::Option(Box::new(frozen_union)), true, 1),
            ("mixed_string_union", mixed_string_union.clone(), true, 2),
            (
                "optional_mixed_string_union",
                IrType::Option(Box::new(mixed_string_union)),
                true,
                2,
            ),
        ];

        for (name, value_ty, expects_pattern, expected_union_variants) in cases {
            let value = TypedExpr::new(
                IrExprKind::Var {
                    name: name.to_string(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                value_ty,
            );
            let target = TypedExpr::new(
                IrExprKind::TypeToken { ty: IrType::String },
                IrType::TypeToken(Box::new(IrType::String)),
            );
            let rendered = emitter
                .emit_builtin_call(&BuiltinFn::IsInstance, &[value, target])?
                .to_string()
                .split_whitespace()
                .collect::<String>();
            assert!(
                !rendered.contains("false"),
                "{name} disagreed with source str identity: {rendered}"
            );
            assert_eq!(
                rendered.contains("matches!"),
                expects_pattern,
                "unexpected {name} shape: {rendered}"
            );
            assert_eq!(
                rendered.matches("::V").count(),
                expected_union_variants,
                "{name} omitted or invented a semantically matching union variant: {rendered}"
            );
        }
        Ok(())
    }

    /// The explicit canonical and retained legacy identities both emit a value matching `list[tuple[int, T]]`.
    #[test]
    fn canonical_and_legacy_enumerate_values_materialize_checked_lists() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let values = TypedExpr::new(
            IrExprKind::Var {
                name: "values".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );

        let canonical = emitter
            .emit_builtin_call(&BuiltinFn::Enumerate, std::slice::from_ref(&values))?
            .to_string()
            .split_whitespace()
            .collect::<String>();
        assert!(
            canonical.contains("collect::<Vec<_>>()"),
            "explicit BuiltinFn::Enumerate must emit a materialized list value: {canonical}"
        );

        assert_eq!(builtins::from_str("enumerate"), Some(BuiltinFnId::Enumerate));
        let legacy = emitter
            .try_emit_builtin_call("enumerate", &[values])?
            .ok_or_else(|| std::io::Error::other("registered legacy enumerate identity must emit"))?
            .to_string()
            .split_whitespace()
            .collect::<String>();
        assert!(
            legacy.contains("collect::<Vec<_>>()"),
            "legacy BuiltinFnId::Enumerate must emit a materialized list value: {legacy}"
        );
        Ok(())
    }

    /// Both retained builtin dispatch paths must emit explicit checks, never profile-sensitive Rust arithmetic.
    #[test]
    fn canonical_and_legacy_abs_sum_emit_one_checked_contract() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let integer = TypedExpr::new(
            IrExprKind::Var {
                name: "integer".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Int,
        );
        let integers = TypedExpr::new(
            IrExprKind::Var {
                name: "integers".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );

        for emitted in [
            emitter.emit_builtin_call(&BuiltinFn::Abs, std::slice::from_ref(&integer))?,
            emitter
                .try_emit_builtin_call("abs", std::slice::from_ref(&integer))?
                .ok_or("legacy abs identity must emit")?,
        ] {
            let rendered = emitted.to_string().split_whitespace().collect::<String>();
            assert!(rendered.contains("checked_abs()"), "{rendered}");
            assert!(rendered.contains("raise_value_error"), "{rendered}");
            assert!(!rendered.contains("wrapping_abs"), "{rendered}");
        }

        for emitted in [
            emitter.emit_builtin_call(&BuiltinFn::Sum, std::slice::from_ref(&integers))?,
            emitter
                .try_emit_builtin_call("sum", std::slice::from_ref(&integers))?
                .ok_or("legacy sum identity must emit")?,
        ] {
            let rendered = emitted.to_string().split_whitespace().collect::<String>();
            assert!(rendered.contains("try_fold(0_i64"), "{rendered}");
            assert!(rendered.contains("checked_add"), "{rendered}");
            assert!(rendered.contains("raise_value_error"), "{rendered}");
            assert!(!rendered.contains("wrapping_add"), "{rendered}");
        }
        Ok(())
    }
}
