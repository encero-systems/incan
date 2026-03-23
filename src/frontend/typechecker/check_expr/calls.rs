//! Check calls, constructors, and builtins.
//!
//! This module handles the main call-expression logic (`foo(...)`), including special-cased
//! builtins like `Ok(...)`/`Err(...)` and runtime helpers like `sleep(...)`. It also provides
//! small utilities to type-check call argument lists consistently.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::{collection_type_id, dict_ty, list_ty, option_ty, result_ty, set_ty};
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::surface::functions::{self as surface_functions, SurfaceFnId};
use incan_core::lang::surface::math;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

impl TypeChecker {
    fn check_model_or_class_constructor_call(
        &mut self,
        type_name: &str,
        fields: &std::collections::HashMap<String, FieldInfo>,
        args: &[CallArg],
        call_span: Span,
    ) {
        // v0.1: only named args for model/class constructors (stable field ordering not guaranteed).
        if args.iter().any(|a| matches!(a, CallArg::Positional(_))) {
            // Typecheck argument expressions regardless, so type errors in expressions still show up.
            self.check_call_args(args);
            self.errors
                .push(errors::positional_constructor_args_not_supported(type_name, call_span));
            return;
        }

        // Track provided fields and validate existence/duplicates/type compatibility.
        let mut provided: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
        for arg in args {
            let CallArg::Named(field_name, expr) = arg else {
                continue;
            };

            // Always typecheck the expression exactly once, even if the key is invalid/duplicate.
            // This avoids double-reporting while still surfacing expression errors.
            let value_ty = self.check_expr(expr);

            let Some((canonical_name, field_info)) = self.resolve_field_info(fields, field_name, true, true) else {
                self.errors
                    .push(errors::missing_field(type_name, field_name, expr.span));
                continue;
            };

            if provided.contains_key(&canonical_name) {
                self.errors.push(errors::duplicate_field_in_call(
                    type_name,
                    canonical_name.as_str(),
                    expr.span,
                ));
                continue;
            }
            provided.insert(canonical_name.clone(), expr.span);

            if !self.types_compatible(&value_ty, &field_info.ty) {
                self.errors.push(errors::field_type_mismatch(
                    field_name,
                    &field_info.ty.to_string(),
                    &value_ty.to_string(),
                    expr.span,
                ));
            }
        }

        // Enforce required fields (those without defaults) are present.
        for (field_name, info) in fields {
            if !info.has_default && !provided.contains_key(field_name) {
                self.errors.push(errors::missing_required_constructor_field(
                    type_name, field_name, call_span,
                ));
            }
        }
    }

    /// Extract the expression from a call argument (positional or named).
    fn call_arg_expr(arg: &CallArg) -> &Spanned<Expr> {
        match arg {
            CallArg::Positional(e) | CallArg::Named(_, e) => e,
        }
    }

    /// Type-check all call arguments (positional and named).
    pub(in crate::frontend::typechecker::check_expr) fn check_call_args(&mut self, args: &[CallArg]) {
        for arg in args {
            self.check_expr(Self::call_arg_expr(arg));
        }
    }

