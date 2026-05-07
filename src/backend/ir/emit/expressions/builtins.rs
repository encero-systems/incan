//! Emit Rust code for built-in function calls.
//!
//! This module handles emission of known built-in functions using enum-based dispatch
//! (`BuiltinFn`). It also contains the legacy string-based fallback for `Call` expressions
//! that haven't been lowered to `BuiltinCall`.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::expr::{BuiltinFn, IrExprKind, TypedExpr};
use super::super::super::ownership::ValueUseSite;
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use incan_core::lang::builtins::{self, BuiltinFnId};
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

impl<'a> IrEmitter<'a> {
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
            BuiltinFn::Print => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(quote! { println!("{}", #a) })
                } else {
                    Ok(quote! { println!() })
                }
            }
            BuiltinFn::Len => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(quote! { ::std::convert::identity(#a.len() as i64) })
                } else {
                    Ok(quote! { 0i64 })
                }
            }
            BuiltinFn::Sum => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);
                    let sum_tokens = if matches!(elem_type, IrType::Bool) {
                        quote! { #a.iter().map(|v| if *v { 1i64 } else { 0i64 }).sum::<i64>() }
                    } else {
                        quote! { #a.iter().sum::<i64>() }
                    };
                    Ok(sum_tokens)
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
                    let a = self.emit_expr(arg)?;
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
                    let a = self.emit_expr(arg)?;
                    Ok(quote! { #a.abs() })
                } else {
                    Ok(quote! { 0 })
                }
            }
            BuiltinFn::Range => self
                .emit_range_call(args)
                .map(|opt| opt.unwrap_or_else(|| quote! { 0..0 })),
            BuiltinFn::Enumerate => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(quote! { #a.iter().enumerate().map(|(idx, value)| (idx as i64, value)) })
                } else {
                    Ok(quote! { std::iter::empty::<(i64, ())>() })
                }
            }
            BuiltinFn::Zip => {
                if args.len() >= 2 {
                    let a = self.emit_expr(&args[0])?;
                    let b = self.emit_expr(&args[1])?;
                    Ok(quote! { #a.iter().zip(#b.iter()) })
                } else {
                    Ok(quote! { std::iter::empty::<((), ())>() })
                }
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
                    Ok(quote! { std::fs::read_to_string(#path) })
                } else {
                    Ok(quote! { Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no path")) })
                }
            }
            BuiltinFn::WriteFile => {
                if args.len() >= 2 {
                    let path = self.emit_expr(&args[0])?;
                    let content = self.emit_expr(&args[1])?;
                    Ok(quote! { std::fs::write(#path, #content).map(|_| ()) })
                } else {
                    Ok(quote! { Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing args")) })
                }
            }
            BuiltinFn::JsonStringify => {
                if let Some(arg) = args.first() {
                    let value = self.emit_expr(arg)?;
                    Ok(quote! {
                        incan_stdlib::json::__private::stringify_or_raise(&#value, std::any::type_name_of_val(&#value))
                    })
                } else {
                    Ok(quote! { String::from("null") })
                }
            }
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
            BuiltinFnId::Print => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(Some(quote! { println!("{}", #a) }))
                } else {
                    Ok(Some(quote! { println!() }))
                }
            }
            BuiltinFnId::Len => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(Some(quote! { ::std::convert::identity(#a.len() as i64) }))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Sum => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    let elem_type = list_elem_type(&arg.ty);

                    let sum_tokens = if matches!(elem_type, IrType::Bool) {
                        quote! { #a.iter().map(|v| if *v { 1i64 } else { 0i64 }).sum::<i64>() }
                    } else {
                        quote! { #a.iter().sum::<i64>() }
                    };
                    Ok(Some(sum_tokens))
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
                    let a = self.emit_expr(arg)?;
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
                    let a = self.emit_expr(arg)?;
                    Ok(Some(quote! { #a.abs() }))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Range => self.emit_range_call(args),
            BuiltinFnId::Enumerate => {
                if let Some(arg) = args.first() {
                    let a = self.emit_expr(arg)?;
                    Ok(Some(
                        quote! { #a.iter().enumerate().map(|(idx, value)| (idx as i64, value)) },
                    ))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::Zip => {
                if args.len() >= 2 {
                    let a = self.emit_expr(&args[0])?;
                    let b = self.emit_expr(&args[1])?;
                    Ok(Some(quote! { #a.iter().zip(#b.iter()) }))
                } else {
                    Ok(None)
                }
            }
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
                    Ok(Some(quote! { std::fs::read_to_string(#path) }))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::WriteFile => {
                if args.len() >= 2 {
                    let path = self.emit_expr(&args[0])?;
                    let content = self.emit_expr(&args[1])?;
                    Ok(Some(quote! { std::fs::write(#path, #content).map(|_| ()) }))
                } else {
                    Ok(None)
                }
            }
            BuiltinFnId::JsonStringify => {
                if let Some(arg) = args.first() {
                    let value = self.emit_expr(arg)?;
                    Ok(Some(quote! {
                        incan_stdlib::json::__private::stringify_or_raise(&#value, std::any::type_name_of_val(&#value))
                    }))
                } else {
                    Ok(None)
                }
            }
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
