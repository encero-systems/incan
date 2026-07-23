//! Trait-bound satisfaction and temporary capability bridges.

use std::collections::{HashMap, HashSet};

use super::TypeChecker;
use crate::frontend::resolved_type_subst::substitute_resolved_type;
use crate::frontend::symbols::{ResolvedType, TypeBoundInfo, TypeInfo};
use crate::frontend::typechecker::helpers::collection_type_id;
use incan_core::interop::is_rust_capability_bound;
use incan_core::lang::callables;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::trait_capabilities::{
    self, TraitCapabilityId, TraitCapabilityInfo, TraitCapabilityType, TraitCapabilityTypeArg,
};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics;

impl TypeChecker {
    /// Render a type-parameter bound with call-site substitutions applied.
    pub(in crate::frontend::typechecker) fn type_bound_display(
        &self,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
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
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if let Some(placeholder_name) = self.active_type_param_name(ty)
            && self.active_type_param_satisfies_bound_info(placeholder_name, bound, bindings)
        {
            return true;
        }
        if bound.name == builtin_traits::as_str(TraitId::Awaitable) {
            let expected_output = bound
                .type_args
                .first()
                .map(|arg| substitute_resolved_type(arg, bindings));
            return self.type_satisfies_awaitable_bound(ty, expected_output.as_ref());
        }
        if let Some(satisfies) = self.function_type_satisfies_callable_bound(ty, bound, bindings) {
            return satisfies;
        }
        if let Some(capability) = self.temporary_trait_capability_for_bound_info(bound, bindings)
            && let Some(satisfies) = self.temporary_trait_capability_supports_type(capability, ty)
        {
            return satisfies;
        }
        if bound.type_args.is_empty() {
            return self.type_satisfies_explicit_bound(ty, &bound.name);
        }
        if is_rust_capability_bound(&bound.name) {
            return true;
        }
        let expected_args = bound
            .type_args
            .iter()
            .map(|arg| substitute_resolved_type(arg, bindings))
            .collect::<Vec<_>>();
        if builtin_traits::from_str(&bound.name).is_some() {
            return self.type_satisfies_nominal_trait_bound_with_args(ty, &bound.name, &expected_args);
        }
        if self.lookup_semantic_trait_info(&bound.name).is_none() {
            return self.type_satisfies_explicit_bound(ty, &bound.name);
        }
        self.type_satisfies_nominal_trait_bound_with_args(ty, &bound.name, &expected_args)
    }

