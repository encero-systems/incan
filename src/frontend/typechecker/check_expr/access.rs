//! Check indexing, slicing, field access, and method calls.
//!
//! These helpers validate access patterns like `xs[i]`, `xs[a:b]`, `obj.field`, and `obj.method(...)`, emitting
//! diagnostics for missing fields/methods and incompatible uses.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::{
    collection_name, collection_type_id, is_frozen_bytes, is_frozen_str, is_intlike_for_index, list_ty, option_ty,
    string_method_return,
};
use incan_core::lang::conventions;
use incan_core::lang::magic_methods;
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::surface::types::{SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME, SEMAPHORE_PERMIT_TYPE_NAME, SurfaceTypeId};
use incan_core::lang::surface::{
    dict_methods, float_methods, frozen_bytes_methods, frozen_dict_methods, frozen_list_methods, frozen_set_methods,
    list_methods, set_methods,
};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::{enum_helpers, surface::option_methods};

use super::TypeChecker;

impl TypeChecker {
    /// Validate method call arguments against a method signature.
    fn validate_method_call_args(
        &mut self,
        params: &[(String, ResolvedType)],
        args: &[CallArg],
        arg_types: &[ResolvedType],
    ) {
        let mut positional: Vec<(ResolvedType, Span)> = Vec::new();
        let mut named: std::collections::HashMap<&str, (ResolvedType, Span)> = std::collections::HashMap::new();

        for (arg, ty) in args.iter().zip(arg_types.iter()) {
            let expr = match arg {
                CallArg::Positional(e) | CallArg::Named(_, e) => e,
            };
            match arg {
                CallArg::Positional(_) => positional.push((ty.clone(), expr.span)),
                CallArg::Named(name, _) => {
                    named.insert(name.as_str(), (ty.clone(), expr.span));
                }
            }
        }

        let mut pos_idx = 0usize;
        for (param_name, param_ty) in params {
            let arg = if let Some(value) = named.get(param_name.as_str()) {
                Some(value)
            } else if pos_idx < positional.len() {
                let value = positional.get(pos_idx);
                pos_idx += 1;
                value
            } else {
                None
            };

            if let Some((arg_ty, arg_span)) = arg
                && !self.types_compatible(arg_ty, param_ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param_ty.to_string(),
                    &arg_ty.to_string(),
                    *arg_span,
                ));
            }
        }
    }

    /// Check if a type is copyable.
    pub(in crate::frontend::typechecker) fn is_copy_type(&self, ty: &ResolvedType) -> bool {
        matches!(
            ty,
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Unit | ResolvedType::Ref(_)
        )
    }

    /// Check if a type is cloneable.
    pub(in crate::frontend::typechecker) fn is_clone_type(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => true,
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => self.is_clone_type(inner),
            ResolvedType::FrozenDict(k, v) => self.is_clone_type(k) && self.is_clone_type(v),
            ResolvedType::Tuple(items) => items.iter().all(|t| self.is_clone_type(t)),
            ResolvedType::Generic(name, args) => {
                if let Some(id) = surface_types::from_str(name.as_str()) {
                    return match id {
                        SurfaceTypeId::Vec => args.first().is_none_or(|t| self.is_clone_type(t)),
                        SurfaceTypeId::HashMap => {
                            let key_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                            let val_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                            key_ok && val_ok
                        }
                        _ => false,
                    };
                }
                match collection_type_id(name.as_str()) {
                    Some(CollectionTypeId::List) | Some(CollectionTypeId::Set) | Some(CollectionTypeId::Option) => {
                        args.first().is_none_or(|t| self.is_clone_type(t))
                    }
                    Some(CollectionTypeId::Dict) => {
                        let key_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                        let val_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                        key_ok && val_ok
                    }
                    Some(CollectionTypeId::Result) => {
                        let ok_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                        let err_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                        ok_ok && err_ok
                    }
                    _ => args.iter().all(|t| self.is_clone_type(t)),
                }
            }
            ResolvedType::Named(name) => {
                if let Some(id) = surface_types::from_str(name.as_str()) {
                    return matches!(id, SurfaceTypeId::Html);
                }
                matches!(
                    self.lookup_type_info(name),
                    Some(TypeInfo::Builtin)
                        | Some(TypeInfo::Class(_))
                        | Some(TypeInfo::Model(_))
                        | Some(TypeInfo::Newtype(_))
                        | Some(TypeInfo::Enum(_))
                )
            }
            ResolvedType::Ref(_) | ResolvedType::Function(_, _) | ResolvedType::SelfType => true,
            ResolvedType::TypeVar(_) => false,
            // RFC 041: provenance is known, but Incan does not yet query Rust for `Copy`/`Clone`; do not assume.
            ResolvedType::RustPath(_) => false,
            ResolvedType::Unknown => true,
        }
    }

    /// Resolve a method on a type's own methods or trait-adopted methods.
    fn resolve_named_method(
        &mut self,
        methods: &std::collections::HashMap<String, MethodInfo>,
        traits: Option<&[String]>,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
    ) -> Option<ResolvedType> {
        if let Some(method_info) = methods.get(method) {
            let params = method_info.params.clone();
            let return_type = method_info.return_type.clone();
            self.validate_method_call_args(&params, args, arg_types);
            return Some(return_type);
        }
        if let Some(traits) = traits {
            for trait_name in traits {
                if let Some(method_info) = self.trait_method_info_resolved(trait_name, method, call_site_span) {
                    let params = method_info.params.clone();
                    let return_type = method_info.return_type.clone();
                    self.validate_method_call_args(&params, args, arg_types);
                    return Some(return_type);
                }
            }
        }
        None
    }

    /// Normalize a tuple index (supports negative indices) and emit bounds errors.
    fn resolve_tuple_index(&mut self, raw_idx: i64, len: usize, span: Span) -> Option<usize> {
        let len_i = len as i64;
        let mut idx = raw_idx;
        if idx < 0 {
            idx += len_i;
        }
        if idx < 0 || idx >= len_i {
            self.errors.push(errors::tuple_index_out_of_bounds(raw_idx, len, span));
            return None;
        }
        Some(idx as usize)
    }

    /// Type-check an indexing expression (`base[index]`) and return the element type.
    pub(in crate::frontend::typechecker::check_expr) fn check_index(
        &mut self,
        base: &Spanned<Expr>,
        index: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let base_ty = self.check_expr(base);
        let index_ty = self.check_expr(index);

        match base_ty {
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) if !args.is_empty() => {
                    if !is_intlike_for_index(&index_ty) {
                        self.errors
                            .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
                    }
                    args[0].clone()
                }
                Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                    let key_ty = &args[0];
                    if !self.types_compatible(&index_ty, key_ty) {
                        self.errors.push(errors::index_type_mismatch(
                            &key_ty.to_string(),
                            &index_ty.to_string(),
                            index.span,
                        ));
                    }
                    args[1].clone()
                }
                Some(CollectionTypeId::Tuple) => {
                    // `Tuple[T1, ...]` (and `tuple[...]` normalized) behaves like a tuple.
                    let elems = args;
                    let Expr::Literal(Literal::Int(raw_idx)) = &index.node else {
                        self.errors.push(errors::tuple_index_requires_int_literal(index.span));
                        return ResolvedType::Unknown;
                    };
                    if let Some(idx) = self.resolve_tuple_index(*raw_idx, elems.len(), span) {
                        return elems.get(idx).cloned().unwrap_or(ResolvedType::Unknown);
                    }
                    ResolvedType::Unknown
                }
                _ => ResolvedType::Unknown,
            },
            ty if matches!(ty, ResolvedType::Str) || is_frozen_str(&ty) => {
                if !is_intlike_for_index(&index_ty) {
                    self.errors
                        .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
                }
                ResolvedType::Str
            }
            ResolvedType::Tuple(elems) => {
                // Guardrail: tuple indexing must be an integer literal so we can bounds-check.
                let Expr::Literal(Literal::Int(raw_idx)) = &index.node else {
                    self.errors.push(errors::tuple_index_requires_int_literal(index.span));
                    return ResolvedType::Unknown;
                };
                if let Some(idx) = self.resolve_tuple_index(*raw_idx, elems.len(), span) {
                    return elems.get(idx).cloned().unwrap_or(ResolvedType::Unknown);
                }
                ResolvedType::Unknown
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Type-check a slicing expression (`base[start:end:step]`) and return the sliced type.
    pub(in crate::frontend::typechecker::check_expr) fn check_slice(
        &mut self,
        base: &Spanned<Expr>,
        slice: &SliceExpr,
        _span: Span,
    ) -> ResolvedType {
        let base_ty = self.check_expr(base);

        let start_ty = slice.start.as_ref().map(|s| self.check_expr(s));
        let end_ty = slice.end.as_ref().map(|e| self.check_expr(e));
        let step_ty = slice.step.as_ref().map(|st| self.check_expr(st));

        // Helper: validate that an already-computed type is int-like (or Unknown during inference).
        let check_intlike_ty = |ty: &ResolvedType, span: Span, errors: &mut Vec<_>| {
            if !is_intlike_for_index(ty) {
                errors.push(errors::index_type_mismatch("int", &ty.to_string(), span));
            }
        };
        // Helper: if a slice component exists, validate its already-computed type using the component span.
        let check_component = |ty_opt: Option<&ResolvedType>, expr_opt: Option<&Spanned<Expr>>, errors: &mut Vec<_>| {
            if let (Some(ty), Some(expr)) = (ty_opt, expr_opt) {
                check_intlike_ty(ty, expr.span, errors);
            }
        };

        match base_ty {
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) => {
                    // Validate slice bounds/step for lists as well (indices must be int-like).
                    check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                    check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                    check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                    ResolvedType::Generic(collection_name(CollectionTypeId::List).to_string(), args)
                }
                _ => ResolvedType::Unknown,
            },
            ResolvedType::Str => {
                // We typecheck each slice component once (above) and reuse the computed types here.
                // This avoids re-walking the same expression multiple times and keeps error reporting
                // anchored to the original component spans.
                check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                ResolvedType::Str
            }
            ty if is_frozen_str(&ty) => {
                // `FrozenStr` is the const-eval / deeply-immutable string type, but for indexing/slicing
                // it behaves like `str`: indices must be int-like (or Unknown during inference).
                // Reuse the exact same helper as `str` (the only difference is the receiver type).
                check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                ResolvedType::Str
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Type-check a field access (`base.field`) and return the field type.
    pub(in crate::frontend::typechecker::check_expr) fn check_field(
        &mut self,
        base: &Spanned<Expr>,
        field: &str,
        span: Span,
    ) -> ResolvedType {
        // Handle builtin math module
        if let Expr::Ident(name) = &base.node
            && name == incan_core::lang::surface::math::MATH_MODULE_NAME
        {
            match field {
                _ if incan_core::lang::surface::math::const_from_str(field).is_some() => {
                    return ResolvedType::Float;
                }
                _ => {}
            }
        }

        let base_ty = self.check_expr(base);

        // Be permissive for unknown receivers: allow field access and continue typechecking.
        if matches!(base_ty, ResolvedType::Unknown) {
            return ResolvedType::Unknown;
        }
        if let ResolvedType::RustPath(path) = &base_ty {
            let Some(meta) = self.rust_item_metadata_for_path(path) else {
                // Metadata backend disabled/unavailable: preserve permissive RFC 005 behavior.
                return ResolvedType::Unknown;
            };
            if let incan_core::interop::RustItemKind::Module(module) = meta.kind
                && let Some(child) = module.children.iter().find(|c| c.name == field)
            {
                return match child.kind_hint {
                    incan_core::interop::RustModuleChildKind::Module
                    | incan_core::interop::RustModuleChildKind::Type
                    | incan_core::interop::RustModuleChildKind::Trait
                    | incan_core::interop::RustModuleChildKind::Other => {
                        ResolvedType::RustPath(format!("{path}::{field}"))
                    }
                    incan_core::interop::RustModuleChildKind::Function => {
                        ResolvedType::Function(Vec::new(), Box::new(ResolvedType::Unknown))
                    }
                    incan_core::interop::RustModuleChildKind::Constant => ResolvedType::Unknown,
                };
            }
            self.errors
                .push(errors::missing_field(format!("rust::{path}").as_str(), field, span));
            return ResolvedType::Unknown;
        }

        let resolve_on = |checker: &mut Self, ty: &ResolvedType| -> ResolvedType {
            match ty {
                ResolvedType::Unknown => ResolvedType::Unknown,
                // Trait default methods typecheck against `Self`, but field access must be declared via
                // `@requires(...)` on the trait.
                ResolvedType::SelfType => checker
                    .trait_required_field_type(field, span)
                    .unwrap_or(ResolvedType::Unknown),
                ResolvedType::Tuple(elements) => {
                    if let Ok(idx) = field.parse::<usize>()
                        && idx < elements.len()
                    {
                        return elements[idx].clone();
                    }
                    checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
                    ResolvedType::Unknown
                }
                ResolvedType::Named(type_name) => {
                    if let Some(type_info) = checker.lookup_type_info(type_name) {
                        match type_info {
                            TypeInfo::Model(model) => {
                                // `.0`, `.1`, ... is tuple-index syntax in the language surface.
                                // RFC 021: Non-identifier aliases like `alias="1"` are valid as wire names,
                                // but are not usable via member access / named-arg / pattern syntax.
                                //
                                // Therefore numeric field spellings do NOT participate in alias lookup on models.
                                if field.parse::<usize>().is_ok() {
                                    checker.errors.push(errors::missing_field(type_name, field, span));
                                    return ResolvedType::Unknown;
                                }
                                if let Some((_, info)) = checker.resolve_field_info(&model.fields, field, true, false) {
                                    return info.ty.clone();
                                }
                            }
                            TypeInfo::Class(class) => {
                                // RFC 021: No alias-aware resolution for classes (models only)
                                if let Some((_, info)) = checker.resolve_field_info(&class.fields, field, false, true) {
                                    return info.ty.clone();
                                }
                            }
                            TypeInfo::Enum(enum_info) => {
                                if enum_info.variants.contains(&field.to_string()) {
                                    return ResolvedType::Named(type_name.clone());
                                }
                            }
                            TypeInfo::Newtype(nt) => {
                                if field == conventions::NEWTYPE_TUPLE_FIELD {
                                    return nt.underlying.clone();
                                }
                            }
                            _ => {}
                        }
                    }
                    checker.errors.push(errors::missing_field(type_name, field, span));
                    ResolvedType::Unknown
                }
                _ => {
                    checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
                    ResolvedType::Unknown
                }
            }
        };

        if let ResolvedType::Generic(name, args) = &base_ty
            && matches!(
                surface_types::from_str(name.as_str()),
                Some(SurfaceTypeId::Json | SurfaceTypeId::Query)
            )
            && args.len() == 1
        {
            if field == "value" {
                return args[0].clone();
            }
            return resolve_on(self, &args[0]);
        }

        resolve_on(self, &base_ty)
    }

    /// Type-check a method call (`base.method(args...)`) and return the method's return type.
    pub(in crate::frontend::typechecker::check_expr) fn check_method_call(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let base_ty = self.check_expr(base);
        // Collect arg types for method-specific validation.
        let arg_types: Vec<ResolvedType> = args
            .iter()
            .map(|arg| match arg {
                CallArg::Positional(e) | CallArg::Named(_, e) => self.check_expr(e),
            })
            .collect();

        // If the receiver type is Unknown, be permissive and do not error on methods.
        if matches!(base_ty, ResolvedType::Unknown) {
            return ResolvedType::Unknown;
        }
        if let ResolvedType::RustPath(path) = &base_ty {
            let Some(ret) = self.rust_method_return_type(path, method) else {
                // With rust-metadata enabled, unresolved Rust methods should surface as diagnostics.
                if cfg!(feature = "rust-metadata") {
                    self.errors
                        .push(errors::missing_method(format!("rust::{path}").as_str(), method, span));
                }
                return ResolvedType::Unknown;
            };
            if !matches!(ret, ResolvedType::Unknown) {
                return ret;
            }
            self.errors
                .push(errors::missing_method(format!("rust::{path}").as_str(), method, span));
            return ResolvedType::Unknown;
        }
        // Trait default methods typecheck against `Self`, so be permissive here too.
        if matches!(base_ty, ResolvedType::SelfType) {
            return ResolvedType::Unknown;
        }

        // Treat Enum.Variant(...) method-style calls as variant constructors
        if let ResolvedType::Named(enum_name) = &base_ty
            && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
            && enum_info.variants.iter().any(|v| v == method)
        {
            // Args were checked above; no strict arity enforcement here.
            let _ = &arg_types; // keep for potential future validation
            return ResolvedType::Named(enum_name.clone());
        }

        // External/runtime-provided concurrency primitives: be permissive for surface types that have no local Incan
        // definition. Types defined in `.incn` source are resolved below through their extracted method signatures.
        if let ResolvedType::Named(name) = &base_ty
            && surface_types::from_str(name.as_str()).is_some()
            && self.lookup_type_info(name).is_none()
        {
            return ResolvedType::Unknown;
        }

        if matches!(
            &base_ty,
            ResolvedType::Named(name) if name == surface_types::as_str(SurfaceTypeId::Semaphore)
        ) {
            return match method {
                "acquire" => ResolvedType::Generic(
                    "Result".to_string(),
                    vec![
                        ResolvedType::Named(SEMAPHORE_PERMIT_TYPE_NAME.to_string()),
                        ResolvedType::Named(SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME.to_string()),
                    ],
                ),
                "try_acquire" => option_ty(ResolvedType::Named(SEMAPHORE_PERMIT_TYPE_NAME.to_string())),
                "available_permits" => ResolvedType::Int,
                _ => ResolvedType::Unknown,
            };
        }

        if let ResolvedType::Generic(name, type_args) = &base_ty
            && surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::Mutex)
        {
            let inner = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
            return match method {
                "lock" => ResolvedType::Generic("MutexGuard".to_string(), vec![inner.clone()]),
                "try_lock" => option_ty(ResolvedType::Generic("MutexGuard".to_string(), vec![inner])),
                _ => ResolvedType::Unknown,
            };
        }

        if let ResolvedType::Generic(name, type_args) = &base_ty
            && surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::RwLock)
        {
            let inner = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
            return match method {
                "read" => ResolvedType::Generic("RwLockReadGuard".to_string(), vec![inner.clone()]),
                "write" => ResolvedType::Generic("RwLockWriteGuard".to_string(), vec![inner.clone()]),
                "try_read" => option_ty(ResolvedType::Generic(
                    "RwLockReadGuard".to_string(),
                    vec![inner.clone()],
                )),
                "try_write" => option_ty(ResolvedType::Generic("RwLockWriteGuard".to_string(), vec![inner])),
                _ => ResolvedType::Unknown,
            };
        }

        // Builtin methods for builtin types (so we don't report missing methods).
        if matches!(base_ty, ResolvedType::Float)
            && let Some(id) = float_methods::from_str(method)
        {
            use float_methods::FloatMethodId as M;
            match id {
                M::IsNan | M::IsInfinite | M::IsFinite => return ResolvedType::Bool,
                _ => return ResolvedType::Float,
            }
        }

        if matches!(base_ty, ResolvedType::Str)
            && let Some(ret) = string_method_return(method, false)
        {
            return ret;
        }

        if is_frozen_str(&base_ty)
            && let Some(ret) = string_method_return(method, true)
        {
            return ret;
        }
        if is_frozen_bytes(&base_ty)
            && let Some(id) = frozen_bytes_methods::from_str(method)
        {
            use frozen_bytes_methods::FrozenBytesMethodId as M;
            match id {
                M::Len => return ResolvedType::Int,
                M::IsEmpty => return ResolvedType::Bool,
            }
        }

        match &base_ty {
            ResolvedType::FrozenList(_) => {
                if let Some(id) = frozen_list_methods::from_str(method) {
                    use frozen_list_methods::FrozenListMethodId as M;
                    match id {
                        M::Len => return ResolvedType::Int,
                        M::IsEmpty => return ResolvedType::Bool,
                    }
                }
            }
            ResolvedType::FrozenSet(_) => {
                if let Some(id) = frozen_set_methods::from_str(method) {
                    use frozen_set_methods::FrozenSetMethodId as M;
                    match id {
                        M::Len => return ResolvedType::Int,
                        M::IsEmpty | M::Contains => return ResolvedType::Bool,
                    }
                }
            }
            ResolvedType::FrozenDict(_, _) => {
                if let Some(id) = frozen_dict_methods::from_str(method) {
                    use frozen_dict_methods::FrozenDictMethodId as M;
                    match id {
                        M::Len => return ResolvedType::Int,
                        M::IsEmpty | M::ContainsKey => return ResolvedType::Bool,
                    }
                }
            }
            _ => {}
        }

        // Option[T] helpers.
        //
        // NOTE: `Dict.get(k)` is backed by Rust `HashMap::get`, which returns `Option<&V>`.
        // We model that as `Option[&V]` internally, so helpers like `.copied()` can typecheck in the same way they do
        // in Rust.
        if base_ty.is_option() {
            let inner = base_ty.option_inner_type().cloned().unwrap_or(ResolvedType::Unknown);
            match option_methods::from_str(method) {
                Some(option_methods::OptionMethodId::Copied) => {
                    // Rust: `Option<&T>::copied() -> Option<T>` (for `T: Copy`).
                    if let ResolvedType::Ref(t) = inner {
                        let t = (*t).clone();
                        if matches!(t, ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool) {
                            return option_ty(t);
                        }
                    }
                }
                Some(option_methods::OptionMethodId::UnwrapOr) => {
                    // Rust: `Option<T>::unwrap_or(default: T) -> T`
                    //
                    // For `Option<&T>`, this is `unwrap_or(default: &T) -> &T`.
                    if let Some(default_ty) = arg_types.first()
                        && !self.types_compatible(default_ty, &inner)
                    {
                        self.errors
                            .push(errors::type_mismatch(&inner.to_string(), &default_ty.to_string(), span));
                    }
                    return inner;
                }
                Some(option_methods::OptionMethodId::Unwrap) => {
                    return inner;
                }
                None => {}
            }
        }

        // FIXME: Too many levels of nesting here.
        if let ResolvedType::Generic(name, type_args) = &base_ty {
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) {
                let elem = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                if let Some(id) = list_methods::from_str(method) {
                    use list_methods::ListMethodId as M;
                    match id {
                        M::Append => {
                            let clone_ty = arg_types.first().unwrap_or(&elem);
                            if let Some(arg0) = arg_types.first()
                                && !self.types_compatible(arg0, &elem)
                            {
                                self.errors
                                    .push(errors::type_mismatch(&elem.to_string(), &arg0.to_string(), span));
                            }
                            if !self.is_copy_type(clone_ty) && !self.is_clone_type(clone_ty) {
                                self.errors
                                    .push(errors::list_append_requires_clone(&clone_ty.to_string(), span));
                            }
                            return ResolvedType::Unit;
                        }
                        M::Pop => return elem,
                        M::Contains => return ResolvedType::Bool,
                        M::Swap | M::Reserve | M::ReserveExact | M::Remove => return ResolvedType::Unit,
                        M::Count | M::Index => return ResolvedType::Int,
                    }
                }
            }
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) {
                let key = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                let val = type_args.get(1).cloned().unwrap_or(ResolvedType::Unknown);
                if let Some(id) = dict_methods::from_str(method) {
                    use dict_methods::DictMethodId as M;
                    match id {
                        M::Keys => return list_ty(key),
                        M::Values => return list_ty(val),
                        // `Dict.get(k)` is backed by Rust `HashMap::get`, which returns `Option<&V>`.
                        // Model this as an internal reference so chained Rust-idiom helpers (like `.copied()`)
                        // typecheck consistently with codegen.
                        M::Get => return option_ty(ResolvedType::Ref(Box::new(val.clone()))),
                        M::Insert => return ResolvedType::Unit,
                    }
                }
            }
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::Set)
                && set_methods::from_str(method).is_some()
            {
                return ResolvedType::Bool;
            }
        }

        if let ResolvedType::Generic(type_name, _type_args) = &base_ty
            && let Some(type_info) = self.lookup_type_info(type_name).cloned()
        {
            match type_info {
                TypeInfo::Model(model) => {
                    if let Some(ret) =
                        self.resolve_named_method(&model.methods, Some(&model.traits), method, args, &arg_types, span)
                    {
                        return ret;
                    }
                }
                TypeInfo::Class(class) => {
                    if let Some(ret) =
                        self.resolve_named_method(&class.methods, Some(&class.traits), method, args, &arg_types, span)
                    {
                        return ret;
                    }
                }
                TypeInfo::Newtype(newtype) => {
                    if let Some(ret) = self.resolve_named_method(&newtype.methods, None, method, args, &arg_types, span)
                    {
                        return ret;
                    }
                    if newtype.is_rusttype
                        && let ResolvedType::RustPath(path) = &newtype.underlying
                        && let Some(ret) = self.rust_method_return_type(path, method)
                    {
                        if !matches!(ret, ResolvedType::Unknown) {
                            return ret;
                        }
                    }
                }
                _ => {}
            }
        }

        // Named types: look up methods from the type definition.
        // If the symbol doesn't exist or isn't a type (e.g., Module/RustItem placeholder), treat it as external and
        // be permissive.
        if let ResolvedType::Named(type_name) = &base_ty {
            match self.lookup_type_info(type_name).cloned() {
                None => {
                    // Symbol not found or not a Type - treat as external, be permissive.
                    return ResolvedType::Unknown;
                }
                Some(type_info) => match type_info {
                    TypeInfo::Model(model) => {
                        if let Some(ret) = self.resolve_named_method(
                            &model.methods,
                            Some(&model.traits),
                            method,
                            args,
                            &arg_types,
                            span,
                        ) {
                            return ret;
                        }
                    }
                    TypeInfo::Class(class) => {
                        if let Some(ret) = self.resolve_named_method(
                            &class.methods,
                            Some(&class.traits),
                            method,
                            args,
                            &arg_types,
                            span,
                        ) {
                            return ret;
                        }
                    }
                    TypeInfo::Enum(_enum_info) => {
                        // Be permissive for common error/display helpers on enums
                        if enum_helpers::from_str(method) == Some(enum_helpers::EnumHelperId::Message) {
                            return ResolvedType::Str;
                        }
                    }
                    TypeInfo::Newtype(nt) => {
                        if let Some(ret) = self.resolve_named_method(&nt.methods, None, method, args, &arg_types, span)
                        {
                            return ret;
                        }
                        if nt.is_rusttype
                            && let ResolvedType::RustPath(path) = &nt.underlying
                            && let Some(ret) = self.rust_method_return_type(path, method)
                        {
                            if !matches!(ret, ResolvedType::Unknown) {
                                return ret;
                            }
                        }
                    }
                    _ => {}
                },
            }
        }

        // For magic helpers that codegen injects (e.g., __class_name__, __fields__), be permissive at typecheck time
        // since they are backend-provided.
        if magic_methods::from_str(method).is_some() {
            return ResolvedType::Unknown;
        }

        // For common external generic types (interop/runtime-provided) that we don't model in the checker, be
        // permissive and do not error on unknown methods.
        if let ResolvedType::Generic(name, _args) = &base_ty
            && self.lookup_type_info(name).is_none()
        {
            return ResolvedType::Unknown;
        }

        // RFC 023: Method calls on generic type variables are permissive.
        //
        // The Rust backend infers the required trait bounds (e.g., `x.clone()` → `T: Clone`).
        // At the Incan typechecker level we allow the call and return the same type variable.
        if self.is_generic_placeholder_type(&base_ty) {
            return base_ty.clone();
        }

        // Guardrail: don't silently return Unknown for missing methods on known user types.
        // For unknown/external types we returned Unknown above without error.
        let base_name_str = base_ty.to_string();
        let skip_error_for_known_runtime = surface_types::from_str(base_name_str.as_str()).is_some();
        if !(matches!(base_ty, ResolvedType::Named(ref n) if self.symbols.lookup(n).is_none())
            || skip_error_for_known_runtime)
        {
            self.errors
                .push(errors::missing_method(&base_ty.to_string(), method, span));
        }
        ResolvedType::Unknown
    }
}