    /// Type-check a JSON/Query constructor call (`Json(...)` / `Query(...)`).
    ///
    /// NOTE: This method is called from multiple dispatch points in the typechecker because
    /// calls can be classified differently by the parser (bare identifier call, constructor call,
    /// builtin call, or model/class constructor). Each dispatch point returns early after handling,
    /// preventing double-checking. See: `check_builtin_call` (surface type dispatch),
    /// `check_call` (fallback paths), and `check_constructor`.
    fn check_json_query_constructor_call(
        &mut self,
        tid: SurfaceTypeId,
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        let mut inner = ResolvedType::Unknown;
        let mut has_inner = false;
        let mut positional_count = 0;
        let mut named_value_count = 0;
        let mut has_invalid_named = false;

        for arg in args {
            match arg {
                CallArg::Positional(e) => {
                    positional_count += 1;
                    if !has_inner {
                        inner = self.check_expr(e);
                        has_inner = true;
                    } else {
                        self.check_expr(e);
                    }
                }
                CallArg::Named(name, e) if name == "value" => {
                    named_value_count += 1;
                    if !has_inner {
                        inner = self.check_expr(e);
                        has_inner = true;
                    } else {
                        self.check_expr(e);
                    }
                }
                CallArg::Named(_, e) => {
                    has_invalid_named = true;
                    self.check_expr(e);
                }
            }
        }

        let total_allowed = positional_count + named_value_count;
        if has_invalid_named || total_allowed != 1 || (positional_count > 0 && named_value_count > 0) {
            let name = surface_types::as_str(tid);
            self.errors
                .push(errors::constructor_single_arg_required(name, args.len(), call_span));
        }

        ResolvedType::Generic(surface_types::as_str(tid).to_string(), vec![inner])
    }

    /// Type-check all call arguments and collect their resolved types.
    fn check_call_arg_types(&mut self, args: &[CallArg]) -> Vec<ResolvedType> {
        args.iter()
            .map(|arg| self.check_expr(Self::call_arg_expr(arg)))
            .collect()
    }

    fn constructor_result_type(&self, name: &str) -> ResolvedType {
        match self.lookup_type_info(name) {
            Some(TypeInfo::Model(model)) if !model.type_params.is_empty() => {
                ResolvedType::Generic(name.to_string(), vec![ResolvedType::Unknown; model.type_params.len()])
            }
            Some(TypeInfo::Class(class)) if !class.type_params.is_empty() => {
                ResolvedType::Generic(name.to_string(), vec![ResolvedType::Unknown; class.type_params.len()])
            }
            Some(TypeInfo::Newtype(newtype)) if !newtype.type_params.is_empty() => {
                ResolvedType::Generic(name.to_string(), vec![ResolvedType::Unknown; newtype.type_params.len()])
            }
            Some(TypeInfo::Enum(enum_info)) if !enum_info.type_params.is_empty() => ResolvedType::Generic(
                name.to_string(),
                vec![ResolvedType::Unknown; enum_info.type_params.len()],
            ),
            _ => ResolvedType::Named(name.to_string()),
        }
    }

    /// Validate a function call against a known function signature and enforce explicit generic bounds.
    fn validate_function_call(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        let arg_types = self.check_call_arg_types(args);
        let mut positional: Vec<(ResolvedType, Span)> = Vec::new();
        let mut named: std::collections::HashMap<&str, (ResolvedType, Span)> = std::collections::HashMap::new();

        for (arg, ty) in args.iter().zip(arg_types.iter()) {
            let expr = Self::call_arg_expr(arg);
            match arg {
                CallArg::Positional(_) => positional.push((ty.clone(), expr.span)),
                CallArg::Named(name, _) => {
                    named.insert(name.as_str(), (ty.clone(), expr.span));
                }
            }
        }

        let mut pos_idx = 0usize;
        let mut type_bindings: std::collections::HashMap<String, ResolvedType> = std::collections::HashMap::new();
        for (param_name, param_ty) in &info.params {
            let arg = if let Some(v) = named.get(param_name.as_str()) {
                Some(v)
            } else if pos_idx < positional.len() {
                let v = positional.get(pos_idx);
                pos_idx += 1;
                v
            } else {
                None
            };

            if let Some((arg_ty, arg_span)) = arg {
                self.infer_type_param_bindings(param_ty, arg_ty, &mut type_bindings);
                if !self.types_compatible(arg_ty, param_ty) {
                    self.errors.push(errors::type_mismatch(
                        &param_ty.to_string(),
                        &arg_ty.to_string(),
                        *arg_span,
                    ));
                }
            }
        }
        self.emit_explicit_bound_errors(func_name, &info.type_param_bounds, &type_bindings, call_span);

        info.return_type.clone()
    }

