//! Temporary trait capabilities and tuple, collection, awaitable and named type bounds.

use super::*;

impl TypeChecker {
    /// Resolve a temporary trait-owned capability bridge for a bound.
    pub(super) fn temporary_trait_capability_for_bound(&self, bound: &str) -> Option<&'static TraitCapabilityInfo> {
        let (module_path, trait_name) = self.resolve_bound_trait_path(bound)?;
        let capability = trait_capabilities::for_trait_path(&module_path, &trait_name)?;
        if !capability.required_type_args.is_empty() {
            return None;
        }
        self.validated_temporary_trait_capability(capability, bound, None, None)
    }

    /// Resolve a temporary capability bridge from a checked bound that may have crossed a package manifest boundary.
    pub(super) fn temporary_trait_capability_for_bound_info(
        &self,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<&'static TraitCapabilityInfo> {
        let capability = if let Some(module_path) = &bound.module_path {
            let trait_name = Self::type_bound_source_name(bound);
            let capability = self.temporary_trait_capability_at_path(module_path, trait_name)?;
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

    /// Resolve a registered bridge through its published trait identity. Equal short names in another package grant
    /// no capability; standard imports and package-qualified bounds must select the same canonical declaration.
    pub(super) fn temporary_trait_capability_at_path(
        &self,
        module_path: &[String],
        trait_name: &str,
    ) -> Option<&'static TraitCapabilityInfo> {
        if let Some(capability) = trait_capabilities::for_trait_path(module_path, trait_name) {
            return Some(capability);
        }
        let selected = self.package_trait_declaration(module_path, trait_name)?;
        trait_capabilities::TRAIT_CAPABILITIES.iter().find(|capability| {
            let standard_path = capability
                .module_path
                .iter()
                .map(|part| (*part).to_string())
                .collect::<Vec<_>>();
            self.package_trait_declaration(&standard_path, capability.trait_name)
                .as_ref()
                == Some(&selected)
        })
    }

    /// Return whether a checked trait bound carries the concrete arguments required by a capability bridge.
    pub(super) fn capability_type_args_match(
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
    pub(super) fn validated_temporary_trait_capability(
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
        let manifest_bound_identifies_capability = module_path.is_some_and(|path| {
            self.temporary_trait_capability_at_path(path, resolved_source_name)
                .is_some_and(|selected| selected.id == capability.id)
        });
        manifest_bound_identifies_capability.then_some(capability)
    }

    /// Resolve a bound spelling to its defining module path and trait name. A retained public Loaf identity already
    /// names its provider module and must not be interpreted as a binding in the consumer's current module.
    pub(in crate::typechecker) fn resolve_bound_trait_path(&self, bound: &str) -> Option<(Vec<String>, String)> {
        if let Some(qualified) = bound.strip_prefix("pub::") {
            let (module, name) = qualified.rsplit_once("::")?;
            let mut path = vec!["pub".to_string()];
            path.extend(module.split("::").map(str::to_string));
            return Some((path, name.to_string()));
        }
        if let Some(path) = self.import_binding_path(bound)
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
    pub(in crate::typechecker) fn temporary_trait_capability_supports_type(
        &self,
        capability: &TraitCapabilityInfo,
        ty: &ResolvedType,
    ) -> Option<bool> {
        self.temporary_trait_capability_supports_type_inner(capability, ty, &mut HashSet::new())
    }

    /// Recursively evaluate a capability family, including transparent validated-newtype composition.
    pub(super) fn temporary_trait_capability_supports_type_inner(
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
    pub(super) fn nominal_type_explicitly_adopts_temporary_capability(
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
        let substitutions = crate::resolved_type_subst::type_param_subst_map(owner_type_params, concrete_type_args);
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
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        adoptions
            .iter()
            .any(|adoption| self.type_bound_implies_bound_info(adoption, &required, &substitutions))
    }

    /// Return compiler-provided `TryFrom[str]` conformance for a concrete Incan newtype.
    pub(super) fn newtype_satisfies_string_try_from(
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

        let substitutions = crate::resolved_type_subst::type_param_subst_map(&info.type_params, type_args);
        let underlying = substitute_resolved_type(&info.underlying, &substitutions);
        let supported = self
            .temporary_trait_capability_supports_type_inner(capability, &underlying, seen_newtypes)
            .unwrap_or(false);
        seen_newtypes.remove(type_name);
        Some(supported)
    }

    /// Return whether a nominal type is a stable scalar value enum category for temporary capability bridges.
    pub(super) fn value_enum_type_satisfies_temporary_trait_capability(&self, type_name: &str) -> bool {
        matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Enum(info)) if info.value_enum.is_some()
        )
    }

    /// Return whether a tuple type satisfies a trait bound.
    pub(super) fn tuple_type_satisfies_bound(&self, items: &[ResolvedType], bound: &str) -> bool {
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
    pub(super) fn collection_type_satisfies_bound(
        &self,
        kind: CollectionTypeId,
        args: &[ResolvedType],
        bound: &str,
    ) -> bool {
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
    pub(super) fn type_satisfies_awaitable_bound(
        &self,
        ty: &ResolvedType,
        expected_output: Option<&ResolvedType>,
    ) -> bool {
        let Some(output_ty) = self.await_output_type_from_type(ty) else {
            return false;
        };
        expected_output.is_none_or(|expected| {
            matches!(output_ty, ResolvedType::Unknown) || self.types_compatible(&output_ty, expected)
        })
    }

    /// Return whether a named user type explicitly satisfies a generic trait bound.
    pub(super) fn named_type_satisfies_bound(&self, type_name: &str, bound: &str) -> bool {
        let spelled = match self.lookup_type_info(type_name) {
            Some(TypeInfo::Builtin) => {
                return matches!(builtin_traits::from_str(bound), Some(TraitId::Clone | TraitId::Debug));
            }
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
            Some(TypeInfo::TypeAlias) | None => return false,
        };
        // An adoption and a bound that spell one declaration differently meet through its identity: a model adopting
        // `std.primitives.errors.Error` satisfies `E with Error` imported through the `std.traits.error` facade.
        spelled || self.type_implements_trait(type_name, bound)
    }
}
