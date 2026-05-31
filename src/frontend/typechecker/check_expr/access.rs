//! Check indexing, slicing, field access, and method calls.
//!
//! These helpers validate access patterns like `xs[i]`, `xs[a:b]`, `obj.field`, and `obj.method(...)`, emitting
//! diagnostics for missing fields/methods and incompatible uses.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::*;
use crate::frontend::typechecker::IdentKind;
use crate::frontend::typechecker::helpers::{
    collection_name, collection_type_id, generator_ty, is_frozen_bytes, is_frozen_str, is_intlike_for_index, list_ty,
    option_ty, string_method_return,
};
use crate::frontend::typechecker::type_info::{RustMethodTraitImportUse, RustTraitImportInfo};
use incan_core::interop::{
    RustCollectionFamily, RustFieldInfo, RustFunctionSig, RustItemKind, metadata_free_method_signature,
};
use incan_core::lang::magic_methods;
use incan_core::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::surface::types::{SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME, SEMAPHORE_PERMIT_TYPE_NAME, SurfaceTypeId};
use incan_core::lang::surface::{
    dict_methods, float_methods, frozen_bytes_methods, frozen_dict_methods, frozen_list_methods, frozen_set_methods,
    iterator_methods, list_methods, result_methods, set_methods,
};
use incan_core::lang::traits::{self as core_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics::NumericFamily;
use incan_core::lang::{conventions, stdlib};
use incan_core::lang::{enum_helpers, surface::option_methods};
use quote::ToTokens;
use syn::{GenericArgument, PathArguments, ReturnType, Type as SynType, TypeParamBound};

use super::TypeChecker;

#[derive(Debug, Clone)]
struct MethodCandidate {
    info: MethodInfo,
    dispatch: Option<crate::frontend::typechecker::ResolvedMethodDispatch>,
}

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

#[derive(Debug, Clone)]
struct RustCallableAliasParam {
    rust_display: String,
    resolved_ty: ResolvedType,
}

#[derive(Debug, Clone)]
struct RustCallableAliasSignature {
    params: Vec<RustCallableAliasParam>,
    return_ty: ResolvedType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericResizeMethodPolicy {
    Lossless,
    Try,
    Wrapping,
    Saturating,
}

/// Diagnostic label for a Rust path receiver in type errors (`rust::{path}`).
fn rust_receiver_display(path: &str) -> String {
    format!("rust::{path}")
}

impl TypeChecker {
    /// Resolve a source-facing Rust field spelling to the metadata field it names.
    ///
    /// Rust raw identifier fields should be written with the Rust source name at Incan field-use sites. For example, a
    /// Rust field declared as `r#type` is accessed as `obj.type` and constructed with `TypeName(type=...)`; emission
    /// rawifies the keyword identifier back to `r#type`. An ordinary Rust field declared as `type_` remains available
    /// only as `obj.type_`.
    pub(in crate::frontend::typechecker::check_expr) fn rust_field_for_source_name<'a>(
        fields: &'a [RustFieldInfo],
        source_name: &str,
    ) -> Option<&'a RustFieldInfo> {
        fields.iter().find(|field| field.name == source_name)
    }

    /// Return the target display for a Rust type alias when the expected destination type names one.
    fn rust_callable_alias_target_display(&self, expected_ty: &ResolvedType) -> Option<String> {
        let ResolvedType::RustPath(path) = expected_ty else {
            return None;
        };
        self.rust_callable_alias_target_display_for_path(path, &mut std::collections::HashSet::new())
    }

    /// Follow Rust type-alias chains until they expose a callable trait object target.
    ///
    /// This is intentionally metadata-driven rather than crate-specific. DataFusion's
    /// `ScalarFunctionImplementation -> Arc<dyn Fn(...)>` chain is one motivating surface, but the compiler must not
    /// special-case DataFusion or require regression tests to compile that heavyweight crate.
    fn rust_callable_alias_target_display_for_path(
        &self,
        path: &str,
        seen: &mut std::collections::HashSet<String>,
    ) -> Option<String> {
        let canonical_path = Self::normalize_rust_namespace_path(path).to_string();
        if !seen.insert(canonical_path.clone()) {
            return None;
        }
        if let Some(metadata) = self.rust_item_metadata_for_path(path)
            && let RustItemKind::Type(type_info) = &metadata.kind
            && let Some(target) = type_info.alias_target.as_ref()
        {
            let display = self.rust_display_for_owner_path(target, canonical_path.as_str());
            if Self::rust_display_has_callable_fn_bound(display.as_str()) {
                return Some(display);
            }
            let (target_base, _) = self.rust_path_base_and_args(display.as_str());
            if target_base != canonical_path
                && let Some(expanded) = self.rust_callable_alias_target_display_for_path(target_base.as_str(), seen)
            {
                return Some(expanded);
            }
            return None;
        }
        Some(self.rust_display_for_owner_path(path, path))
            .filter(|display| Self::rust_display_has_callable_fn_bound(display.as_str()))
    }

    /// Parse a Rust callable alias target such as `Arc<dyn Fn(&[T]) -> Result<U, E> + Send + Sync>`.
    fn rust_callable_alias_signature(&self, expected_ty: &ResolvedType) -> Option<RustCallableAliasSignature> {
        let target_display = self.rust_callable_alias_target_display(expected_ty)?;
        let ty = syn::parse_str::<SynType>(&target_display).ok()?;
        let fn_bound = Self::rust_callable_fn_bound(&ty)?;

        let params = fn_bound
            .inputs
            .iter()
            .map(|input| {
                let rust_display = Self::compact_rust_display(&input.to_token_stream().to_string());
                RustCallableAliasParam {
                    resolved_ty: self.resolved_param_type_from_rust_display(&rust_display),
                    rust_display,
                }
            })
            .collect::<Vec<_>>();
        let return_ty = match &fn_bound.output {
            ReturnType::Default => ResolvedType::Unit,
            ReturnType::Type(_, ty) => {
                let rust_display = Self::compact_rust_display(&ty.to_token_stream().to_string());
                self.resolved_type_from_rust_display(&rust_display)
            }
        };
        Some(RustCallableAliasSignature { params, return_ty })
    }

    /// Return whether a Rust display type contains a callable trait-object target.
    fn rust_display_has_callable_fn_bound(display: &str) -> bool {
        let Ok(ty) = syn::parse_str::<SynType>(display) else {
            return false;
        };
        Self::rust_callable_fn_bound(&ty).is_some()
    }

    /// Return the `Fn(...) -> ...` bound carried by a Rust callable trait-object target.
    fn rust_callable_fn_bound(ty: &SynType) -> Option<&syn::ParenthesizedGenericArguments> {
        let trait_object = Self::rust_callable_trait_object(ty)?;
        trait_object.bounds.iter().find_map(|bound| {
            let TypeParamBound::Trait(trait_bound) = bound else {
                return None;
            };
            let segment = trait_bound.path.segments.last()?;
            if !matches!(segment.ident.to_string().as_str(), "Fn" | "FnMut" | "FnOnce") {
                return None;
            }
            let PathArguments::Parenthesized(args) = &segment.arguments else {
                return None;
            };
            Some(args)
        })
    }

    /// Find the Rust trait-object type wrapped by a callable alias target.
    fn rust_callable_trait_object(ty: &SynType) -> Option<&syn::TypeTraitObject> {
        match ty {
            SynType::TraitObject(trait_object) => Some(trait_object),
            SynType::Group(group) => Self::rust_callable_trait_object(&group.elem),
            SynType::Paren(paren) => Self::rust_callable_trait_object(&paren.elem),
            SynType::Path(path) => {
                let segment = path.path.segments.last()?;
                let PathArguments::AngleBracketed(args) = &segment.arguments else {
                    return None;
                };
                args.args.iter().find_map(|arg| match arg {
                    GenericArgument::Type(inner) => Self::rust_callable_trait_object(inner),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Check a closure expression against a Rust callable alias.
    fn check_closure_with_rust_callable_alias(
        &mut self,
        expr: &Spanned<Expr>,
        signature: &RustCallableAliasSignature,
    ) -> ResolvedType {
        let Expr::Closure(params, body) = &expr.node else {
            return self.check_expr(expr);
        };
        if params.len() != signature.params.len() {
            self.errors.push(errors::builtin_arity(
                "closure",
                signature.params.len(),
                params.len(),
                expr.span,
            ));
            return ResolvedType::Unknown;
        }

        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types = params
            .iter()
            .zip(signature.params.iter())
            .map(|(param, expected)| {
                let ty = expected.resolved_ty.clone();
                self.symbols.define(Symbol {
                    name: param.node.name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: param.span,
                    scope: 0,
                });
                CallableParam::named(param.node.name.clone(), ty, param.node.kind)
            })
            .collect::<Vec<_>>();

        let return_ty = self.check_expr_with_expected(body, Some(&signature.return_ty));
        if !matches!(return_ty, ResolvedType::Unknown) && !self.types_compatible(&return_ty, &signature.return_ty) {
            self.errors.push(errors::type_mismatch(
                &signature.return_ty.to_string(),
                &return_ty.to_string(),
                body.span,
            ));
        }

        self.current_return_error_type = prev_return_error_type;
        self.in_async_body = prev_in_async_body;
        self.symbols.exit_scope();

        self.type_info.rust.closure_param_type_displays.insert(
            (expr.span.start, expr.span.end),
            signature
                .params
                .iter()
                .map(|param| param.rust_display.clone())
                .collect(),
        );

        let closure_ty = ResolvedType::Function(param_types, Box::new(signature.return_ty.clone()));
        self.record_expr_type(expr.span, closure_ty.clone());
        closure_ty
    }

    /// Check a method argument against a Rust callable alias.
    fn check_method_arg_with_rust_callable_alias(
        &mut self,
        arg: &CallArg,
        signature: Option<&RustCallableAliasSignature>,
    ) -> ResolvedType {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => {
                if let Some(signature) = signature
                    && matches!(expr.node, Expr::Closure(_, _))
                {
                    return self.check_closure_with_rust_callable_alias(expr, signature);
                }
                self.check_expr(expr)
            }
        }
    }

    /// Return whether `method` names an RFC 070 `Result[T, E]` combinator.
    fn result_combinator_name(method: &str) -> bool {
        matches!(
            result_methods::from_str(method),
            Some(
                result_methods::ResultMethodId::Map
                    | result_methods::ResultMethodId::MapErr
                    | result_methods::ResultMethodId::AndThen
                    | result_methods::ResultMethodId::OrElse
                    | result_methods::ResultMethodId::Inspect
                    | result_methods::ResultMethodId::InspectErr
            )
        )
    }

    /// Resolve a callable function or callable object to its parameter and return types.
    fn callable_signature_for_value_type(
        &mut self,
        ty: &ResolvedType,
        span: Span,
    ) -> Option<(Vec<CallableParam>, ResolvedType)> {
        match ty {
            ResolvedType::Function(params, ret) => Some((params.clone(), ret.as_ref().clone())),
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => {
                let type_info = self.lookup_semantic_type_info(name).cloned()?;
                let methods = match type_info {
                    TypeInfo::Model(model) => model.methods,
                    TypeInfo::Class(class) => class.methods,
                    TypeInfo::Enum(en) => en.methods,
                    TypeInfo::Newtype(newtype) => newtype.methods,
                    _ => return None,
                };
                let Some(call) = methods.get("__call__") else {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__call__", span));
                    return None;
                };
                Some(self.method_types_substituting_call_site_self(call, ty))
            }
            ResolvedType::Unknown => Some((Vec::new(), ResolvedType::Unknown)),
            _ => {
                self.errors
                    .push(errors::missing_method(&ty.to_string(), "__call__", span));
                None
            }
        }
    }

    /// Validate the callback passed to one `Result[T, E]` combinator and return its output type.
    fn validate_result_combinator_callback(
        &mut self,
        _method: &str,
        callback_ty: &ResolvedType,
        input_ty: &ResolvedType,
        expected_ret: Option<&ResolvedType>,
        span: Span,
    ) -> ResolvedType {
        let Some((params, ret)) = self.callable_signature_for_value_type(callback_ty, span) else {
            return ResolvedType::Unknown;
        };
        if params.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some(param) = params.first()
            && !self.types_compatible(input_ty, &param.ty)
        {
            self.errors.push(errors::type_mismatch(
                &param.ty.to_string(),
                &input_ty.to_string(),
                span,
            ));
        }
        if let Some(expected) = expected_ret
            && !self.types_compatible(&ret, expected)
        {
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &ret.to_string(), span));
        }
        ret
    }

    /// Typecheck one RFC 070 `Result[T, E]` combinator method call.
    fn check_result_combinator_method(
        &mut self,
        ok_ty: ResolvedType,
        err_ty: ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> ResolvedType {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }
        let Some(callback_ty) = arg_types.first() else {
            return ResolvedType::Unknown;
        };
        let Some(method_id) = result_methods::from_str(method) else {
            return ResolvedType::Unknown;
        };
        match method_id {
            result_methods::ResultMethodId::Map => {
                let ret = self.validate_result_combinator_callback(method, callback_ty, &ok_ty, None, span);
                ResolvedType::Generic("Result".to_string(), vec![ret, err_ty])
            }
            result_methods::ResultMethodId::MapErr => {
                let ret = self.validate_result_combinator_callback(method, callback_ty, &err_ty, None, span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, ret])
            }
            result_methods::ResultMethodId::AndThen => {
                let expected = ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Unknown, err_ty.clone()]);
                let ret = self.validate_result_combinator_callback(method, callback_ty, &ok_ty, Some(&expected), span);
                let ResolvedType::Generic(name, args) = ret else {
                    return ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Unknown, err_ty]);
                };
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 {
                    return ResolvedType::Generic(name, args);
                }
                ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Unknown, err_ty])
            }
            result_methods::ResultMethodId::OrElse => {
                let expected = ResolvedType::Generic("Result".to_string(), vec![ok_ty.clone(), ResolvedType::Unknown]);
                let ret = self.validate_result_combinator_callback(method, callback_ty, &err_ty, Some(&expected), span);
                let ResolvedType::Generic(name, args) = ret else {
                    return ResolvedType::Generic("Result".to_string(), vec![ok_ty, ResolvedType::Unknown]);
                };
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 {
                    return ResolvedType::Generic(name, args);
                }
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, ResolvedType::Unknown])
            }
            result_methods::ResultMethodId::Inspect => {
                self.validate_result_combinator_callback(method, callback_ty, &ok_ty, Some(&ResolvedType::Unit), span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty])
            }
            result_methods::ResultMethodId::InspectErr => {
                self.validate_result_combinator_callback(method, callback_ty, &err_ty, Some(&ResolvedType::Unit), span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty])
            }
            result_methods::ResultMethodId::Unwrap | result_methods::ResultMethodId::UnwrapOr => ResolvedType::Unknown,
        }
    }

    /// Bind `list.repeat(...)` arguments to the helper's fixed `value` and `count` parameters.
    ///
    /// This mirrors ordinary fixed-parameter call binding closely enough for named arguments while keeping unpacking
    /// rejected: `list.repeat` has no rest parameters and lowering expects the two canonical arguments explicitly.
    fn bind_builtin_list_repeat_args<'a>(
        &mut self,
        args: &'a [CallArg],
        span: Span,
    ) -> (Option<&'a Spanned<Expr>>, Option<&'a Spanned<Expr>>, bool) {
        let helper = BuiltinCollectionHelperId::ListRepeat;
        let callee = collection_helpers::full_name(helper);
        let mut value = None;
        let mut count = None;
        let mut valid = true;
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    let target = match positional_index {
                        0 => Some((&mut value, "value")),
                        1 => Some((&mut count, "count")),
                        _ => None,
                    };
                    positional_index += 1;
                    if let Some((slot, name)) = target {
                        if slot.is_some() {
                            self.errors
                                .push(errors::duplicate_call_argument(callee, name, expr.span));
                            self.check_expr(expr);
                            valid = false;
                        } else {
                            *slot = Some(expr);
                        }
                    } else {
                        self.check_expr(expr);
                        valid = false;
                    }
                }
                CallArg::Named(name, expr) => {
                    let slot = match name.as_str() {
                        "value" => Some(&mut value),
                        "count" => Some(&mut count),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        if slot.is_some() {
                            self.errors
                                .push(errors::duplicate_call_argument(callee, name, expr.span));
                            self.check_expr(expr);
                            valid = false;
                        } else {
                            *slot = Some(expr);
                        }
                    } else {
                        self.errors
                            .push(errors::unknown_keyword_argument(callee, name, expr.span));
                        self.check_expr(expr);
                        valid = false;
                    }
                }
                CallArg::PositionalUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "*", expr.span));
                    self.check_expr(expr);
                    valid = false;
                }
                CallArg::KeywordUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "**", expr.span));
                    self.check_expr(expr);
                    valid = false;
                }
            }
        }

        if args.len() != 2 {
            self.errors.push(errors::builtin_arity(callee, 2, args.len(), span));
            valid = false;
        }
        if value.is_none() {
            self.errors
                .push(errors::missing_required_argument(callee, "value", span));
            valid = false;
        }
        if count.is_none() {
            self.errors
                .push(errors::missing_required_argument(callee, "count", span));
            valid = false;
        }

        (value, count, valid)
    }

    /// Type-check the built-in `list.repeat(value, count)` helper.
    ///
    /// The receiver is the built-in `list` collection surface, not a runtime list value, so this has to run before
    /// ordinary receiver expression checking would report `list` as an unknown value symbol.
    fn check_builtin_list_repeat_call(&mut self, args: &[CallArg], span: Span) -> ResolvedType {
        let (value_arg, count_arg, valid_args) = self.bind_builtin_list_repeat_args(args, span);

        let value_ty = value_arg
            .map(|expr| self.check_expr(expr))
            .unwrap_or(ResolvedType::Unknown);

        if let Some(count_arg) = count_arg {
            let count_ty = self.check_expr(count_arg);
            if !self.types_compatible(&count_ty, &ResolvedType::Int) {
                self.errors
                    .push(errors::type_mismatch("int", &count_ty.to_string(), span));
            }
        }

        if !matches!(value_ty, ResolvedType::Unknown) && !self.is_copy_type(&value_ty) && !self.is_clone_type(&value_ty)
        {
            self.errors
                .push(errors::list_repeat_requires_clone(&value_ty.to_string(), span));
        }

        if !valid_args {
            return ResolvedType::Unknown;
        }

        list_ty(value_ty)
    }

    /// Return whether `base` resolves to the built-in `list` collection type surface.
    fn is_builtin_list_surface_receiver(&self, base: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &base.node else {
            return false;
        };
        name == collection_helpers::receiver(BuiltinCollectionHelperId::ListRepeat)
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::List)
            && self
                .lookup_symbol(name)
                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Builtin)))
    }

    /// Return the canonical stdlib iterator trait name from the shared language registry.
    fn iterator_protocol_name() -> &'static str {
        core_traits::as_str(TraitId::Iterator)
    }

    /// Return the canonical RFC 088 iterable protocol trait spelling.
    fn iterable_protocol_name() -> &'static str {
        core_traits::as_str(TraitId::Iterable)
    }

    /// Construct the protocol-facing `Iterator[T]` type used by RFC 088 adapter method typing.
    fn iterator_protocol_ty(elem: ResolvedType) -> ResolvedType {
        ResolvedType::Generic(Self::iterator_protocol_name().to_string(), vec![elem])
    }

    /// Return whether `name` is the canonical RFC 088 iterable protocol trait spelling.
    fn is_iterable_protocol_name(name: &str) -> bool {
        name == Self::iterable_protocol_name()
    }

    /// Return whether `name` is the canonical RFC 088 iterator protocol trait spelling.
    fn is_iterator_protocol_name(name: &str) -> bool {
        name == Self::iterator_protocol_name()
    }

    /// Return the element type for values that can participate in the RFC 088 iterator protocol surface.
    ///
    /// This intentionally recognizes both explicit trait-typed values (`Iterator[T]` / `Iterable[T]`) and builtin
    /// collection values that have an obvious frontend iterator element type. It is a typechecker-only surface helper;
    /// lowering and emission use the same protocol shape to route known iterator methods through dedicated backend
    /// handling.
    fn iterable_protocol_element_type(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args)
                if (Self::is_iterator_protocol_name(name) || Self::is_iterable_protocol_name(name))
                    && args.len() == 1 =>
            {
                args.first().cloned()
            }
            ResolvedType::Generic(name, args)
                if matches!(
                    collection_type_id(name.as_str()),
                    Some(
                        CollectionTypeId::List
                            | CollectionTypeId::Set
                            | CollectionTypeId::FrozenList
                            | CollectionTypeId::FrozenSet
                    )
                ) && args.len() == 1 =>
            {
                args.first().cloned()
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Return the element type for values that are already typed as `Iterator[T]`.
    fn iterator_protocol_element_type(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args) if Self::is_iterator_protocol_name(name) && args.len() == 1 => {
                args.first().cloned()
            }
            _ => None,
        }
    }

    /// Validate fixed-arity RFC 088 method calls and report the same arity diagnostic style as other builtin calls.
    fn validate_iterator_method_arity(&mut self, method: &str, expected: usize, found: usize, span: Span) -> bool {
        if found == expected {
            return true;
        }
        self.errors.push(errors::builtin_arity(
            &format!("{}.{method}", Self::iterator_protocol_name()),
            expected,
            found,
            span,
        ));
        false
    }

    /// Build a resolved callable type from parameter and return types for adapter diagnostics.
    fn iterator_callback_ty(params: Vec<ResolvedType>, ret: ResolvedType) -> ResolvedType {
        ResolvedType::Function(
            params.into_iter().map(CallableParam::positional).collect(),
            Box::new(ret),
        )
    }

    /// Reject `.batch(size)` calls when a non-positive literal size is visible to the frontend.
    fn validate_iterator_batch_size_literal(&mut self, args: &[CallArg], span: Span) {
        let Some(CallArg::Positional(expr)) = args.first() else {
            return;
        };
        let Expr::Literal(Literal::Int(value)) = &expr.node else {
            return;
        };
        if value.value > 0 {
            return;
        }
        self.errors.push(CompileError::type_error(
            "Iterator.batch() size must be greater than zero".to_string(),
            span,
        ));
    }

    /// Return whether `method` consumes the receiver under RFC 088 terminal semantics.
    fn is_iterator_terminal_method(method: &str) -> bool {
        matches!(
            method,
            "collect" | "count" | "reduce" | "fold" | "any" | "all" | "find" | "for_each" | "sum"
        )
    }

    /// Validate `.sum()` item types against the backend-supported summable item surface.
    fn iterator_sum_output_type(&mut self, elem: &ResolvedType, span: Span) -> ResolvedType {
        if self.iterator_sum_underlying_type(elem).is_some() {
            return elem.clone();
        }
        self.errors.push(CompileError::type_error(
            format!(
                "Iterator.sum() requires int, float, or a newtype over a summable type; found {}",
                elem
            ),
            span,
        ));
        ResolvedType::Unknown
    }

    /// Return the primitive summation carrier for an iterator item type.
    ///
    /// Primitive numeric types carry themselves. Newtypes recursively carry their underlying summable type, which lets
    /// `.sum()` accept transparent domain wrappers while still rejecting non-summable shapes.
    fn iterator_sum_underlying_type(&self, elem: &ResolvedType) -> Option<ResolvedType> {
        match elem {
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Unknown => Some(elem.clone()),
            ResolvedType::Named(name) => self.newtype_sum_underlying_type(name, &[]),
            ResolvedType::Generic(name, args) => self.newtype_sum_underlying_type(name, args),
            _ => None,
        }
    }

    /// Resolve the summation carrier for a newtype, applying generic type arguments before checking the underlying.
    fn newtype_sum_underlying_type(&self, name: &str, args: &[ResolvedType]) -> Option<ResolvedType> {
        let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(name) else {
            return None;
        };
        let underlying = if args.is_empty() {
            newtype.underlying.clone()
        } else {
            let subst = type_param_subst_map(&newtype.type_params, args);
            substitute_resolved_type(&newtype.underlying, &subst)
        };
        self.iterator_sum_underlying_type(&underlying)
    }

    /// Track the narrow same-binding case after a terminal iterator method consumes a direct local binding.
    fn mark_direct_iterator_binding_consumed(&mut self, base: &Spanned<Expr>, method: &str, span: Span) {
        if !Self::is_iterator_terminal_method(method) {
            return;
        }
        let Expr::Ident(name) = &base.node else {
            return;
        };
        self.consumed_iterator_bindings.insert(name.clone(), span);
    }

    /// Validate an adapter callback whose return type is fully specified by the method contract.
    fn validate_iterator_callback_return(
        &mut self,
        method: &str,
        actual: &ResolvedType,
        params: Vec<ResolvedType>,
        ret: ResolvedType,
        span: Span,
    ) {
        if matches!(actual, ResolvedType::Unknown) {
            return;
        }
        let expected = Self::iterator_callback_ty(params, ret);
        if !self.types_compatible(actual, &expected) {
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
        }
        if !matches!(actual, ResolvedType::Function(_, _)) {
            self.errors.push(errors::missing_method(
                &actual.to_string(),
                &format!("__call__ for {method} callback"),
                span,
            ));
        }
    }

    /// Validate a mapping-style callback and return its concrete output type when it is known.
    fn iterator_mapping_callback_return_type(
        &mut self,
        actual: &ResolvedType,
        param_ty: ResolvedType,
        span: Span,
    ) -> ResolvedType {
        let ResolvedType::Function(params, ret) = actual else {
            if !matches!(actual, ResolvedType::Unknown) {
                let expected = Self::iterator_callback_ty(vec![param_ty], ResolvedType::Unknown);
                self.errors
                    .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
            }
            return ResolvedType::Unknown;
        };
        if params.len() != 1 || !self.types_compatible(&params[0].ty, &param_ty) {
            let expected = Self::iterator_callback_ty(vec![param_ty], (**ret).clone());
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
        }
        (**ret).clone()
    }

    /// Typecheck one RFC 088 iterator/iterable adapter or terminal method.
    ///
    /// The frontend treats these protocol methods as a typed surface even when the receiver is a builtin collection
    /// whose methods are not represented as ordinary user-declared methods. Backend lowering and emission classify the
    /// same method family as known iterator calls.
    fn resolve_iterator_protocol_method_call(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let elem = self.iterable_protocol_element_type(base_ty)?;
        let iterator_elem = self
            .iterator_protocol_element_type(base_ty)
            .unwrap_or_else(|| elem.clone());
        let method_id = iterator_methods::from_str(method)?;
        use iterator_methods::IteratorMethodId as M;

        match method_id {
            M::Iter => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(Self::iterator_protocol_ty(elem))
            }
            M::Map => {
                if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                }
                let mapped = self.iterator_mapping_callback_return_type(
                    arg_types.first().unwrap_or(&ResolvedType::Unknown),
                    iterator_elem,
                    span,
                );
                Some(Self::iterator_protocol_ty(mapped))
            }
            M::Filter | M::TakeWhile | M::SkipWhile => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem.clone()],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            M::FlatMap => {
                if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                }
                let returned = self.iterator_mapping_callback_return_type(
                    arg_types.first().unwrap_or(&ResolvedType::Unknown),
                    iterator_elem,
                    span,
                );
                let Some(flat_elem) = self.iterable_protocol_element_type(&returned) else {
                    if !matches!(returned, ResolvedType::Unknown) {
                        let expected = ResolvedType::Generic(
                            Self::iterable_protocol_name().to_string(),
                            vec![ResolvedType::Unknown],
                        );
                        self.errors.push(errors::type_mismatch(
                            &expected.to_string(),
                            &returned.to_string(),
                            span,
                        ));
                    }
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                };
                Some(Self::iterator_protocol_ty(flat_elem))
            }
            M::Take | M::Skip => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                    && !self.types_compatible(arg_ty, &ResolvedType::Int)
                {
                    self.errors
                        .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            M::Chain => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                {
                    let expected = Self::iterator_protocol_ty(iterator_elem.clone());
                    if !self.types_compatible(arg_ty, &expected) {
                        self.errors
                            .push(errors::type_mismatch(&expected.to_string(), &arg_ty.to_string(), span));
                    }
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            M::Enumerate => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(Self::iterator_protocol_ty(ResolvedType::Tuple(vec![
                    ResolvedType::Int,
                    iterator_elem,
                ])))
            }
            M::Zip => {
                if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                }
                let other_elem = arg_types
                    .first()
                    .and_then(|arg_ty| self.iterable_protocol_element_type(arg_ty))
                    .unwrap_or_else(|| {
                        if let Some(arg_ty) = arg_types.first()
                            && !matches!(arg_ty, ResolvedType::Unknown)
                        {
                            let expected = ResolvedType::Generic(
                                Self::iterator_protocol_name().to_string(),
                                vec![ResolvedType::Unknown],
                            );
                            self.errors
                                .push(errors::type_mismatch(&expected.to_string(), &arg_ty.to_string(), span));
                        }
                        ResolvedType::Unknown
                    });
                Some(Self::iterator_protocol_ty(ResolvedType::Tuple(vec![
                    iterator_elem,
                    other_elem,
                ])))
            }
            M::Batch => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                    && !self.types_compatible(arg_ty, &ResolvedType::Int)
                {
                    self.errors
                        .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
                }
                self.validate_iterator_batch_size_literal(args, span);
                Some(Self::iterator_protocol_ty(list_ty(iterator_elem)))
            }
            M::Collect => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(list_ty(iterator_elem))
            }
            M::Count => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(ResolvedType::Int)
            }
            M::Any | M::All => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(ResolvedType::Bool)
            }
            M::Find => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem.clone()],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(option_ty(iterator_elem))
            }
            M::Reduce | M::Fold => {
                if !self.validate_iterator_method_arity(method, 2, args.len(), span) {
                    return Some(ResolvedType::Unknown);
                }
                let acc_ty = arg_types.first().cloned().unwrap_or(ResolvedType::Unknown);
                self.validate_iterator_callback_return(
                    method,
                    arg_types.get(1).unwrap_or(&ResolvedType::Unknown),
                    vec![acc_ty.clone(), iterator_elem],
                    acc_ty.clone(),
                    span,
                );
                Some(acc_ty)
            }
            M::ForEach => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem],
                        ResolvedType::Unit,
                        span,
                    );
                }
                Some(ResolvedType::Unit)
            }
            M::Sum => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(self.iterator_sum_output_type(&iterator_elem, span))
            }
        }
    }

    /// Return backend dispatch metadata for stdlib trait calls that must lower to explicit UFCS paths.
    fn explicit_trait_dispatch_for_backend(
        &self,
        trait_name: &str,
        type_args: Vec<ResolvedType>,
        origin_module_path: Option<&[String]>,
    ) -> Option<crate::frontend::typechecker::ResolvedMethodDispatch> {
        let module_path = origin_module_path
            .map(<[String]>::to_vec)
            .or_else(|| self.stdlib_cache.loaded_trait_module_path(trait_name))?;
        let is_std_io_binary_trait = module_path.len() == 2
            && module_path[0] == "std"
            && module_path[1] == "io"
            && matches!(trait_name, "BinaryRead" | "BinaryWrite");
        let is_std_traits_indexing_trait = module_path.len() == 3
            && module_path[0] == "std"
            && module_path[1] == "traits"
            && module_path[2] == "indexing"
            && trait_name == "Index";
        (is_std_io_binary_trait || is_std_traits_indexing_trait)
            .then(|| self.resolved_trait_dispatch(trait_name, type_args, Some(&module_path)))
    }

    /// Build a backend trait-dispatch record using the originating stdlib module when it is known.
    fn resolved_trait_dispatch(
        &self,
        trait_name: &str,
        type_args: Vec<ResolvedType>,
        origin_module_path: Option<&[String]>,
    ) -> crate::frontend::typechecker::ResolvedMethodDispatch {
        let trait_path = self
            .stdlib_trait_module_path_for_backend(trait_name, origin_module_path)
            .filter(|segments| segments.first().is_some_and(|segment| segment == "std"))
            .map(|segments| {
                let module_path = segments.into_iter().skip(1).collect::<Vec<_>>().join("::");
                format!("crate::__incan_std::{module_path}::{trait_name}")
            })
            .unwrap_or_else(|| trait_name.to_string());
        crate::frontend::typechecker::ResolvedMethodDispatch::Trait { trait_path, type_args }
    }

    /// Return the stdlib module path that should qualify an emitted trait path.
    fn stdlib_trait_module_path_for_backend(
        &self,
        trait_name: &str,
        origin_module_path: Option<&[String]>,
    ) -> Option<Vec<String>> {
        origin_module_path
            .map(<[String]>::to_vec)
            .or_else(|| self.stdlib_cache.loaded_trait_module_path(trait_name))
    }

    /// Return whether an expression names an enum type rather than an enum value.
    fn is_enum_type_name_expr(&self, expr: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &expr.node else {
            return false;
        };
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Enum(_))))
    }

    /// Typecheck built-in numeric resize helpers using the expected result type as the target.
    fn check_numeric_resize_method(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let source = super::super::numeric_type_id_for_compat(base_ty)?;
        let policy = match method {
            "resize" => NumericResizeMethodPolicy::Lossless,
            "try_resize" => NumericResizeMethodPolicy::Try,
            "wrapping_resize" => NumericResizeMethodPolicy::Wrapping,
            "saturating_resize" => NumericResizeMethodPolicy::Saturating,
            _ => return None,
        };
        if !type_args.is_empty() {
            self.errors
                .push(errors::type_mismatch("no type arguments", "type arguments", span));
            return Some(ResolvedType::Unknown);
        }
        if !args.is_empty() {
            self.errors
                .push(errors::type_mismatch("no arguments", "arguments", span));
            return Some(ResolvedType::Unknown);
        }

        let target_ty = match policy {
            NumericResizeMethodPolicy::Try => match expected_return_ty {
                Some(ResolvedType::Generic(name, args))
                    if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option) && args.len() == 1 =>
                {
                    args[0].clone()
                }
                _ => {
                    self.errors.push(errors::type_mismatch(
                        "contextual Option[numeric] target",
                        expected_return_ty
                            .map(ToString::to_string)
                            .as_deref()
                            .unwrap_or("unknown target"),
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
            },
            _ => match expected_return_ty {
                Some(ty) => ty.clone(),
                None => {
                    self.errors.push(errors::type_mismatch(
                        "contextual numeric target",
                        "unknown target",
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
            },
        };
        let Some(target) = super::super::numeric_type_id_for_compat(&target_ty) else {
            self.errors
                .push(errors::type_mismatch("numeric target", &target_ty.to_string(), span));
            return Some(ResolvedType::Unknown);
        };

        let source_info = incan_core::lang::types::numerics::info_for(source);
        let target_info = incan_core::lang::types::numerics::info_for(target);
        let integer_to_integer = matches!(
            (source_info.family, target_info.family),
            (
                NumericFamily::SignedInteger | NumericFamily::UnsignedInteger,
                NumericFamily::SignedInteger | NumericFamily::UnsignedInteger
            )
        );
        match policy {
            NumericResizeMethodPolicy::Lossless => {
                if !super::super::numeric_type_losslessly_widens_to(source, target) {
                    self.errors.push(
                        errors::type_mismatch("lossless numeric resize target", &target_ty.to_string(), span)
                            .with_hint(
                                "Use try_resize(), wrapping_resize(), or saturating_resize() for explicit lossy integer resizing.",
                            ),
                    );
                    return Some(ResolvedType::Unknown);
                }
                Some(target_ty)
            }
            NumericResizeMethodPolicy::Try
            | NumericResizeMethodPolicy::Wrapping
            | NumericResizeMethodPolicy::Saturating => {
                if !integer_to_integer {
                    self.errors.push(errors::type_mismatch(
                        "integer resize target",
                        &target_ty.to_string(),
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                match policy {
                    NumericResizeMethodPolicy::Try => Some(option_ty(target_ty)),
                    _ => Some(target_ty),
                }
            }
        }
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
    fn field_overlay_value_type_for_nominal(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        let (type_name, type_args) = match ty {
            ResolvedType::Named(name) => (name.as_str(), None),
            ResolvedType::Generic(name, args) => (name.as_str(), Some(args.as_slice())),
            _ => return None,
        };
        let type_info = self.lookup_semantic_type_info(type_name)?;
        let (type_params, fields): (&[String], Vec<&FieldInfo>) = match type_info {
            TypeInfo::Model(model) => (model.type_params.as_slice(), model.fields.values().collect()),
            TypeInfo::Class(class) => (
                class.type_params.as_slice(),
                class
                    .fields
                    .values()
                    .filter(|field| matches!(field.visibility, Visibility::Public))
                    .collect(),
            ),
            _ => return None,
        };
        if fields.is_empty() {
            return Some(ResolvedType::Unit);
        }
        let subst = type_args.map(|args| type_param_subst_map(type_params, args));
        let mut value_types: Vec<ResolvedType> = fields
            .into_iter()
            .map(|field| match &subst {
                Some(subst) => substitute_resolved_type(&field.ty, subst),
                None => field.ty.clone(),
            })
            .collect();
        value_types.sort_by_key(|ty| ty.to_string());
        value_types.dedup();
        if value_types.len() == 1 {
            value_types.pop()
        } else {
            Some(union_ty(value_types))
        }
    }

    /// Return the declared surface type for a compiler-provided reflection magic method.
    ///
    /// These methods do not have user-visible source declarations, so method-call checking uses this helper to expose
    /// their synthesized return types while preserving nominal receiver-specific field value typing.
    fn reflection_magic_method_return_type(&self, receiver_ty: &ResolvedType, method: &str) -> Option<ResolvedType> {
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => Some(ResolvedType::Str),
            Some(magic_methods::MagicMethodId::Fields) => Some(ResolvedType::FrozenList(Box::new(
                ResolvedType::Named(surface_types::as_str(SurfaceTypeId::FieldInfo).to_string()),
            ))),
            Some(magic_methods::MagicMethodId::FieldValue) => {
                self.field_overlay_value_type_for_nominal(receiver_ty).map(option_ty)
            }
            Some(magic_methods::MagicMethodId::FieldItems) => self
                .field_overlay_value_type_for_nominal(receiver_ty)
                .map(|value_ty| list_ty(ResolvedType::Tuple(vec![ResolvedType::Str, value_ty]))),
            _ => None,
        }
    }

    /// Return the receiver-independent reflection result type available through an inferred generic capability.
    fn generic_reflection_magic_method_return_type(&self, method: &str) -> Option<ResolvedType> {
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => Some(ResolvedType::Str),
            Some(magic_methods::MagicMethodId::Fields) => Some(ResolvedType::FrozenList(Box::new(
                ResolvedType::Named(surface_types::as_str(SurfaceTypeId::FieldInfo).to_string()),
            ))),
            _ => None,
        }
    }

    /// Validate a reflection magic-method call.
    fn validate_reflection_magic_call(
        &mut self,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) {
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let expected_arity = match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName)
            | Some(magic_methods::MagicMethodId::Fields)
            | Some(magic_methods::MagicMethodId::FieldItems) => 0,
            Some(magic_methods::MagicMethodId::FieldValue) => 1,
            _ => return,
        };
        if args.len() != expected_arity {
            self.errors
                .push(errors::builtin_arity(method, expected_arity, args.len(), span));
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
            Some(magic_methods::MagicMethodId::FieldValue | magic_methods::MagicMethodId::FieldItems) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_))
                )
            }
            _ => false,
        }
    }

    /// Return the canonical Rust path for a receiver type.
    fn rust_canonical_path_for_receiver_type(&self, ty: &ResolvedType) -> Option<String> {
        match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.rust_canonical_path_for_receiver_type(inner),
            ResolvedType::RustPath(path) => Some(path.clone()),
            ResolvedType::Named(name) => self.rust_canonical_path_for_nominal_receiver(name, None),
            ResolvedType::Generic(name, args) => {
                self.rust_canonical_path_for_nominal_receiver(name, Some(args.as_slice()))
            }
            _ => None,
        }
    }

    /// Return the canonical Rust path for a nominal receiver.
    fn rust_canonical_path_for_nominal_receiver(
        &self,
        name: &str,
        type_args: Option<&[ResolvedType]>,
    ) -> Option<String> {
        if let Some(TypeInfo::Newtype(newtype)) = self.lookup_semantic_type_info(name)
            && newtype.is_rusttype
        {
            let underlying = if let Some(args) = type_args {
                let subst = type_param_subst_map(&newtype.type_params, args);
                substitute_resolved_type(&newtype.underlying, &subst)
            } else {
                newtype.underlying.clone()
            };
            if let Some(path) = self.rust_path_for_rusttype_underlying(&underlying) {
                return Some(path);
            }
        }

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

    /// Resolve a declared field on a nominal user-defined type, applying generic substitutions when available.
    ///
    /// This keeps field access on `Named(Type)` and `Generic(Type[...])` owners on the same path instead of letting
    /// generic owners fall through to "missing field" diagnostics despite having declared fields.
    pub(in crate::frontend::typechecker) fn resolve_nominal_field_type(
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
                if enum_info.variants.contains(&field.to_string()) || enum_info.variant_aliases.contains_key(field) {
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
                        return Some(self.resolved_function_type_from_rust_sig_for_path(&sig, false, path));
                    }
                    if let Some(meta) = self.rust_item_metadata_for_path(path)
                        && let RustItemKind::Type(info) = &meta.kind
                        && let Some(rust_field) = Self::rust_field_for_source_name(&info.fields, field)
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

    /// Resolve a declared computed property on a nominal user-defined type, applying generic substitutions.
    fn resolve_nominal_property_type(
        &mut self,
        type_name: &str,
        type_args: Option<&[ResolvedType]>,
        property: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let type_info = self.lookup_semantic_type_info(type_name)?;
        match type_info {
            TypeInfo::Model(model) => {
                let info = model.properties.get(property)?;
                let return_type = if let Some(args) = type_args {
                    let subst = type_param_subst_map(&model.type_params, args);
                    substitute_resolved_type(&info.return_type, &subst)
                } else {
                    info.return_type.clone()
                };
                self.type_info
                    .record_computed_property_access(span, type_name, property);
                Some(return_type)
            }
            TypeInfo::Class(class) => {
                let info = class.properties.get(property)?;
                let owner = info.owner.as_deref().unwrap_or(type_name);
                if matches!(info.visibility, Visibility::Private) && self.current_method_owner.as_deref() != Some(owner)
                {
                    self.errors.push(errors::private_property(type_name, property, span));
                    return Some(ResolvedType::Unknown);
                }
                let return_type = if let Some(args) = type_args {
                    let subst = type_param_subst_map(&class.type_params, args);
                    substitute_resolved_type(&info.return_type, &subst)
                } else {
                    info.return_type.clone()
                };
                self.type_info
                    .record_computed_property_access(span, type_name, property);
                Some(return_type)
            }
            _ => None,
        }
    }

    /// Return whether the receiver has a computed property with this name.
    pub(in crate::frontend::typechecker::check_expr) fn receiver_has_computed_property(
        &mut self,
        receiver_ty: &ResolvedType,
        property: &str,
        span: Span,
    ) -> bool {
        match receiver_ty {
            ResolvedType::Named(type_name) => self
                .resolve_nominal_property_type(type_name, None, property, span)
                .is_some(),
            ResolvedType::Generic(type_name, type_args) => self
                .resolve_nominal_property_type(type_name, Some(type_args.as_slice()), property, span)
                .is_some(),
            ResolvedType::TypeVar(name) => self
                .resolve_generic_placeholder_property(name, property, span)
                .is_some(),
            _ => false,
        }
    }

    /// Resolve a computed property on an active generic placeholder bound (`T with Trait[...]`).
    fn resolve_generic_placeholder_property(
        &mut self,
        placeholder_name: &str,
        property: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let mut active_bounds = Vec::new();
        for frame in self.current_type_param_bound_details.iter().rev() {
            if let Some(bounds) = frame.get(placeholder_name) {
                active_bounds = bounds.clone();
                break;
            }
        }
        for bound in &active_bounds {
            if let Some(info) = self.trait_property_info_resolved_for_adoption(bound, property, span) {
                self.type_info
                    .record_computed_property_access(span, &bound.name, property);
                return Some(info.return_type);
            }
        }
        None
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
        let Some(metadata) = self.rust_item_metadata_for_path(rust_path) else {
            if let Some(import_use) = self.record_unique_rust_trait_import_for_unresolved_receiver_call(method, span)
                && let Some(sig) = import_use.signature.as_ref()
            {
                let callable_display = format!("rust::{rust_path}.{method}");
                let ret = self.validate_rust_method_call(
                    callable_display.as_str(),
                    sig,
                    args,
                    arg_types,
                    preserves_lookup_arg_shape,
                    span,
                );
                return Some(Self::substitute_rust_self_type(ret, rust_path));
            }
            if let Some(ret) = self.validate_metadata_free_rust_method_call(
                rust_path,
                method,
                args,
                arg_types,
                preserves_lookup_arg_shape,
                span,
            ) {
                return Some(ret);
            }
            return None;
        };
        match &metadata.kind {
            RustItemKind::Type(_) => {
                let Some(sig) = self.rust_method_signature(rust_path, method) else {
                    if let Some(import_use) = self.record_rust_extension_trait_import_for_call(&metadata, method, span)
                        && let Some(sig) = import_use.signature.as_ref()
                    {
                        let callable_display = format!("rust::{rust_path}.{method}");
                        let ret = self.validate_rust_method_call(
                            callable_display.as_str(),
                            sig,
                            args,
                            arg_types,
                            preserves_lookup_arg_shape,
                            span,
                        );
                        return Some(Self::substitute_rust_self_type(ret, rust_path));
                    }
                    if let Some(ret) = self.validate_metadata_free_rust_method_call(
                        rust_path,
                        method,
                        args,
                        arg_types,
                        preserves_lookup_arg_shape,
                        span,
                    ) {
                        return Some(ret);
                    }
                    // Stay permissive when no unambiguous imported trait or trait method signature can be selected.
                    return Some(ResolvedType::Unknown);
                };
                if Self::rust_signature_has_receiver(&sig)
                    && sig.params[1..].iter().any(|param| {
                        let normalized = param.type_display.replace(' ', "");
                        Self::rust_display_type_var_name(normalized.as_str()).is_some()
                    })
                {
                    self.type_info.record_regular_method_arg_shape(receiver_span, method);
                }
                let callable_display = format!("rust::{rust_path}.{method}");
                let ret = self.validate_rust_method_call(
                    callable_display.as_str(),
                    &sig,
                    args,
                    arg_types,
                    preserves_lookup_arg_shape,
                    span,
                );
                Some(Self::substitute_rust_self_type(ret, rust_path))
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

    /// Validate one metadata-free Rust method compatibility rule through the ordinary Rust-boundary path.
    fn validate_metadata_free_rust_method_call(
        &mut self,
        rust_path: &str,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
    ) -> Option<ResolvedType> {
        let sig: RustFunctionSig = metadata_free_method_signature(rust_path, method)?;
        let callable_display = format!("rust::{rust_path}.{method}");
        let error_count = self.errors.len();
        let ret = self.validate_rust_method_call(
            callable_display.as_str(),
            &sig,
            args,
            arg_types,
            preserves_lookup_arg_shape,
            span,
        );
        if self.errors.len() > error_count {
            Some(ResolvedType::Unknown)
        } else {
            Some(Self::substitute_rust_self_type(ret, rust_path))
        }
    }

    /// Record the imported Rust extension trait needed for a method call when metadata proves a unique match.
    ///
    /// Rust method lookup needs the trait binding in scope even though the emitted call remains `receiver.method(...)`.
    /// The typechecker has both receiver metadata and import metadata, so it is the narrowest layer that can select the
    /// import without falling back to backend method-name heuristics.
    fn record_rust_extension_trait_import_for_call(
        &mut self,
        receiver_metadata: &incan_core::interop::RustItemMetadata,
        method: &str,
        span: Span,
    ) -> Option<RustMethodTraitImportUse> {
        let RustItemKind::Type(type_info) = &receiver_metadata.kind else {
            return None;
        };
        let matches = self
            .type_info
            .rust
            .trait_imports
            .iter()
            .filter_map(|(binding, import)| {
                Self::rust_trait_import_matches_receiver(type_info, import, method).map(|signature| {
                    RustMethodTraitImportUse {
                        binding: binding.clone(),
                        trait_path: import.trait_path.clone(),
                        method: method.to_string(),
                        signature,
                    }
                })
            })
            .collect::<Vec<_>>();
        let [import_use] = matches.as_slice() else {
            return None;
        };
        self.type_info
            .record_rust_method_trait_import_use(span, import_use.clone());
        Some(import_use.clone())
    }

    /// Record a unique imported Rust trait method when receiver metadata is unavailable.
    ///
    /// rust-inspect can miss generated or re-export-heavy concrete types while still extracting the imported trait or
    /// falling back to core extension-trait vocabulary. In that case the import itself is enough for Rust method
    /// lookup; a recovered signature only adds call-site parameter shape metadata.
    fn record_unique_rust_trait_import_for_unresolved_receiver_call(
        &mut self,
        method: &str,
        span: Span,
    ) -> Option<RustMethodTraitImportUse> {
        let matches = self
            .type_info
            .rust
            .trait_imports
            .iter()
            .filter(|(_, import)| import.methods.contains(method))
            .map(|(binding, import)| RustMethodTraitImportUse {
                binding: binding.clone(),
                trait_path: import.trait_path.clone(),
                method: method.to_string(),
                signature: Self::rust_trait_method_signature(import, method),
            })
            .collect::<Vec<_>>();
        let [import_use] = matches.as_slice() else {
            return None;
        };
        self.type_info
            .record_rust_method_trait_import_use(span, import_use.clone());
        Some(import_use.clone())
    }

    /// Return the trait method signature when `import` is implemented by `type_info` and declares `method`.
    fn rust_trait_import_matches_receiver(
        type_info: &incan_core::interop::RustTypeInfo,
        import: &RustTraitImportInfo,
        method: &str,
    ) -> Option<Option<incan_core::interop::RustFunctionSig>> {
        if !import.methods.contains(method) {
            return None;
        }
        let trait_suffix = Self::rust_trait_path_suffix(&import.trait_path);
        let implemented = type_info.implemented_traits.iter().any(|implemented| {
            implemented.path == import.trait_path
                || Some(implemented.path.as_str()) == import.definition_path.as_deref()
                || Self::rust_trait_path_suffix(&implemented.path) == trait_suffix
        });
        implemented.then(|| Self::rust_trait_method_signature(import, method))
    }

    /// Return the imported trait method signature when metadata supplied it.
    fn rust_trait_method_signature(
        import: &RustTraitImportInfo,
        method: &str,
    ) -> Option<incan_core::interop::RustFunctionSig> {
        import.method_signatures.get(method).cloned()
    }

    /// Check if a type is copyable.
    pub(in crate::frontend::typechecker) fn is_copy_type(&self, ty: &ResolvedType) -> bool {
        matches!(
            ty,
            ResolvedType::Int
                | ResolvedType::Float
                | ResolvedType::Numeric(_)
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
            | ResolvedType::Numeric(_)
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
            ResolvedType::TypeVar(name) => self.active_type_param_has_builtin_bound(name, TraitId::Clone),
            ResolvedType::CallSiteInfer => false,
            // RFC 041: provenance is known, but Incan does not yet query Rust for `Copy`/`Clone`; do not assume.
            ResolvedType::RustPath(_) => false,
            ResolvedType::Unknown => true,
        }
    }

    /// Return whether an active type parameter has a builtin bound.
    fn active_type_param_has_builtin_bound(&self, type_param: &str, trait_id: TraitId) -> bool {
        let expected = core_traits::as_str(trait_id);
        self.current_type_param_bound_details.iter().rev().any(|frame| {
            frame.get(type_param).is_some_and(|bounds| {
                bounds
                    .iter()
                    .any(|bound| bound.name == expected || Self::type_bound_source_name(bound) == expected)
            })
        })
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
    pub(in crate::frontend::typechecker) fn resolve_named_method(
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
            let trait_entries = trait_adoptions
                .map(|trait_adoptions| {
                    trait_adoptions
                        .iter()
                        .filter_map(|adoption| {
                            self.trait_method_entry_resolved_for_adoption(adoption, method, call_site_span)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let candidates = overloads
                .iter()
                .cloned()
                .map(|info| {
                    let dispatch = trait_entries
                        .iter()
                        .find(|entry| {
                            info.trait_target
                                .as_ref()
                                .is_none_or(|target| target.name == entry.origin_trait)
                                && self.method_sigs_compatible(&info, &entry.info)
                                && self.method_sigs_compatible(&entry.info, &info)
                        })
                        .and_then(|entry| {
                            self.explicit_trait_dispatch_for_backend(
                                &entry.origin_trait,
                                entry.origin_type_args.clone(),
                                entry.origin_module_path.as_deref(),
                            )
                        });
                    MethodCandidate { info, dispatch }
                })
                .collect::<Vec<_>>();
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
                if let Some(entry) = self.trait_method_entry_resolved_for_adoption(adoption, method, call_site_span) {
                    let dispatch = self.explicit_trait_dispatch_for_backend(
                        &entry.origin_trait,
                        entry.origin_type_args.clone(),
                        entry.origin_module_path.as_deref(),
                    );
                    candidates.push(MethodCandidate {
                        info: entry.info,
                        dispatch,
                    });
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
        candidates: &[MethodCandidate],
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut viable: Vec<(usize, MethodCandidate)> = candidates
            .iter()
            .filter_map(|candidate| {
                self.method_candidate_match_score(&candidate.info, args, arg_types, receiver_ty, expected_return_ty)
                    .map(|score| (score, candidate.clone()))
            })
            .collect();

        if viable.is_empty() && expected_return_ty.is_some() {
            viable = candidates
                .iter()
                .filter_map(|candidate| {
                    self.method_candidate_match_score(&candidate.info, args, arg_types, receiver_ty, None)
                        .map(|score| (score, candidate.clone()))
                })
                .collect();
        }

        if viable.is_empty() {
            return candidates.first().map(|candidate| {
                if let Some(dispatch) = candidate.dispatch.clone() {
                    self.type_info
                        .record_resolved_method_call(call_site_span, method, dispatch);
                    let (params, _) = self.method_types_substituting_call_site_self(&candidate.info, receiver_ty);
                    self.type_info
                        .record_call_site_callable_params_for_dispatch(call_site_span, &params);
                }
                self.check_generic_method_call(
                    method,
                    candidate.info.clone(),
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                )
            });
        }

        let best_score = viable.iter().map(|(score, _)| *score).max().unwrap_or(0);
        let mut best = viable
            .into_iter()
            .filter(|(score, _)| *score == best_score)
            .map(|(_, method_info)| method_info)
            .collect::<Vec<_>>();

        if best.len() == 1 {
            let candidate = best.remove(0);
            if let Some(dispatch) = candidate.dispatch.clone() {
                self.type_info
                    .record_resolved_method_call(call_site_span, method, dispatch);
                let (params, _) = self.method_types_substituting_call_site_self(&candidate.info, receiver_ty);
                self.type_info
                    .record_call_site_callable_params_for_dispatch(call_site_span, &params);
            }
            return Some(self.check_generic_method_call(
                method,
                candidate.info,
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
            ));
        }

        self.errors
            .push(errors::ambiguous_trait_method_call(method, call_site_span));
        Some(ResolvedType::Unknown)
    }

    /// Resolve a source-defined method when its owner has exactly one direct implementation for the requested name.
    ///
    /// Ordinary method checking computes argument types before overload selection. That is useful for overloaded
    /// methods, but it is actively harmful for unambiguous source methods because collection literals can be checked
    /// before their parameter context is known. This path lets the declared method signature drive argument checking
    /// directly, the same way function calls do.
    fn resolve_unambiguous_source_method_without_arg_prepass(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let type_name = match base_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return None,
        };
        let type_info = self.lookup_semantic_type_info(type_name).cloned().or_else(|| {
            if type_name == "Logger" {
                self.stdlib_cache
                    .lookup_type(&["std".to_string(), "logging".to_string()], "Logger")
            } else {
                None
            }
        })?;
        match type_info {
            TypeInfo::Model(model) => {
                let method_info = match model.method_overloads.get(method) {
                    Some(overloads) if overloads.len() == 1 => overloads[0].clone(),
                    Some(_) => return None,
                    None => model.methods.get(method)?.clone(),
                };
                Some(self.check_generic_method_call(method, method_info, type_args, args, &[], span, base_ty))
            }
            TypeInfo::Class(class) => {
                let method_info = match class.method_overloads.get(method) {
                    Some(overloads) if overloads.len() == 1 => overloads[0].clone(),
                    Some(_) => return None,
                    None => class.methods.get(method)?.clone(),
                };
                Some(self.check_generic_method_call(method, method_info, type_args, args, &[], span, base_ty))
            }
            TypeInfo::Enum(en) => {
                let method_info = match en.method_overloads.get(method) {
                    Some(overloads) if overloads.len() == 1 => overloads[0].clone(),
                    Some(_) => return None,
                    None => en.methods.get(method)?.clone(),
                };
                Some(self.check_generic_method_call(method, method_info, type_args, args, &[], span, base_ty))
            }
            TypeInfo::Newtype(nt) => {
                let resolved_method = self.resolve_newtype_method_name(&nt, method);
                let method_info = match nt.method_overloads.get(resolved_method) {
                    Some(overloads) if overloads.len() == 1 => overloads[0].clone(),
                    Some(_) => return None,
                    None => nt.methods.get(resolved_method)?.clone(),
                };
                let ret =
                    self.check_generic_method_call(resolved_method, method_info, type_args, args, &[], span, base_ty);
                if nt.is_rusttype {
                    self.maybe_record_rusttype_return_coercion(&nt, resolved_method, &ret, span);
                }
                Some(ret)
            }
            _ => None,
        }
    }

    /// Return a compatibility score for one method candidate.
    ///
    /// Compatibility admits useful coercions such as lossless numeric widening, but overload selection must prefer the
    /// candidate that most directly matches the call site. Exact argument and contextual return matches therefore score
    /// higher than merely compatible matches; ties remain ambiguous.
    fn method_candidate_match_score(
        &self,
        candidate: &MethodInfo,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<usize> {
        let (params, return_type) = self.method_types_substituting_call_site_self(candidate, receiver_ty);
        let mut score = 0usize;
        if let Some(expected) = expected_return_ty
            && !self.types_compatible(&return_type, expected)
        {
            return None;
        } else if let Some(expected) = expected_return_ty {
            score += self.type_match_score(&return_type, expected);
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
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                        normal_bound[positional_index] = true;
                        positional_index += 1;
                    } else if let Some(param) = rest_positional {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                    } else {
                        return None;
                    }
                }
                CallArg::Named(name, _) => {
                    if !named_seen.insert(name.as_str()) {
                        return None;
                    }
                    if let Some((normal_idx, param)) = normal_params
                        .iter()
                        .enumerate()
                        .find(|(_, param)| param.name() == Some(name.as_str()))
                    {
                        if normal_bound[normal_idx] {
                            return None;
                        }
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                        normal_bound[normal_idx] = true;
                    } else if let Some(param) = rest_keyword {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                    } else {
                        return None;
                    }
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => return Some(score),
            }
        }
        normal_params
            .iter()
            .zip(normal_bound.iter())
            .all(|(param, bound)| *bound || param.has_default)
            .then_some(score)
    }

    /// Score how directly `actual` matches `expected` for overload ranking.
    fn type_match_score(&self, actual: &ResolvedType, expected: &ResolvedType) -> usize {
        if actual == expected {
            return 16;
        }

        match (actual, expected) {
            (ResolvedType::Unknown, _)
            | (_, ResolvedType::Unknown)
            | (ResolvedType::TypeVar(_), _)
            | (_, ResolvedType::TypeVar(_))
            | (ResolvedType::CallSiteInfer, _)
            | (_, ResolvedType::CallSiteInfer) => 0,
            (ResolvedType::Generic(actual_name, actual_args), ResolvedType::Generic(expected_name, expected_args))
                if actual_name == expected_name && actual_args.len() == expected_args.len() =>
            {
                4 + actual_args
                    .iter()
                    .zip(expected_args.iter())
                    .map(|(actual_arg, expected_arg)| self.type_match_score(actual_arg, expected_arg))
                    .sum::<usize>()
            }
            (ResolvedType::Tuple(actual_items), ResolvedType::Tuple(expected_items))
                if actual_items.len() == expected_items.len() =>
            {
                4 + actual_items
                    .iter()
                    .zip(expected_items.iter())
                    .map(|(actual_item, expected_item)| self.type_match_score(actual_item, expected_item))
                    .sum::<usize>()
            }
            (ResolvedType::FrozenList(actual_item), ResolvedType::FrozenList(expected_item)) => {
                4 + self.type_match_score(actual_item, expected_item)
            }
            (
                ResolvedType::FrozenDict(actual_key, actual_value),
                ResolvedType::FrozenDict(expected_key, expected_value),
            ) => {
                4 + self.type_match_score(actual_key, expected_key)
                    + self.type_match_score(actual_value, expected_value)
            }
            _ if self.rust_type_identities_compatible(actual, expected) == Some(true) => 12,
            _ => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Resolve a method call on a generic placeholder using the active `T with Trait[...]` bound stack.
    pub(in crate::frontend::typechecker) fn resolve_generic_placeholder_method(
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
            if let Some(entry) = self.trait_method_entry_resolved_for_adoption(bound, method, call_site_span) {
                let dispatch = self.explicit_trait_dispatch_for_backend(
                    &entry.origin_trait,
                    entry.origin_type_args.clone(),
                    entry.origin_module_path.as_deref(),
                );
                candidates.push(MethodCandidate {
                    info: entry.info,
                    dispatch,
                });
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
    pub(in crate::frontend::typechecker) fn resolve_newtype_method_name<'a>(
        &self,
        newtype: &'a NewtypeInfo,
        method: &'a str,
    ) -> &'a str {
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
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), incan_ret, span);
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

    /// Recognize `TypeName[T]` receiver expressions used for type-owned calls.
    fn resolve_type_index_expression(&self, base_ty: &ResolvedType, base: &Spanned<Expr>) -> Option<ResolvedType> {
        let ResolvedType::Named(_) = base_ty else {
            return None;
        };
        if !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName)) {
            return None;
        }
        Some(ResolvedType::Unknown)
    }

    /// Return whether `ty` is the compiler-owned `std.json.JsonValue` wrapper over the raw runtime carrier.
    fn is_std_json_value_type(&self, ty: &ResolvedType) -> bool {
        let type_name = match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return false,
        };
        if !stdlib::is_json_value_type_name(type_name) {
            return false;
        }
        matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Newtype(info))
                if matches!(
                    &info.underlying,
                    ResolvedType::RustPath(path) if path == stdlib::JSON_VALUE_RUST_PATH
                ) || matches!(&info.underlying, ResolvedType::Named(name) if name == "RustJsonValue")
        )
    }

    /// Return whether an index type can select one of `JsonValue`'s source-authored `__getitem__` overloads.
    fn is_std_json_value_index_type(index_ty: &ResolvedType) -> bool {
        matches!(index_ty, ResolvedType::Int | ResolvedType::Str | ResolvedType::Unknown) || is_frozen_str(index_ty)
    }

    /// Type-check an indexing expression (`base[index]`) and return the element type.
    pub(in crate::frontend::typechecker::check_expr) fn check_index(
        &mut self,
        base: &Spanned<Expr>,
        index: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let base_ty = self.check_type_receiver_expr(base);
        if let Some(ty) = self.resolve_type_index_expression(&base_ty, base) {
            return ty;
        }
        let index_ty = self.check_expr(index);
        if self.is_std_json_value_type(&base_ty) && !Self::is_std_json_value_index_type(&index_ty) {
            self.errors.push(errors::json_value_index_type_mismatch(
                &index_ty.to_string(),
                index.span,
            ));
            return option_ty(ResolvedType::Named(stdlib::JSON_VALUE_TYPE_NAME.to_string()));
        }

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
            ty if self.is_user_operator_receiver(&ty) => {
                if let Some(ret) = self.resolve_index_dunder(&ty, index, &index_ty, span) {
                    ret
                } else {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__getitem__", span));
                    ResolvedType::Unknown
                }
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
        let base_ty = self.check_type_receiver_expr(base);

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
                        return self.resolved_function_type_from_rust_sig_for_path(&sig, false, path);
                    }
                    if let Some(params) = self.rust_variant_callable_params(path, field) {
                        return ResolvedType::Function(params, Box::new(ResolvedType::RustPath(path.to_string())));
                    }
                    if let RustItemKind::Type(info) = &meta.kind
                        && let Some(rust_field) = Self::rust_field_for_source_name(&info.fields, field)
                    {
                        self.type_info
                            .record_rust_field_access_name(span, rust_field.name.clone());
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
            if field == "__name__" && checker.is_generic_placeholder_type(ty) {
                return ResolvedType::Str;
            }
            match ty {
                ResolvedType::Unknown => ResolvedType::Unknown,
                // Trait default methods typecheck against `Self`, but field access must be declared via
                // `@requires(...)` on the trait.
                ResolvedType::SelfType => checker
                    .current_trait_properties
                    .as_ref()
                    .and_then(|properties| properties.get(field))
                    .map(|info| {
                        checker.type_info.record_computed_property_access(span, "Self", field);
                        info.return_type.clone()
                    })
                    .or_else(|| checker.trait_required_field_type(field, span))
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
                ResolvedType::Function(_, _) if field == "__name__" => ResolvedType::Str,
                ResolvedType::Named(type_name) => {
                    if let Some(field_ty) = checker.resolve_nominal_field_type(type_name, None, field, span) {
                        return field_ty;
                    }
                    if let Some(property_ty) = checker.resolve_nominal_property_type(type_name, None, field, span) {
                        return property_ty;
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
                    if let Some(property_ty) =
                        checker.resolve_nominal_property_type(type_name, Some(type_args.as_slice()), field, span)
                    {
                        return property_ty;
                    }
                    checker.errors.push(errors::missing_field(type_name, field, span));
                    ResolvedType::Unknown
                }
                ResolvedType::TypeVar(name) => {
                    if field == "__name__" {
                        return ResolvedType::Str;
                    }
                    if let Some(property_ty) = checker.resolve_generic_placeholder_property(name, field, span) {
                        return property_ty;
                    }
                    checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
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

    /// Validate the RFC 006 `Generator[T].map(fn)` helper and return the mapped element type.
    fn generator_map_return_type(
        &mut self,
        elem: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> ResolvedType {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }

        let Some(arg_ty) = arg_types.first() else {
            return ResolvedType::Unknown;
        };
        let ResolvedType::Function(params, ret) = arg_ty else {
            self.errors.push(errors::type_mismatch(
                &format!("({elem}) -> _"),
                &arg_ty.to_string(),
                span,
            ));
            return ResolvedType::Unknown;
        };
        let [param] = params.as_slice() else {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return ResolvedType::Unknown;
        };
        if !self.types_compatible(elem, &param.ty) {
            self.errors
                .push(errors::type_mismatch(&param.ty.to_string(), &elem.to_string(), span));
        }
        ret.as_ref().clone()
    }

    /// Validate the RFC 006 `Generator[T].filter(predicate)` helper.
    fn validate_generator_filter_arg(
        &mut self,
        elem: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return;
        }

        let Some(arg_ty) = arg_types.first() else {
            return;
        };
        let ResolvedType::Function(params, ret) = arg_ty else {
            self.errors.push(errors::type_mismatch(
                &format!("({elem}) -> bool"),
                &arg_ty.to_string(),
                span,
            ));
            return;
        };
        let [param] = params.as_slice() else {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return;
        };
        if !self.types_compatible(elem, &param.ty) {
            self.errors
                .push(errors::type_mismatch(&param.ty.to_string(), &elem.to_string(), span));
        }
        if !self.types_compatible(ret, &ResolvedType::Bool) {
            self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
        }
    }

    /// Validate the RFC 006 `Generator[T].take(count)` helper.
    fn validate_generator_take_arg(&mut self, args: &[CallArg], arg_types: &[ResolvedType], span: Span) {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one int argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return;
        }
        if let Some(arg_ty) = arg_types.first()
            && !self.types_compatible(arg_ty, &ResolvedType::Int)
        {
            self.errors
                .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
        }
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
        if Self::is_explicit_builtin_namespace_expr(base) {
            let result = self.check_explicit_builtin_call(method, args, span);
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                return ResolvedType::Unknown;
            }
            return result;
        }

        if self.is_builtin_list_surface_receiver(base)
            && method == collection_helpers::member(BuiltinCollectionHelperId::ListRepeat)
        {
            if !type_args.is_empty() {
                self.errors.push(errors::type_mismatch(
                    "inferred type arguments",
                    "explicit type arguments",
                    span,
                ));
            }
            self.type_info
                .expressions
                .ident_kinds
                .insert((base.span.start, base.span.end), IdentKind::TypeName);
            return self.check_builtin_list_repeat_call(args, span);
        }

        let base_ty = self.check_type_receiver_expr(base);

        // If the receiver type is Unknown, be permissive and do not error on methods.
        if matches!(base_ty, ResolvedType::Unknown) {
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }
        if method == "to_vec"
            && args.is_empty()
            && matches!(
                base_ty,
                ResolvedType::Ref(ref inner) | ResolvedType::RefMut(ref inner)
                    if matches!(
                        inner.as_ref(),
                        ResolvedType::Generic(name, _)
                            if collection_type_id(name.as_str()) == Some(CollectionTypeId::List)
                    )
            )
            && let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = base_ty
        {
            return *inner;
        }

        if method == "to_vec"
            && args.is_empty()
            && matches!(
                base_ty,
                ResolvedType::Ref(ref inner) | ResolvedType::RefMut(ref inner)
                    if matches!(
                        inner.as_ref(),
                        ResolvedType::RustPath(path)
                            if Self::rust_ref_to_vec_returns_bytes(path)
                                || Self::rust_to_vec_receiver_is_known_byte_output(base)
                    )
            )
        {
            return ResolvedType::Bytes;
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

        if let Some(ret) =
            self.resolve_unambiguous_source_method_without_arg_prepass(&base_ty, method, type_args, args, span)
        {
            return ret;
        }

        let contextual_rust_callable = expected_return_ty.and_then(|expected| {
            if args.len() == 1 {
                self.rust_callable_alias_signature(expected)
            } else {
                None
            }
        });

        // Collect arg types for method-specific validation.
        let arg_types: Vec<ResolvedType> = args
            .iter()
            .map(|arg| self.check_method_arg_with_rust_callable_alias(arg, contextual_rust_callable.as_ref()))
            .collect();

        if self.receiver_has_computed_property(&base_ty, method, span) {
            self.errors.push(errors::property_called_as_method(method, span));
            return ResolvedType::Unknown;
        }

        if let Some(ret) = self.resolve_iterator_protocol_method_call(&base_ty, method, args, &arg_types, span) {
            self.mark_direct_iterator_binding_consumed(base, method, span);
            return ret;
        }

        if let Some(path) = self.rust_canonical_path_for_receiver_type(&base_ty) {
            if let Some(params) = self.rust_variant_callable_params(&path, method) {
                if !type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(span));
                }
                let arg_types = self.check_call_arg_types_for_params(args, &params);
                let mut type_bindings = std::collections::HashMap::new();
                self.validate_callable_arg_bindings(
                    format!("rust::{path}.{method}").as_str(),
                    &params,
                    args,
                    &arg_types,
                    &mut type_bindings,
                    span,
                );
                self.type_info.record_call_site_callable_params_exact(span, &params);
                return ResolvedType::RustPath(path);
            }
            if let Some(ret) = Self::known_rust_path_method_return(path.as_str(), method) {
                return ret;
            }
            let Some(ret) = self.resolve_rust_path_method_call(&path, method, args, &arg_types, base.span, span) else {
                // Metadata backend disabled/unavailable: preserve permissive RFC 005 behavior.
                return ResolvedType::Unknown;
            };
            return ret;
        }

        if let Some(ret) = self.check_numeric_resize_method(&base_ty, method, type_args, args, span, expected_return_ty)
        {
            return ret;
        }
        // Trait default methods typecheck against `Self`, so be permissive here too.
        if matches!(base_ty, ResolvedType::SelfType) {
            return ResolvedType::Unknown;
        }

        if self.nominal_type_supports_reflection_magic(&base_ty, method)
            && let Some(ret) = self.reflection_magic_method_return_type(&base_ty, method)
        {
            self.validate_reflection_magic_call(method, type_args, args, span);
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
            && (enum_info.variants.iter().any(|v| v == method) || enum_info.variant_aliases.contains_key(method))
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
        if (matches!(base_ty, ResolvedType::Float)
            || matches!(
                base_ty,
                ResolvedType::Numeric(
                    incan_core::lang::types::numerics::NumericTypeId::F32
                        | incan_core::lang::types::numerics::NumericTypeId::F64
                )
            ))
            && let Some(id) = float_methods::from_str(method)
        {
            use float_methods::FloatMethodId as M;
            match id {
                M::IsNan | M::IsInfinite | M::IsFinite => return ResolvedType::Bool,
                _ => return ResolvedType::Float,
            }
        }
        if matches!(base_ty, ResolvedType::Bytes) && method == "as_slice" && args.is_empty() {
            return ResolvedType::Bytes;
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
                        let is_unresolved_rust_generic = matches!(&t, ResolvedType::RustPath(path) if TypeChecker::rust_display_type_var_name(path).is_some());
                        if self.is_copy_type(&t) || self.is_generic_placeholder_type(&t) || is_unresolved_rust_generic {
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

        if let ResolvedType::Generic(name, type_args) = &base_ty
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
            && type_args.len() == 2
        {
            let ok_ty = type_args[0].clone();
            match result_methods::from_str(method) {
                Some(result_methods::ResultMethodId::Unwrap) => {
                    if !args.is_empty() {
                        self.errors.push(errors::type_mismatch(
                            "no arguments",
                            &format!("{} argument(s)", args.len()),
                            span,
                        ));
                    }
                    return ok_ty;
                }
                Some(result_methods::ResultMethodId::UnwrapOr) => {
                    if let Some(default_ty) = arg_types.first()
                        && !self.types_compatible(default_ty, &ok_ty)
                    {
                        self.errors
                            .push(errors::type_mismatch(&ok_ty.to_string(), &default_ty.to_string(), span));
                    }
                    if args.len() != 1 {
                        self.errors.push(errors::type_mismatch(
                            "one default argument",
                            &format!("{} argument(s)", args.len()),
                            span,
                        ));
                    }
                    return ok_ty;
                }
                _ => {}
            }
        }

        if let ResolvedType::Generic(name, type_args) = &base_ty
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
            && type_args.len() == 2
            && Self::result_combinator_name(method)
        {
            return self.check_result_combinator_method(
                type_args[0].clone(),
                type_args[1].clone(),
                method,
                args,
                &arg_types,
                span,
            );
        }

        // FIXME: Too many levels of nesting here.
        if let ResolvedType::Generic(name, type_args) = &base_ty {
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator) {
                let elem = type_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                use iterator_methods::IteratorMethodId as M;
                match iterator_methods::from_str(method) {
                    Some(M::Map) => {
                        let mapped = self.generator_map_return_type(&elem, args, &arg_types, span);
                        return generator_ty(mapped);
                    }
                    Some(M::Filter) => {
                        self.validate_generator_filter_arg(&elem, args, &arg_types, span);
                        return generator_ty(elem);
                    }
                    Some(M::Take) => {
                        self.validate_generator_take_arg(args, &arg_types, span);
                        return generator_ty(elem);
                    }
                    Some(M::Collect) => {
                        if !args.is_empty() {
                            self.errors.push(errors::type_mismatch(
                                "no arguments",
                                &format!("{} argument(s)", args.len()),
                                span,
                            ));
                        }
                        return list_ty(elem);
                    }
                    _ => {}
                }
            }

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
                        M::Clone => {
                            if !args.is_empty() {
                                self.errors.push(errors::type_mismatch(
                                    "no arguments",
                                    &format!("{} argument(s)", args.len()),
                                    span,
                                ));
                            }
                            if !self.is_copy_type(&elem) && !self.is_clone_type(&elem) {
                                self.errors
                                    .push(errors::list_clone_requires_clone(&elem.to_string(), span));
                            }
                            return list_ty(elem.clone());
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

        if let Some(ret) =
            self.resolve_union_clone_trait_method_call(&base_ty, method, type_args, args, &arg_types, span)
        {
            return ret;
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
                        Some(&newtype.method_overloads),
                        Some(&newtype.trait_adoptions),
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
                            Some(&nt.method_overloads),
                            Some(&nt.trait_adoptions),
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
            if let Some(ret) = self.generic_reflection_magic_method_return_type(method) {
                self.validate_reflection_magic_call(method, type_args, args, span);
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

    /// Resolve methods supplied by Clone for anonymous union wrappers.
    fn resolve_union_clone_trait_method_call(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        if !receiver_ty.is_union() {
            return None;
        }

        let adoption = TypeBoundInfo {
            name: core_traits::as_str(TraitId::Clone).to_string(),
            source_name: None,
            type_args: Vec::new(),
            module_path: None,
        };
        let method_info = self.trait_method_info_resolved_for_adoption(&adoption, method, span)?;
        if !self.is_clone_type(receiver_ty) {
            self.errors.push(CompileError::type_error(
                format!("Union type '{receiver_ty}' cannot use '{method}(...)' because not all variants are cloneable"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        Some(self.check_generic_method_call(method, method_info, type_args, args, arg_types, span, receiver_ty))
    }

    /// Return known method result types for Rust imports when rust-inspect metadata is not specific enough.
    fn known_rust_path_method_return(path: &str, method: &str) -> Option<ResolvedType> {
        use incan_core::lang::types::numerics::NumericTypeId as N;

        match (path, method) {
            ("xxhash_rust::xxh32::Xxh32", "digest") => Some(ResolvedType::Numeric(N::U32)),
            ("xxhash_rust::xxh64::Xxh64", "digest") => Some(ResolvedType::Numeric(N::U64)),
            ("xxhash_rust::xxh3::Xxh3Default", "digest") => Some(ResolvedType::Numeric(N::U64)),
            ("xxhash_rust::xxh3::Xxh3Default", "digest128") => Some(ResolvedType::Numeric(N::U128)),
            _ => None,
        }
    }

    /// Return whether a borrowed Rust value's `to_vec` method produces Incan `bytes`.
    fn rust_ref_to_vec_returns_bytes(path: &str) -> bool {
        path.starts_with("[u8") || (path.contains("GenericArray") && (path.contains("<u8") || path.contains("u8,")))
    }

    /// Return whether a `to_vec` receiver shape is known to produce digest bytes even when rust-inspect erases it.
    fn rust_to_vec_receiver_is_known_byte_output(base: &Spanned<Expr>) -> bool {
        match &base.node {
            Expr::MethodCall(_, method, _, _) => matches!(method.as_str(), "as_bytes" | "digest" | "finalize_reset"),
            _ => false,
        }
    }
}
