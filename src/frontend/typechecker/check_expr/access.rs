//! Check indexing, slicing, field access, and method calls.
//!
//! These helpers validate access patterns like `xs[i]`, `xs[a:b]`, `obj.field`, and `obj.method(...)`, emitting
//! diagnostics for missing fields/methods and incompatible uses.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::{
    collection_name, collection_type_id, is_frozen_bytes, is_frozen_str, is_intlike_for_index, list_ty, option_ty,
    string_method_return,
};
use incan_core::interop::{CoercionPolicy, RustCollectionFamily, RustItemKind};
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

struct ValueEnumGeneratedCall<'a> {
    enum_name: &'a str,
    value_enum: &'a ValueEnumInfo,
    method: &'a str,
    base_is_type_name: bool,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    arg_types: &'a [ResolvedType],
    span: Span,
}

/// Diagnostic label for a Rust path receiver in type errors (`rust::{path}`).
fn rust_receiver_display(path: &str) -> String {
    format!("rust::{path}")
}

impl TypeChecker {
    /// Return whether an expression names an enum type rather than an enum value.
    fn is_enum_type_name_expr(&self, expr: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &expr.node else {
            return false;
        };
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Enum(_))))
    }

    /// Build the `Option[Enum]` return type for generated `from_value(...)`.
    fn value_enum_from_value_return_type(enum_name: &str) -> ResolvedType {
        option_ty(ResolvedType::Named(enum_name.to_string()))
    }

    /// Typecheck generated value-enum helpers reached through member-access call syntax.
    fn check_value_enum_generated_method_call(&mut self, call: ValueEnumGeneratedCall<'_>) -> Option<ResolvedType> {
        match call.method {
            "value" if !call.base_is_type_name => {
                if !call.type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(call.span));
                }
                if !call.args.is_empty() {
                    self.errors.push(errors::builtin_arity(
                        &format!("{}.value()", call.enum_name),
                        0,
                        call.args.len(),
                        call.span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                Some(call.value_enum.value_type.resolved_type())
            }
            "from_value" if call.base_is_type_name => {
                if !call.type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(call.span));
                }
                if call.args.len() != 1 {
                    self.errors.push(errors::builtin_arity(
                        &format!("{}.from_value()", call.enum_name),
                        1,
                        call.args.len(),
                        call.span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                let expected = call.value_enum.value_type.resolved_type();
                if let Some((arg_ty, arg)) = call.arg_types.first().zip(call.args.first()) {
                    let expr = match arg {
                        CallArg::Positional(expr)
                        | CallArg::Named(_, expr)
                        | CallArg::PositionalUnpack(expr)
                        | CallArg::KeywordUnpack(expr) => expr,
                    };
                    if !self.types_compatible(arg_ty, &expected) {
                        self.errors.push(errors::type_mismatch(
                            &expected.to_string(),
                            &arg_ty.to_string(),
                            expr.span,
                        ));
                    }
                }
                Some(Self::value_enum_from_value_return_type(call.enum_name))
            }
            _ => None,
        }
    }

    /// Resolve known fields on compiler-provided surface record types.
    ///
    /// This currently covers `std.reflection.FieldInfo` so callers can typecheck member access on values returned by
    /// `__fields__()` without requiring an explicit `FieldInfo` import at the use site.
    fn resolve_surface_type_field_type(&self, type_name: &str, field: &str) -> Option<ResolvedType> {
        match surface_types::from_str(type_name) {
            Some(SurfaceTypeId::FieldInfo) => match field {
                "name" | "wire_name" | "type_name" => Some(ResolvedType::FrozenStr),
                "alias" | "description" => Some(option_ty(ResolvedType::FrozenStr)),
                "has_default" => Some(ResolvedType::Bool),
                "extra" => Some(ResolvedType::FrozenDict(
                    Box::new(ResolvedType::FrozenStr),
                    Box::new(ResolvedType::FrozenStr),
                )),
                _ => None,
            },
            _ => None,
        }
    }

    /// Return the surface-level result type for built-in reflection magic methods.
    ///
    /// These methods are compiler-provided rather than declared in user-visible source, so the typechecker must model
    /// their return types directly.
    fn reflection_magic_method_return_type(&self, method: &str) -> Option<ResolvedType> {
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => Some(ResolvedType::Str),
            Some(magic_methods::MagicMethodId::Fields) => Some(ResolvedType::FrozenList(Box::new(
                ResolvedType::Named(surface_types::as_str(SurfaceTypeId::FieldInfo).to_string()),
            ))),
            _ => None,
        }
    }

    /// Report whether a nominal type is allowed to use a given reflection magic method.
    ///
    /// Support is intentionally method-specific: `__class_name__()` is limited to models and classes, while
    /// `__fields__()` also applies to newtypes.
    fn nominal_type_supports_reflection_magic(&self, ty: &ResolvedType, method: &str) -> bool {
        let type_name = match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name.as_str(),
            _ => return false,
        };
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_))
                )
            }
            Some(magic_methods::MagicMethodId::Fields) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
                )
            }
            _ => false,
        }
    }

    fn rust_canonical_path_for_receiver_type(&self, ty: &ResolvedType) -> Option<String> {
        match ty {
            ResolvedType::RustPath(path) => Some(path.clone()),
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                let id = self.symbols.lookup(name)?;
                let sym = self.symbols.get(id)?;
                let SymbolKind::RustItem(info) = &sym.kind else {
                    return None;
                };
                match info.binding {
                    RustImportBindingKind::CrateRoot => None,
                    RustImportBindingKind::RootedPath | RustImportBindingKind::FromImport => Some(info.path.clone()),
                }
            }
            _ => None,
        }
    }

    /// Resolve a declared field on a nominal user-defined type, applying generic substitutions when available.
    ///
    /// This keeps field access on `Named(Type)` and `Generic(Type[...])` owners on the same path instead of letting
    /// generic owners fall through to "missing field" diagnostics despite having declared fields.
    fn resolve_nominal_field_type(
        &mut self,
        type_name: &str,
        type_args: Option<&[ResolvedType]>,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        if let Some(surface_ty) = self.resolve_surface_type_field_type(type_name, field) {
            return Some(surface_ty);
        }
        let type_info = self.lookup_semantic_type_info(type_name)?;

        let field_info = match type_info {
            TypeInfo::Model(model) => {
                // `.0`, `.1`, ... is tuple-index syntax in the language surface.
                // RFC 021: Non-identifier aliases like `alias="1"` are valid as wire names, but are not usable via
                // member access / named-arg / pattern syntax.
                //
                // Therefore numeric field spellings do NOT participate in alias lookup on models.
                if field.parse::<usize>().is_ok() {
                    self.errors.push(errors::missing_field(type_name, field, span));
                    return Some(ResolvedType::Unknown);
                }
                let (_, info) = self.resolve_field_info(&model.fields, field, true, false)?;
                if let Some(args) = type_args {
                    let subst = type_param_subst_map(&model.type_params, args);
                    return Some(substitute_resolved_type(&info.ty, &subst));
                }
                info.ty.clone()
            }
            TypeInfo::Class(class) => {
                // RFC 021: No alias-aware resolution for classes (models only)
                let (_, info) = self.resolve_field_info(&class.fields, field, false, true)?;
                let owner = info.owner.as_deref().unwrap_or(type_name);
                if matches!(info.visibility, Visibility::Private) && self.current_method_owner.as_deref() != Some(owner)
                {
                    self.errors.push(errors::private_field(type_name, field, span));
                    return Some(ResolvedType::Unknown);
                }
                if let Some(args) = type_args {
                    let subst = type_param_subst_map(&class.type_params, args);
                    return Some(substitute_resolved_type(&info.ty, &subst));
                }
                info.ty.clone()
            }
            TypeInfo::Enum(enum_info) => {
                if enum_info.variants.contains(&field.to_string()) {
                    return Some(if let Some(args) = type_args {
                        ResolvedType::Generic(type_name.to_string(), args.to_vec())
                    } else {
                        ResolvedType::Named(type_name.to_string())
                    });
                }
                if field == "from_value"
                    && let Some(value_enum) = &enum_info.value_enum
                {
                    return Some(ResolvedType::Function(
                        vec![CallableParam::positional(value_enum.value_type.resolved_type())],
                        Box::new(Self::value_enum_from_value_return_type(type_name)),
                    ));
                }
                return None;
            }
            TypeInfo::Newtype(nt) if nt.is_rusttype => {
                if let ResolvedType::RustPath(path) = &nt.underlying {
                    if let Some(sig) = self.rust_associated_function_signature(path, field) {
                        return Some(self.resolved_function_type_from_rust_sig(&sig, false));
                    }
                    if let Some(meta) = self.rust_item_metadata_for_path(path)
                        && let RustItemKind::Type(info) = &meta.kind
                        && let Some(rust_field) = info.fields.iter().find(|f| f.name == field)
                    {
                        return Some(self.resolved_type_from_rust_shape(&rust_field.type_shape));
                    }
                }
                return None;
            }
            TypeInfo::Newtype(nt) if field == conventions::NEWTYPE_TUPLE_FIELD => {
                return Some(nt.underlying.clone());
            }
            _ => return None,
        };

        Some(field_info)
    }

    /// Resolve and validate a method call on a rust-inspect-backed path.
    ///
    /// Returns:
    /// - `None` when rust-inspect data is unavailable (caller should preserve permissive fallback behavior)
    /// - `Some(ty)` when metadata exists and the call was resolved (or diagnosed as invalid)
    fn resolve_rust_path_method_call(
        &mut self,
        rust_path: &str,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver_span: Span,
        span: Span,
    ) -> Option<ResolvedType> {
        let preserves_lookup_arg_shape = RustCollectionFamily::for_canonical_path(rust_path)
            .is_some_and(|family| family.preserves_lookup_arg_shape(method));
        if preserves_lookup_arg_shape {
            self.type_info.record_regular_method_arg_shape(receiver_span, method);
        }
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        match &metadata.kind {
            RustItemKind::Type(_) => {
                let Some(sig) = self.rust_method_signature(rust_path, method) else {
                    // Metadata only covers inherent methods; trait-provided or extension methods are
                    // not yet extracted. Stay permissive rather than false-positiving on valid calls.
                    return Some(ResolvedType::Unknown);
                };
                let callable_display = format!("rust::{rust_path}.{method}");
                Some(self.validate_rust_method_call(
                    callable_display.as_str(),
                    &sig,
                    args,
                    arg_types,
                    preserves_lookup_arg_shape,
                    span,
                ))
            }
            RustItemKind::Unsupported { description } => {
                self.errors.push(errors::rust_item_shape_not_supported(
                    rust_path,
                    description.as_str(),
                    span,
                ));
                Some(ResolvedType::Unknown)
            }
            // Function, Trait, Module, Constant: metadata is incomplete for method surfaces.
            // Stay permissive and let rustc catch genuine errors at compile time.
            _ => Some(ResolvedType::Unknown),
        }
    }

    /// Check if a type is copyable.
    pub(in crate::frontend::typechecker) fn is_copy_type(&self, ty: &ResolvedType) -> bool {
        matches!(
            ty,
            ResolvedType::Int
                | ResolvedType::Float
                | ResolvedType::Bool
                | ResolvedType::Unit
                | ResolvedType::Ref(_)
                | ResolvedType::RefMut(_)
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
            ResolvedType::Ref(_) | ResolvedType::RefMut(_) | ResolvedType::Function(_, _) | ResolvedType::SelfType => {
                true
            }
            ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer => false,
            // RFC 041: provenance is known, but Incan does not yet query Rust for `Copy`/`Clone`; do not assume.
            ResolvedType::RustPath(_) => false,
            ResolvedType::Unknown => true,
        }
    }

    /// [`ResolvedType::SelfType`] in a trait method signature means the receiver type for this call site.
    fn concrete_type_for_trait_self(&self, receiver: &ResolvedType) -> ResolvedType {
        match receiver {
            ResolvedType::Generic(name, args) => ResolvedType::Generic(name.clone(), args.clone()),
            ResolvedType::Named(name) => {
                let n_params = self
                    .lookup_semantic_type_info(name)
                    .map(|info| match info {
                        TypeInfo::Model(m) => m.type_params.len(),
                        TypeInfo::Class(c) => c.type_params.len(),
                        TypeInfo::Enum(e) => e.type_params.len(),
                        TypeInfo::Newtype(n) => n.type_params.len(),
                        _ => 0,
                    })
                    .unwrap_or(0);
                if n_params > 0 {
                    ResolvedType::Generic(name.clone(), vec![ResolvedType::Unknown; n_params])
                } else {
                    receiver.clone()
                }
            }
            _ => receiver.clone(),
        }
    }

    /// Replace every [`ResolvedType::SelfType`] in `ty` using the **call-site** receiver.
    ///
    /// For method **bodies**, `TypeChecker::concretize_self_type_in_annotation` in `check_decl.rs` maps `Self` to the
    /// owner's `self_ty` while checking the implementation. At a **call site**, `Self` means the instantiated
    /// receiver (for example `DataFrame[Order]` when calling on `x: DataFrame[Order]`).
    fn substitute_self_in_resolved_type(&self, ty: ResolvedType, receiver: &ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::SelfType => self.concrete_type_for_trait_self(receiver),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name,
                args.into_iter()
                    .map(|a| self.substitute_self_in_resolved_type(a, receiver))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|a| self.substitute_self_in_resolved_type(a, receiver))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::FrozenDict(k, v) => ResolvedType::FrozenDict(
                Box::new(self.substitute_self_in_resolved_type(*k, receiver)),
                Box::new(self.substitute_self_in_resolved_type(*v, receiver)),
            ),
            ResolvedType::Ref(inner) => {
                ResolvedType::Ref(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            other => other,
        }
    }

    /// Build formal parameter types and return type for a method call, replacing [`ResolvedType::SelfType`] with the
    /// instantiated receiver.
    ///
    /// [`MethodInfo`] in the symbol table stores `Self` literally. At a call site, `Self` means the concrete receiver
    /// type (for example the `T` of `List[T]` when calling on a `List[int]` value). Both inherent and trait dispatch
    /// use this before [`Self::validate_method_call_args`] and before RFC 054 explicit type-argument substitution and
    /// inference (`check_generic_method_call` in the `calls` submodule).
    ///
    /// Returns `(params, return_type)` with `Self` resolved; method-level type parameters may still appear as
    /// [`ResolvedType::TypeVar`] until inference completes.
    pub(in crate::frontend::typechecker::check_expr) fn method_types_substituting_call_site_self(
        &self,
        method_info: &MethodInfo,
        receiver_ty: &ResolvedType,
    ) -> (Vec<CallableParam>, ResolvedType) {
        let params = method_info
            .params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: self.substitute_self_in_resolved_type(param.ty.clone(), receiver_ty),
                kind: param.kind,
                has_default: param.has_default,
            })
            .collect();
        let return_type = self.substitute_self_in_resolved_type(method_info.return_type.clone(), receiver_ty);
        (params, return_type)
    }

    /// Resolve a method on a type's own methods or trait-adopted methods.
    ///
    /// Inherent methods and trait-provided methods both substitute `Self` in formal parameters and the return type
    /// using the call-site receiver so generic carriers typecheck consistently (#237).
    #[allow(clippy::too_many_arguments)]
    fn resolve_named_method(
        &mut self,
        methods: &std::collections::HashMap<String, MethodInfo>,
        method_overloads: Option<&std::collections::HashMap<String, Vec<MethodInfo>>>,
        trait_adoptions: Option<&[TypeBoundInfo]>,
        method: &str,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        if let Some(overloads) = method_overloads.and_then(|overloads| overloads.get(method)) {
            return self.resolve_method_overload(
                method,
                overloads,
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
                expected_return_ty,
            );
        }
        if let Some(method_info) = methods.get(method) {
            return Some(self.check_generic_method_call(
                method,
                method_info.clone(),
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
            ));
        }
        if let Some(trait_adoptions) = trait_adoptions {
            let mut candidates = Vec::new();
            for adoption in trait_adoptions {
                if let Some(method_info) =
                    self.trait_method_info_resolved_for_adoption(adoption, method, call_site_span)
                {
                    candidates.push(method_info);
                }
            }
            if !candidates.is_empty() {
                return self.resolve_method_overload(
                    method,
                    &candidates,
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                    expected_return_ty,
                );
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    /// Select a same-name method overload using call arguments and an optional expected return type.
    fn resolve_method_overload(
        &mut self,
        method: &str,
        candidates: &[MethodInfo],
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut viable: Vec<MethodInfo> = candidates
            .iter()
            .filter(|candidate| {
                self.method_candidate_matches_call(candidate, args, arg_types, receiver_ty, expected_return_ty)
            })
            .cloned()
            .collect();

        if viable.is_empty() && expected_return_ty.is_some() {
            viable = candidates
                .iter()
                .filter(|candidate| self.method_candidate_matches_call(candidate, args, arg_types, receiver_ty, None))
                .cloned()
                .collect();
        }

        match viable.as_slice() {
            [method_info] => Some(self.check_generic_method_call(
                method,
                method_info.clone(),
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
            )),
            [] => candidates.first().map(|method_info| {
                self.check_generic_method_call(
                    method,
                    method_info.clone(),
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                )
            }),
            _ => {
                self.errors
                    .push(errors::ambiguous_trait_method_call(method, call_site_span));
                Some(ResolvedType::Unknown)
            }
        }
    }

    /// Return whether one method candidate is compatible with the supplied arguments and expected result type.
    fn method_candidate_matches_call(
        &self,
        candidate: &MethodInfo,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> bool {
        let (params, return_type) = self.method_types_substituting_call_site_self(candidate, receiver_ty);
        if let Some(expected) = expected_return_ty
            && !self.types_compatible(&return_type, expected)
        {
            return false;
        }
        let normal_params: Vec<&CallableParam> =
            params.iter().filter(|param| param.kind == ParamKind::Normal).collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);
        let mut normal_bound = vec![false; normal_params.len()];
        let mut positional_index = 0usize;
        let mut named_seen = std::collections::HashSet::new();
        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            match arg {
                CallArg::Positional(_) => {
                    if let Some(param) = normal_params.get(positional_index) {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return false;
                        }
                        normal_bound[positional_index] = true;
                        positional_index += 1;
                    } else if let Some(param) = rest_positional {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                CallArg::Named(name, _) => {
                    if !named_seen.insert(name.as_str()) {
                        return false;
                    }
                    if let Some((normal_idx, param)) = normal_params
                        .iter()
                        .enumerate()
                        .find(|(_, param)| param.name() == Some(name.as_str()))
                    {
                        if normal_bound[normal_idx] {
                            return false;
                        }
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return false;
                        }
                        normal_bound[normal_idx] = true;
                    } else if let Some(param) = rest_keyword {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => return true,
            }
        }
        normal_params
            .iter()
            .zip(normal_bound.iter())
            .all(|(param, bound)| *bound || param.has_default)
    }

    #[allow(clippy::too_many_arguments)]
    /// Resolve a method call on a generic placeholder using the active `T with Trait[...]` bound stack.
    fn resolve_generic_placeholder_method(
        &mut self,
        placeholder_name: &str,
        method: &str,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut active_bounds = Vec::new();
        for frame in self.current_type_param_bound_details.iter().rev() {
            if let Some(bounds) = frame.get(placeholder_name) {
                active_bounds = bounds.clone();
                break;
            }
        }
        let mut candidates = Vec::new();
        for bound in &active_bounds {
            if let Some(method_info) = self.trait_method_info_resolved_for_adoption(bound, method, call_site_span) {
                candidates.push(method_info);
            }
        }
        if candidates.is_empty() {
            return None;
        }
        self.resolve_method_overload(
            method,
            &candidates,
            explicit_type_args,
            args,
            arg_types,
            call_site_span,
            receiver_ty,
            expected_return_ty,
        )
    }

    /// Resolve a newtype/rusttype rebound method alias to its target method name.
    fn resolve_newtype_method_name<'a>(&self, newtype: &'a NewtypeInfo, method: &'a str) -> &'a str {
        newtype
            .method_rebindings
            .get(method)
            .map(String::as_str)
            .unwrap_or(method)
    }

    /// When a `rusttype` method resolves against an underlying Rust method with metadata, compare the Rust return type
    /// against the Incan-declared return type. If they differ in a coercible way (e.g. `&str` vs `String`), record a
    /// return coercion for `span` so lowering can wrap the call in `InteropCoerce`.
    ///
    /// This runs unconditionally but is a no-op when the metadata cache is empty (i.e. when the `rust-inspect` feature
    /// is not enabled or no workspace was loaded).
    fn maybe_record_rusttype_return_coercion(
        &mut self,
        nt: &NewtypeInfo,
        method: &str,
        incan_ret: &ResolvedType,
        span: Span,
    ) {
        // Only relevant for rusttypes backed by a known Rust path.
        let ResolvedType::RustPath(underlying_path) = &nt.underlying else {
            return;
        };
        // Consult metadata for the actual Rust return type.
        let Some(sig) = self.rust_method_signature(underlying_path, method) else {
            return;
        };
        let normalized = sig.return_type.replace(' ', "");
        // ---- `&str` → `String` (Incan `str` = Rust `String`) ----
        let is_borrowed_str = normalized == "&str" || (normalized.starts_with("&'") && normalized.ends_with("str"));
        if is_borrowed_str && matches!(incan_ret, ResolvedType::Str) {
            self.type_info.rust_return_coercions.insert(
                (span.start, span.end),
                crate::frontend::typechecker::RustArgCoercionInfo {
                    rust_target_type: "String".to_string(),
                    target_type: ResolvedType::Str,
                    kind: crate::frontend::typechecker::RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
                },
            );
            return;
        }
        // ---- `&[u8]` → `Vec<u8>` (Incan `bytes` = Rust `Vec<u8>`) ----
        let is_borrowed_bytes = normalized == "&[u8]" || (normalized.starts_with("&'") && normalized.ends_with("[u8]"));
        if is_borrowed_bytes && matches!(incan_ret, ResolvedType::Bytes) {
            self.type_info.rust_return_coercions.insert(
                (span.start, span.end),
                crate::frontend::typechecker::RustArgCoercionInfo {
                    rust_target_type: "Vec<u8>".to_string(),
                    target_type: ResolvedType::Bytes,
                    kind: crate::frontend::typechecker::RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
                },
            );
        }
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
                    if let Some(idx) = self.resolve_tuple_index(raw_idx.value, elems.len(), span) {
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
                if let Some(idx) = self.resolve_tuple_index(raw_idx.value, elems.len(), span) {
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
        let base_ty = self.check_expr(base);

        // Imported modules use symbol-driven metadata resolution.
        if let Some((module_name, module_path)) = self.imported_module_for_expr(base) {
            if let Some(info) = self.resolve_imported_module_constant_member(&module_path, field) {
                return info.ty;
            }
            if let Some(info) = self.resolve_imported_module_function_member(&module_path, field) {
                let callable = format!("{module_name}.{field}");
                if !info.type_params.is_empty() {
                    self.errors
                        .push(errors::generic_function_reference(callable.as_str(), span));
                    return ResolvedType::Unknown;
                }
                return Self::function_info_to_resolved_function_type(&info);
            }
        }

        // Be permissive for unknown receivers: allow field access and continue typechecking.
        if matches!(base_ty, ResolvedType::Unknown) {
            return ResolvedType::Unknown;
        }

        if let ResolvedType::RustPath(path) = &base_ty {
            let Some(meta) = self.rust_item_metadata_for_path(path) else {
                // Metadata backend disabled/unavailable: preserve permissive RFC 005 behavior.
                return ResolvedType::Unknown;
            };
            match &meta.kind {
                RustItemKind::Module(module) => {
                    if let Some(child) = module.children.iter().find(|c| c.name == field) {
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
                    // Module membership from rust-analyzer is authoritative.
                    self.errors
                        .push(errors::missing_field(rust_receiver_display(path).as_str(), field, span));
                    return ResolvedType::Unknown;
                }
                RustItemKind::Type(_) => {
                    if let Some(sig) = self.rust_associated_function_signature(path, field) {
                        return self.resolved_function_type_from_rust_sig(&sig, false);
                    }
                    if let RustItemKind::Type(info) = &meta.kind
                        && let Some(rust_field) = info.fields.iter().find(|f| f.name == field)
                    {
                        return self.resolved_type_from_rust_shape(&rust_field.type_shape);
                    }
                    // Metadata may still be missing constants, type aliases, trait-provided items, or private fields.
                    // Stay permissive when no exact field surface is available.
                    return ResolvedType::Unknown;
                }
                RustItemKind::Unsupported { description } => {
                    self.errors
                        .push(errors::rust_item_shape_not_supported(path, description.as_str(), span));
                    return ResolvedType::Unknown;
                }
                // Function, Trait, Constant: metadata coverage is incomplete, stay permissive.
                _ => return ResolvedType::Unknown,
            };
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
                    if let Some(field_ty) = checker.resolve_nominal_field_type(type_name, None, field, span) {
                        return field_ty;
                    }
                    checker.errors.push(errors::missing_field(type_name, field, span));
                    ResolvedType::Unknown
                }
                ResolvedType::Generic(type_name, type_args) => {
                    if let Some(field_ty) =
                        checker.resolve_nominal_field_type(type_name, Some(type_args.as_slice()), field, span)
                    {
                        return field_ty;
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
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.check_method_call_with_expected(base, method, type_args, args, span, None)
    }

    /// Type-check a method call with an optional expected result type for overload disambiguation.
    pub(in crate::frontend::typechecker::check_expr) fn check_method_call_with_expected(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let base_ty = self.check_expr(base);

        // If the receiver type is Unknown, be permissive and do not error on methods.
        if matches!(base_ty, ResolvedType::Unknown) {
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }

        if let Some((module_name, module_path)) = self.imported_module_for_expr(base) {
            if let Some(info) = self.resolve_imported_module_function_member(&module_path, method) {
                let callable = format!("{module_name}.{method}");
                return self.validate_stdlib_module_function_call(callable.as_str(), &info, type_args, args, span);
            }
            self.errors
                .push(errors::missing_method(module_name.as_str(), method, span));
            return ResolvedType::Unknown;
        }

        // Collect arg types for method-specific validation.
        let arg_types: Vec<ResolvedType> = args
            .iter()
            .map(|arg| match arg {
                CallArg::Positional(e)
                | CallArg::Named(_, e)
                | CallArg::PositionalUnpack(e)
                | CallArg::KeywordUnpack(e) => self.check_expr(e),
            })
            .collect();

        if let Some(path) = self.rust_canonical_path_for_receiver_type(&base_ty) {
            let Some(ret) = self.resolve_rust_path_method_call(&path, method, args, &arg_types, base.span, span) else {
                // Metadata backend disabled/unavailable: preserve permissive RFC 005 behavior.
                return ResolvedType::Unknown;
            };
            return ret;
        }
        // Trait default methods typecheck against `Self`, so be permissive here too.
        if matches!(base_ty, ResolvedType::SelfType) {
            return ResolvedType::Unknown;
        }

        if self.nominal_type_supports_reflection_magic(&base_ty, method)
            && let Some(ret) = self.reflection_magic_method_return_type(method)
        {
            return ret;
        }

        let base_is_type_name = self.is_enum_type_name_expr(base);
        if let ResolvedType::Named(enum_name) = &base_ty
            && let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name)
            && let Some(value_enum) = enum_info.value_enum.clone()
            && let Some(ret) = self.check_value_enum_generated_method_call(ValueEnumGeneratedCall {
                enum_name,
                value_enum: &value_enum,
                method,
                base_is_type_name,
                type_args,
                args,
                arg_types: &arg_types,
                span,
            })
        {
            return ret;
        }

        // Treat Enum.Variant(...) method-style calls as variant constructors
        if let ResolvedType::Named(enum_name) = &base_ty
            && let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name)
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
            && self.lookup_semantic_type_info(name).is_none()
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
                    if let ResolvedType::Ref(t) | ResolvedType::RefMut(t) = inner {
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
                        M::Extend => {
                            let other_list_ty = list_ty(elem.clone());
                            if let Some(arg0) = arg_types.first()
                                && !self.types_compatible(arg0, &other_list_ty)
                            {
                                self.errors.push(errors::type_mismatch(
                                    &other_list_ty.to_string(),
                                    &arg0.to_string(),
                                    span,
                                ));
                            }
                            if !self.is_copy_type(&elem) && !self.is_clone_type(&elem) {
                                self.errors
                                    .push(errors::list_extend_requires_clone(&elem.to_string(), span));
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
            && let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned()
        {
            match type_info {
                TypeInfo::Model(model) => {
                    let trait_adoptions = self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                    if let Some(ret) = self.resolve_named_method(
                        &model.methods,
                        Some(&model.method_overloads),
                        Some(&trait_adoptions),
                        method,
                        type_args,
                        args,
                        &arg_types,
                        span,
                        &base_ty,
                        expected_return_ty,
                    ) {
                        return ret;
                    }
                }
                TypeInfo::Class(class) => {
                    let trait_adoptions = self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                    if let Some(ret) = self.resolve_named_method(
                        &class.methods,
                        Some(&class.method_overloads),
                        Some(&trait_adoptions),
                        method,
                        type_args,
                        args,
                        &arg_types,
                        span,
                        &base_ty,
                        expected_return_ty,
                    ) {
                        return ret;
                    }
                }
                TypeInfo::Enum(en) => {
                    let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                    if let Some(ret) = self.resolve_named_method(
                        &en.methods,
                        Some(&en.method_overloads),
                        Some(&trait_adoptions),
                        method,
                        type_args,
                        args,
                        &arg_types,
                        span,
                        &base_ty,
                        expected_return_ty,
                    ) {
                        return ret;
                    }
                }
                TypeInfo::Newtype(newtype) => {
                    let resolved_method = self.resolve_newtype_method_name(&newtype, method);
                    if let Some(ret) = self.resolve_named_method(
                        &newtype.methods,
                        None,
                        None,
                        resolved_method,
                        type_args,
                        args,
                        &arg_types,
                        span,
                        &base_ty,
                        expected_return_ty,
                    ) {
                        if newtype.is_rusttype {
                            self.maybe_record_rusttype_return_coercion(&newtype, resolved_method, &ret, span);
                        }
                        return ret;
                    }
                    if newtype.is_rusttype
                        && let ResolvedType::RustPath(path) = &newtype.underlying
                        && let Some(ret) =
                            self.resolve_rust_path_method_call(path, resolved_method, args, &arg_types, base.span, span)
                    {
                        return ret;
                    }
                }
                _ => {}
            }
        }

        // Named types: look up methods from the type definition.
        // If the symbol doesn't exist or isn't a type (e.g., Module/RustItem placeholder), treat it as external and
        // be permissive.
        if let ResolvedType::Named(type_name) = &base_ty {
            match self.lookup_semantic_type_info(type_name).cloned() {
                None => {
                    // Symbol not found or not a Type - treat as external, be permissive.
                    return ResolvedType::Unknown;
                }
                Some(type_info) => match type_info {
                    TypeInfo::Model(model) => {
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                        if let Some(ret) = self.resolve_named_method(
                            &model.methods,
                            Some(&model.method_overloads),
                            Some(&trait_adoptions),
                            method,
                            type_args,
                            args,
                            &arg_types,
                            span,
                            &base_ty,
                            expected_return_ty,
                        ) {
                            return ret;
                        }
                    }
                    TypeInfo::Class(class) => {
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                        if let Some(ret) = self.resolve_named_method(
                            &class.methods,
                            Some(&class.method_overloads),
                            Some(&trait_adoptions),
                            method,
                            type_args,
                            args,
                            &arg_types,
                            span,
                            &base_ty,
                            expected_return_ty,
                        ) {
                            return ret;
                        }
                    }
                    TypeInfo::Enum(en) => {
                        if enum_helpers::from_str(method) == Some(enum_helpers::EnumHelperId::Message) {
                            return ResolvedType::Str;
                        }
                        let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                        if let Some(ret) = self.resolve_named_method(
                            &en.methods,
                            Some(&en.method_overloads),
                            Some(&trait_adoptions),
                            method,
                            type_args,
                            args,
                            &arg_types,
                            span,
                            &base_ty,
                            expected_return_ty,
                        ) {
                            return ret;
                        }
                    }
                    TypeInfo::Newtype(nt) => {
                        let resolved_method = self.resolve_newtype_method_name(&nt, method);
                        if let Some(ret) = self.resolve_named_method(
                            &nt.methods,
                            None,
                            None,
                            resolved_method,
                            type_args,
                            args,
                            &arg_types,
                            span,
                            &base_ty,
                            expected_return_ty,
                        ) {
                            // When the method body is abstract and the underlying Rust type is known,
                            // check whether the actual Rust return type needs a coercion (e.g. &str → String).
                            if nt.is_rusttype {
                                self.maybe_record_rusttype_return_coercion(&nt, resolved_method, &ret, span);
                            }
                            return ret;
                        }
                        if nt.is_rusttype
                            && let ResolvedType::RustPath(path) = &nt.underlying
                            && let Some(ret) = self.resolve_rust_path_method_call(
                                path,
                                resolved_method,
                                args,
                                &arg_types,
                                base.span,
                                span,
                            )
                        {
                            return ret;
                        }
                    }
                    _ => {}
                },
            }
        }

        // Reflection magic helpers are modeled explicitly above and should error on unsupported receivers rather than
        // silently degrading to Unknown. Keep the older permissive fallback only for the remaining backend-only magic.
        if let Some(id) = magic_methods::from_str(method)
            && !matches!(
                id,
                magic_methods::MagicMethodId::ClassName | magic_methods::MagicMethodId::Fields
            )
        {
            return ResolvedType::Unknown;
        }

        // For common external generic types (interop/runtime-provided) that we don't model in the checker, be
        // permissive and do not error on unknown methods.
        if let ResolvedType::Generic(name, _args) = &base_ty
            && self.lookup_semantic_type_info(name).is_none()
        {
            return ResolvedType::Unknown;
        }

        // RFC 023: Method calls on generic type variables are permissive.
        //
        // The Rust backend infers the required trait bounds (e.g., `x.clone()` → `T: Clone`).
        // At the Incan typechecker level we allow the call and return the same type variable.
        if self.is_generic_placeholder_type(&base_ty) {
            if let Some(placeholder_name) = self.generic_placeholder_name(&base_ty).map(str::to_string)
                && let Some(ret) = self.resolve_generic_placeholder_method(
                    &placeholder_name,
                    method,
                    type_args,
                    args,
                    &arg_types,
                    span,
                    &base_ty,
                    expected_return_ty,
                )
            {
                return ret;
            }
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
