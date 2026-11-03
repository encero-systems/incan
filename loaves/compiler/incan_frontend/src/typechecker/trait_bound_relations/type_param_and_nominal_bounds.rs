//! Active type parameter bounds, bound identity and implication, and nominal and primitive trait bounds.

use super::*;

impl TypeChecker {
    /// Return the active generic placeholder name represented by `ty`.
    pub(in crate::typechecker) fn active_type_param_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
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

    /// Return the declared bounds of the active type parameter `name` from the innermost frame that declares it.
    ///
    /// This is the only reading of the bound frames: an outer frame's parameter of the same name is shadowed, so its
    /// bounds grant no trait, method or native-call authority inside the inner declaration.
    pub(in crate::typechecker) fn innermost_type_param_bounds(&self, name: &str) -> Option<&[TypeBoundInfo]> {
        self.current_type_param_bound_details
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .map(Vec::as_slice)
    }

    /// Whether the checker sees every method the bounds of the active type parameter `placeholder_name` give it: each
    /// bound names a trait whose declaration it has, or a `CallableN` trait (#1561).
    ///
    /// A bound on a Rust trait, or on a trait the checker cannot resolve, may declare methods the checker does not
    /// know, so a method call through such a parameter is left to the Rust compiler.
    pub(in crate::typechecker) fn active_type_param_bounds_are_visible(&self, placeholder_name: &str) -> bool {
        let Some(bounds) = self.innermost_type_param_bounds(placeholder_name) else {
            return false;
        };
        bounds.iter().all(|bound| {
            self.lookup_trait_adoption_info(bound).is_some() || self.callable_trait_for_bound(bound).is_some()
        })
    }

    /// Whether a trait a bound of the active type parameter `placeholder_name` names, or one of its supertraits,
    /// declares a member called `member`: a method, a method alias or a property (#1561).
    ///
    /// A method call through such a bound that does not resolve failed for a reason reported where it arises (two
    /// supertraits that disagree on the member), not because no bound declares it.
    pub(in crate::typechecker) fn active_type_param_bound_declares_member(
        &self,
        placeholder_name: &str,
        member: &str,
    ) -> bool {
        let Some(bounds) = self.innermost_type_param_bounds(placeholder_name) else {
            return false;
        };
        let declares = |info: &crate::symbols::TraitInfo| {
            info.methods.contains_key(member)
                || info.method_aliases.contains_key(member)
                || info.properties.contains_key(member)
        };
        bounds.iter().any(|bound| {
            self.lookup_trait_adoption_info(bound).is_some_and(declares)
                || self
                    .semantic_supertrait_closure(&bound.name)
                    .iter()
                    .any(|(name, _)| self.lookup_semantic_trait_info(name).is_some_and(declares))
        })
    }

    /// Check whether an active generic placeholder already carries the bound required by a nested generic call.
    pub(in crate::typechecker) fn active_type_param_satisfies_bound_info(
        &self,
        placeholder_name: &str,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        self.innermost_type_param_bounds(placeholder_name)
            .is_some_and(|bounds| {
                bounds
                    .iter()
                    .any(|active| self.type_bound_implies_bound_info(active, required, bindings))
            })
    }