    /// Infer concrete type bindings for generic type parameters from a parameter/argument type pair.
    fn infer_type_param_bindings(
        &self,
        expected: &ResolvedType,
        actual: &ResolvedType,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        match expected {
            ResolvedType::TypeVar(name) => {
                bindings
                    .entry(name.clone())
                    .and_modify(|existing| {
                        if !self.types_compatible(actual, existing) {
                            *existing = ResolvedType::Unknown;
                        }
                    })
                    .or_insert_with(|| actual.clone());
            }
            ResolvedType::Generic(name, expected_args) => {
                if let ResolvedType::Generic(actual_name, actual_args) = actual
                    && name == actual_name
                {
                    for (e, a) in expected_args.iter().zip(actual_args.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                }
            }
            ResolvedType::Function(expected_params, expected_ret) => {
                if let ResolvedType::Function(actual_params, actual_ret) = actual {
                    for (e, a) in expected_params.iter().zip(actual_params.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                    self.infer_type_param_bindings(expected_ret, actual_ret, bindings);
                }
            }
            ResolvedType::Tuple(expected_items) => {
                if let ResolvedType::Tuple(actual_items) = actual {
                    for (e, a) in expected_items.iter().zip(actual_items.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                }
            }
            ResolvedType::FrozenList(inner) => {
                if let ResolvedType::FrozenList(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::FrozenSet(inner) => {
                if let ResolvedType::FrozenSet(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::FrozenDict(k, v) => {
                if let ResolvedType::FrozenDict(actual_k, actual_v) = actual {
                    self.infer_type_param_bindings(k, actual_k, bindings);
                    self.infer_type_param_bindings(v, actual_v, bindings);
                }
            }
            ResolvedType::Ref(inner) => {
                if let ResolvedType::Ref(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            _ => {}
        }
    }

    /// Emit diagnostics when inferred concrete generic bindings violate explicit `with` bounds.
    fn emit_explicit_bound_errors(
        &mut self,
        func_name: &str,
        bounds_by_param: &std::collections::HashMap<String, Vec<String>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        for (type_param, bounds) in bounds_by_param {
            let Some(actual_ty) = bindings.get(type_param) else {
                continue;
            };
            for bound in bounds {
                if !self.type_satisfies_explicit_bound(actual_ty, bound) {
                    self.errors.push(errors::generic_bound_not_satisfied(
                        func_name,
                        type_param,
                        bound,
                        &actual_ty.to_string(),
                        call_span,
                    ));
                }
            }
        }
    }

    /// Best-effort check whether a concrete type satisfies an explicit generic bound.
    fn type_satisfies_explicit_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        match ty {
            // Unknown / still-generic types are kept permissive to avoid cascading errors.
            ResolvedType::Unknown | ResolvedType::TypeVar(_) => true,
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => self.primitive_type_satisfies_bound(ty, bound),
            ResolvedType::Tuple(items) => self.tuple_type_satisfies_bound(items, bound),
            ResolvedType::FrozenList(inner) => self.collection_type_satisfies_bound(
                CollectionTypeId::FrozenList,
                std::slice::from_ref(inner.as_ref()),
                bound,
            ),
            ResolvedType::FrozenSet(inner) => self.collection_type_satisfies_bound(
                CollectionTypeId::FrozenSet,
                std::slice::from_ref(inner.as_ref()),
                bound,
            ),
            ResolvedType::FrozenDict(k, v) => {
                let pair = [k.as_ref().clone(), v.as_ref().clone()];
                self.collection_type_satisfies_bound(CollectionTypeId::FrozenDict, &pair, bound)
            }
            ResolvedType::Generic(name, args) => {
                if let Some(kind) = collection_type_id(name.as_str()) {
                    self.collection_type_satisfies_bound(kind, args, bound)
                } else {
                    self.named_type_satisfies_bound(name, bound)
                }
            }
            ResolvedType::Named(type_name) => self.named_type_satisfies_bound(type_name, bound),
            ResolvedType::Ref(inner) => self.type_satisfies_explicit_bound(inner, bound),
            ResolvedType::Function(_, _) | ResolvedType::SelfType => false,
        }
    }

    fn primitive_type_satisfies_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        if bound == derives::as_str(DeriveId::Copy) {
            return self.is_copy_type(ty);
        }

        match builtin_traits::from_str(bound) {
            Some(TraitId::Clone | TraitId::Debug | TraitId::Display) => matches!(
                ty,
                ResolvedType::Int
                    | ResolvedType::Float
                    | ResolvedType::Bool
                    | ResolvedType::Str
                    | ResolvedType::Bytes
                    | ResolvedType::FrozenStr
                    | ResolvedType::FrozenBytes
                    | ResolvedType::Unit
            ),
            Some(TraitId::Default) => matches!(
                ty,
                ResolvedType::Int
                    | ResolvedType::Float
                    | ResolvedType::Bool
                    | ResolvedType::Str
                    | ResolvedType::Bytes
                    | ResolvedType::FrozenStr
                    | ResolvedType::FrozenBytes
                    | ResolvedType::Unit
            ),
            Some(TraitId::Eq | TraitId::Ord | TraitId::Hash) => matches!(
                ty,
                ResolvedType::Int
                    | ResolvedType::Bool
                    | ResolvedType::Str
                    | ResolvedType::Bytes
                    | ResolvedType::FrozenStr
                    | ResolvedType::FrozenBytes
                    | ResolvedType::Unit
            ),
            Some(TraitId::PartialEq | TraitId::PartialOrd) => matches!(
                ty,
                ResolvedType::Int
                    | ResolvedType::Float
                    | ResolvedType::Bool
                    | ResolvedType::Str
                    | ResolvedType::Bytes
                    | ResolvedType::FrozenStr
                    | ResolvedType::FrozenBytes
                    | ResolvedType::Unit
            ),
            _ => false,
        }
    }

    fn tuple_type_satisfies_bound(&self, items: &[ResolvedType], bound: &str) -> bool {
        match builtin_traits::from_str(bound) {
            Some(
                TraitId::Clone
                | TraitId::Debug
                | TraitId::Default
                | TraitId::Eq
                | TraitId::PartialEq
                | TraitId::Ord
                | TraitId::PartialOrd
                | TraitId::Hash,
            ) => items.iter().all(|item| self.type_satisfies_explicit_bound(item, bound)),
            _ => false,
        }
    }

    fn collection_type_satisfies_bound(&self, kind: CollectionTypeId, args: &[ResolvedType], bound: &str) -> bool {
        let all_args_satisfy = || args.iter().all(|arg| self.type_satisfies_explicit_bound(arg, bound));
        match builtin_traits::from_str(bound) {
            Some(TraitId::Clone | TraitId::Debug) => all_args_satisfy(),
            Some(TraitId::Default) => matches!(
                kind,
                CollectionTypeId::List
                    | CollectionTypeId::FrozenList
                    | CollectionTypeId::Dict
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::Set
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::Option
            ),
            Some(TraitId::Eq | TraitId::PartialEq) => all_args_satisfy(),
            Some(TraitId::Ord | TraitId::PartialOrd) => {
                matches!(
                    kind,
                    CollectionTypeId::List
                        | CollectionTypeId::FrozenList
                        | CollectionTypeId::Tuple
                        | CollectionTypeId::Option
                ) && all_args_satisfy()
            }
            Some(TraitId::Hash) => {
                matches!(
                    kind,
                    CollectionTypeId::List
                        | CollectionTypeId::FrozenList
                        | CollectionTypeId::Tuple
                        | CollectionTypeId::Option
                ) && all_args_satisfy()
            }
            _ => false,
        }
    }

    fn named_type_satisfies_bound(&self, type_name: &str, bound: &str) -> bool {
        match self.lookup_type_info(type_name) {
            Some(TypeInfo::Builtin) => matches!(builtin_traits::from_str(bound), Some(TraitId::Clone | TraitId::Debug)),
            Some(TypeInfo::Model(info)) => {
                info.traits.iter().any(|t| t == bound) || info.derives.iter().any(|d| d == bound)
            }
            Some(TypeInfo::Class(info)) => {
                info.traits.iter().any(|t| t == bound) || info.derives.iter().any(|d| d == bound)
            }
            Some(TypeInfo::Enum(info)) => {
                // Enums do not carry explicit trait adoption; best-effort via derive names in symbol metadata is
                // absent. Keep conservative and require explicit evidence where available.
                let _ = info;
                false
            }
            Some(TypeInfo::Newtype(_)) => false,
            Some(TypeInfo::TypeAlias) => false,
            None => false,
        }
    }

    /// Handle a known builtin call (if the callee is a builtin name).
    fn check_builtin_call(&mut self, name: &str, args: &[CallArg], call_span: Span) -> Option<ResolvedType> {
        let has_function_symbol = self
            .symbols
            .lookup(name)
            .and_then(|id| self.symbols.get(id))
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Function(_)));

        // Constructors (variant-like)
        if let Some(cid) = constructors::from_str(name) {
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
        if let Some(bid) = builtins::from_str(name) {
            if bid == BuiltinFnId::Sleep && !has_function_symbol {
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
                BuiltinFnId::Sleep => {
                    if let Some(arg) = args.first() {
                        let arg_expr = Self::call_arg_expr(arg);
                        let arg_ty = self.check_expr(arg_expr);
                        if !self.types_compatible(&arg_ty, &ResolvedType::Float) {
                            self.errors
                                .push(errors::type_mismatch("float", &arg_ty.to_string(), arg_expr.span));
                        }
                    }
                    Some(ResolvedType::Unit)
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

    /// Type-check a call expression and return its result type.
    pub(in crate::frontend::typechecker::check_expr) fn check_call(
        &mut self,
        callee: &Spanned<Expr>,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        // Special-case: Enum variant constructor syntax `Enum.Variant(...)`.
        // If callee is a field access where the base resolves to a known enum type
        // and the field name matches a variant, treat this as a constructor and
        // return the enum type.
        if let Expr::Field(base, variant_name) = &callee.node {
            let base_ty = self.check_expr(base);
            if let ResolvedType::Named(enum_name) = &base_ty
                && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
                && enum_info.variants.iter().any(|v| v == variant_name)
            {
                self.check_call_args(args);
                return ResolvedType::Named(enum_name.clone());
            }
        }

        // Handle math module function calls (math.sqrt, math.sin, etc.)
        if let Expr::Field(base, method) = &callee.node
            && let Expr::Ident(module) = &base.node
            && module == math::MATH_MODULE_NAME
        {
            self.check_call_args(args);
            if math::fn_from_str(method.as_str()).is_some() {
                return ResolvedType::Float;
            }
        }

        if let Expr::Ident(name) = &callee.node {
            let marker_binding_in_scope = self
                .symbols
                .lookup(name)
                .and_then(|id| self.symbols.get(id))
                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Function(_)) && sym.scope == 0);
            if self.testing_marker_import_bindings.contains(name) && marker_binding_in_scope {
                self.check_call_args(args);
                self.errors
                    .push(errors::testing_marker_runtime_call_not_supported(name, span));
                return ResolvedType::Unknown;
            }

            if let Some(result) = self.check_builtin_call(name, args, span) {
                return result;
            }

            if let Some(func_info) = self.lookup_symbol(name).and_then(|sym| match &sym.kind {
                SymbolKind::Function(info) => Some(info.clone()),
                _ => None,
            }) {
                return self.validate_function_call(name, &func_info, args, span);
            }

            // RFC 042: traits are abstract — reject `TraitName(...)` constructor syntax.
            if self
                .lookup_symbol(name)
                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Trait(_)))
            {
                self.check_call_args(args);
                self.errors.push(errors::cannot_instantiate_trait(name, span));
                return ResolvedType::Unknown;
            }

            let in_scope = self.symbols.lookup(name).is_some();
            if in_scope && let Some(tid) = surface_types::from_str(name) {
                if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                    return self.check_json_query_constructor_call(tid, args, span);
                }
                if matches!(tid, SurfaceTypeId::Html) {
                    return ResolvedType::Named(surface_types::as_str(tid).to_string());
                }
            }

            // Strict validated construction: `@derive(Validate)` models must be constructed via `TypeName.new(...)`.
            if let Some(TypeInfo::Model(m)) = self.lookup_type_info(name)
                && m.derives
                    .iter()
                    .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate))
            {
                // Still typecheck argument expressions for better downstream errors.
                self.check_call_args(args);
                self.errors
                    .push(errors::validate_derive_disallows_raw_construction(name, span));
                return ResolvedType::Unknown;
            }

            // Model/class constructor calls: validate field arguments at the Incan level.
            // NOTE: `lookup_type_info` returns a reference into `self`, so we clone the needed field map to avoid
            // borrow conflicts (we need `&mut self` for validation).
            let ctor_fields: Option<std::collections::HashMap<String, FieldInfo>> =
                self.lookup_type_info(name).and_then(|info| match info {
                    TypeInfo::Model(m) => Some(m.fields.clone()),
                    TypeInfo::Class(c) => Some(c.fields.clone()),
                    _ => None,
                });
            if let Some(fields) = ctor_fields {
                self.check_model_or_class_constructor_call(name, &fields, args, span);
                if in_scope && let Some(tid) = surface_types::from_str(name) {
                    if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                        return self.check_json_query_constructor_call(tid, args, span);
                    }
                    if matches!(tid, SurfaceTypeId::Html) {
                        return ResolvedType::Named(surface_types::as_str(tid).to_string());
                    }
                }
                return self.constructor_result_type(name);
            }
        }

        let callee_ty = self.check_expr(callee);
        self.check_call_args(args);

        match callee_ty {
            ResolvedType::Function(_, ret) => *ret,
            ResolvedType::Named(name) => match self.lookup_symbol(&name).map(|s| &s.kind) {
                Some(SymbolKind::Type(_)) => self.constructor_result_type(&name),
                Some(SymbolKind::Variant(info)) => ResolvedType::Named(info.enum_name.clone()),
                _ => ResolvedType::Unknown,
            },
            _ => ResolvedType::Unknown,
        }
    }

    /// Type-check a constructor-like call (`TypeName(...)` / `VariantName(...)`).
    pub(in crate::frontend::typechecker::check_expr) fn check_constructor(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.check_call_args(args);

        if self
            .lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Trait(_)))
        {
            self.errors.push(errors::cannot_instantiate_trait(name, span));
            return ResolvedType::Unknown;
        }

        if self.symbols.lookup(name).is_some()
            && let Some(tid) = surface_types::from_str(name)
        {
            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                return self.check_json_query_constructor_call(tid, args, span);
            }
            if matches!(tid, SurfaceTypeId::Html) {
                return ResolvedType::Named(surface_types::as_str(tid).to_string());
            }
        }

        match self.lookup_symbol(name).map(|s| &s.kind) {
            Some(SymbolKind::Type(_)) => self.constructor_result_type(name),
            Some(SymbolKind::Variant(info)) => ResolvedType::Named(info.enum_name.clone()),
            Some(_) => ResolvedType::Unknown,
            None => {
                self.errors.push(errors::unknown_symbol(name, span));
                ResolvedType::Unknown
            }
        }
    }
}
