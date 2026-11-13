//! Semantic fact stores, semantic types, and their conversion from checked types.

use super::*;

impl TypeCheckInfo {
    /// Return whether the method call at `span` calls a source method whose receiver is `mut self`.
    pub fn method_call_takes_mutable_receiver(&self, span: Span) -> bool {
        self.calls.mutable_receiver_method_calls.contains(&span)
    }

    /// Return the checked source path associated with one active import-derived binding.
    pub fn import_binding_path(&self, local_name: &str) -> Option<&[String]> {
        self.import_bindings.path(local_name)
    }

    /// Export a backend-neutral fact snapshot for consumers that should not depend on typed AST or Rust IR shapes.
    ///
    /// This is the first bridge into the v0.5 semantic fact store. It deliberately reuses facts that the typechecker
    /// has already proven, keyed by source spans, and avoids introducing a separate semantic authority.
    pub fn semantic_fact_store(&self, module_path: &[String]) -> SemanticFactStore {
        self.semantic_fact_store_with_package(module_path, None)
    }

    /// Export a backend-neutral fact snapshot with an optional canonical package identity.
    ///
    /// Package names are a compilation-session concern, not something a typechecker can safely reconstruct from one
    /// module path. Command boundaries that know the manifest package must call this form so package registry subjects
    /// retain their real identity. The legacy no-package form remains for module-only consumers and deliberately uses
    /// a visibly synthetic fallback rather than pretending the module name is the package name.
    pub fn semantic_fact_store_with_package(
        &self,
        module_path: &[String],
        package_identity: Option<&str>,
    ) -> SemanticFactStore {
        let module_identity = semantic_module_identity(module_path);
        let mut facts = Vec::new();
        let declarations = self.semantic_reference_declarations();
        self.record_semantic_type_facts(&module_identity, &mut facts);
        Self::record_semantic_declaration_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_reference_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_iteration_facts(&module_identity, &declarations, &mut facts);
        self.record_semantic_registry_facts(&module_identity, package_identity, &mut facts);
        facts.sort();
        let mut store = SemanticFactStore::new();
        for fact in facts {
            store.insert(fact);
        }
        store
    }