    /// Return whether an active type parameter explicitly or transitively carries one builtin trait bound.
    pub(in crate::typechecker) fn active_type_param_satisfies_builtin_bound(
        &self,
        placeholder_name: &str,
        required: TraitId,
    ) -> bool {
        let required = TypeBoundInfo {
            name: builtin_traits::as_str(required).to_string(),
            source_name: None,
            type_args: Vec::new(),
            module_path: None,
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.active_type_param_satisfies_bound_info(placeholder_name, &required, &HashMap::new())
    }

    /// Return the resolved source trait item name for a bound, falling back to the visible spelling.
    pub(in crate::typechecker) fn type_bound_source_name(bound: &TypeBoundInfo) -> &str {
        bound
            .source_name
            .as_deref()
            .unwrap_or_else(|| bound.name.rsplit('.').next().unwrap_or(bound.name.as_str()))
    }

    /// Return whether a checked bound names the builtin trait `trait_id`: the stdlib declaration, imported under any
    /// spelling, or the builtin's own name with no import or local trait claiming it.
    pub(in crate::typechecker) fn bound_is_builtin_trait(&self, bound: &TypeBoundInfo, trait_id: TraitId) -> bool {
        let is_builtin_name = |name: &str| builtin_traits::from_str(name) == Some(trait_id);
        if bound.name.starts_with("pub::") && builtin_traits::from_bound_str(&bound.name) == Some(trait_id) {
            return true;
        }
        if let Some(module_path) = &bound.module_path {
            let name = Self::type_bound_source_name(bound);
            let qualified = if module_path.first().map(String::as_str) == Some("pub") {
                format!("{}::{name}", module_path.join("::"))
            } else {
                format!("{}.{name}", module_path.join("."))
            };
            return builtin_traits::from_bound_str(&qualified) == Some(trait_id);
        }
        match self.import_binding_path(&bound.name) {
            Some(path) => {
                let qualified = if path.first().map(String::as_str) == Some("pub") {
                    path.join("::")
                } else {
                    path.join(".")
                };
                builtin_traits::from_bound_str(&qualified) == Some(trait_id)
            }
            None => {
                is_builtin_name(Self::type_bound_source_name(bound))
                    && (!matches!(
                        self.lookup_symbol(&bound.name).map(|symbol| &symbol.kind),
                        Some(SymbolKind::Trait(_))
                    ) || self
                        .symbols
                        .lookup(&bound.name)
                        .and_then(|symbol| self.symbols.identity_of(symbol))
                        .is_some_and(|identity| identity.origin == incan_semantics_core::SymbolOrigin::Builtin))
            }
        }
    }

    /// Return the canonical source identity for a checked trait bound when it can be resolved.
    pub(super) fn type_bound_identity(&self, bound: &TypeBoundInfo) -> Option<(Vec<String>, String)> {
        if let Some(module_path) = &bound.module_path {
            return Some((module_path.clone(), Self::type_bound_source_name(bound).to_string()));
        }
        let (module_path, resolved_name) = self.resolve_bound_trait_path(&bound.name)?;
        Some((module_path, bound.source_name.clone().unwrap_or(resolved_name)))
    }

    /// Return whether two bound records identify the same trait, accounting for import aliases.
    pub(super) fn type_bound_names_match(&self, left: &TypeBoundInfo, right: &TypeBoundInfo) -> bool {
        match (self.type_bound_identity(left), self.type_bound_identity(right)) {
            (Some(left), Some(right)) => left == right,
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => {
                left.name == right.name && Self::type_bound_source_name(left) == Self::type_bound_source_name(right)
            }
        }
    }

    /// Compare one active and required bound after applying call-site substitutions.
    pub(super) fn type_bound_args_match(
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
    pub(super) fn type_bound_implies_bound_info(
        &self,
        active: &TypeBoundInfo,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if self.type_bound_names_match(active, required) && self.type_bound_args_match(active, required, bindings) {
            return true;
        }
        if self.builtin_comparison_bound_implies(active, required) {
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
        let substitutions = crate::resolved_type_subst::type_param_subst_map(&active_trait.type_params, &active_args);
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
                    implementation_type_params: Vec::new(),
                    inferred: false,
                };
                self.type_bound_names_match(&candidate, required)
                    && self.type_bound_args_match(&candidate, required, bindings)
            })
    }

