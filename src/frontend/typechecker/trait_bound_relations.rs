//! Trait-bound satisfaction and temporary capability bridges.

use std::collections::HashMap;

use super::TypeChecker;
use crate::frontend::resolved_type_subst::substitute_resolved_type;
use crate::frontend::symbols::{ResolvedType, TypeBoundInfo, TypeInfo};
use crate::frontend::typechecker::helpers::collection_type_id;
use incan_core::interop::is_rust_capability_bound;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::trait_capabilities::{self, TraitCapabilityInfo, TraitCapabilityType};
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
        if let Some(capability) = self.temporary_trait_capability_for_bound_info(bound)
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

    /// Return the active generic placeholder name represented by `ty`.
    fn active_type_param_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
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
                if !Self::type_bound_names_match(active, required) {
                    continue;
                }
                if required.type_args.is_empty() {
                    return true;
                }
                if active.type_args.len() != required.type_args.len() {
                    continue;
                }
                let expected = required
                    .type_args
                    .iter()
                    .map(|arg| substitute_resolved_type(arg, bindings));
                let actual = active
                    .type_args
                    .iter()
                    .map(|arg| substitute_resolved_type(arg, bindings));
                if expected
                    .zip(actual)
                    .all(|(left, right)| self.types_compatible(&left, &right))
                {
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

    /// Return whether two bound records identify the same trait, accounting for import aliases.
    fn type_bound_names_match(left: &TypeBoundInfo, right: &TypeBoundInfo) -> bool {
        if left.name == right.name {
            return true;
        }
        left.module_path == right.module_path
            && left.module_path.is_some()
            && Self::type_bound_source_name(left) == Self::type_bound_source_name(right)
    }

    /// Check whether `ty` satisfies a nominal trait bound `bound_trait` under RFC 042 semantics.
    fn type_satisfies_nominal_trait_bound(&self, ty: &ResolvedType, bound_trait: &str) -> bool {
        match ty {
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
            TypeInfo::Newtype(newtype) => (newtype.type_params.as_slice(), newtype.trait_adoptions.as_slice(), None),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return false,
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
        self.validated_temporary_trait_capability(capability, bound, None, None)
    }

    /// Resolve a temporary capability bridge from a checked bound that may have crossed a package manifest boundary.
    fn temporary_trait_capability_for_bound_info(&self, bound: &TypeBoundInfo) -> Option<&'static TraitCapabilityInfo> {
        if let Some(module_path) = &bound.module_path {
            let trait_name = Self::type_bound_source_name(bound);
            let capability = trait_capabilities::for_trait_path(module_path, trait_name)?;
            return self.validated_temporary_trait_capability(
                capability,
                &bound.name,
                bound.source_name.as_deref(),
                Some(module_path),
            );
        }
        self.temporary_trait_capability_for_bound(&bound.name)
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
        let manifest_bound_identifies_capability = source_name == Some(capability.trait_name)
            && module_path.is_some_and(|path| trait_capabilities::module_path_matches(capability, path));
        manifest_bound_identifies_capability.then_some(capability)
    }

    /// Resolve a bound spelling to its defining module path and trait name.
    fn resolve_bound_trait_path(&self, bound: &str) -> Option<(Vec<String>, String)> {
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

    /// Return temporary trait satisfaction for proven source type families.
    fn temporary_trait_capability_supports_type(
        &self,
        capability: &TraitCapabilityInfo,
        ty: &ResolvedType,
    ) -> Option<bool> {
        match ty {
            ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer => Some(true),
            ResolvedType::Int => Some(trait_capabilities::supports_type(capability, TraitCapabilityType::Int)),
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
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                self.temporary_trait_capability_supports_type(capability, inner)
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
            ResolvedType::Float
            | ResolvedType::FrozenStr
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

    /// Return whether a nominal type is a stable scalar value enum category for temporary capability bridges.
    fn value_enum_type_satisfies_temporary_trait_capability(&self, type_name: &str) -> bool {
        matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Enum(info)) if info.value_enum.is_some()
        )
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
            Some(TypeInfo::Newtype(info)) => info.traits.iter().any(|t| t == bound),
            Some(TypeInfo::TypeAlias) => false,
            None => false,
        }
    }
}