    /// Retain the checker's expression and function types without resolving source syntax a second time.
    pub(super) fn record_semantic_type_facts(&self, module_identity: &str, facts: &mut Vec<SemanticFact>) {
        for (&span, ty) in &self.expressions.expr_types {
            facts.push(SemanticFact::new(
                CompilerNodeId::expression_at(module_identity, span),
                SemanticFactKind::Type,
                SemanticFactValue::semantic_type(self.semantic_type(ty)),
            ));
        }

        for (&span, target) in &self.expressions.source_targets {
            facts.push(SemanticFact::new(
                CompilerNodeId::expression_at(module_identity, span),
                SemanticFactKind::SymbolTarget,
                SemanticFactValue::source_target(semantic_source_target_from_typecheck(target)),
            ));
        }
        for (name, binding) in &self.declarations.function_bindings {
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration(module_identity, name),
                SemanticFactKind::Type,
                SemanticFactValue::semantic_type(
                    self.qualify_semantic_type(semantic_type_from_function_binding(binding)),
                ),
            ));
        }
    }

    /// Select physical checked declarations that can own references; generated members retain their nominal owner.
    pub(super) fn semantic_reference_declarations(&self) -> BTreeSet<&CanonicalSymbolId> {
        self.declarations
            .declaration_identities
            .values()
            .chain(self.declarations.member_declaration_identities.values())
            .filter(|identity| !self.is_compiler_generated_member_identity(identity))
            .filter(|identity| {
                !matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Local
                        | SemanticSourceTargetKind::Parameter
                        | SemanticSourceTargetKind::Receiver
                        | SemanticSourceTargetKind::GenericBinder
                        | SemanticSourceTargetKind::Module
                        | SemanticSourceTargetKind::Builtin
                        | SemanticSourceTargetKind::RustItem
                        | SemanticSourceTargetKind::Other(_)
                )
            })
            .collect()
    }

    /// Record declaration identities and the containing owner required by a field or enum variant.
    pub(super) fn record_semantic_declaration_facts(
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for identity in declarations {
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration_at(module_identity, identity.declaration_span),
                SemanticFactKind::DeclarationIdentity,
                SemanticFactValue::canonical_identity((*identity).clone()),
            ));
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Field | SemanticSourceTargetKind::Variant
            ) && let Some(owner) = incan_semantics_core::dependencies::closest_declaring_owner(
                declarations.iter().copied().filter(|candidate| *candidate != *identity),
                identity.declaration_span,
            ) {
                facts.push(SemanticFact::new(
                    CompilerNodeId::declaration_at(module_identity, identity.declaration_span),
                    SemanticFactKind::RequiredMemberOwner,
                    SemanticFactValue::canonical_identity(owner.clone()),
                ));
            }
        }
    }

    /// Give nested bindings stable owner-relative ordinals without using process-local scope identifiers.
    pub(super) fn stable_declaration_contexts(
        &self,
        declarations: &BTreeSet<&CanonicalSymbolId>,
    ) -> BTreeMap<CanonicalSymbolId, CanonicalStableDeclarationContext> {
        let mut stable_context_groups = BTreeMap::<
            (CanonicalSymbolId, SymbolNamespace, SemanticSourceTargetKind, String),
            BTreeSet<CanonicalSymbolId>,
        >::new();
        for identity in self.declarations.nested_declaration_identities.values() {
            if identity.scope_discriminant.is_none() {
                continue;
            }
            let Some(owner) = incan_semantics_core::dependencies::closest_declaring_owner(
                declarations.iter().copied(),
                identity.declaration_span,
            ) else {
                continue;
            };
            stable_context_groups
                .entry((
                    owner.clone(),
                    identity.namespace,
                    identity.kind.clone(),
                    identity.declaration_name.clone(),
                ))
                .or_default()
                .insert(identity.clone());
        }
        let mut stable_contexts = BTreeMap::new();
        for ((owner, _, _, _), identities) in stable_context_groups {
            let mut identities = identities.into_iter().collect::<Vec<_>>();
            identities.sort_by_key(|identity| identity.declaration_span);
            for (binding_ordinal, identity) in identities.into_iter().enumerate() {
                if let Ok(binding_ordinal) = u32::try_from(binding_ordinal) {
                    stable_contexts.insert(
                        identity,
                        CanonicalStableDeclarationContext {
                            owner: owner.clone(),
                            binding_ordinal,
                        },
                    );
                }
            }
        }
        stable_contexts
    }

    /// Project written references and nominal type leaves once for both dependency execution and inspection.
    pub(super) fn record_semantic_reference_facts(
        &self,
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        let stable_contexts = self.stable_declaration_contexts(declarations);
        for (&span, identity) in &self.references.resolved_identities {
            let subject = CompilerNodeId::expression_at(module_identity, span);
            Self::record_semantic_reference(facts, &subject, span, identity, declarations);
            let stable_context = self
                .references
                .stable_declaration_contexts
                .get(&span)
                .cloned()
                .or_else(|| stable_contexts.get(identity).cloned());
            if let Some(context) = stable_context {
                facts.push(SemanticFact::new(
                    subject.clone(),
                    SemanticFactKind::StableDeclarationContext,
                    SemanticFactValue::stable_declaration_context(context),
                ));
            }
            if let Some(owner) = self.body_ir_reference_requirement(span, identity) {
                facts.push(SemanticFact::new(
                    subject.clone(),
                    SemanticFactKind::RequiredReferenceTarget,
                    SemanticFactValue::canonical_identity(owner.clone()),
                ));
            }
        }

        for (&span, identities) in &self.expressions.expression_type_identities {
            let source_span = span;
            for (indices, identity) in identities {
                let subject = CompilerNodeId::expression_type_component_at(module_identity, span, indices);
                Self::record_semantic_reference(facts, &subject, source_span, identity, declarations);
            }
        }
    }

    /// Keep each synthetic iteration call's canonical target separate from the written iterable's call facts.
    /// Dependency loading must see the selected defaults before Body IR lowers the loop into calls.
    pub(super) fn record_semantic_iteration_facts(
        &self,
        module_identity: &str,
        declarations: &BTreeSet<&CanonicalSymbolId>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for (&span, hooks) in &self.protocols.body_ir_iterations {
            for (role, hook) in [("iter", hooks.iter.as_ref()), ("next", Some(&hooks.next))] {
                let Some(hook) = hook else {
                    continue;
                };
                let Some(identity) = &hook.canonical else {
                    continue;
                };
                let subject = CompilerNodeId::protocol_call_at(module_identity, span, role);
                Self::record_semantic_reference(facts, &subject, span, identity, declarations);
            }
        }
    }

    /// Attach one exact canonical target and its unique nearest owner, shared by written and synthetic references.
    pub(super) fn record_semantic_reference(
        facts: &mut Vec<SemanticFact>,
        subject: &CompilerNodeId,
        span: incan_semantics_core::HirSourceSpan,
        identity: &CanonicalSymbolId,
        declarations: &BTreeSet<&CanonicalSymbolId>,
    ) {
        facts.push(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::ReferenceSpan,
            SemanticFactValue::SourceSpan(span),
        ));
        facts.push(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::SymbolIdentity,
            SemanticFactValue::canonical_identity(identity.clone()),
        ));
        if let Some(owner) =
            incan_semantics_core::dependencies::closest_declaring_owner(declarations.iter().copied(), span)
        {
            facts.push(SemanticFact::new(
                subject.clone(),
                SemanticFactKind::ReferenceOwner,
                SemanticFactValue::canonical_identity(owner.clone()),
            ));
        }
    }

    /// Retain registry subjects from their checked declarations and the session's explicit package identity.
    pub(super) fn record_semantic_registry_facts(
        &self,
        module_identity: &str,
        package_identity: Option<&str>,
        facts: &mut Vec<SemanticFact>,
    ) {
        for description in &self.registry.descriptions {
            let registry = match &description.registry {
                RegistryDescriptionRegistry::Local { binding } => CompilerNodeId::declaration(module_identity, binding),
                RegistryDescriptionRegistry::Imported {
                    module_path, binding, ..
                } => CompilerNodeId::declaration(&module_path.join("::"), binding),
            };
            facts.push(SemanticFact::new(
                CompilerNodeId::declaration(module_identity, &description.declaration_name),
                SemanticFactKind::Registry,
                SemanticFactValue::registry_entry(SemanticRegistryEntry {
                    registry,
                    key: description.key.clone(),
                    descriptor: description.descriptor.clone(),
                    subject_kind: description.subject_kind,
                    subject_identity: format!("{module_identity}.{}", description.declaration_name),
                }),
            ));
        }

        for entry in &self.registry.explicit_entries {
            let (subject, subject_identity) = match entry.subject_kind {
                SemanticRegistrySubjectKind::CompilationUnit => {
                    (CompilerNodeId::module(module_identity), module_identity.to_owned())
                }
                SemanticRegistrySubjectKind::Package => {
                    let package_identity = package_identity
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{module_identity}::package"));
                    (CompilerNodeId::package(&package_identity), package_identity)
                }
                SemanticRegistrySubjectKind::Function | SemanticRegistrySubjectKind::Method => {
                    continue;
                }
            };
            facts.push(SemanticFact::new(
                subject,
                SemanticFactKind::Registry,
                SemanticFactValue::registry_entry(SemanticRegistryEntry {
                    registry: CompilerNodeId::declaration(module_identity, &entry.registry_name),
                    key: entry.key.clone(),
                    descriptor: entry.descriptor.clone(),
                    subject_kind: entry.subject_kind,
                    subject_identity,
                }),
            ));
        }
    }

    /// Return the resolved type recorded for the expression at `span`, if any.
    pub fn expr_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.expr_types.get(&span)
    }

    /// Admit registry-owned task, race and synchronization carriers without granting an equal-spelled source owner
    /// any SDK authority. Both checking and Body IR use this same provenance boundary.
    pub(crate) fn runtime_owner_admitted(&self, id: surface_types::SurfaceTypeId) -> bool {
        if !matches!(
            surface_types::category(id),
            SurfaceTypeCategory::AsyncTask | SurfaceTypeCategory::AsyncRace | SurfaceTypeCategory::AsyncSync
        ) {
            return false;
        }
        if surface_types::stdlib_module_path(id).is_none() {
            return false;
        }
        let name = surface_types::as_str(id);
        let Some(identity) = self
            .declarations
            .named_type_identities
            .get(name)
            .or_else(|| self.declarations.body_ir_native_type_identities.get(name))
        else {
            return true;
        };
        match &identity.origin {
            SymbolOrigin::Module(owner) => surface_types::is_exported_from_module(id, &owner.join(".")),
            SymbolOrigin::Package { library, module_path } => {
                surface_types::is_exported_from_package(id, library, module_path)
            }
            SymbolOrigin::RustCrate(owner) => surface_types::from_runtime_rust_path(&owner.join("::")) == Some(id),
            _ => false,
        }
    }

    /// Project a checked type without erasing its nominal owners. Carrier identities come only from the checker's
    /// accepted bindings; an unresolved name remains unresolved rather than borrowing another module's declaration.
    pub fn semantic_type(&self, ty: &ResolvedType) -> IncanType {
        self.qualify_semantic_type(semantic_type_from_resolved(ty))
    }

    /// Retain checker-owned nominal heads and leaves after an active declaration's binders have been classified.
    /// Direct bindings and resolved module-qualified references are equally authoritative paths to one declaration.
    /// Body IR consumes this result directly and must not rebuild identities from normalized source names.
    pub fn qualify_semantic_type(&self, ty: IncanType) -> IncanType {
        match ty {
            IncanType::Named(name) => {
                let identity = self
                    .declarations
                    .named_type_identities
                    .get(&name)
                    .or_else(|| self.declarations.body_ir_native_type_identities.get(&name))
                    .or_else(|| {
                        self.declarations
                            .qualified_type_references
                            .values()
                            .find(|reference| match &reference.resolved {
                                ResolvedType::Named(route) | ResolvedType::Generic(route, _) => route == &name,
                                _ => false,
                            })
                            .map(|reference| &reference.identity)
                    });
                let Some(identity) = identity else {
                    return IncanType::Named(name);
                };
                let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(identity) else {
                    return IncanType::Named(identity.declaration_name.clone());
                };
                if self
                    .declarations
                    .named_type_identities
                    .get(&key)
                    .is_some_and(|binding| binding != identity)
                {
                    return IncanType::Unsupported("conflicting checked nominal carrier binding".into());
                }
                IncanType::Named(key)
            }
            IncanType::Generic { base, args } => match self.qualify_semantic_type(IncanType::Named(base)) {
                IncanType::Named(base) => IncanType::Generic {
                    base,
                    args: args.into_iter().map(|ty| self.qualify_semantic_type(ty)).collect(),
                },
                refused => refused,
            },
            IncanType::Tuple(items) => {
                IncanType::Tuple(items.into_iter().map(|ty| self.qualify_semantic_type(ty)).collect())
            }
            IncanType::Ref(inner) => IncanType::Ref(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::NativeArray { element, length } => IncanType::NativeArray {
                element: Box::new(self.qualify_semantic_type(*element)),
                length,
            },
            IncanType::RefMut(inner) => IncanType::RefMut(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(self.qualify_semantic_type(*inner))),
            IncanType::Function {
                mut params,
                return_type,
            } => {
                for parameter in &mut params {
                    parameter.ty = self.qualify_semantic_type(parameter.ty.clone());
                }
                IncanType::Function {
                    params,
                    return_type: Box::new(self.qualify_semantic_type(*return_type)),
                }
            }
            ty => ty,
        }
    }
}