    /// Whether a bound on the builtin `Eq` or `Ord` implies the builtin bound `required`, as Rust's `Eq: PartialEq` and
    /// `Ord: Eq + PartialOrd` do (#1561).
    ///
    /// A module that names the builtin without importing it binds a stub that declares no supertraits, so `T with Ord`
    /// did not meet a callee's `T with Eq`, although the generated Rust bound does. The relation is the one the derive
    /// table records for `@derive(Eq)` and `@derive(Ord)`, which bring exactly those traits with them.
    pub(super) fn builtin_comparison_bound_implies(&self, active: &TypeBoundInfo, required: &TypeBoundInfo) -> bool {
        let Some(active_derive) = [TraitId::Eq, TraitId::Ord]
            .into_iter()
            .find(|trait_id| self.bound_is_builtin_trait(active, *trait_id))
            .and_then(|trait_id| derives::from_str(builtin_traits::as_str(trait_id)))
        else {
            return false;
        };
        derives::implied_derives(active_derive).iter().any(|implied| {
            builtin_traits::from_str(derives::as_str(*implied))
                .is_some_and(|trait_id| self.bound_is_builtin_trait(required, trait_id))
        })
    }

    /// Check whether `ty` satisfies a nominal trait bound `bound_trait` under RFC 042 semantics.
    pub(super) fn type_satisfies_nominal_trait_bound(&self, ty: &ResolvedType, bound_trait: &str) -> bool {
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
    pub(super) fn type_satisfies_nominal_trait_bound_with_args(
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
    pub(super) fn type_implements_trait_with_args(
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

        let owner_subst = crate::resolved_type_subst::type_param_subst_map(owner_type_params, concrete_type_args);
        for adoption in adoptions {
            let Some(adopted_info) = self.lookup_trait_adoption_info(adoption) else {
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

            let subst = crate::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
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
    pub(super) fn trait_args_match(&self, actual_args: &[ResolvedType], expected_args: &[ResolvedType]) -> bool {
        actual_args.len() == expected_args.len()
            && actual_args
                .iter()
                .zip(expected_args.iter())
                .all(|(actual, expected)| self.types_compatible(actual, expected))
    }

    /// Return whether a primitive type satisfies a builtin or registry-backed temporary capability bound.
    pub(super) fn primitive_type_satisfies_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        if bound == derives::as_str(DeriveId::Copy) {
            return self.is_copy_type(ty);
        }
        if let Some(capability) = self.temporary_trait_capability_for_bound(bound)
            && let Some(satisfies) = self.temporary_trait_capability_supports_type(capability, ty)
        {
            return satisfies;
        }

        // Every exact-width numeric type provides what its Rust primitive does: each one clones, copies, displays,
        // debugs, defaults and compares, and the integer types also have total equality, ordering and hashing, which
        // `f32` lacks as `float` does. A value that displays satisfies `Display` (see the display rule).
        let exact_numeric = matches!(ty, ResolvedType::Numeric(_));
        let exact_integer = matches!(ty, ResolvedType::Numeric(id) if numerics::is_integer(*id));
        match builtin_traits::from_str(bound) {
            Some(TraitId::Clone | TraitId::Debug | TraitId::Display) => {
                exact_numeric
                    || matches!(
                        ty,
                        ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Str
                            | ResolvedType::Bytes
                            | ResolvedType::FrozenStr
                            | ResolvedType::FrozenBytes
                            | ResolvedType::Unit
                    )
            }
            Some(TraitId::Default) => {
                exact_numeric
                    || matches!(
                        ty,
                        ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Str
                            | ResolvedType::Bytes
                            | ResolvedType::FrozenStr
                            | ResolvedType::FrozenBytes
                            | ResolvedType::Unit
                    )
            }
            Some(TraitId::Awaitable) => self.type_satisfies_awaitable_bound(ty, None),
            Some(TraitId::Eq | TraitId::Ord | TraitId::Hash) => {
                exact_integer
                    || matches!(
                        ty,
                        ResolvedType::Int
                            | ResolvedType::Bool
                            | ResolvedType::Str
                            | ResolvedType::Bytes
                            | ResolvedType::FrozenStr
                            | ResolvedType::FrozenBytes
                            | ResolvedType::Unit
                    )
            }
            Some(TraitId::PartialEq | TraitId::PartialOrd) => {
                exact_numeric
                    || matches!(
                        ty,
                        ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Str
                            | ResolvedType::Bytes
                            | ResolvedType::FrozenStr
                            | ResolvedType::FrozenBytes
                            | ResolvedType::Unit
                    )
            }
            _ => false,
        }
    }
}
