//! Builtin, surface-function, and stdlib-module call dispatch.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Span, Spanned, Type};
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{FunctionInfo, ResolvedType};
use crate::frontend::typechecker::helpers::{collection_type_id, dict_ty, list_ty, option_ty, result_ty, set_ty};
use incan_core::lang::builtins::{self as core_builtins, BuiltinFnId};
use incan_core::lang::surface::constructors::{self as surface_constructors, ConstructorId};
use incan_core::lang::surface::functions::{self as surface_functions, SurfaceFnId};
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_core::lang::types::collections::CollectionTypeId;

impl TypeChecker {
    fn validate_stdlib_module_call_arity(
        &mut self,
        callable: &str,
        expected: usize,
        args: &[CallArg],
        span: Span,
    ) -> bool {
        if args.len() != expected {
            self.errors
                .push(errors::builtin_arity(callable, expected, args.len(), span));
            return false;
        }
        true
    }

    /// Type-check a stdlib module function call with an explicit arity gate.
    ///
    /// This always delegates to [`Self::validate_function_call`] so type-related diagnostics are still emitted, but if
    /// arity validation fails the returned type is forced to [`ResolvedType::Unknown`] to avoid propagating a
    /// misleading inferred result.
    pub(in crate::frontend::typechecker::check_expr) fn validate_stdlib_module_function_call(
        &mut self,
        callable: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        let arity_ok = self.validate_stdlib_module_call_arity(callable, info.params.len(), args, call_span);
        let resolved = self.validate_function_call(callable, info, explicit_type_args, args, call_span);
        if arity_ok { resolved } else { ResolvedType::Unknown }
    }

    // ---- Rust boundary matching and coercion recording ----