/// Return whether a callable parameter type carries borrow shape that lowering cannot recover from the callee alone.
pub(super) fn callable_param_needs_boundary_snapshot(ty: &ResolvedType) -> bool {
    if ty.is_union() {
        return true;
    }
    match ty {
        ResolvedType::Ref(_) | ResolvedType::RefMut(_) | ResolvedType::FrozenStr | ResolvedType::FrozenBytes => true,
        ResolvedType::Function(params, ret) => {
            params
                .iter()
                .any(|param| callable_param_needs_boundary_snapshot(&param.ty))
                || callable_param_needs_boundary_snapshot(ret)
        }
        ResolvedType::Generic(_, args) => args.iter().any(callable_param_needs_boundary_snapshot),
        ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
            callable_param_needs_boundary_snapshot(inner)
        }
        ResolvedType::FrozenDict(key, value) => {
            callable_param_needs_boundary_snapshot(key) || callable_param_needs_boundary_snapshot(value)
        }
        ResolvedType::Tuple(items) => items.iter().any(callable_param_needs_boundary_snapshot),
        _ => false,
    }
}

/// Render the compiler-owned module identity used by semantic fact subjects.
pub(super) fn semantic_module_identity(module_path: &[String]) -> String {
    incan_semantics_core::module_identity_for_path(module_path)
}

