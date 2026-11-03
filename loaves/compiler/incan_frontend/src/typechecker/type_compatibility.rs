//! Validated-newtype coercions, rigid type parameters, and type compatibility.

use super::*;

impl TypeChecker {
    // ========================================================================
    // Type compatibility (shared helper)
    // ========================================================================

    /// Record an RFC 017 validated-newtype coercion when `actual` may flow into `expected` at an approved site.
    ///
    /// This deliberately stays outside [`Self::types_compatible`]: ordinary compatibility queries have no source span
    /// and are used in contexts where implicit coercion must not be inserted.
    pub fn record_validated_newtype_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> bool {
        let Some(coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Record an RFC 017 model/class field coercion that should aggregate validation errors.
    pub fn record_validated_newtype_field_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        field_name: &str,
        span: Span,
    ) -> bool {
        let Some(mut coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        coercion.mode = ValidatedNewtypeCoercionMode::AggregateField {
            field_name: field_name.to_string(),
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Return the coercion metadata needed to convert `actual` into expected newtype `expected`.
    pub(super) fn validated_newtype_coercion(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<ValidatedNewtypeCoercionInfo> {
        if actual == expected {
            return None;
        }
        let steps = self.validated_newtype_coercion_steps(actual, expected, &mut HashSet::new(), span)?;
        Some(ValidatedNewtypeCoercionInfo {
            steps,
            target_type: expected.clone(),
            mode: ValidatedNewtypeCoercionMode::FailFast,
        })
    }

    /// Build the ordered underlying-to-target chain for one validated-newtype coercion.
    pub(super) fn validated_newtype_coercion_steps(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        visiting: &mut HashSet<String>,
        span: Span,
    ) -> Option<Vec<ValidatedNewtypeCoercionStep>> {
        let target_name = match expected {
            ResolvedType::Named(name) => name,
            // Generic newtypes need type-parameter substitution for their underlying type. Keep that out of the
            // initial implicit path instead of silently guessing.
            ResolvedType::Generic(_, _) => return None,
            _ => return None,
        };
        if !visiting.insert(target_name.clone()) {
            self.errors.push(errors::newtype_coercion_cycle(target_name, span));
            return None;
        }

        let Some(TypeInfo::Newtype(newtype)) = self.lookup_semantic_type_info(target_name).cloned() else {
            visiting.remove(target_name);
            return None;
        };
        if newtype.is_rusttype {
            visiting.remove(target_name);
            return None;
        }

        let mut steps = if Self::validated_newtype_source_matches_underlying(actual, &newtype.underlying) {
            Vec::new()
        } else {
            self.validated_newtype_coercion_steps(actual, &newtype.underlying, visiting, span)?
        };
        if !newtype.implicit_coercion_enabled {
            self.errors
                .push(errors::implicit_newtype_coercion_disabled(target_name, span));
            visiting.remove(target_name);
            return Some(Vec::new());
        }
        let ctor = self.validated_newtype_ctor_name(target_name, &newtype);
        let ctor_identity = ctor
            .as_deref()
            .and_then(|name| newtype.methods.get(name))
            .and_then(|method| method.identity.clone());
        steps.push(ValidatedNewtypeCoercionStep {
            newtype_name: target_name.clone(),
            owner_identity: self.checked_nominal_type_identities().get(target_name).cloned(),
            ctor,
            ctor_identity,
            constraints: if self.validated_newtype_ctor_name(target_name, &newtype).is_some() {
                Vec::new()
            } else {
                newtype.constraints.clone()
            },
        });
        visiting.remove(target_name);
        Some(steps)
    }

    /// Return whether a source value can be passed to a newtype's underlying constructor without primitive parsing.
    pub(super) fn validated_newtype_source_matches_underlying(
        actual: &ResolvedType,
        underlying: &ResolvedType,
    ) -> bool {
        matches!(actual, ResolvedType::Unknown | ResolvedType::TypeVar(_)) || actual == underlying
    }

    /// Retain a selected hook in the consumer's owner binding, including when an import renamed that owner.
    /// The caller must first validate the hook through `validated_newtype_ctor_name`; only its proven Ok owner is
    /// rebound, preserving the original parameters, error type, and canonical method identity.
    pub(super) fn newtype_constructor_binding(
        method: &MethodInfo,
        owner: ResolvedType,
    ) -> type_info::FunctionBindingInfo {
        let mut return_type = method.return_type.clone();
        if let ResolvedType::Generic(_, arguments) = &mut return_type
            && let Some(success) = arguments.first_mut()
        {
            *success = owner;
        }
        type_info::FunctionBindingInfo {
            is_async: method.is_async,
            params: method.params.clone(),
            return_type,
            identity: method.identity.clone(),
        }
    }

    /// Return the checked-construction hook only when its collected signature returns this newtype's exact owner.
    /// Source aliases and retained carriers compare through their admitted declaration identities.
    pub fn validated_newtype_ctor_name(&self, newtype_name: &str, newtype: &NewtypeInfo) -> Option<String> {
        let method = newtype.methods.get(conventions::NEWTYPE_FROM_UNDERLYING_METHOD)?;
        if method.receiver.is_some() || method.params.len() != 1 {
            return None;
        }
        if !Self::validated_newtype_source_matches_underlying(&method.params[0].ty, &newtype.underlying) {
            return None;
        }
        let ResolvedType::Generic(result_name, result_args) = &method.return_type else {
            return None;
        };
        if collection_type_id(result_name.as_str()) != Some(CollectionTypeId::Result) {
            return None;
        }
        let ok_ty = result_args.first()?;
        if !(matches!(ok_ty, ResolvedType::Named(name) if name == newtype_name)
            || matches!(ok_ty, ResolvedType::SelfType)
            || self.retained_nominal_types_compatible(ok_ty, &ResolvedType::Named(newtype_name.to_owned()))
                == Some(true))
        {
            return None;
        }
        let err_ty = result_args.get(1)?;
        if !matches!(err_ty, ResolvedType::Named(name) if name == "ValidationError") {
            return None;
        }
        Some(conventions::NEWTYPE_FROM_UNDERLYING_METHOD.to_string())
    }

    /// Return the type parameter `ty` names when it is a type parameter of an enclosing generic declaration, seen from
    /// inside that declaration's body, where it is one fixed type whatever a caller later picks for it (#1561).
    ///
    /// Inside the body such a parameter is a named placeholder in scope; a callee's own type parameters, still to be
    /// inferred at a call, are type variables and are not rigid.
    pub(super) fn rigid_type_param_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        let ResolvedType::Named(_) = ty else {
            return None;
        };
        self.generic_placeholder_name(ty)
            .and_then(|_| self.active_type_param_name(ty))
    }

    /// Whether a value of type `actual` is a value of the enclosing declaration's type parameter `type_param` (#1561).
    ///
    /// A value of a concrete type is not one: `return 0` from `def f[T](x: T) -> T` is refused, as the generated Rust
    /// refuses it, because a caller may pick any type for `T`. Only a value of that same parameter is, together with a
    /// borrowed one (`&T`, which the destination copies), a value whose type is still to be inferred (an unresolved
    /// callee type parameter) and a union whose members all are. An unknown value, left to the error that made it
    /// unknown, and a never-returning one are accepted.
    pub(super) fn value_is_rigid_type_param(&self, actual: &ResolvedType, type_param: &str) -> bool {
        match actual {
            ResolvedType::Named(name) => name == type_param,
            ResolvedType::Unknown | ResolvedType::Never | ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer => {
                true
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.value_is_rigid_type_param(inner, type_param),
            ResolvedType::Generic(name, members) if name == UNION_TYPE_NAME => members
                .iter()
                .all(|member| self.value_is_rigid_type_param(member, type_param)),
            _ => false,
        }
    }

    /// Whether a value of the enclosing declaration's type parameter `type_param` (of type `actual`, the parameter
    /// itself) can be used where `expected` is required (#1561).
    ///
    /// Inside its declaration a type parameter is one type the declaration does not choose, so a value of it is not a
    /// value of a concrete type: `return x` from `def f[T](x: T) -> int` is refused, as the generated Rust refuses it.
    /// A bound makes it a value of a trait the bound implies (`T with Shape` where a `Shape` is expected). A
    /// `CallableN` bound does not make it a function value, since an adopting model meets the bound too; a bound on
    /// a trait the checker cannot see (a Rust `Fn` marker) may, and is left to the Rust compiler. A union takes the
    /// value when one member does, an `Option` when its payload does, and a borrow when the borrowed type does. A
    /// destination still to be inferred (a callee's type parameter, a placeholder of no enclosing declaration),
    /// `Self`, a Rust type and a name the checker does not know are left to the checks that own them.
    pub(super) fn rigid_type_param_value_fits(
        &self,
        type_param: &str,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> bool {
        match expected {
            ResolvedType::Named(name) if name == type_param => true,
            ResolvedType::Unknown
            | ResolvedType::Never
            | ResolvedType::TypeVar(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_) => true,
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.types_compatible(actual, inner),
            ResolvedType::Generic(name, members) if name == UNION_TYPE_NAME => {
                members.iter().any(|member| self.types_compatible(actual, member))
            }
            _ if expected.is_option() => expected
                .option_inner_type()
                .is_some_and(|inner| self.types_compatible(actual, inner)),
            ResolvedType::Function(..) => !self.active_type_param_bounds_are_visible(type_param),
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                if self.is_generic_placeholder_type(expected) {
                    return true;
                }
                if self.lookup_semantic_trait_info(name).is_some() {
                    return self.active_type_param_implies_trait_type(type_param, expected);
                }
                self.lookup_semantic_type_info(name).is_none() && collection_type_id(name.as_str()).is_none()
            }
            _ => false,
        }
    }

    /// Whether the bounds of the enclosing declaration's type parameter `type_param` imply the trait `trait_ty` names
    /// (`Shape`, or `Iterable[int]` with its type arguments), so a value of the parameter is a value of that trait.
    pub(super) fn active_type_param_implies_trait_type(&self, type_param: &str, trait_ty: &ResolvedType) -> bool {
        let (name, type_args) = match trait_ty {
            ResolvedType::Named(name) => (name, Vec::new()),
            ResolvedType::Generic(name, args) => (name, args.clone()),
            _ => return false,
        };
        let required = TypeBoundInfo {
            name: name.clone(),
            source_name: None,
            type_args,
            module_path: None,
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.active_type_param_satisfies_bound_info(type_param, &required, &HashMap::new())
    }

    /// Return whether `actual` can be used where `expected` is required, with a recursion cap for pathological unions.
    pub fn types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        const MAX_TYPE_COMPATIBILITY_DEPTH: usize = 512;
        if self.native_queries.type_compatibility_depth.get() >= MAX_TYPE_COMPATIBILITY_DEPTH {
            return false;
        }
        self.native_queries
            .type_compatibility_depth
            .set(self.native_queries.type_compatibility_depth.get() + 1);
        let _guard = TypeCompatibilityDepthGuard {
            depth: &self.native_queries.type_compatibility_depth,
        };
        self.types_compatible_unchecked(actual, expected)
    }

    /// Check if two types are compatible for assignment or comparison.
    ///
    /// Returns `true` if `actual` can be used where `expected` is required. Handles `Unknown` (error recovery), type
    /// variables (generics), and recursive checks for generics, functions, and tuples.
    pub(super) fn types_compatible_unchecked(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        let expanded_actual = self.expand_type_aliases(actual.clone());
        let expanded_expected = self.expand_type_aliases(expected.clone());
        if &expanded_actual != actual || &expanded_expected != expected {
            return self.types_compatible(&expanded_actual, &expanded_expected);
        }

        if let Some(matches) = self.public_library_type_identities_compatible(actual, expected) {
            return matches;
        }

        if actual == expected {
            return true;
        }

        if let Some(matches) = self.retained_nominal_types_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = self.module_qualified_nominals_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = helpers::decimal_types_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = self.rust_type_identities_compatible(actual, expected) {
            return matches;
        }
        self.types_compatible_inference_and_unions(actual, expected)
    }

    /// Resolve inference, numeric and union rules before nominal trait or container compatibility.
    pub(super) fn types_compatible_inference_and_unions(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Never, _) => true,
            (ResolvedType::Unknown, _) | (_, ResolvedType::Unknown) => true,
            (actual, expected) if let Some(type_param) = self.rigid_type_param_name(expected) => {
                self.value_is_rigid_type_param(actual, type_param)
            }
            (ResolvedType::TypeVar(_), _) | (_, ResolvedType::TypeVar(_)) => true,
            (actual, expected) if let Some(type_param) = self.rigid_type_param_name(actual) => {
                self.rigid_type_param_value_fits(type_param, actual, expected)
            }
            (actual, _) if self.is_generic_placeholder_type(actual) => true,
            (_, expected) if self.is_generic_placeholder_type(expected) => true,
            (ResolvedType::CallSiteInfer, _) | (_, ResolvedType::CallSiteInfer) => true,
            (ResolvedType::TypeToken(actual_inner), ResolvedType::TypeToken(expected_inner)) => {
                self.nested_types_compatible(actual_inner, expected_inner)
            }
            (ResolvedType::SelfType, ResolvedType::Generic(trait_name, _))
                if self.current_trait_name.as_deref() == Some(trait_name.as_str()) =>
            {
                true
            }
            (actual, expected) if self.numeric_values_compatible(actual, expected) => true,
            (
                ResolvedType::Generic(actual_name, actual_members),
                ResolvedType::Generic(expected_name, expected_members),
            ) if actual_name == UNION_TYPE_NAME && expected_name == UNION_TYPE_NAME => {
                actual_members.iter().all(|actual_member| {
                    expected_members
                        .iter()
                        .any(|expected_member| self.nested_types_compatible(actual_member, expected_member))
                })
            }
            (ResolvedType::Generic(name, members), expected) if name == UNION_TYPE_NAME => members
                .iter()
                .all(|member| self.nested_types_compatible(member, expected)),
            (actual, ResolvedType::Generic(name, members))
                if name == UNION_TYPE_NAME
                    && actual.is_option()
                    && actual
                        .option_inner_type()
                        .is_some_and(|inner| matches!(inner, ResolvedType::Unknown))
                    && members.iter().any(|member| matches!(member, ResolvedType::Unit)) =>
            {
                true
            }
            (actual, ResolvedType::Generic(name, members)) if name == UNION_TYPE_NAME => {
                members.iter().any(|member| self.types_compatible(actual, member))
            }
            (actual, expected) if expected.is_option() && !actual.is_option() => {
                let Some(inner) = expected.option_inner_type() else {
                    return false;
                };
                self.types_compatible(actual, inner)
            }
            (ResolvedType::Generic(name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator)
                    && expected_args.len() == 1
                    && (trait_name == builtin_traits::as_str(TraitId::Iterable)
                        || trait_name == builtin_traits::as_str(TraitId::Iterator)
                        || trait_name == builtin_traits::as_str(TraitId::IntoIterator)) =>
            {
                actual_args.len() == 1 && self.nested_types_compatible(&actual_args[0], &expected_args[0])
            }

            _ => self.types_compatible_trait_upcasts(actual, expected),
        }
    }

    /// Check nullary trait references and instantiated supertrait upcasts before iterable trait adapters.
    pub(super) fn types_compatible_trait_upcasts(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Named(type_name), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, trait_name)
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(super_info) = self.lookup_semantic_trait_info(trait_name) else {
                        return false;
                    };
                    if !super_info.type_params.is_empty() {
                        return false;
                    }
                    self.trait_is_supertrait_of(type_name, trait_name)
                        && self
                            .instantiated_supertrait_args(type_name, actual_args, trait_name)
                            .is_some_and(|inst| inst.is_empty())
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }

            // ---- Context: RFC 042 — `Subtrait[T…]` assignable to `Supertrait[T…]` (trait upcast) ----
            (
                ResolvedType::Generic(subtrait_name, actual_args),
                ResolvedType::Generic(supertrait_name, expected_args),
            ) if self.lookup_semantic_trait_info(subtrait_name).is_some()
                && self.lookup_semantic_trait_info(supertrait_name).is_some() =>
            {
                let Some(instantiated) = self.instantiated_supertrait_args(subtrait_name, actual_args, supertrait_name)
                else {
                    return false;
                };
                if instantiated.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated.iter())
                    .all(|(e, a)| self.nested_types_compatible(a, e))
            }

            _ => self.types_compatible_iterable_traits(actual, expected),
        }
    }

