//! Generic call-site inference, monomorph recording, and explicit bound validation.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Span, Spanned, Type};
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::frontend::symbols::{CallableParam, FunctionInfo, MethodInfo, ResolvedType, TypeInfo};
use crate::frontend::typechecker::helpers::collection_type_id;
use incan_core::interop::is_rust_capability_bound;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;

impl TypeChecker {
    /// Validate generic function call type arguments, value arguments, and explicit type-parameter bounds.
    pub(in crate::frontend::typechecker::check_expr::calls) fn validate_function_call(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        let mut seeded_type_bindings: std::collections::HashMap<String, ResolvedType> =
            std::collections::HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != info.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    func_name,
                    info.type_params.len(),
                    explicit_type_args.len(),
                    call_span,
                ));
            } else {
                let resolved_explicit: Vec<ResolvedType> = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect();
                seeded_type_bindings = type_param_subst_map_call_site(&info.type_params, &resolved_explicit);
            }
        }
        let params_with_explicit: Vec<CallableParam> = info
            .params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: substitute_resolved_type(&param.ty, &seeded_type_bindings),
                kind: param.kind,
                has_default: param.has_default,
            })
            .collect();
        let arg_types = self.check_call_arg_types_for_params(args, &params_with_explicit);
        let mut type_bindings = seeded_type_bindings;
        self.validate_callable_arg_bindings(
            func_name,
            &params_with_explicit,
            args,
            &arg_types,
            &mut type_bindings,
            call_span,
        );
        self.emit_explicit_bound_errors(
            func_name,
            &info.type_param_bounds,
            &info.type_param_bound_details,
            &type_bindings,
            call_span,
        );

        let explicit_arity_ok = explicit_type_args.is_empty() || explicit_type_args.len() == info.type_params.len();
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(func_name, &info.type_params, &type_bindings, call_span);
            self.record_call_site_monomorph_if_complete(call_span, &info.type_params, &type_bindings);
        }

        substitute_resolved_type(&info.return_type, &type_bindings)
    }

    fn assert_call_site_type_params_inferred(
        &mut self,
        callee: &str,
        type_params: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        span: Span,
    ) {
        for p in type_params {
            let ok = match bindings.get(p) {
                Some(ty) => !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer),
                None => false,
            };
            if !ok {
                self.errors
                    .push(errors::call_site_type_inference_unresolved(callee, p, span));
            }
        }
    }

    fn record_call_site_monomorph_if_complete(
        &mut self,
        call_span: Span,
        type_params: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        let mut out: Vec<ResolvedType> = Vec::new();
        for p in type_params {
            let Some(ty) = bindings.get(p) else {
                return;
            };
            if matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer) {
                return;
            }
            out.push(ty.clone());
        }
        self.type_info
            .call_site_monomorph_type_args
            .insert((call_span.start, call_span.end), out);
    }

    /// Type-check a resolved [`MethodInfo`] for a call site that may include explicit bracketed type arguments (RFC
    /// 054).
    ///
    /// Pipeline role: invoked from [`TypeChecker::resolve_named_method`] after a concrete method has been chosen
    /// (inherent or trait).
    ///
    /// This runs the full generic call-site path for methods:
    /// - Validates arity when `explicit_type_args` is nonempty.
    /// - Builds a partial substitution map (skipping [`ResolvedType::CallSiteInfer`] for `_` slots), applies it to the
    ///   method’s declared parameter and return types, then substitutes call-site `Self` via
    ///   [`TypeChecker::method_types_substituting_call_site_self`].
    /// - Validates value arguments against the specialized formals, then runs [`Self::infer_type_param_bindings`] so
    ///   remaining type parameters are filled from argument types.
    /// - Enforces explicit `with` bounds, requires every method type parameter to be concretely bound when brackets
    ///   were present, and records `TypeCheckInfo::call_site_monomorph_type_args` for lowering.
    ///
    /// # Parameters
    ///
    /// - `method`: Method name (for diagnostics).
    /// - `method_info`: Declared [`MethodInfo`] for that method (owned and temporarily mutated for substitution).
    /// - `explicit_type_args`: AST types inside `[...]` before `(`; empty if the call omitted brackets.
    /// - `args` / `arg_types`: Call arguments and their already-checked types (parallel to `args`).
    /// - `call_site_span`: Span of the whole `MethodCall` expression (monomorph snapshot key).
    /// - `receiver_ty`: Resolved type of the receiver expression.
    ///
    /// # Returns
    ///
    /// The method’s return type after substituting inferred bindings into `return_type` (post–`Self` substitution).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::frontend::typechecker::check_expr) fn check_generic_method_call(
        &mut self,
        method: &str,
        mut method_info: MethodInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
    ) -> ResolvedType {
        let mut type_bindings: std::collections::HashMap<String, ResolvedType> = std::collections::HashMap::new();
        let explicit_arity_ok =
            explicit_type_args.is_empty() || explicit_type_args.len() == method_info.type_params.len();

        // ---- RFC 054: explicit bracketed type arguments (partial map; `_` → CallSiteInfer omitted) ----
        if !explicit_type_args.is_empty() {
            if !explicit_arity_ok {
                self.errors.push(errors::explicit_type_arg_arity(
                    method,
                    method_info.type_params.len(),
                    explicit_type_args.len(),
                    call_site_span,
                ));
            } else {
                let resolved: Vec<ResolvedType> = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect();
                type_bindings = type_param_subst_map_call_site(&method_info.type_params, &resolved);
                method_info.params = method_info
                    .params
                    .iter()
                    .map(|param| CallableParam {
                        name: param.name.clone(),
                        ty: substitute_resolved_type(&param.ty, &type_bindings),
                        kind: param.kind,
                        has_default: param.has_default,
                    })
                    .collect();
                method_info.return_type = substitute_resolved_type(&method_info.return_type, &type_bindings);
            }
        }

        // ---- Call-site `Self`, value-arg compatibility ----
        let (params, return_type) = self.method_types_substituting_call_site_self(&method_info, receiver_ty);
        self.validate_callable_arg_bindings(method, &params, args, arg_types, &mut type_bindings, call_site_span);
        self.type_info.record_call_site_callable_params(call_site_span, &params);

        self.emit_explicit_bound_errors(
            method,
            &method_info.type_param_bounds,
            &method_info.type_param_bound_details,
            &type_bindings,
            call_site_span,
        );

        // ---- Require concrete bindings; snapshot monomorphs for lowering when brackets were used ----
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(
                method,
                &method_info.type_params,
                &type_bindings,
                call_site_span,
            );
            self.record_call_site_monomorph_if_complete(call_site_span, &method_info.type_params, &type_bindings);
        }

        substitute_resolved_type(&return_type, &type_bindings)
    }

    /// Infer concrete type bindings for generic type parameters from a parameter/argument type pair.
    ///
    /// This walks matching container structure recursively so constructor field checks and function calls can recover
    /// bindings such as `T -> String` from shapes like `Boxed[T]` versus `Boxed[String]`.
    pub(in crate::frontend::typechecker::check_expr::calls) fn infer_type_param_bindings(
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
                        self.infer_type_param_bindings(&e.ty, &a.ty, bindings);
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
                } else if let ResolvedType::RefMut(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::RefMut(inner) => {
                if let ResolvedType::RefMut(actual_inner) = actual {
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
        bound_details_by_param: &std::collections::HashMap<String, Vec<crate::frontend::symbols::TypeBoundInfo>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        for (type_param, bounds) in bounds_by_param {
            let Some(actual_ty) = bindings.get(type_param) else {
                continue;
            };
            if let Some(details) = bound_details_by_param.get(type_param)
                && !details.is_empty()
            {
                for bound in details {
                    if !self.type_satisfies_explicit_bound_info(actual_ty, bound, bindings) {
                        self.errors.push(errors::generic_bound_not_satisfied(
                            func_name,
                            type_param,
                            &self.type_bound_display(bound, bindings),
                            &actual_ty.to_string(),
                            call_span,
                        ));
                    }
                }
                continue;
            }
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

    /// Render a type-parameter bound with call-site substitutions applied.
    fn type_bound_display(
        &self,
        bound: &crate::frontend::symbols::TypeBoundInfo,
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> String {
        if bound.type_args.is_empty() {
            return bound.name.clone();
        }
        let args = bound
            .type_args
            .iter()
            .map(|arg| substitute_resolved_type(arg, bindings).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}[{}]", bound.name, args)
    }

    /// Return whether a type satisfies one explicit bound, including generic trait arguments.
    pub(crate) fn type_satisfies_explicit_bound_info(
        &self,
        ty: &ResolvedType,
        bound: &crate::frontend::symbols::TypeBoundInfo,
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> bool {
        if bound.type_args.is_empty() {
            return self.type_satisfies_explicit_bound(ty, &bound.name);
        }
        if is_rust_capability_bound(&bound.name) {
            return true;
        }
        if builtin_traits::from_str(&bound.name).is_some() || self.lookup_semantic_trait_info(&bound.name).is_none() {
            return self.type_satisfies_explicit_bound(ty, &bound.name);
        }
        let expected_args = bound
            .type_args
            .iter()
            .map(|arg| substitute_resolved_type(arg, bindings))
            .collect::<Vec<_>>();
        self.type_satisfies_nominal_trait_bound_with_args(ty, &bound.name, &expected_args)
    }

    /// Best-effort check whether a concrete type satisfies an explicit generic bound.
    fn type_satisfies_explicit_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        // `std.rust` markers (`Send`, `Sync`, …) are enforced when lowering to Rust, not here.
        if is_rust_capability_bound(bound) {
            return true;
        }
        // For non-builtin traits, apply nominal trait/supertrait compatibility (RFC 042) directly.
        //
        // This keeps capability checks language-general and avoids ad hoc receiver-category gating.
        if builtin_traits::from_str(bound).is_none() && self.lookup_semantic_trait_info(bound).is_some() {
            return self.type_satisfies_nominal_trait_bound(ty, bound);
        }
        match ty {
            // Unknown / still-generic types are kept permissive to avoid cascading errors.
            ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer => true,
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
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
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.type_satisfies_explicit_bound(inner, bound),
            ResolvedType::Function(_, _) | ResolvedType::SelfType => false,
        }
    }

    /// Check whether `ty` satisfies a nominal trait bound `bound_trait` under RFC 042 semantics.
    ///
    /// This path is used for non-builtin traits. It intentionally reuses existing trait compatibility helpers:
    /// - concrete adopters satisfy direct and transitive supertraits via `type_implements_trait`
    /// - trait-typed values satisfy broader traits via `trait_is_supertrait_of`
    fn type_satisfies_nominal_trait_bound(&self, ty: &ResolvedType, bound_trait: &str) -> bool {
        match ty {
            // Keep unknown / generic placeholders permissive to avoid cascading diagnostics.
            ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer => true,
            ResolvedType::Named(type_name) => {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, bound_trait)
                } else {
                    self.type_implements_trait(type_name, bound_trait)
                }
            }
            ResolvedType::Generic(type_name, _args) => {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, bound_trait)
                } else if self.lookup_semantic_type_info(type_name).is_some() {
                    self.type_implements_trait(type_name, bound_trait)
                } else {
                    false
                }
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                self.type_satisfies_nominal_trait_bound(inner, bound_trait)
            }
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Tuple(_)
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::Function(_, _)
            | ResolvedType::SelfType => false,
        }
    }

    /// Return whether a nominal type satisfies a trait bound with exact expected trait arguments.
    fn type_satisfies_nominal_trait_bound_with_args(
        &self,
        ty: &ResolvedType,
        bound_trait: &str,
        expected_args: &[ResolvedType],
    ) -> bool {
        match ty {
            ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer => true,
            ResolvedType::Named(type_name) => {
                self.type_implements_trait_with_args(type_name, &[], bound_trait, expected_args)
            }
            ResolvedType::Generic(type_name, type_args) => {
                self.type_implements_trait_with_args(type_name, type_args, bound_trait, expected_args)
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                self.type_satisfies_nominal_trait_bound_with_args(inner, bound_trait, expected_args)
            }
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Tuple(_)
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::Function(_, _)
            | ResolvedType::SelfType => false,
        }
    }

    /// Check a concrete model/class adoption list for a matching generic trait instantiation.
    fn type_implements_trait_with_args(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        bound_trait: &str,
        expected_args: &[ResolvedType],
    ) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return false;
        };
        let (owner_type_params, adoptions, derives) = match info {
            TypeInfo::Model(model) => (
                model.type_params.as_slice(),
                model.trait_adoptions.as_slice(),
                Some(model.derives.as_slice()),
            ),
            TypeInfo::Class(class) => (
                class.type_params.as_slice(),
                class.trait_adoptions.as_slice(),
                Some(class.derives.as_slice()),
            ),
            TypeInfo::Enum(en) => (
                en.type_params.as_slice(),
                en.trait_adoptions.as_slice(),
                Some(en.derives.as_slice()),
            ),
            TypeInfo::Builtin | TypeInfo::Newtype(_) | TypeInfo::TypeAlias => return false,
        };

        if expected_args.is_empty()
            && derives.is_some_and(|items| items.iter().any(|derive| derive == bound_trait))
            && self.lookup_semantic_trait_info(bound_trait).is_some()
        {
            return true;
        }

        let owner_subst =
            crate::frontend::resolved_type_subst::type_param_subst_map(owner_type_params, concrete_type_args);
        for adoption in adoptions {
            let Some(adopted_info) = self.lookup_semantic_trait_info(&adoption.name) else {
                continue;
            };
            let direct_args = if adoption.type_args.is_empty() {
                concrete_type_args
                    .iter()
                    .take(adopted_info.type_params.len())
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                adoption
                    .type_args
                    .iter()
                    .map(|arg| substitute_resolved_type(arg, &owner_subst))
                    .collect::<Vec<_>>()
            };
            if direct_args.len() != adopted_info.type_params.len() {
                continue;
            }
            if adoption.name == bound_trait && self.trait_args_match(&direct_args, expected_args) {
                return true;
            }

            let subst =
                crate::frontend::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
            for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(&adoption.name) {
                if supertrait_name != bound_trait {
                    continue;
                }
                let instantiated = supertrait_args
                    .iter()
                    .map(|arg| substitute_resolved_type(arg, &subst))
                    .collect::<Vec<_>>();
                if self.trait_args_match(&instantiated, expected_args) {
                    return true;
                }
            }
        }
        false
    }

    /// Compare instantiated trait arguments using the typechecker's compatibility relation.
    fn trait_args_match(&self, actual_args: &[ResolvedType], expected_args: &[ResolvedType]) -> bool {
        actual_args.len() == expected_args.len()
            && actual_args
                .iter()
                .zip(expected_args.iter())
                .all(|(actual, expected)| self.types_compatible(actual, expected))
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

    /// Return whether a named user type explicitly satisfies a generic trait bound.
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
                info.traits.iter().any(|t| t == bound) || info.derives.iter().any(|d| d == bound)
            }
            Some(TypeInfo::Newtype(_)) => false,
            Some(TypeInfo::TypeAlias) => false,
            None => false,
        }
    }
}
