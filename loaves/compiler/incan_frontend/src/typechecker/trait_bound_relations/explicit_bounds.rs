//! Explicit generic bounds: display, targets, callable markers, future bounds and builtin derive bounds.

use super::*;

impl TypeChecker {
    /// Render a type-parameter bound with call-site substitutions applied.
    pub(in crate::typechecker) fn type_bound_display(
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

    /// Classify what a `with` bound names, for the bound relation and the remedy a violation offers (#1373, #1033).
    ///
    /// Anything the checker can satisfy as a trait — a source trait, a builtin trait, an imported Rust trait, or a
    /// Rust capability marker — is a trait target. A name that resolves to a declared or builtin type is a type
    /// target, which only that type satisfies. An active type-parameter placeholder and an unresolvable name keep
    /// the trait wording, which is the only actionable one for them.
    pub(in crate::typechecker) fn generic_bound_target(&self, bound: &str) -> GenericBoundTarget {
        if bound.starts_with("::")
            || is_rust_capability_bound(bound)
            || builtin_traits::from_str(bound).is_some()
            || self.lookup_semantic_trait_info(bound).is_some()
        {
            return GenericBoundTarget::Trait;
        }
        match self.foreign_trait_bound_requires_native_check(bound) {
            Some(true) => return GenericBoundTarget::Trait,
            Some(false) => return GenericBoundTarget::Type,
            None => {}
        }
        let is_active_type_param = self
            .current_type_param_bound_details
            .iter()
            .any(|frame| frame.contains_key(bound));
        // A builtin type is a type in either spelling (`int` or `Int`), whether or not that spelling is a symbol.
        if !is_active_type_param
            && (ResolvedType::builtin_simple(bound).is_some() || self.lookup_type_info(bound).is_some())
        {
            return GenericBoundTarget::Type;
        }
        GenericBoundTarget::Trait
    }

    /// The type a `with` bound names, when it names a type rather than a trait (#1033).
    ///
    /// A type in bound position constrains the type parameter to that type, so `with int` keys an overload set on
    /// the type argument itself (RFC 107). Only a name [`Self::generic_bound_target`] classifies as a type qualifies,
    /// so a trait bound keeps its meaning and a type never shadows a trait of the same name. The name resolves the
    /// way a parameter annotation does, aliases expanded, so a builtin, a model and an alias of either agree.
    pub(super) fn bound_names_a_type(&self, bound: &str) -> Option<ResolvedType> {
        if self.generic_bound_target(bound) != GenericBoundTarget::Type {
            return None;
        }
        match self.expand_type_aliases(resolve_type(&Type::Simple(bound.to_string()), &self.symbols)) {
            ResolvedType::Unknown | ResolvedType::TypeVar(_) => None,
            resolved => Some(resolved),
        }
    }

    /// Return whether a type satisfies one explicit bound, including generic trait arguments.
    ///
    /// An `Eq` or `Hash` bound the checker inferred from a callable's body (#1758) holds unless the derive relation
    /// knows the type lacks the derive.
    pub fn type_satisfies_explicit_bound_info(
        &self,
        ty: &ResolvedType,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if Self::function_value_misses_future_bound(ty, &bound.name) {
            return false;
        }
        if let Some(satisfies) = self.ownership_view_satisfies_explicit_bound_info(ty, bound, bindings) {
            return satisfies;
        }
        // An inferred `Eq` or `Hash` bound (#1758) holds unless the type is known to lack the derive.
        if bound.inferred
            && let Some(derive) = derives::from_str(&bound.name)
        {
            return !matches!(self.derive_support(ty, derive), DeriveSupport::Missing(_));
        }
        if let Some(defer) = self.foreign_trait_bound_requires_native_check(&bound.name) {
            return defer;
        }
        if let Some(placeholder_name) = self.active_type_param_name(ty)
            && self.active_type_param_satisfies_bound_info(placeholder_name, bound, bindings)
        {
            return true;
        }
        if self.trait_self_satisfies_own_bound(ty, bound, bindings) {
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
        if let Some(satisfies) = self.function_type_satisfies_callable_marker(ty, bound, bindings) {
            return satisfies;
        }
        if let Some(capability) = self.temporary_trait_capability_for_bound_info(bound, bindings)
            && let Some(satisfies) = self.temporary_trait_capability_supports_type(capability, ty)
        {
            return satisfies;
        }
        if bound.type_args.is_empty() {
            if let Some(published) = self.published_nominal_bound_name(bound) {
                // This identity already proves a source trait. Do not require its canonical provider path to be a
                // visible consumer binding before comparing the nominal's checked adoptions.
                return self.type_satisfies_nominal_trait_bound(ty, &published);
            }
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
        // RFC 088 gives builtin collections and generators the `Iterable[T]` protocol without a source-level
        // adoption entry. Reuse the assignment relation that owns those protocol conversions so a generic bound sees
        // the same iterable set as a parameter or return annotation.
        if self.bound_is_builtin_trait(bound, TraitId::Iterable) && expected_args.len() == 1 {
            let expected = ResolvedType::Generic(builtin_traits::as_str(TraitId::Iterable).to_string(), expected_args);
            return self.types_compatible(ty, &expected);
        }
        if builtin_traits::from_str(&bound.name).is_some() {
            return self.type_satisfies_nominal_trait_bound_with_args(ty, &bound.name, &expected_args);
        }
        if self.lookup_semantic_trait_info(&bound.name).is_none() {
            return self.type_satisfies_explicit_bound(ty, &bound.name);
        }
        self.type_satisfies_nominal_trait_bound_with_args(ty, &bound.name, &expected_args)
    }

    /// Return whether an open `Self` inside a trait default satisfies that trait with the same type arguments.
    ///
    /// `Self` represents the eventual adopter while the default body is checked, so no concrete adoption record
    /// exists yet. The active trait declaration is nevertheless the exact contract every adopter must satisfy. This
    /// admits only that declaration and its own open generic parameters; another trait or another instantiation must
    /// still be proved independently.
    fn trait_self_satisfies_own_bound(
        &self,
        ty: &ResolvedType,
        required: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        if !matches!(ty, ResolvedType::SelfType) {
            return false;
        }
        let Some(active) = self.active_trait_self_bound(required) else {
            return false;
        };
        Self::trait_self_bound_args_match(&active, required, bindings)
    }

    /// Return the active trait instantiation that an open trait-default `Self` is guaranteed to adopt.
    ///
    /// This is also the source relation generic call inference uses to close a method type parameter from `Self`.
    /// The trait identity is checked before exposing arguments so a distinct same-shaped trait grants no capability.
    pub(in crate::typechecker) fn active_trait_self_bound(&self, required: &TypeBoundInfo) -> Option<TypeBoundInfo> {
        let Some(trait_name) = self.current_trait_name.as_ref() else {
            return None;
        };
        let Some(trait_info) = self.lookup_semantic_trait_info(trait_name) else {
            return None;
        };
        let active = TypeBoundInfo {
            name: trait_name.clone(),
            source_name: self.trait_bound_source_name(trait_name),
            type_args: trait_info
                .type_params
                .iter()
                .map(|parameter| ResolvedType::TypeVar(parameter.clone()))
                .collect(),
            module_path: self.trait_bound_module_path(trait_name),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        let mut resolved_required = required.clone();
        if resolved_required.module_path.is_none() {
            resolved_required.module_path = self.trait_bound_module_path(&required.name);
        }
        if resolved_required.source_name.is_none() {
            resolved_required.source_name = self.trait_bound_source_name(&required.name);
        }
        self.type_bound_names_match(&active, &resolved_required)
            .then_some(active)
    }

    /// Compare an active trait's open parameters with the required bound after call-site inference.
    ///
    /// Ordinary compatibility deliberately admits open type variables. That is too broad here: `Self` satisfies only
    /// the same closed trait instantiation, so an uninferred or differently instantiated argument must not pass.
    fn trait_self_bound_args_match(
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
                    substitute_resolved_type(actual, bindings) == substitute_resolved_type(expected, bindings)
                })
    }

    /// Delegate a source-trait bound through a canonical ownership view when that view exposes the whole trait.
    ///
    /// A shared `Borrowed[T]` view cannot promise a trait that includes a `mut self` member because generic dispatch
    /// could select that member. An exclusive returned `Reference[T]` view exposes both receiver modes. The referent
    /// must independently satisfy the bound; the view does not manufacture an adoption or weaken generic arguments.
    fn ownership_view_satisfies_explicit_bound_info(
        &self,
        ty: &ResolvedType,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<bool> {
        let ResolvedType::Generic(name, arguments) = ty else {
            return None;
        };
        let [referent] = arguments.as_slice() else {
            return None;
        };
        let mode = self.explicit_ownership_mode(name)?;
        if !matches!(mode, OwnershipMode::Borrowed | OwnershipMode::Reference) {
            return None;
        }
        let trait_info = self.lookup_trait_adoption_info(bound)?;
        if mode == OwnershipMode::Borrowed
            && (Self::trait_declares_mutable_receiver(trait_info)
                || self
                    .semantic_supertrait_closure(&bound.name)
                    .iter()
                    .filter_map(|(name, _)| self.lookup_semantic_trait_info(name))
                    .any(Self::trait_declares_mutable_receiver))
        {
            return Some(false);
        }
        Some(self.type_satisfies_explicit_bound_info(referent, bound, bindings))
    }

    /// Return whether any method of this trait requires exclusive receiver access.
    fn trait_declares_mutable_receiver(info: &crate::symbols::TraitInfo) -> bool {
        info.methods
            .values()
            .any(|method| method.receiver == Some(Receiver::Mutable))
    }

    /// Keep a published plain trait bound in its declaring module rather than resolving its bare name in the
    /// consumer. Registry-owned builtin and Rust capability bounds retain their existing semantic relations.
    /// An admitted visible alias keeps its spelling when it names the same declaration; its public projection may
    /// be usable even when the declaration's canonical source module is private.
    pub(super) fn published_nominal_bound_name(&self, bound: &TypeBoundInfo) -> Option<String> {
        if is_rust_capability_bound(&bound.name)
            || builtin_traits::from_str(Self::type_bound_source_name(bound))
                .is_some_and(|id| self.bound_is_builtin_trait(bound, id))
        {
            return None;
        }
        let module_path = bound.module_path.as_deref()?;
        let identity = self.package_trait_declaration(module_path, Self::type_bound_source_name(bound))?;
        let visible = self
            .resolve_bound_trait_path(&bound.name)
            .and_then(|(path, name)| self.package_trait_declaration(&path, &name));
        if visible.as_ref() == Some(&identity) {
            return None;
        }
        Some(format!(
            "{}::{}",
            canonical_module_identity(&identity)?,
            identity.declaration_name
        ))
    }

    /// Match a function or closure value against the exact `std.traits.callable.CallableN` signature.
    pub(super) fn function_type_satisfies_callable_bound(
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

    /// Match a function value against the parameter list of an RFC 041 `Fn`-family capability marker.
    ///
    /// `Fn[int]`, `FnMut[int]` and `FnOnce[int]` name the callable's parameters and nothing else: the return type is
    /// whatever the value passed returns. The generated Rust asks for exactly that shape (the canonical `CallableN`
    /// bound with a free return type, #1716), so a function whose arity or parameter types do not match the marker is
    /// refused here rather than by rustc. A value that is not a function answers `None`: a nominal type can satisfy
    /// the callable requirement through its own `__call__` adoption, which the marker cannot see.
    pub(super) fn function_type_satisfies_callable_marker(
        &self,
        ty: &ResolvedType,
        bound: &TypeBoundInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<bool> {
        if !is_rust_callable_capability_bound(&bound.name) {
            return None;
        }
        let ResolvedType::Function(params, _) = ty else {
            return None;
        };
        if params.len() != bound.type_args.len() {
            return Some(false);
        }
        let params_match = params
            .iter()
            .zip(&bound.type_args)
            .all(|(actual, expected)| self.types_compatible(&actual.ty, &substitute_resolved_type(expected, bindings)));
        Some(params_match)
    }

    /// Refuse the RFC 041 `Fn`-family markers a declaration's type parameters carry where no bound can stand for
    /// them (#1716).
    ///
    /// A marker lowers to the canonical `CallableN` bound by its parameter count, with the return type left to the
    /// call that passes the value. Two shapes have no such bound: a marker naming more parameters than the callable
    /// vocabulary spells ([`callables::max_arity`]), on any declaration, and a marker on a nominal declaration's
    /// type parameter (`model Holder[F with Fn[int]]`), whose return type no call can determine. Both are refused
    /// at the declaration, before a call is checked against them, with the alternative to write in the message.
    /// The count is checked first, so a nominal owner hears about the count when both apply.
    pub(in crate::typechecker) fn refuse_unsupported_callable_markers(
        &mut self,
        type_params: &[TypeParam],
        owner: CallableMarkerOwner<'_>,
    ) {
        let limit = callables::max_arity();
        for type_param in type_params {
            for bound in &type_param.bounds {
                if !is_rust_callable_capability_bound(&bound.name) {
                    continue;
                }
                let parameter_types = bound
                    .type_args
                    .iter()
                    .map(|arg| arg.node.to_string())
                    .collect::<Vec<_>>();
                let marker = if parameter_types.is_empty() {
                    bound.name.clone()
                } else {
                    format!("{}[{}]", bound.name, parameter_types.join(", "))
                };
                let refusal = if parameter_types.len() > limit {
                    CallableMarkerRefusal::ParameterCount {
                        count: parameter_types.len(),
                        limit,
                    }
                } else if let CallableMarkerOwner::Nominal { kind, name } = owner {
                    CallableMarkerRefusal::NominalOwner {
                        owner_kind: kind,
                        owner_name: name,
                    }
                } else {
                    continue;
                };
                self.errors.push(errors::callable_marker_not_supported(
                    &marker,
                    &type_param.name,
                    &parameter_types,
                    refusal,
                    type_param.span,
                ));
            }
        }
    }

    /// Resolve a checked bound to the canonical source callable trait registry. Published package spellings must
    /// select the same admitted declaration as the standard namespace; an equal name or path suffix grants no
    /// callable behavior, and missing provider evidence does not fall back to source-name recognition.
    pub(in crate::typechecker) fn callable_trait_for_bound(
        &self,
        bound: &TypeBoundInfo,
    ) -> Option<callables::CallableTraitId> {
        let (module_path, trait_name) = self.type_bound_identity(bound)?;
        if callables::module_path_matches(&module_path) {
            return callables::from_str(&trait_name);
        }
        let selected = self.package_trait_declaration(&module_path, &trait_name)?;
        let callable = callables::from_str(&selected.declaration_name)?;
        let standard_path = callables::MODULE_PATH
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>();
        let standard = self.package_trait_declaration(&standard_path, callables::info_for(callable).name)?;
        (selected == standard).then_some(callable)
    }

    /// Return whether a bound names a capability marker that requires a future (`RuntimeFuture`), under any spelling.
    ///
    /// A bound read from a provider's checked signature may carry a module or Rust path, so the marker is matched by
    /// its last path segment.
    pub(in crate::typechecker) fn bound_requires_future(bound: &str) -> bool {
        let marker = bound
            .rsplit(['.', ':'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(bound);
        is_rust_future_capability_bound(marker)
    }

    /// Return whether `ty` is a function value checked against a bound that requires a future (#1772).
    ///
    /// Every other type is admitted at a Rust capability marker and left to the build, because the checker gives an
    /// `async def` call's result its output type and cannot tell it from a plain value of that type. A function value
    /// is never a future, whatever it returns, so it is refused here: this runs before the foreign-trait deferral so
    /// that a marker imported from `rust::` in a provider's own scope cannot hide it.
    pub(super) fn function_value_misses_future_bound(ty: &ResolvedType, bound: &str) -> bool {
        matches!(ty, ResolvedType::Function(_, _)) && Self::bound_requires_future(bound)
    }

    /// Best-effort check whether a concrete type satisfies an explicit generic bound.
    pub(in crate::typechecker) fn type_satisfies_explicit_bound(&self, ty: &ResolvedType, bound: &str) -> bool {
        if Self::function_value_misses_future_bound(ty, bound) {
            return false;
        }
        // A bound that names a type is satisfied by that type and no other (#1033).
        if let Some(bound_type) = self.bound_names_a_type(bound) {
            return self.expand_type_aliases(ty.clone()) == bound_type;
        }
        if let Some(defer) = self.foreign_trait_bound_requires_native_check(bound) {
            return defer;
        }
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
        // A `Display` bound asks for the display rule's `Display`, which a `__str__`, an enum's declared values,
        // `@derive(Display)` or an `Error` adoption provides (#1748).
        if builtin_traits::from_str(bound) == Some(TraitId::Display)
            && let Some(satisfies) = self.display_bound_satisfied(ty)
        {
            return satisfies;
        }
        // A builtin derive (`Clone`, `Copy`, `Eq`, `Ord`, ...) is answered by the derive relation wherever it knows the
        // type, automatic and implied derives included (#1870); an unknown answer keeps the fallbacks below.
        if let Some(derive) = self.builtin_derive_bound(bound) {
            match self.derive_support(ty, derive) {
                DeriveSupport::Supported => return true,
                DeriveSupport::Missing(_) => return false,
                DeriveSupport::Unknown => {}
            }
        }
        if builtin_traits::from_str(bound).is_none() && self.lookup_semantic_trait_info(bound).is_some() {
            return self.type_satisfies_nominal_trait_bound(ty, bound);
        }
        match ty {
            ResolvedType::Never
            | ResolvedType::Unknown
            | ResolvedType::TypeVar(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::Trait(_)
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

    /// Return the builtin derive a bound or trait adoption names, when its spelling reaches the builtin trait.
    ///
    /// The builtin traits are the `std.derives.*` traits the stdlib registry records, `Copy` included although it has
    /// no builtin trait identity of its own. An imported trait that merely shares a builtin's name (`from unrelated
    /// import Eq`) is not the builtin; an import alias of the builtin (`from std.derives.comparison import Eq as
    /// Equality`) is.
    pub(in crate::typechecker) fn builtin_derive_bound(&self, bound: &str) -> Option<DeriveId> {
        let names_builtin_trait = |name: &str| stdlib::trait_method_module_segments(name).is_some();
        let path = match self.import_binding_path(bound) {
            Some(path) => path.to_vec(),
            // A module-qualified spelling (`comparison.Ord` after `from std.derives import comparison`) names the
            // trait through its module's import (#1561).
            None => match bound.rsplit_once('.') {
                Some((module_spelling, trait_name)) => {
                    let mut path = self.module_path_for_imported_name(module_spelling)?;
                    path.push(trait_name.to_string());
                    path
                }
                None => return derives::from_str(bound).filter(|_| names_builtin_trait(bound)),
            },
        };
        let (trait_name, module_path) = path.split_last()?;
        let derive = derives::from_str(trait_name).filter(|_| names_builtin_trait(trait_name))?;
        stdlib::trait_method_module_segments(trait_name)
            .is_some_and(|builtin_module| builtin_module == module_path)
            .then_some(derive)
    }

    /// Keep imported Rust bounds at the native validation boundary rather than requiring an Incan trait adoption.
    ///
    /// Rust decides whether the lowered scalar, container, or model implements the foreign trait. This does not
    /// prove an implementation: lowering must retain the bound, and native compilation remains required. Positive
    /// non-trait metadata rejects the bound; local Incan traits never enter this path, even when names coincide.
    pub(super) fn foreign_trait_bound_requires_native_check(&self, bound: &str) -> Option<bool> {
        // Absolute Rust paths are produced from resolved imports and retained in checked provider signatures.
        // Native Incan module paths use dot notation, so this does not bypass nominal source-trait validation.
        if bound.starts_with("::") {
            return Some(true);
        }
        let symbol = self.lookup_symbol(bound)?;
        let SymbolKind::RustItem(info) = &symbol.kind else {
            return None;
        };
        Some(
            info.metadata
                .as_ref()
                .is_none_or(|metadata| matches!(metadata.kind, incan_lang::interop::RustItemKind::Trait(_))),
        )
    }
}