    /// Check builtin iterable carriers and generic concrete implementations without weakening arity checks.
    pub(super) fn types_compatible_iterable_traits(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Generic(actual_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable)
                    && expected_args.len() == 1
                    && actual_args.len() == 1 =>
            {
                let actual_is_iterable = actual_name == builtin_traits::as_str(TraitId::Iterator)
                    || matches!(
                        collection_type_id(actual_name.as_str()),
                        Some(
                            CollectionTypeId::List
                                | CollectionTypeId::Set
                                | CollectionTypeId::FrozenList
                                | CollectionTypeId::FrozenSet
                        )
                    );
                actual_is_iterable && self.nested_types_compatible(&actual_args[0], &expected_args[0])
            }
            (ResolvedType::FrozenList(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.nested_types_compatible(actual, &expected_args[0])
            }
            (ResolvedType::FrozenSet(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.nested_types_compatible(actual, &expected_args[0])
            }

            // RFC 042: `Concrete[T]` assignable to generic trait annotation `Trait[T]` (and similar).
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some()
                    && self.lookup_semantic_trait_info(type_name).is_none()
                    && self.lookup_semantic_type_info(type_name).is_some() =>
            {
                let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, actual_args, trait_name)
                else {
                    return false;
                };
                let concrete_arity_ok = match self.lookup_semantic_type_info(type_name) {
                    Some(TypeInfo::Model(m)) => actual_args.len() == m.type_params.len(),
                    Some(TypeInfo::Class(c)) => actual_args.len() == c.type_params.len(),
                    Some(TypeInfo::Newtype(n)) => actual_args.len() == n.type_params.len(),
                    Some(TypeInfo::Enum(e)) => actual_args.len() == e.type_params.len(),
                    _ => true,
                };
                if !concrete_arity_ok {
                    return false;
                }
                if instantiated_args.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated_args.iter())
                    .all(|(e, a)| self.nested_types_compatible(a, e))
            }

            _ => self.types_compatible_named_generics(actual, expected),
        }
    }

    /// Check bare nominal trait references and the existing unchecked generic-name compatibility surface.
    pub(super) fn types_compatible_named_generics(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Named(type_name), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(instantiated_args) = self.instantiated_supertrait_args(type_name, &[], trait_name) else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(e, a)| self.nested_types_compatible(a, e))
                } else {
                    let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, &[], trait_name)
                    else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(expected, actual)| self.nested_types_compatible(actual, expected))
                }
            }
            // Allow bare surface generic types (e.g. `Json`) to match `Json[T]` when used without args.
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name && self.nominal_has_type_parameters(name) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name && self.nominal_has_type_parameters(name) =>
            {
                true
            }
            _ => self.types_compatible_borrowed_and_frozen(actual, expected),
        }
    }

    /// Return whether a semantic nominal declaration has parameters, including unchecked bare annotations.
    pub(super) fn nominal_has_type_parameters(&self, name: &str) -> bool {
        match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(model)) => !model.type_params.is_empty(),
            Some(TypeInfo::Class(class)) => !class.type_params.is_empty(),
            Some(TypeInfo::Newtype(newtype)) => !newtype.type_params.is_empty(),
            Some(TypeInfo::Enum(info)) => !info.type_params.is_empty(),
            _ => false,
        }
    }

    /// Check reference materialization and frozen carrier compatibility before general structural matching.
    pub(super) fn types_compatible_borrowed_and_frozen(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            // Internal references: `&mut T` may satisfy `&T`, but not the reverse.
            (ResolvedType::Ref(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::RefMut(b)) => self.nested_types_compatible(a, b),
            // A borrowed source model or class becomes an owned value where one is required; Body IR clones it there.
            (ResolvedType::Ref(inner), expected)
                if !matches!(expected, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
                    && self.borrowed_source_value_materializes(actual) =>
            {
                self.types_compatible(inner, expected)
            }
            // Frozen const-eval values are compatible with their frozen wrappers. Ordinary runtime strings do not
            // implicitly become frozen values here; const-eval is responsible for freezing literal values first.
            (ResolvedType::FrozenStr, ResolvedType::FrozenStr) => true,
            (ResolvedType::FrozenStr, ResolvedType::Named(name))
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr) =>
            {
                true
            }
            // `FrozenStr` is a read-only string wrapper; allow it where `str` is expected.
            (ResolvedType::FrozenStr, ResolvedType::Str) => true,
            (ResolvedType::Named(name), ResolvedType::Str)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr) =>
            {
                true
            }
            (ResolvedType::FrozenBytes, ResolvedType::FrozenBytes) => true,
            (ResolvedType::FrozenBytes, ResolvedType::Named(name))
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes) =>
            {
                true
            }
            (ResolvedType::FrozenBytes, ResolvedType::Bytes) => true,
            // `bytes` and `list[u8]` share the same owned representation. This lets APIs use collection helpers
            // such as `list.repeat` when constructing byte buffers without falling back to manual Vec assembly.
            (ResolvedType::Generic(name, args), ResolvedType::Bytes)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List)
                    && args.len() == 1
                    && matches!(args[0], ResolvedType::Numeric(id) if incan_lang::lang::types::numerics::as_str(id) == "u8") =>
            {
                true
            }
            // Allow `FrozenBytes` where `bytes` is expected.
            (ResolvedType::Named(name), ResolvedType::Bytes)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes) =>
            {
                true
            }
            (ResolvedType::FrozenList(a), ResolvedType::FrozenList(b)) => self.nested_types_compatible(a, b),
            (ResolvedType::FrozenList(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.nested_types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenList(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.nested_types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenSet(a), ResolvedType::FrozenSet(b)) => self.nested_types_compatible(a, b),
            (ResolvedType::FrozenSet(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.nested_types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenSet(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.nested_types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::FrozenDict(k2, v2)) => {
                self.nested_types_compatible(k1, k2) && self.nested_types_compatible(v1, v2)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.nested_types_compatible(k1, &args[0]) && self.nested_types_compatible(v1, &args[1])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenDict(k2, v2))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.nested_types_compatible(&args[0], k2) && self.nested_types_compatible(&args[1], v2)
            }
            _ => self.types_compatible_structural_values(actual, expected),
        }
    }

    /// Compare tuples, generic instances and callable signatures, then apply the existing permissive Rust boundary.
    pub(super) fn types_compatible_structural_values(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            // Treat `Tuple` as both:
            // - a concrete tuple type: `Tuple[T1, T2, ...]`
            // - a supertype for any tuple when used without args: `Tuple`
            //
            // A checked annotation never produces the bare form any more (`resolve_type_checked` refuses it, #1717);
            // the arm remains for the shared resolver's unchecked callers, which still spell "any tuple" this way.
            (ResolvedType::Tuple(_), ResolvedType::Named(name))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                true
            }
            (ResolvedType::Tuple(elems), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Generic(name, args), ResolvedType::Tuple(elems))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Generic(n1, a1), ResolvedType::Generic(n2, a2)) => {
                n1 == n2
                    && a1.len() == a2.len()
                    && a1
                        .iter()
                        .zip(a2.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Function(p1, r1), ResolvedType::Function(p2, r2)) => {
                // The `mut` marker decides how the argument is passed, so it must agree exactly (#1790).
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(t1, t2)| {
                        t1.kind == t2.kind
                            && t1.is_mut == t2.is_mut
                            && self.callable_param_types_compatible(&t1.ty, &t2.ty)
                    })
                    && self.nested_types_compatible(r1, r2)
            }
            (ResolvedType::Tuple(e1), ResolvedType::Tuple(e2)) => {
                e1.len() == e2.len()
                    && e1
                        .iter()
                        .zip(e2.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            // Rust names one item several ways (`std::boxed::Box`, `alloc::boxed::Box`, a dependency's re-export of
            // `alloc`). Rust metadata records the ancestral spelling; Incan source imports the `std` one. Compare in
            // the ancestral namespace so every re-export of the same item is just another alias of it.
            (ResolvedType::RustPath(a), ResolvedType::RustPath(b)) => {
                incan_lang::interop::ancestral_rust_display(a) == incan_lang::interop::ancestral_rust_display(b)
            }
            // Without full Rust type knowledge, treat any `RustPath` as compatible with non-Rust surfaces so mixed
            // Incan/Rust-typed expressions stay checkable (RFC 005/041 permissive model).
            (ResolvedType::RustPath(_), _) | (_, ResolvedType::RustPath(_)) => true,
            _ => false,
        }
    }

    /// Check compatibility of a type written inside another type: a type argument, a tuple element, a callable
    /// parameter or return, a referenced type, or a member of a union value.
    ///
    /// A numeric type there matches only its own canonical type (`int` and `i64` are one type). RFC 009's lossless
    /// widening converts one value, and generated code converts it where the value reaches its destination; the values
    /// a collection, tuple, `Option` value or callable holds or produces have no such conversion, so `list[i8]` is not
    /// a `list[int]` and `(int) -> i8` is not an `(int) -> int`.
    pub(super) fn nested_types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        let depth = self.native_queries.nested_type_compatibility_depth.get();
        self.native_queries.nested_type_compatibility_depth.set(depth + 1);
        let compatible = self.types_compatible(actual, expected);
        self.native_queries.nested_type_compatibility_depth.set(depth);
        compatible
    }

    /// Return whether a numeric value type is compatible with a numeric destination type: through lossless widening
    /// for a value (see the assignment table in the numeric semantics reference), and only by canonical identity inside
    /// another type (see [`Self::nested_types_compatible`]).
    pub(super) fn numeric_values_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        if self.native_queries.nested_type_compatibility_depth.get() == 0 {
            return numeric_lossless_compatible(actual, expected);
        }
        let actual_id = numeric_type_id_for_compat(actual);
        actual_id.is_some() && actual_id == numeric_type_id_for_compat(expected)
    }

    /// Function parameters must preserve borrow shape exactly.
    ///
    /// Ordinary value compatibility is intentionally permissive at Rust boundaries, but callback signatures are called
    /// by Rust with a fixed argument ABI. A function that takes `T` or `&T` is not a valid substitute for one that Rust
    /// will call as `FnMut(&mut T)`.
    pub(super) fn callable_param_types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Ref(actual_inner), ResolvedType::Ref(expected_inner))
            | (ResolvedType::RefMut(actual_inner), ResolvedType::RefMut(expected_inner)) => {
                self.nested_types_compatible(actual_inner, expected_inner)
            }
            (ResolvedType::Ref(_) | ResolvedType::RefMut(_), _)
            | (_, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => false,
            _ => self.nested_types_compatible(actual, expected),
        }
    }
}

/// Return whether two resolved numeric types are compatible through lossless widening.
pub(super) fn numeric_lossless_compatible(actual: &ResolvedType, expected: &ResolvedType) -> bool {
    let Some(actual_id) = numeric_type_id_for_compat(actual) else {
        return false;
    };
    let Some(expected_id) = numeric_type_id_for_compat(expected) else {
        return false;
    };
    numeric_type_losslessly_widens_to(actual_id, expected_id)
}