    /// Match a function or closure value against the exact `std.traits.callable.CallableN` signature.
    fn function_type_satisfies_callable_bound(
        &self,
        ty: &ResolvedType,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<bool> {
        let callable = self.callable_trait_for_bound(bound)?;
        let ResolvedType::Function(params, return_type) = ty else {
            return None;
        };
        let arity = callables::info_for(callable).arity;
        if params.len() != arity || bound.type_args.len() != arity + 1 {
            return Some(false);
        }
        let expected = bound
            .type_args
            .iter()
            .map(|arg| substitute_resolved_type(arg, bindings))
            .collect::<Vec<_>>();
        let params_match = params
            .iter()
            .zip(&expected[..arity])
            .all(|(actual, expected)| self.types_compatible(&actual.ty, expected));
        Some(params_match && self.types_compatible(return_type, &expected[arity]))
    }

    /// Resolve a checked bound to the canonical source callable trait registry.
    pub(in crate::frontend::typechecker) fn callable_trait_for_bound(
        &self,
        bound: &TypeBoundInfo,
    ) -> Option<callables::CallableTraitId> {
        if let Some(module_path) = &bound.module_path {
            if !callables::module_path_matches(module_path) {
                return None;
            }
            return callables::from_str(Self::type_bound_source_name(bound));
        }
        let (module_path, trait_name) = self.resolve_bound_trait_path(&bound.name)?;
        callables::module_path_matches(&module_path)
            .then(|| callables::from_str(&trait_name))
            .flatten()
    }

    /// Best-effort check whether a concrete type satisfies an explicit generic bound.
    pub(in crate::frontend::typechecker) fn type_satisfies_explicit_bound(
        &self,
        ty: &ResolvedType,
        bound: &str,
    ) -> bool {
        if bound == builtin_traits::as_str(TraitId::Awaitable) {
            return self.type_satisfies_awaitable_bound(ty, None);
        }
        if is_rust_capability_bound(bound) {
            return true;
        }
        if let Some(capability) = self.temporary_trait_capability_for_bound(bound)
            && let Some(satisfies) = self.temporary_trait_capability_supports_type(capability, ty)
        {
            return satisfies;
        }
        if builtin_traits::from_str(bound).is_none() && self.lookup_semantic_trait_info(bound).is_some() {
            return self.type_satisfies_nominal_trait_bound(ty, bound);
        }
        match ty {
            ResolvedType::Never
            | ResolvedType::Unknown
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
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) | ResolvedType::TypeToken(inner) => {
                self.type_satisfies_explicit_bound(inner, bound)
            }
            // Incan closures lower as non-`move` Rust `Fn` values. Their captures are borrowed, so the callable value
            // itself is cloneable even when a borrowed referent is not. This lets source-owned lazy adapters retain a
            // callback under ordinary Incan value semantics without reducing the API to Rust function pointers.
            ResolvedType::Function(_, _) => bound == derives::as_str(DeriveId::Clone),
            ResolvedType::SelfType => false,
        }
    }

