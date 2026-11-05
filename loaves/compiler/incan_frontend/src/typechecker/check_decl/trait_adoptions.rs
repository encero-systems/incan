//! Trait conformance self types, method signature compatibility, trait instantiation, and Body IR adoption facts.

use super::*;

impl TypeChecker {
    /// Replace every nested `Self` occurrence in an annotation with the concrete owner type used for this method body.
    ///
    /// Method return validation runs against the concrete owning type, not the abstract declaration surface, so
    /// containers like `List[Self]` must be concretized recursively before compatibility checks.
    ///
    /// **Call sites** use `substitute_self_in_resolved_type` in `check_expr/access.rs` instead: there `Self` is
    /// resolved from the **instantiated** receiver expression (e.g. `Carrier[Order]` for `x.filter(...)`).
    pub(super) fn concretize_self_type_in_annotation(ty: &ResolvedType, self_ty: &ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::SelfType => self_ty.clone(),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| Self::concretize_self_type_in_annotation(arg, self_ty))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .iter()
                    .map(|item| Self::concretize_self_type_in_annotation(item, self_ty))
                    .collect(),
            ),
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .iter()
                    .map(|param| crate::symbols::CallableParam {
                        name: param.name.clone(),
                        ty: Self::concretize_self_type_in_annotation(&param.ty, self_ty),
                        surface_type_name: param.surface_type_name.clone(),
                        kind: param.kind,
                        has_default: param.has_default,
                        is_partial_preset: param.is_partial_preset,
                        is_mut: param.is_mut,
                    })
                    .collect(),
                Box::new(Self::concretize_self_type_in_annotation(ret, self_ty)),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(Self::concretize_self_type_in_annotation(key, self_ty)),
                Box::new(Self::concretize_self_type_in_annotation(value, self_ty)),
            ),
            ResolvedType::Ref(inner) => {
                ResolvedType::Ref(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            _ => ty.clone(),
        }
    }

    /// Return the generic-aware concrete surface for a nominal type that is adopting a trait.
    ///
    /// Trait requirements retain `Self` in their collected signatures. Before comparing one of those requirements
    /// with a model, class, newtype, or enum implementation, replace `Self` with the adopting nominal type. This is
    /// necessary for nested occurrences such as `ReaderChunks[Self]`, not just a bare `Self` return.
    pub(super) fn trait_conformance_self_type(&self, type_name: &str) -> ResolvedType {
        let type_params = self
            .lookup_type_info(type_name)
            .map(|info| match info {
                TypeInfo::Class(info) => info.type_params.clone(),
                TypeInfo::Model(info) => info.type_params.clone(),
                TypeInfo::Newtype(info) => info.type_params.clone(),
                TypeInfo::Enum(info) => info.type_params.clone(),
                TypeInfo::Builtin | TypeInfo::TypeAlias => Vec::new(),
            })
            .unwrap_or_default();
        if type_params.is_empty() {
            ResolvedType::Named(type_name.to_string())
        } else {
            ResolvedType::Generic(
                type_name.to_string(),
                type_params.into_iter().map(ResolvedType::TypeVar).collect(),
            )
        }
    }

    /// Concretize `Self` in a trait method requirement for one nominal adopter.
    pub(super) fn concretize_trait_method_requirement(
        &self,
        method: &MethodInfo,
        self_ty: &ResolvedType,
    ) -> MethodInfo {
        let mut concrete = method.clone();
        concrete.params = method
            .params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: Self::concretize_self_type_in_annotation(&param.ty, self_ty),
                surface_type_name: param.surface_type_name.clone(),
                kind: param.kind,
                has_default: param.has_default,
                is_partial_preset: param.is_partial_preset,
                is_mut: param.is_mut,
            })
            .collect();
        concrete.return_type = Self::concretize_self_type_in_annotation(&method.return_type, self_ty);
        concrete
    }

    /// Materialize concrete `Self` in the methods declared by a nominal trait adopter.
    pub(super) fn concretize_trait_conformance_method_overloads(
        &self,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        self_ty: &ResolvedType,
    ) -> HashMap<String, Vec<MethodInfo>> {
        method_overloads
            .iter()
            .map(|(method_name, overloads)| {
                (
                    method_name.clone(),
                    overloads
                        .iter()
                        .map(|method| self.concretize_trait_method_requirement(method, self_ty))
                        .collect(),
                )
            })
            .collect()
    }

    /// Union of method names reachable through the given traits, including transitive supertrait methods (RFC 042).
    ///
    /// Used when validating field `@alias` metadata so aliases cannot collide with callable members surfaced through
    /// trait adoption.
    pub(super) fn collect_trait_method_names(&self, traits: &[Spanned<TraitBound>]) -> HashSet<String> {
        self.trait_method_origins(traits.iter().map(|trait_ref| trait_ref.node.name.as_str()))
            .into_keys()
            .collect()
    }

    /// Validate per-field metadata (`@alias`, etc.) on a model-like type against canonical field names and trait method
    /// names.
    pub(super) fn validate_field_metadata(
        &mut self,
        type_name: &str,
        fields: &[Spanned<FieldDecl>],
        method_names: &HashSet<String>,
    ) {
        let canonical_names: HashSet<String> = fields.iter().map(|f| f.node.name.clone()).collect();
        let mut builtin_member_names: HashSet<&'static str> = HashSet::new();
        for info in magic_methods::MAGIC_METHODS {
            builtin_member_names.insert(info.canonical);
            builtin_member_names.extend(info.aliases);
        }
        let mut seen_aliases: HashMap<String, Span> = HashMap::new();

        for field in fields {
            let Some(alias) = field.node.metadata.alias.as_ref() else {
                continue;
            };

            if alias.trim().is_empty() {
                self.errors.push(errors::empty_alias(field.span));
                continue;
            }

            if canonical_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_canonical(type_name, alias, field.span));
            }

            if method_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_method(type_name, alias, field.span));
            }
            if builtin_member_names.contains(alias.as_str()) {
                self.errors
                    .push(errors::alias_collides_with_builtin(type_name, alias, field.span));
            }

            if let BindingRegistration::Collision { existing } =
                register_binding(&mut seen_aliases, alias.clone(), field.span)
            {
                self.errors
                    .push(errors::duplicate_alias(type_name, alias, existing, field.span));
            }
        }
    }

    /// Render a named method signature for compatibility diagnostics.
    pub(super) fn method_sig_string_named(&self, method_name: &str, m: &MethodInfo) -> String {
        let recv = match m.receiver {
            Some(Receiver::Mutable) => "mut self",
            Some(Receiver::Immutable) => "self",
            None => "",
        };
        let mut parts: Vec<String> = Vec::new();
        if !recv.is_empty() {
            parts.push(recv.to_string());
        }
        for param in &m.params {
            if let Some(name) = param.name() {
                parts.push(format!("{name}: {}", param.ty));
            }
        }
        let async_kw = if m.is_async { "async " } else { "" };
        format!(
            "{async_kw}def {name}({params}) -> {ret}",
            name = method_name,
            params = parts.join(", "),
            ret = m.return_type
        )
    }

    /// Return whether two method signatures are compatible.
    pub(in crate::typechecker) fn method_sigs_compatible(&self, expected: &MethodInfo, found: &MethodInfo) -> bool {
        if expected.receiver != found.receiver {
            return false;
        }
        if expected.is_async != found.is_async {
            return false;
        }
        if expected.params.len() != found.params.len() {
            return false;
        }
        for (expected_param, found_param) in expected.params.iter().zip(found.params.iter()) {
            if expected_param.kind != found_param.kind {
                return false;
            }
            if !self.types_compatible(&expected_param.ty, &found_param.ty) {
                return false;
            }
        }
        self.types_compatible(&expected.return_type, &found.return_type)
    }

    /// Return whether two methods have the same call-time parameter shape, ignoring return type.
    pub(super) fn method_call_shapes_same(&self, left: &MethodInfo, right: &MethodInfo) -> bool {
        if left.receiver != right.receiver {
            return false;
        }
        if left.is_async != right.is_async {
            return false;
        }
        if left.params.len() != right.params.len() {
            return false;
        }
        left.params
            .iter()
            .zip(right.params.iter())
            .all(|(left_param, right_param)| left_param.kind == right_param.kind && left_param.ty == right_param.ty)
    }

    /// True if `ancestor` appears in the transitive supertrait closure of trait `descendant` (RFC 042).
    pub(super) fn is_strict_supertrait_name(&self, ancestor: &str, descendant: &str) -> bool {
        self.semantic_supertrait_closure(descendant)
            .iter()
            .any(|(name, _)| name == ancestor)
    }

    /// Drop supertrait obligations shadowed by a more derived trait in the same obligation group.
    pub(super) fn filter_supertrait_dominated_entries(
        &self,
        entries: Vec<(String, MethodInfo)>,
    ) -> Vec<(String, MethodInfo)> {
        let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        entries
            .into_iter()
            .filter(|(ta, _)| {
                !names
                    .iter()
                    .any(|tb| ta != tb && self.is_strict_supertrait_name(ta, tb))
            })
            .collect()
    }

    /// Drop supertrait property obligations shadowed by a more derived trait in the same obligation group.
    pub(super) fn filter_supertrait_dominated_property_entries(
        &self,
        entries: Vec<(String, PropertyInfo)>,
    ) -> Vec<(String, PropertyInfo)> {
        let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        entries
            .into_iter()
            .filter(|(ta, _)| {
                !names
                    .iter()
                    .any(|tb| ta != tb && self.is_strict_supertrait_name(ta, tb))
            })
            .collect()
    }

    /// Apply resolved type arguments to one trait definition so conformance uses the instantiated contract.
    pub(super) fn instantiate_trait_info(&self, trait_info: &TraitInfo, args: &[ResolvedType]) -> TraitInfo {
        let subst = type_param_subst_map(&trait_info.type_params, args);
        TraitInfo {
            type_params: trait_info.type_params.clone(),
            supertraits: trait_info
                .supertraits
                .iter()
                .map(|(name, super_args)| {
                    (
                        name.clone(),
                        super_args
                            .iter()
                            .map(|arg| substitute_resolved_type(arg, &subst))
                            .collect(),
                    )
                })
                .collect(),
            methods: trait_info
                .methods
                .iter()
                .map(|(name, info)| (name.clone(), substitute_method_info(info, &subst)))
                .collect(),
            method_aliases: trait_info.method_aliases.clone(),
            properties: trait_info
                .properties
                .iter()
                .map(|(name, info)| (name.clone(), substitute_property_info(info, &subst)))
                .collect(),
            requires: trait_info
                .requires
                .iter()
                .map(|(field, ty)| (field.clone(), substitute_resolved_type(ty, &subst)))
                .collect(),
        }
    }

    /// Resolve one model/class trait adoption and instantiate generic trait parameters from explicit `with Trait[T]`.
    pub(super) fn resolve_adopted_trait_info(
        &mut self,
        bound: &TraitBound,
        adoption_span: Span,
    ) -> Option<(TraitInfo, Vec<ResolvedType>)> {
        let trait_name = bound.name.as_str();
        let trait_info = self.lookup_trait_info(trait_name)?.clone();
        if bound.type_args.is_empty() {
            return Some((trait_info, Vec::new()));
        }

        let resolved_args: Vec<ResolvedType> = bound
            .type_args
            .iter()
            .map(|arg| self.resolve_type_checked(arg))
            .collect();
        if resolved_args.len() != trait_info.type_params.len() {
            self.errors.push(errors::trait_adoption_bound_arity_mismatch(
                trait_name,
                trait_info.type_params.len(),
                resolved_args.len(),
                adoption_span,
            ));
            return None;
        }

        let instantiated = self.instantiate_trait_info(&trait_info, &resolved_args);
        Some((instantiated, resolved_args))
    }

    /// Resolve a previously collected trait adoption into instantiated trait metadata for validation.
    pub(super) fn resolve_collected_trait_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        adoption_span: Span,
    ) -> Option<ResolvedTraitAdoption> {
        let trait_info = self.lookup_trait_info(&adoption.name)?.clone();
        if adoption.type_args.len() != trait_info.type_params.len() {
            self.errors.push(errors::trait_adoption_bound_arity_mismatch(
                &adoption.name,
                trait_info.type_params.len(),
                adoption.type_args.len(),
                adoption_span,
            ));
            return None;
        }
        let info = if adoption.type_args.is_empty() {
            trait_info
        } else {
            self.instantiate_trait_info(&trait_info, &adoption.type_args)
        };
        Some(ResolvedTraitAdoption {
            name: adoption.name.clone(),
            info,
            args: adoption.type_args.clone(),
            module_path: adoption.module_path.clone(),
            span: adoption_span,
        })
    }

    /// Snapshot resolved derive slots for Body IR without modifying legacy method or adoption tables.
    pub(super) fn record_body_ir_derived_methods(&mut self, owner: &str, adoption: &ResolvedTraitAdoption) {
        let arguments: Vec<_> = adoption
            .info
            .type_params
            .iter()
            .cloned()
            .zip(adoption.args.iter().cloned())
            .collect();
        let methods = self
            .type_info
            .declarations
            .body_ir_derived_methods
            .entry(owner.to_string())
            .or_default();
        for method in adoption.info.methods.values() {
            let Some(identity) = &method.identity else {
                continue;
            };
            if methods.iter().any(|retained| &retained.method == identity) {
                continue;
            }
            methods.push(crate::typechecker::type_info::BodyIrDerivedMethod {
                method: identity.clone(),
                has_default: method.has_body,
                type_arguments: arguments.clone(),
            });
        }
        methods.sort_by(|left, right| left.method.cmp(&right.method));
    }

    /// Snapshot checked nominal adoptions independently of method slots, preserving empty marker protocols.
    ///
    /// Only already proven symbol or provider-member identities are retained. Missing identity metadata grants no
    /// adoption fact; legacy conformance and derive tables remain unchanged.
    pub(super) fn record_body_ir_trait_adoptions(&mut self, owner: &str, adoptions: &[ResolvedTraitAdoption]) {
        let mut retained = Vec::new();
        for adoption in adoptions {
            let Some(trait_identity) = self.body_ir_trait_adoption_identity(adoption) else {
                continue;
            };
            let fact = super::type_info::BodyIrCheckedTraitAdoption {
                fields: self.body_ir_required_fields(owner, adoption, &trait_identity),
                trait_identity,
                arguments: adoption.args.clone(),
                methods: self.body_ir_adoption_method_identities(adoption),
                span: adoption.span,
            };
            if !retained.contains(&fact) {
                retained.push(fact);
            }
        }
        self.type_info
            .declarations
            .body_ir_trait_adoptions
            .insert(owner.to_owned(), retained);
    }

    /// Retain only stored field identities already proven by required-field conformance; absent metadata grants no
    /// projection authority. Inherited class fields retain their original declaring identities.
    pub(super) fn body_ir_required_fields(
        &self,
        owner: &str,
        adoption: &ResolvedTraitAdoption,
        trait_identity: &CanonicalSymbolId,
    ) -> Vec<incan_semantics_core::body_ir::TraitFieldImplementation> {
        let fields = match self.lookup_type_info(owner) {
            Some(TypeInfo::Model(info)) => &info.fields,
            Some(TypeInfo::Class(info)) => &info.fields,
            _ => return Vec::new(),
        };
        let mut traits = vec![(trait_identity, &adoption.info)];
        for (name, _) in self.supertrait_closure.get(&adoption.name).into_iter().flatten() {
            let Some(identity) = self
                .symbols
                .lookup(name)
                .and_then(|symbol| self.symbols.identity_of(symbol))
            else {
                continue;
            };
            let Some(info) = self.lookup_trait_info(name) else {
                continue;
            };
            traits.push((identity, info));
        }
        let mut bindings = traits
            .into_iter()
            .flat_map(|(identity, info)| {
                info.requires.iter().filter_map(move |(name, _)| {
                    Some(incan_semantics_core::body_ir::TraitFieldImplementation {
                        requirement: Self::synthetic_member_identity(identity, name, SemanticSourceTargetKind::Field),
                        implementation: fields.get(name)?.identity.clone()?,
                    })
                })
            })
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| left.requirement.cmp(&right.requirement));
        bindings.dedup();
        bindings
    }

    /// Retain the complete checked slot set, distinguishing a real marker from incomplete imported metadata.
    pub(super) fn body_ir_adoption_method_identities(
        &self,
        adoption: &ResolvedTraitAdoption,
    ) -> Option<Vec<CanonicalSymbolId>> {
        let mut identities = adoption
            .info
            .methods
            .iter()
            .map(|(name, method)| {
                self.type_info
                    .traits
                    .method_identities
                    .get(&(adoption.name.clone(), name.clone()))
                    .cloned()
                    .or_else(|| method.identity.clone())
            })
            .collect::<Option<Vec<_>>>()?;
        identities.sort();
        identities.dedup();
        Some(identities)
    }

    /// Read the selected declaration identity through the same checked binding that supplied trait metadata.
    /// Hidden derive-bundle bindings resolve through their authenticated provider member rather than their alias.
    pub(super) fn body_ir_trait_adoption_identity(
        &mut self,
        adoption: &ResolvedTraitAdoption,
    ) -> Option<CanonicalSymbolId> {
        let identity = self
            .symbols
            .lookup(&adoption.name)
            .and_then(|symbol| self.symbols.identity_of(symbol));
        if let Some(identity) = identity.filter(|identity| identity.kind == SemanticSourceTargetKind::Trait) {
            return Some(identity.clone());
        }
        let path = self.derive_trait_path(&adoption.name);
        let (name, module) = path.split_last()?;
        if module.is_empty() {
            return None;
        }
        self.dependency_member_identity(&ImportPath::simple(module.to_vec()), name)
            .or_else(|| self.loaf_member_identity(module, name))
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
    }

    /// Expand checked source derive bundles in the Body IR record; the legacy native selection is left intact.
    /// Canonical builtin markers remain selected, and later JSON protocol additions update this same record.
    pub(super) fn expand_body_ir_model_derives(&mut self, owner: &str, derives: &[String]) {
        let Some(original) = self.type_info.declarations.body_ir_model_derives.get(owner) else {
            return;
        };
        let mut expanded = original.clone();
        for derive in derives {
            let path = self.derive_trait_path(derive);
            let module = path.join(".");
            let keys = if let Some(traits) = self.dependency_derivable_modules.get(&module) {
                traits.iter().map(|name| format!("{module}.{name}")).collect::<Vec<_>>()
            } else if self.dependency_trait_rust_derive_paths.contains_key(&module) {
                vec![module]
            } else {
                continue;
            };
            expanded.retain(|name| name != derive || derives::from_str(name).is_some());
            for key in keys {
                for path in self.dependency_trait_rust_derive_paths.get(&key).into_iter().flatten() {
                    if !expanded.contains(path) {
                        expanded.push(path.clone());
                    }
                }
            }
        }
        self.type_info
            .declarations
            .body_ir_model_derives
            .insert(owner.to_string(), expanded);
    }

    /// Return the first `@derive` span for diagnostics tied to synthetic derive adoptions.
    pub(super) fn first_derive_span(decorators: &[Spanned<Decorator>]) -> Span {
        decorators
            .iter()
            .find(|decorator| decorators::from_str(decorator.node.name.as_str()) == Some(DecoratorId::Derive))
            .map(|decorator| decorator.span)
            .unwrap_or_default()
    }

    /// Recursively collect abstract trait properties after applying adoption-time type arguments.
    pub(super) fn collect_instantiated_trait_abstract_property_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, String, PropertyInfo)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (property_name, property_info) in &trait_info.properties {
            if !property_info.has_body {
                out.push((property_name.clone(), trait_name.to_string(), property_info.clone()));
            }
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_abstract_property_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                seen,
                out,
            );
        }
    }

    /// Collect all methods from an instantiated trait and its supertraits with type arguments applied.
    pub(super) fn collect_instantiated_trait_method_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        origin_module_path: Option<&[String]>,
        seen: &mut HashSet<String>,
        out: &mut Vec<TraitMethodEntry>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (method_name, method_info) in &trait_info.methods {
            out.push(TraitMethodEntry {
                method_name: method_name.clone(),
                origin_trait: trait_name.to_string(),
                origin_type_args: trait_args.to_vec(),
                origin_module_path: origin_module_path.map(<[String]>::to_vec),
                implementation_type_params: Vec::new(),
                info: method_info.clone(),
            });
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.supertrait_info_for_methods(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(&supertrait_info, supertrait_args);
            self.collect_instantiated_trait_method_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                origin_module_path,
                seen,
                out,
            );
        }
    }

    /// Return the declaration a supertrait name gives method lookup: its visible or semantic declaration, or, when
    /// that is missing or a compiler stub without methods, the builtin trait's standard package declaration
    /// once [`Self::load_builtin_supertrait_sources`] has loaded it. An adopter of `Ord` that imports only `Ord`
    /// reaches the `__ne__` default of its `Eq` supertrait this way (#1561).
    pub(super) fn supertrait_info_for_methods(&self, name: &str) -> Option<TraitInfo> {
        let semantic = self.lookup_semantic_trait_info(name);
        if semantic.is_some_and(|info| !info.methods.is_empty()) {
            return semantic.cloned();
        }
        self.standard_traits.get(name).cloned().or_else(|| semantic.cloned())
    }

    /// Load the standard package declaration of each builtin trait among `supertraits` and their own
    /// supertraits, so [`Self::supertrait_info_for_methods`] can read it.
    pub(super) fn load_builtin_supertrait_sources(&mut self, supertraits: &[(String, Vec<ResolvedType>)]) {
        let mut pending = supertraits.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
        let mut seen = HashSet::new();
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(info) = self.standard_trait(&name) {
                pending.extend(info.supertraits.iter().map(|(supertrait, _)| supertrait.clone()));
            }
            if let Some(info) = self.lookup_semantic_trait_info(&name) {
                pending.extend(info.supertraits.iter().map(|(supertrait, _)| supertrait.clone()));
            }
        }
    }

    /// Collect all properties from an instantiated trait and its supertraits with type arguments applied.
    pub(super) fn collect_instantiated_trait_property_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, String, PropertyInfo)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (property_name, property_info) in &trait_info.properties {
            out.push((property_name.clone(), trait_name.to_string(), property_info.clone()));
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_property_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                seen,
                out,
            );
        }
    }

    /// Return all trait-backed method entries required by one resolved adoption.
    pub(super) fn trait_method_entries_for_adoption(&self, adoption: &ResolvedTraitAdoption) -> Vec<TraitMethodEntry> {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        self.collect_instantiated_trait_method_entries(
            &adoption.name,
            &adoption.info,
            &adoption.args,
            adoption.module_path.as_deref(),
            &mut seen,
            &mut entries,
        );
        entries
    }
}