    /// Determine whether `arg_ty` can flow into `target_ty` via `rusttype` boundary rules.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_builtin_call(
        &mut self,
        name: &str,
        args: &[CallArg],
        call_span: Span,
    ) -> Option<ResolvedType> {
        let has_function_symbol = self.has_non_builtin_function_definition(name);

        // Constructors (variant-like)
        if let Some(cid) = surface_constructors::from_str(name) {
            return match cid {
                ConstructorId::Ok | ConstructorId::Err => {
                    let arg_types = self.check_call_arg_types(args);
                    let current_result = self.symbols.current_return_type().and_then(|ty| match ty {
                        ResolvedType::Generic(name, args)
                            if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
                                && args.len() >= 2 =>
                        {
                            Some((args[0].clone(), args[1].clone()))
                        }
                        _ => None,
                    });
                    let current_ok = current_result
                        .as_ref()
                        .map(|(ok_ty, _)| ok_ty.clone())
                        .unwrap_or(ResolvedType::Unknown);
                    let current_err = current_result
                        .as_ref()
                        .map(|(_, err_ty)| err_ty.clone())
                        .or_else(|| self.current_return_error_type.clone())
                        .unwrap_or(ResolvedType::Unknown);
                    let inferred_arg = arg_types.first().cloned().unwrap_or(ResolvedType::Unknown);

                    let (ok_ty, err_ty) = if cid == ConstructorId::Ok {
                        // `Ok(...)` must reflect the payload type so return checking can catch mismatches against the
                        // declared `Result[T, E]`.
                        let ok_ty = if current_ok == ResolvedType::Unit
                            && matches!(
                                inferred_arg,
                                ResolvedType::Generic(ref name, ref args)
                                    if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option)
                                        && args.len() == 1
                                        && matches!(args[0], ResolvedType::Unknown)
                            ) {
                            ResolvedType::Unit
                        } else {
                            inferred_arg
                        };
                        (ok_ty, current_err)
                    } else {
                        // `Err(...)` mirrors the actual error payload while preserving any known enclosing `Ok` type.
                        (current_ok, inferred_arg)
                    };
                    Some(result_ty(ok_ty, err_ty))
                }
                ConstructorId::Some => {
                    let arg_types = self.check_call_arg_types(args);
                    let inner = arg_types.first().cloned().unwrap_or(ResolvedType::Unknown);
                    Some(option_ty(inner))
                }
                ConstructorId::None => Some(option_ty(ResolvedType::Unknown)),
            };
        }

        // Core builtin functions (registry-driven)
        if let Some(bid) = core_builtins::from_str(name) {
            if has_function_symbol {
                return None;
            }
            return match bid {
                BuiltinFnId::Print => {
                    self.check_call_args(args);
                    Some(ResolvedType::Unit)
                }
                BuiltinFnId::Len => {
                    self.check_call_args(args);
                    Some(ResolvedType::Int)
                }
                BuiltinFnId::Sum => {
                    self.check_call_args(args);
                    Some(ResolvedType::Int)
                }
                BuiltinFnId::Min | BuiltinFnId::Max => {
                    if args.len() != 1 {
                        self.errors.push(errors::builtin_arity(name, 1, args.len(), call_span));
                        self.check_call_args(args);
                        return Some(ResolvedType::Unknown);
                    }
                    let arg_expr = Self::call_arg_expr(&args[0]);
                    let arg_ty = self.check_expr(arg_expr);

                    // Only support list-like collections for now.
                    let inner = if let ResolvedType::Generic(n, type_args) = &arg_ty {
                        if matches!(
                            collection_type_id(n.as_str()),
                            Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                        ) {
                            type_args.first().cloned().unwrap_or(ResolvedType::Unknown)
                        } else {
                            ResolvedType::Unknown
                        }
                    } else if let ResolvedType::FrozenList(t) = &arg_ty {
                        (**t).clone()
                    } else {
                        ResolvedType::Unknown
                    };

                    if matches!(inner, ResolvedType::Unknown) {
                        self.errors
                            .push(errors::builtin_expects_list(name, &arg_ty.to_string(), call_span));
                        return Some(ResolvedType::Unknown);
                    }

                    // Require comparable scalar element types (keep narrow for now).
                    match inner {
                        ResolvedType::Int
                        | ResolvedType::Float
                        | ResolvedType::Bool
                        | ResolvedType::Str
                        | ResolvedType::FrozenStr => Some(inner),
                        other => {
                            self.errors.push(errors::builtin_list_element_type_not_supported(
                                name,
                                &other.to_string(),
                                call_span,
                            ));
                            Some(ResolvedType::Unknown)
                        }
                    }
                }
                BuiltinFnId::Str => {
                    self.check_call_args(args);
                    Some(ResolvedType::Str)
                }
                BuiltinFnId::Int => {
                    self.check_call_args(args);
                    Some(ResolvedType::Int)
                }
                BuiltinFnId::Float => {
                    self.check_call_args(args);
                    Some(ResolvedType::Float)
                }
                BuiltinFnId::Bool => {
                    if args.len() != 1 {
                        self.errors.push(errors::builtin_arity(name, 1, args.len(), call_span));
                        self.check_call_args(args);
                        return Some(ResolvedType::Bool);
                    }
                    let arg_expr = Self::call_arg_expr(&args[0]);
                    let arg_ty = self.check_expr(arg_expr);

                    let ok = matches!(
                        arg_ty,
                        ResolvedType::Bool
                            | ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Str
                            | ResolvedType::FrozenStr
                            | ResolvedType::Bytes
                            | ResolvedType::FrozenBytes
                            | ResolvedType::Unknown
                    ) || matches!(
                        &arg_ty,
                        ResolvedType::Generic(n, _)
                            if matches!(
                                collection_type_id(n.as_str()),
                                Some(
                                    CollectionTypeId::List
                                        | CollectionTypeId::FrozenList
                                        | CollectionTypeId::Dict
                                        | CollectionTypeId::FrozenDict
                                        | CollectionTypeId::Set
                                        | CollectionTypeId::FrozenSet
                                        | CollectionTypeId::Tuple
                                        | CollectionTypeId::Option
                                        | CollectionTypeId::Result
                                )
                            )
                    ) || matches!(
                        arg_ty,
                        ResolvedType::FrozenList(_) | ResolvedType::FrozenDict(_, _) | ResolvedType::FrozenSet(_)
                    );

                    if !ok {
                        self.errors
                            .push(errors::builtin_bool_type_not_supported(&arg_ty.to_string(), call_span));
                    }
                    Some(ResolvedType::Bool)
                }
                BuiltinFnId::Abs => {
                    self.check_call_args(args);
                    Some(ResolvedType::Int)
                }
                BuiltinFnId::Range => {
                    self.check_call_args(args);
                    Some(list_ty(ResolvedType::Int))
                }
                BuiltinFnId::Enumerate => {
                    // enumerate(xs) -> List[(int, T)] (simple)
                    let mut inner_ty = ResolvedType::Unknown;
                    if let Some(arg) = args.first() {
                        let iter_ty = self.check_expr(Self::call_arg_expr(arg));
                        if let ResolvedType::Generic(name, type_args) = &iter_ty
                            && (name == surface_types::as_str(SurfaceTypeId::Vec)
                                || matches!(
                                    collection_type_id(name.as_str()),
                                    Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                                ))
                            && !type_args.is_empty()
                        {
                            inner_ty = type_args[0].clone();
                        }
                    }
                    self.check_call_args(args);
                    Some(list_ty(ResolvedType::Tuple(vec![ResolvedType::Int, inner_ty])))
                }
                BuiltinFnId::Zip => {
                    // zip(a, b) -> List[(T1, T2)] (simple)
                    let mut ty1 = ResolvedType::Unknown;
                    let mut ty2 = ResolvedType::Unknown;
                    if args.len() >= 2 {
                        let iter1_ty = self.check_expr(Self::call_arg_expr(&args[0]));
                        let iter2_ty = self.check_expr(Self::call_arg_expr(&args[1]));
                        if let ResolvedType::Generic(name, type_args) = &iter1_ty
                            && (name == surface_types::as_str(SurfaceTypeId::Vec)
                                || matches!(
                                    collection_type_id(name.as_str()),
                                    Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                                ))
                            && !type_args.is_empty()
                        {
                            ty1 = type_args[0].clone();
                        }
                        if let ResolvedType::Generic(name, type_args) = &iter2_ty
                            && (name == surface_types::as_str(SurfaceTypeId::Vec)
                                || matches!(
                                    collection_type_id(name.as_str()),
                                    Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                                ))
                            && !type_args.is_empty()
                        {
                            ty2 = type_args[0].clone();
                        }
                    }
                    self.check_call_args(args);
                    Some(list_ty(ResolvedType::Tuple(vec![ty1, ty2])))
                }
                BuiltinFnId::Sorted => {
                    if args.len() != 1 {
                        self.errors.push(errors::builtin_arity(name, 1, args.len(), call_span));
                        self.check_call_args(args);
                        return Some(ResolvedType::Unknown);
                    }
                    let arg_expr = Self::call_arg_expr(&args[0]);
                    let arg_ty = self.check_expr(arg_expr);

                    let inner = if let ResolvedType::Generic(n, type_args) = &arg_ty {
                        if matches!(
                            collection_type_id(n.as_str()),
                            Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                        ) {
                            type_args.first().cloned().unwrap_or(ResolvedType::Unknown)
                        } else {
                            ResolvedType::Unknown
                        }
                    } else if let ResolvedType::FrozenList(t) = &arg_ty {
                        (**t).clone()
                    } else {
                        ResolvedType::Unknown
                    };

                    if matches!(inner, ResolvedType::Unknown) {
                        self.errors
                            .push(errors::builtin_expects_list(name, &arg_ty.to_string(), call_span));
                        return Some(ResolvedType::Unknown);
                    }

                    match inner {
                        ResolvedType::Int
                        | ResolvedType::Float
                        | ResolvedType::Bool
                        | ResolvedType::Str
                        | ResolvedType::FrozenStr => Some(list_ty(inner)),
                        other => {
                            self.errors.push(errors::builtin_list_element_type_not_supported(
                                name,
                                &other.to_string(),
                                call_span,
                            ));
                            Some(ResolvedType::Unknown)
                        }
                    }
                }
                BuiltinFnId::ReadFile => {
                    self.check_call_args(args);
                    Some(result_ty(ResolvedType::Str, ResolvedType::Str))
                }
                BuiltinFnId::WriteFile => {
                    self.check_call_args(args);
                    Some(result_ty(ResolvedType::Unit, ResolvedType::Str))
                }
                BuiltinFnId::JsonStringify => {
                    self.check_call_args(args);
                    Some(ResolvedType::Str)
                }
            };
        }

        // Surface/runtime functions (registry-driven)
        if let Some(fid) = surface_functions::from_str(name) {
            if !has_function_symbol {
                return None;
            }
            return match fid {
                SurfaceFnId::SleepMs => {
                    if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        if !self.types_compatible(&arg_ty, &ResolvedType::Int) {
                            self.errors
                                .push(errors::type_mismatch("int", &arg_ty.to_string(), arg_expr.span));
                        }
                    }
                    Some(ResolvedType::Unit)
                }
                SurfaceFnId::Timeout | SurfaceFnId::TimeoutMs | SurfaceFnId::SelectTimeout => {
                    if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        let (expected_name, expected_ty) = if fid == SurfaceFnId::Timeout {
                            ("float", ResolvedType::Float)
                        } else {
                            ("int", ResolvedType::Int)
                        };
                        if !self.types_compatible(&arg_ty, &expected_ty) {
                            self.errors
                                .push(errors::type_mismatch(expected_name, &arg_ty.to_string(), arg_expr.span));
                        }
                    }
                    self.check_call_args(args);
                    Some(ResolvedType::Unknown)
                }
                SurfaceFnId::YieldNow => Some(ResolvedType::Unit),
                SurfaceFnId::Spawn | SurfaceFnId::SpawnBlocking => {
                    self.check_call_args(args);
                    Some(ResolvedType::Generic(
                        surface_types::as_str(SurfaceTypeId::JoinHandle).to_string(),
                        vec![ResolvedType::Unknown],
                    ))
                }
                SurfaceFnId::Channel => {
                    self.check_call_args(args);
                    let inner = ResolvedType::Unknown;
                    Some(ResolvedType::Tuple(vec![
                        ResolvedType::Generic(
                            surface_types::as_str(SurfaceTypeId::Sender).to_string(),
                            vec![inner.clone()],
                        ),
                        ResolvedType::Generic(surface_types::as_str(SurfaceTypeId::Receiver).to_string(), vec![inner]),
                    ]))
                }
                SurfaceFnId::UnboundedChannel => {
                    self.check_call_args(args);
                    Some(ResolvedType::Tuple(vec![
                        ResolvedType::Generic(
                            surface_types::as_str(SurfaceTypeId::Sender).to_string(),
                            vec![ResolvedType::Unknown],
                        ),
                        ResolvedType::Generic(
                            surface_types::as_str(SurfaceTypeId::Receiver).to_string(),
                            vec![ResolvedType::Unknown],
                        ),
                    ]))
                }
                SurfaceFnId::Oneshot => {
                    self.check_call_args(args);
                    Some(ResolvedType::Tuple(vec![
                        ResolvedType::Generic(
                            surface_types::as_str(SurfaceTypeId::OneshotSender).to_string(),
                            vec![ResolvedType::Unknown],
                        ),
                        ResolvedType::Generic(
                            surface_types::as_str(SurfaceTypeId::OneshotReceiver).to_string(),
                            vec![ResolvedType::Unknown],
                        ),
                    ]))
                }
            };
        }

        // Surface types that behave like constructors and whose result type depends on args.
        if let Some(tid) = surface_types::from_str(name) {
            return match tid {
                SurfaceTypeId::Json | SurfaceTypeId::Query => {
                    Some(self.check_json_query_constructor_call(tid, args, call_span))
                }
                SurfaceTypeId::Mutex => {
                    let inner = if let Some(arg) = args.first() {
                        self.check_expr(Self::call_arg_expr(arg))
                    } else {
                        ResolvedType::Unknown
                    };
                    Some(ResolvedType::Generic(
                        surface_types::as_str(SurfaceTypeId::Mutex).to_string(),
                        vec![inner],
                    ))
                }
                SurfaceTypeId::RwLock => {
                    let inner = if let Some(arg) = args.first() {
                        self.check_expr(Self::call_arg_expr(arg))
                    } else {
                        ResolvedType::Unknown
                    };
                    Some(ResolvedType::Generic(
                        surface_types::as_str(SurfaceTypeId::RwLock).to_string(),
                        vec![inner],
                    ))
                }
                SurfaceTypeId::Semaphore => {
                    self.check_call_args(args);
                    Some(ResolvedType::Named(
                        surface_types::as_str(SurfaceTypeId::Semaphore).to_string(),
                    ))
                }
                SurfaceTypeId::Barrier => {
                    self.check_call_args(args);
                    Some(ResolvedType::Named(
                        surface_types::as_str(SurfaceTypeId::Barrier).to_string(),
                    ))
                }
                _ => None,
            };
        }

        // Python-like type conversion helpers (surface). These are not part of `lang::builtins`.
        if let Some(cid) = collection_type_id(name) {
            return match cid {
                CollectionTypeId::Dict => {
                    let (key_ty, val_ty) = if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        match &arg_ty {
                            ResolvedType::Generic(name, type_args)
                                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict)
                                    && type_args.len() >= 2 =>
                            {
                                (type_args[0].clone(), type_args[1].clone())
                            }
                            _ => (ResolvedType::Unknown, ResolvedType::Unknown),
                        }
                    } else {
                        (ResolvedType::Unknown, ResolvedType::Unknown)
                    };
                    Some(dict_ty(key_ty, val_ty))
                }
                CollectionTypeId::List => {
                    let elem_ty = if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        match &arg_ty {
                            ResolvedType::Generic(name, type_args)
                                if (name == surface_types::as_str(SurfaceTypeId::Vec)
                                    || matches!(
                                        collection_type_id(name.as_str()),
                                        Some(
                                            CollectionTypeId::List
                                                | CollectionTypeId::Set
                                                | CollectionTypeId::FrozenList
                                                | CollectionTypeId::FrozenSet
                                        )
                                    ))
                                    && !type_args.is_empty() =>
                            {
                                type_args[0].clone()
                            }
                            ResolvedType::Str => ResolvedType::Str,
                            _ => ResolvedType::Unknown,
                        }
                    } else {
                        ResolvedType::Unknown
                    };
                    Some(list_ty(elem_ty))
                }
                CollectionTypeId::Set => {
                    let elem_ty = if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        match &arg_ty {
                            ResolvedType::Generic(name, type_args)
                                if (name == surface_types::as_str(SurfaceTypeId::Vec)
                                    || matches!(
                                        collection_type_id(name.as_str()),
                                        Some(
                                            CollectionTypeId::List
                                                | CollectionTypeId::Set
                                                | CollectionTypeId::FrozenList
                                                | CollectionTypeId::FrozenSet
                                        )
                                    ))
                                    && !type_args.is_empty() =>
                            {
                                type_args[0].clone()
                            }
                            _ => ResolvedType::Unknown,
                        }
                    } else {
                        ResolvedType::Unknown
                    };
                    Some(set_ty(elem_ty))
                }
                _ => None,
            };
        }

        None
    }
}