    /// Return the active generic placeholder name represented by `ty`.
    pub(in crate::frontend::typechecker) fn active_type_param_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        let name = match ty {
            ResolvedType::TypeVar(name) | ResolvedType::Named(name) => name,
            _ => return None,
        };
        self.current_type_param_bound_details
            .iter()
            .rev()
            .any(|frame| frame.contains_key(name))
            .then_some(name.as_str())
    }

    /// Check whether an active generic placeholder already carries the bound required by a nested generic call.
    fn active_type_param_satisfies_bound_info(
        &self,
        placeholder_name: &str,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        for frame in self.current_type_param_bound_details.iter().rev() {
            let Some(active_bounds) = frame.get(placeholder_name) else {
                continue;
            };
            for active in active_bounds {
                if self.type_bound_implies_bound_info(active, required, bindings) {
                    return true;
                }
            }
            return false;
        }
        false
    }

    /// Return the resolved source trait item name for a bound, falling back to the visible spelling.
    pub(in crate::frontend::typechecker) fn type_bound_source_name(bound: &TypeBoundInfo) -> &str {
        bound
            .source_name
            .as_deref()
            .unwrap_or_else(|| bound.name.rsplit('.').next().unwrap_or(bound.name.as_str()))
    }

    /// Return the canonical source identity for a checked trait bound when it can be resolved.
    fn type_bound_identity(&self, bound: &TypeBoundInfo) -> Option<(Vec<String>, String)> {
        if let Some(module_path) = &bound.module_path {
            return Some((module_path.clone(), Self::type_bound_source_name(bound).to_string()));
        }
        let (module_path, resolved_name) = self.resolve_bound_trait_path(&bound.name)?;
        Some((module_path, bound.source_name.clone().unwrap_or(resolved_name)))
    }

    /// Return whether two bound records identify the same trait, accounting for import aliases.
    fn type_bound_names_match(&self, left: &TypeBoundInfo, right: &TypeBoundInfo) -> bool {
        match (self.type_bound_identity(left), self.type_bound_identity(right)) {
            (Some(left), Some(right)) => left == right,
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => {
                left.name == right.name && Self::type_bound_source_name(left) == Self::type_bound_source_name(right)
            }
        }
    }

    /// Compare one active and required bound after applying call-site substitutions.
    fn type_bound_args_match(
        &self,
        active: &TypeBoundInfo,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if required.type_args.is_empty() {
            return true;
        }
        active.type_args.len() == required.type_args.len()
            && active
                .type_args
                .iter()
                .zip(&required.type_args)
                .all(|(actual, expected)| {
                    let actual = substitute_resolved_type(actual, bindings);
                    let expected = substitute_resolved_type(expected, bindings);
                    self.types_compatible(&actual, &expected)
                })
    }

    /// Return whether one checked bound directly or transitively guarantees another bound.
    fn type_bound_implies_bound_info(
        &self,
        active: &TypeBoundInfo,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if self.type_bound_names_match(active, required) && self.type_bound_args_match(active, required, bindings) {
            return true;
        }

        let Some(active_trait) = self.lookup_semantic_trait_info(&active.name) else {
            return false;
        };
        let active_args = active
            .type_args
            .iter()
            .map(|arg| substitute_resolved_type(arg, bindings))
            .collect::<Vec<_>>();
        if active_args.len() != active_trait.type_params.len() {
            return false;
        }
        let substitutions =
            crate::frontend::resolved_type_subst::type_param_subst_map(&active_trait.type_params, &active_args);
        self.semantic_supertrait_closure(&active.name)
            .into_iter()
            .any(|(name, type_args)| {
                let (module_path, resolved_name) = self.resolve_bound_trait_path(&name).unzip();
                let candidate = TypeBoundInfo {
                    name,
                    source_name: resolved_name,
                    type_args: type_args
                        .iter()
                        .map(|arg| substitute_resolved_type(arg, &substitutions))
                        .collect(),
                    module_path,
                };
                self.type_bound_names_match(&candidate, required)
                    && self.type_bound_args_match(&candidate, required, bindings)
            })
    }

    /// Check whether `ty` satisfies a nominal trait bound `bound_trait` under RFC 042 semantics.
    fn type_satisfies_nominal_trait_bound(&self, ty: &ResolvedType, bound_trait: &str) -> bool {
        match ty {
            ResolvedType::Never
            | ResolvedType::Unknown
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
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) | ResolvedType::TypeToken(inner) => {
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
            ResolvedType::Never
            | ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer => true,
            ResolvedType::Named(type_name) => {
                self.type_implements_trait_with_args(type_name, &[], bound_trait, expected_args)
            }
            ResolvedType::Generic(type_name, type_args) => {
                self.type_implements_trait_with_args(type_name, type_args, bound_trait, expected_args)
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) | ResolvedType::TypeToken(inner) => {
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
            TypeInfo::Newtype(newtype) => (
                newtype.type_params.as_slice(),
                newtype.trait_adoptions.as_slice(),
                Some(newtype.derives.as_slice()),
            ),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return false,
        };

        if expected_args.is_empty()
            && derives.is_some_and(|items| {
                items
                    .iter()
                    .any(|derive| self.builtin_derive_satisfies_trait(derive, bound_trait))
            })
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
            if self.trait_name_matches(&adoption.name, bound_trait)
                && self.trait_args_match(&direct_args, expected_args)
            {
                return true;
            }

            let subst =
                crate::frontend::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
            for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(&adoption.name) {
                if !self.trait_name_matches(&supertrait_name, bound_trait) {
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

    /// Return whether a primitive type satisfies a builtin or registry-backed temporary capability bound.
    fn primitive_type_satisfies_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        if bound == derives::as_str(DeriveId::Copy) {
            return self.is_copy_type(ty);
        }
        if let Some(capability) = self.temporary_trait_capability_for_bound(bound)
            && let Some(satisfies) = self.temporary_trait_capability_supports_type(capability, ty)
        {
            return satisfies;
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
            Some(TraitId::Awaitable) => self.type_satisfies_awaitable_bound(ty, None),
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

    /// Resolve a temporary trait-owned capability bridge for a bound.
    fn temporary_trait_capability_for_bound(&self, bound: &str) -> Option<&'static TraitCapabilityInfo> {
        let (module_path, trait_name) = self.resolve_bound_trait_path(bound)?;
        let capability = trait_capabilities::for_trait_path(&module_path, &trait_name)?;
        if !capability.required_type_args.is_empty() {
            return None;
        }
        self.validated_temporary_trait_capability(capability, bound, None, None)
    }

    /// Resolve a temporary capability bridge from a checked bound that may have crossed a package manifest boundary.
    fn temporary_trait_capability_for_bound_info(
        &self,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<&'static TraitCapabilityInfo> {
        let capability = if let Some(module_path) = &bound.module_path {
            let trait_name = Self::type_bound_source_name(bound);
            let capability = trait_capabilities::for_trait_path(module_path, trait_name)?;
            self.validated_temporary_trait_capability(
                capability,
                &bound.name,
                bound.source_name.as_deref(),
                Some(module_path),
            )?
        } else {
            let (module_path, trait_name) = self.resolve_bound_trait_path(&bound.name)?;
            let capability = trait_capabilities::for_trait_path(&module_path, &trait_name)?;
            self.validated_temporary_trait_capability(capability, &bound.name, None, None)?
        };
        self.capability_type_args_match(capability, bound, bindings)
            .then_some(capability)
    }

    /// Return whether a checked trait bound carries the concrete arguments required by a capability bridge.
    fn capability_type_args_match(
        &self,
        capability: &TraitCapabilityInfo,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        capability.required_type_args.len() == bound.type_args.len()
            && capability
                .required_type_args
                .iter()
                .zip(&bound.type_args)
                .all(|(required, actual)| {
                    let actual = substitute_resolved_type(actual, bindings);
                    matches!((required, actual), (TraitCapabilityTypeArg::Str, ResolvedType::Str))
                })
    }

    /// Validate that a temporary capability bridge points at a real trait with the required semantic surface.
    fn validated_temporary_trait_capability(
        &self,
        capability: &'static TraitCapabilityInfo,
        visible_bound: &str,
        source_name: Option<&str>,
        module_path: Option<&[String]>,
    ) -> Option<&'static TraitCapabilityInfo> {
        let info = self
            .lookup_semantic_trait_info(visible_bound)
            .or_else(|| source_name.and_then(|name| self.lookup_semantic_trait_info(name)))
            .or_else(|| self.lookup_semantic_trait_info(capability.trait_name));
        if let Some(info) = info
            && capability
                .required_methods
                .iter()
                .all(|method| info.methods.contains_key(*method))
        {
            return Some(capability);
        }
        let resolved_source_name = source_name.unwrap_or_else(|| {
            visible_bound
                .rsplit(['.', ':'])
                .find(|segment| !segment.is_empty())
                .unwrap_or(visible_bound)
        });
        let manifest_bound_identifies_capability = resolved_source_name == capability.trait_name
            && module_path.is_some_and(|path| trait_capabilities::module_path_matches(capability, path));
        manifest_bound_identifies_capability.then_some(capability)
    }

    /// Resolve a bound spelling to its defining module path and trait name.
    pub(in crate::frontend::typechecker) fn resolve_bound_trait_path(
        &self,
        bound: &str,
    ) -> Option<(Vec<String>, String)> {
        if let Some(path) = self.import_aliases.get(bound)
            && path.len() >= 2
        {
            let trait_name = path.last()?.clone();
            let module_path = path[..path.len() - 1].to_vec();
            return Some((module_path, trait_name));
        }
        if !bound.contains('.') {
            let module_path = self.current_module_path.clone()?;
            return Some((module_path, bound.to_string()));
        }
        let (module_name, trait_name) = bound.rsplit_once('.')?;
        let module_path = self.module_path_for_imported_name(module_name)?;
        Some((module_path, trait_name.to_string()))
    }

    /// Return temporary trait satisfaction when known, provisionally accepting unresolved inference categories.
    pub(in crate::frontend::typechecker) fn temporary_trait_capability_supports_type(
        &self,
        capability: &TraitCapabilityInfo,
        ty: &ResolvedType,
    ) -> Option<bool> {
        self.temporary_trait_capability_supports_type_inner(capability, ty, &mut HashSet::new())
    }

    /// Recursively evaluate a capability family, including transparent validated-newtype composition.
    fn temporary_trait_capability_supports_type_inner(
        &self,
        capability: &TraitCapabilityInfo,
        ty: &ResolvedType,
        seen_newtypes: &mut HashSet<String>,
    ) -> Option<bool> {
        match ty {
            ResolvedType::Never | ResolvedType::Unknown | ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer => {
                Some(true)
            }
            // A Rust-backed value has no source-owned `TryFrom[str]` contract. Treating it as provisionally
            // supported causes the compiler to synthesize an impl with an unsatisfied Rust backing bound when a
            // source newtype wraps a host type (for example Tokio synchronization primitives).
            ResolvedType::RustPath(_) => Some(false),
            ResolvedType::Int => Some(trait_capabilities::supports_type(capability, TraitCapabilityType::Int)),
            ResolvedType::Float => Some(trait_capabilities::supports_type(
                capability,
                TraitCapabilityType::Float,
            )),
            ResolvedType::Bool => Some(trait_capabilities::supports_type(capability, TraitCapabilityType::Bool)),
            ResolvedType::Str => Some(trait_capabilities::supports_type(capability, TraitCapabilityType::Str)),
            ResolvedType::Bytes => Some(trait_capabilities::supports_type(
                capability,
                TraitCapabilityType::Bytes,
            )),
            ResolvedType::Numeric(id) => Some(trait_capabilities::supports_type(
                capability,
                TraitCapabilityType::Numeric(*id),
            )),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) | ResolvedType::TypeToken(inner) => {
                self.temporary_trait_capability_supports_type_inner(capability, inner, seen_newtypes)
            }
            ResolvedType::Generic(name, args)
                if numerics::decimal_constructor_from_str(name.as_str()).is_some()
                    && args.len() == 2
                    && args
                        .iter()
                        .all(|arg| matches!(arg, ResolvedType::TypeVar(value) if value.parse::<u8>().is_ok())) =>
            {
                Some(trait_capabilities::supports_type(
                    capability,
                    TraitCapabilityType::Decimal,
                ))
            }
            ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)
                if self.value_enum_type_satisfies_temporary_trait_capability(type_name) =>
            {
                Some(trait_capabilities::supports_type(
                    capability,
                    TraitCapabilityType::ValueEnum,
                ))
            }
            ResolvedType::Named(type_name)
                if matches!(
                    capability.id,
                    TraitCapabilityId::StringTryFrom | TraitCapabilityId::IteratorSum
                ) =>
            {
                Some(
                    self.nominal_type_explicitly_adopts_temporary_capability(type_name, &[], capability)
                        || self
                            .newtype_satisfies_string_try_from(type_name, &[], capability, seen_newtypes)
                            .unwrap_or(false),
                )
            }
            ResolvedType::Generic(type_name, type_args)
                if matches!(
                    capability.id,
                    TraitCapabilityId::StringTryFrom | TraitCapabilityId::IteratorSum
                ) =>
            {
                Some(
                    self.nominal_type_explicitly_adopts_temporary_capability(type_name, type_args, capability)
                        || self
                            .newtype_satisfies_string_try_from(type_name, type_args, capability, seen_newtypes)
                            .unwrap_or(false),
                )
            }
            ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Tuple(_)
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::Function(_, _)
            | ResolvedType::SelfType => Some(false),
            ResolvedType::Generic(_, _) | ResolvedType::Named(_) => None,
        }
    }

    /// Match a concrete type's checked adoption or supertrait closure against a temporary capability identity.
    ///
    /// Package consumers may carry the adoption in `.incnlib` metadata without importing the provider's trait symbol
    /// into their own source scope. Canonical module/source identity and concrete trait arguments are sufficient.
    fn nominal_type_explicitly_adopts_temporary_capability(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        capability: &TraitCapabilityInfo,
    ) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return false;
        };
        let (owner_type_params, adoptions) = match info {
            TypeInfo::Model(model) => (model.type_params.as_slice(), model.trait_adoptions.as_slice()),
            TypeInfo::Class(class) => (class.type_params.as_slice(), class.trait_adoptions.as_slice()),
            TypeInfo::Enum(en) => (en.type_params.as_slice(), en.trait_adoptions.as_slice()),
            TypeInfo::Newtype(newtype) => (newtype.type_params.as_slice(), newtype.trait_adoptions.as_slice()),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return false,
        };
        if owner_type_params.len() != concrete_type_args.len() {
            return false;
        }
        let substitutions =
            crate::frontend::resolved_type_subst::type_param_subst_map(owner_type_params, concrete_type_args);
        let required = TypeBoundInfo {
            name: capability.trait_name.to_string(),
            source_name: Some(capability.trait_name.to_string()),
            type_args: capability
                .required_type_args
                .iter()
                .map(|arg| match arg {
                    TraitCapabilityTypeArg::Str => ResolvedType::Str,
                })
                .collect(),
            module_path: Some(
                capability
                    .module_path
                    .iter()
                    .map(|segment| (*segment).to_string())
                    .collect(),
            ),
        };
        adoptions
            .iter()
            .any(|adoption| self.type_bound_implies_bound_info(adoption, &required, &substitutions))
    }

    /// Return compiler-provided `TryFrom[str]` conformance for a concrete Incan newtype.
    fn newtype_satisfies_string_try_from(
        &self,
        type_name: &str,
        type_args: &[ResolvedType],
        capability: &TraitCapabilityInfo,
        seen_newtypes: &mut HashSet<String>,
    ) -> Option<bool> {
        let Some(TypeInfo::Newtype(info)) = self.lookup_semantic_type_info(type_name) else {
            return None;
        };
        if info.is_rusttype || info.type_params.len() != type_args.len() || !seen_newtypes.insert(type_name.to_string())
        {
            return Some(false);
        }

        let substitutions = crate::frontend::resolved_type_subst::type_param_subst_map(&info.type_params, type_args);
        let underlying = substitute_resolved_type(&info.underlying, &substitutions);
        let supported = self
            .temporary_trait_capability_supports_type_inner(capability, &underlying, seen_newtypes)
            .unwrap_or(false);
        seen_newtypes.remove(type_name);
        Some(supported)
    }

    /// Return whether a nominal type is a stable scalar value enum category for temporary capability bridges.
    fn value_enum_type_satisfies_temporary_trait_capability(&self, type_name: &str) -> bool {
        matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Enum(info)) if info.value_enum.is_some()
        )
    }

    /// Return whether a tuple type satisfies a trait bound.
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

    /// Return whether a collection type satisfies a trait bound.
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

    /// Return whether `ty` is one of the checked await-realization paths for `Awaitable[T]`.
    fn type_satisfies_awaitable_bound(&self, ty: &ResolvedType, expected_output: Option<&ResolvedType>) -> bool {
        let Some(output_ty) = self.await_output_type_from_type(ty) else {
            return false;
        };
        expected_output.is_none_or(|expected| {
            matches!(output_ty, ResolvedType::Unknown) || self.types_compatible(&output_ty, expected)
        })
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
            Some(TypeInfo::Newtype(info)) => {
                info.traits.iter().any(|trait_name| trait_name == bound)
                    || info
                        .derives
                        .iter()
                        .any(|derive| self.builtin_derive_satisfies_trait(derive, bound))
            }
            Some(TypeInfo::TypeAlias) => false,
            None => false,
        }
    }
}