/// Convert a typechecker source-target artifact into the backend-neutral fact payload.
pub(super) fn semantic_source_target_from_typecheck(target: &SourceTargetInfo) -> SemanticSourceTarget {
    SemanticSourceTarget::from_kind_str(target.module_path.clone(), target.name.clone(), target.kind.as_str())
}

/// Convert a resolved function binding into the semantic callable type stored on declaration facts.
pub(super) fn semantic_type_from_function_binding(binding: &FunctionBindingInfo) -> IncanType {
    IncanType::Function {
        params: binding
            .params
            .iter()
            .map(semantic_callable_param_from_resolved)
            .collect(),
        return_type: Box::new(semantic_type_from_resolved(&binding.return_type)),
    }
}

/// Convert the current typechecker type universe into the backend-neutral Incan semantic type model.
///
/// This converts structural shape and preserves source spellings, not nominal authority. Checked fact and Body IR
/// consumers must use [`TypeCheckInfo::semantic_type`] so the accepting checker's nominal owners survive the
/// boundary. Builtin tuple annotations and inferred tuple literals share one structural semantic shape.
pub fn semantic_type_from_resolved(ty: &ResolvedType) -> IncanType {
    match ty {
        ResolvedType::Never => IncanType::Never,
        ResolvedType::Int => IncanType::Primitive(IncanPrimitiveType::Int),
        ResolvedType::Float => IncanType::Primitive(IncanPrimitiveType::Float),
        ResolvedType::Numeric(id) => IncanType::Primitive(IncanPrimitiveType::Numeric(*id)),
        ResolvedType::Bool => IncanType::Primitive(IncanPrimitiveType::Bool),
        ResolvedType::Str => IncanType::Primitive(IncanPrimitiveType::Str),
        ResolvedType::Bytes => IncanType::Primitive(IncanPrimitiveType::Bytes),
        ResolvedType::FrozenStr => IncanType::Primitive(IncanPrimitiveType::FrozenStr),
        ResolvedType::FrozenBytes => IncanType::Primitive(IncanPrimitiveType::FrozenBytes),
        ResolvedType::FrozenList(elem) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenList).to_string(),
            args: vec![semantic_type_from_resolved(elem)],
        },
        ResolvedType::FrozenDict(key, value) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenDict).to_string(),
            args: vec![semantic_type_from_resolved(key), semantic_type_from_resolved(value)],
        },
        ResolvedType::FrozenSet(elem) => IncanType::Generic {
            base: collection_types::as_str(CollectionTypeId::FrozenSet).to_string(),
            args: vec![semantic_type_from_resolved(elem)],
        },
        ResolvedType::Unit => IncanType::Primitive(IncanPrimitiveType::Unit),
        ResolvedType::Named(name) => IncanType::Named(name.clone()),
        ResolvedType::Trait(identity) => IncanType::Trait(identity.clone()),
        ResolvedType::Generic(base, args) if collection_types::from_str(base) == Some(CollectionTypeId::Tuple) => {
            IncanType::Tuple(args.iter().map(semantic_type_from_resolved).collect())
        }
        ResolvedType::Generic(base, args)
            if incan_lang::lang::types::numerics::decimal_constructor_from_str(base).is_some() =>
        {
            match args.as_slice() {
                [ResolvedType::TypeVar(precision), ResolvedType::TypeVar(scale)] => {
                    match (precision.parse(), scale.parse()) {
                        (Ok(precision), Ok(scale)) => IncanType::Decimal { precision, scale },
                        _ => IncanType::Unknown,
                    }
                }
                _ => IncanType::Unknown,
            }
        }
        ResolvedType::Generic(base, args) => IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(semantic_type_from_resolved).collect(),
        },
        ResolvedType::Function(params, return_type) => IncanType::Function {
            params: params.iter().map(semantic_callable_param_from_resolved).collect(),
            return_type: Box::new(semantic_type_from_resolved(return_type)),
        },
        ResolvedType::TypeToken(inner) => IncanType::TypeToken(Box::new(semantic_type_from_resolved(inner))),
        ResolvedType::Tuple(items) => IncanType::Tuple(items.iter().map(semantic_type_from_resolved).collect()),
        ResolvedType::TypeVar(name) => IncanType::TypeVar(name.clone()),
        ResolvedType::SelfType => IncanType::SelfType,
        ResolvedType::Ref(inner) => IncanType::Ref(Box::new(semantic_type_from_resolved(inner))),
        ResolvedType::RefMut(inner) => IncanType::RefMut(Box::new(semantic_type_from_resolved(inner))),
        // A builtin's own Rust carrier is that builtin, whichever spelling names it.
        ResolvedType::RustPath(path) => incan_semantics_core::RuntimeBuiltinType::from_carrier_path(path)
            .map_or_else(|| IncanType::RustInteropPath(path.clone()), IncanType::NativeBuiltin),
        ResolvedType::CallSiteInfer => IncanType::Infer,
        ResolvedType::Unknown => IncanType::Unknown,
    }
}

/// Convert typechecker callable parameter metadata into semantic callable parameter metadata.
pub(in crate::typechecker) fn semantic_callable_param_from_resolved(param: &CallableParam) -> IncanCallableParam {
    IncanCallableParam {
        name: param.name.clone(),
        ty: semantic_type_from_resolved(&param.ty),
        kind: match param.kind {
            ParamKind::Normal => IncanCallableParamKind::Normal,
            ParamKind::RestPositional => IncanCallableParamKind::RestPositional,
            ParamKind::RestKeyword => IncanCallableParamKind::RestKeyword,
        },
        has_default: param.has_default,
        is_partial_preset: param.is_partial_preset,
        is_mut: param.is_mut,
        input_access: incan_semantics_core::IncanInputAccess::Inferred,
        native_owned: false,
    }
}
