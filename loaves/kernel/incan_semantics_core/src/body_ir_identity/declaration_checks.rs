//! Well-formedness of static, decorated, native enum and trait implementation declarations, and canonical call
//! identities.

use super::*;

impl BodyIrModule {
    /// Prove a static belongs to this module and its initializer matches its exact retained carrier. A computed
    /// initializer's body must carry the static's own canonical identity and declaration id, so a module cannot attach
    /// another declaration's body to it. A `const` that is not a retained constant is read-only storage of the same
    /// shape.
    pub fn is_well_formed_static_declaration(&self, declaration: &crate::body_ir::StaticDeclaration) -> bool {
        self.is_well_formed_static_declaration_in_project(declaration, std::slice::from_ref(self))
    }

    /// Validate storage against its owning module and the project's single authenticated callable inventory.
    /// A compiled decorator wrapper may retain its declaration frame on the entry module rather than this owner.
    pub fn is_well_formed_static_declaration_in_project(
        &self,
        declaration: &crate::body_ir::StaticDeclaration,
        modules: &[BodyIrModule],
    ) -> bool {
        let kind = match declaration.canonical.kind {
            SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Method => {
                return self.decorated_bindings.iter().any(|binding| {
                    binding.original_body.canonical.as_ref() == Some(&declaration.canonical)
                        && self.is_well_formed_decorated_binding_in_project(binding, modules)
                });
            }
            SemanticSourceTargetKind::Const => SemanticSourceTargetKind::Const,
            _ => SemanticSourceTargetKind::Static,
        };
        let Some(declaration_id) =
            self.declaration_id_for_canonical(&declaration.canonical, SymbolNamespace::OrdinaryLexical, kind)
        else {
            return false;
        };
        if let StaticInitializer::Computed(body) = &declaration.initial
            && (body.canonical.as_ref() != Some(&declaration.canonical) || body.decl_id != declaration_id)
        {
            return false;
        }
        static_initializer_matches_parts(&declaration.ty, &declaration.initial)
    }

    /// Prove an RFC 036 original and wrapper belong to the same source declaration and obey the checked replacement
    /// lifetime. Only module replacements acquire initialized storage; originals never replace ordinary lookup. An
    /// async original remains the coroutine producer while its authenticated wrapper is the synchronous future
    /// constructor held by the replacement. A compiled wrapper contributes its declaration frame; its implementation
    /// remains in its Loaf.
    pub fn is_well_formed_decorated_binding(&self, binding: &crate::body_ir::DecoratedBinding) -> bool {
        self.is_well_formed_decorated_binding_in_project(binding, std::slice::from_ref(self))
    }

    /// Validate a replacement against its module's storage and the retained project-wide callable declarations.
    /// Context contributes authenticated signatures only; it grants no additional original implementation body.
    pub fn is_well_formed_decorated_binding_in_project(
        &self,
        binding: &crate::body_ir::DecoratedBinding,
        modules: &[BodyIrModule],
    ) -> bool {
        if binding.initialization == crate::body_ir::DecoratedInitialization::PerInvocation {
            return false;
        }
        let original = &binding.original_body;
        if let Some(initializer) = &binding.initializer {
            return binding.initialization == crate::body_ir::DecoratedInitialization::PerInstantiation
                && self.is_well_formed_generic_decorated_binding(original, initializer);
        }
        if binding.initialization != crate::body_ir::DecoratedInitialization::ModuleStorage {
            return false;
        }
        let mut replacements = self
            .static_declarations
            .iter()
            .filter(|storage| original.canonical.as_ref() == Some(&storage.canonical));
        let Some(replacement) = replacements.next() else {
            return false;
        };
        if replacements.next().is_some() {
            return false;
        }
        let StaticInitializer::Computed(initializer) = &replacement.initial else {
            return false;
        };
        original.canonical.as_ref() == Some(&replacement.canonical)
            && matches!(replacement.canonical.kind, SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Method)
            && original.type_parameters.is_empty()
            && self.body_has_canonical_direct_call_id(original)
            && initializer.canonical == original.canonical
            && initializer.decl_id == original.decl_id
            && modules.iter().flat_map(Self::callable_declaration_frames).any(|body| {
                body.canonical == original.canonical
                    && matches!(&replacement.ty, IncanType::Function { params, return_type }
                        if params.len() == body.params.len()
                            && params.iter().zip(&body.params).all(|(left, right)| decorated_parameter_matches(body, left, right))
                            && **return_type == body.return_type)
            })
            && static_initializer_matches_parts(&replacement.ty, &replacement.initial)
    }

    /// Validate a per-instantiation initializer against its original and wrapper, without granting module storage.
    /// All three frames retain their canonical direct-call identity and exact formal binders; another declaration
    /// cannot provide any frame.
    pub(super) fn is_well_formed_generic_decorated_binding(&self, original: &Body, initializer: &Body) -> bool {
        let Some(identity) = &original.canonical else {
            return false;
        };
        if identity.kind != SemanticSourceTargetKind::Function
            || original.type_parameters.is_empty()
            || original.type_parameters.iter().any(String::is_empty)
            || original.type_parameters.iter().collect::<BTreeSet<_>>().len() != original.type_parameters.len()
            || original.is_async
            || initializer.is_async
            || !self.body_has_canonical_direct_call_id(original)
            || !self.body_has_canonical_direct_call_id(initializer)
            || initializer.canonical != original.canonical
            || initializer.decl_id != original.decl_id
            || initializer.type_parameters != original.type_parameters
            || !initializer.params.is_empty()
            || !initializer.param_locals.is_empty()
            || self
                .static_declarations
                .iter()
                .any(|storage| &storage.canonical == identity)
        {
            return false;
        }
        let IncanType::Function { params, return_type } = &initializer.return_type else {
            return false;
        };
        let mut wrappers = self
            .callable_declaration_frames()
            .filter(|body| body.canonical == original.canonical);
        let Some(wrapper) = wrappers.next() else {
            return false;
        };
        wrappers.next().is_none()
            && wrapper.decl_id == original.decl_id
            && self.body_has_canonical_direct_call_id(wrapper)
            && !wrapper.is_async
            && wrapper.type_parameters == original.type_parameters
            && params.len() == wrapper.params.len()
            && params
                .iter()
                .zip(&wrapper.params)
                .all(|(left, right)| decorated_parameter_matches(wrapper, left, right))
            && **return_type == wrapper.return_type
    }

    /// Whether a normal enum layout and every variant belong to their exact declared owner. A native enum may retain
    /// only its checked Copy capability beside that identity; the adapter verifies it against the concrete Rust type.
    pub fn is_well_formed_native_enum_declaration(&self, declaration: &crate::body_ir::EnumDeclaration) -> bool {
        if declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
        }
        if matches!(declaration.canonical.origin, crate::SymbolOrigin::RustCrate(_)) {
            let Some(mut expected) = crate::body_ir::native_enum_declaration(
                &declaration.native_item,
                declaration.type_parameters.clone(),
                declaration
                    .variants
                    .iter()
                    .map(|variant| (variant.name.clone(), variant.fields.clone()))
                    .collect(),
            ) else {
                return false;
            };
            if declaration.derives.len() > 1 || declaration.derives.iter().any(|derive| derive != "Copy") {
                return false;
            }
            expected.derives.clone_from(&declaration.derives);
            return declaration == &expected
                && crate::body_ir::native_nominal_module(&declaration.canonical).as_deref()
                    == Some(self.module_id.path())
                && !declaration.variants.is_empty()
                && declaration
                    .variants
                    .iter()
                    .map(|variant| &variant.name)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == declaration.variants.len();
        }
        self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Enum,
        ) == Some(declaration.direct_declaration_id.clone())
            && declaration.canonical.declaration_name == declaration.name
            && !declaration.variants.is_empty()
            && declaration
                .variants
                .iter()
                .map(|value| &value.name)
                .collect::<BTreeSet<_>>()
                .len()
                == declaration.variants.len()
            && declaration
                .variants
                .iter()
                .map(|value| &value.canonical)
                .collect::<BTreeSet<_>>()
                .len()
                == declaration.variants.len()
            && declaration.variants.iter().all(|variant| {
                variant.canonical.namespace == SymbolNamespace::Member
                    && variant.canonical.kind == SemanticSourceTargetKind::Variant
                    && variant.canonical.scope_discriminant.is_none()
                    && variant.canonical.origin == declaration.canonical.origin
                    && variant.direct_declaration_id
                        == CompilerNodeId::declaration_at(self.module_id.path(), variant.canonical.declaration_span)
                    && declares_member(&declaration.canonical, &variant.canonical)
                    && variant.canonical.declaration_name == variant.name
            })
    }

    /// Whether `id` is a span-derived declaration identity of exactly the shape lowering emits for this module.
    pub fn is_own_span_declaration_id(&self, id: &CompilerNodeId) -> bool {
        if self.module_id.kind() != CompilerNodeKind::Module || id.kind() != CompilerNodeKind::Declaration {
            return false;
        }
        let prefix = format!("{}#decl.", self.module_id.path());
        let Some(span) = id.path().strip_prefix(&prefix) else {
            return false;
        };
        let span = match span.split_once("/expansion.") {
            Some((range, expansion))
                if expansion.len() == 64 && expansion.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                range
            }
            Some(_) => return false,
            None => span,
        };
        let Some((start, end)) = span.split_once("..") else {
            return false;
        };
        matches!(
            (start.parse::<usize>(), end.parse::<usize>()),
            (Ok(start), Ok(end)) if start <= end
        )
    }

    /// The physical declaration id of an already-retained canonical identity, when that identity is a module-level
    /// declaration of this module in `expected_namespace` with `expected_kind`.
    pub fn declaration_id_for_canonical(
        &self,
        identity: &CanonicalSymbolId,
        expected_namespace: SymbolNamespace,
        expected_kind: SemanticSourceTargetKind,
    ) -> Option<CompilerNodeId> {
        let owner = canonical_module_identity(identity)?;
        (identity.namespace == expected_namespace
            && identity.kind == expected_kind
            && identity.scope_discriminant.is_none()
            && owner == self.module_id.path())
        .then(|| CompilerNodeId::declaration_at(self.module_id.path(), identity.declaration_span))
    }

    /// Whether a body is governed by its own type parameters or by the checked parameters of its enclosing owner.
    /// A receiver method need not repeat its owner's binders on the method declaration; it still requires caller
    /// instantiation and must not become a compiled nongeneric Loaf callable. An open Self receiver or return slot
    /// also requires specialization even when its trait and method declare no type parameters.
    pub fn body_requires_instantiation(&self, body: &Body) -> bool {
        if !body.type_parameters.is_empty()
            || body
                .locals
                .first()
                .is_some_and(|local| local.ty == crate::IncanType::SelfType)
        {
            return true;
        }
        let Some(identity) = &body.canonical else {
            return false;
        };
        if identity.namespace != SymbolNamespace::Member {
            return false;
        }
        self.nominal_declarations.iter().any(|owner| {
            !owner.type_parameters.is_empty()
                && owner.canonical.origin == identity.origin
                && declares_member(&owner.canonical, identity)
        }) || self.enum_declarations.iter().any(|owner| {
            !owner.type_parameters.is_empty()
                && owner.canonical.origin == identity.origin
                && declares_member(&owner.canonical, identity)
        })
    }

    /// Whether a function, source Partial forwarding body, or retained nominal method or computed-property getter has
    /// its own canonical declaration identity and span-derived direct-call id. Methods and getters must belong to a
    /// retained nominal, normal enum or trait owner whose span contains them; an instance receiver must match that
    /// owner. A getter additionally requires its exact implicit immutable receiver and no ordinary or method-scoped
    /// parameters.
    pub fn body_has_canonical_direct_call_id(&self, body: &Body) -> bool {
        body.direct_call_id == CompilerNodeId::declaration_at(self.module_id.path(), body.span)
            && body.canonical.as_ref().is_some_and(|canonical| {
                let namespace = match canonical.kind {
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial => {
                        SymbolNamespace::OrdinaryLexical
                    }
                    SemanticSourceTargetKind::Method | SemanticSourceTargetKind::Property => SymbolNamespace::Member,
                    _ => return false,
                };
                if canonical.kind == SemanticSourceTargetKind::Property && !is_getter_frame(body, canonical) {
                    return false;
                }
                canonical.declaration_name == body.name
                    && self.declaration_id_for_canonical(canonical, namespace, canonical.kind.clone())
                        == Some(body.direct_call_id.clone())
                    && (matches!(
                        canonical.kind,
                        SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial
                    ) || self.nominal_declarations.iter().any(|owner| {
                        self.is_well_formed_nominal_declaration(owner)
                            && (canonical.kind != SemanticSourceTargetKind::Property
                                || matches!(
                                    owner.canonical.kind,
                                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                                ))
                            && canonical.origin == owner.canonical.origin
                            && declares_member(&owner.canonical, canonical)
                            && body.locals.first().is_none_or(|receiver| {
                                !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                    || receiver.ty == nominal_receiver_type(owner)
                            })
                    }) || self.enum_declarations.iter().any(|owner| {
                        canonical.kind == SemanticSourceTargetKind::Method
                            && self.is_well_formed_native_enum_declaration(owner)
                            && canonical.origin == owner.canonical.origin
                            && declares_member(&owner.canonical, canonical)
                            && body.locals.first().is_none_or(|receiver| {
                                !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                    || receiver.ty == crate::IncanType::Named(owner.carrier_name())
                            })
                    }) || self.trait_declarations.iter().any(|owner| {
                        canonical.kind == SemanticSourceTargetKind::Method
                            && self
                                .declaration_id_for_canonical(
                                    owner,
                                    SymbolNamespace::OrdinaryLexical,
                                    SemanticSourceTargetKind::Trait,
                                )
                                .is_some()
                            && canonical.origin == owner.origin
                            && declares_member(owner, canonical)
                            && body.locals.first().is_none_or(|receiver| {
                                !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                    || receiver.ty == crate::IncanType::SelfType
                            })
                    }))
            })
    }

    /// Validate a concrete trait slot against retained physical owners (a nominal or a normal enum) and implementation
    /// bodies.
    ///
    /// A default belongs to a retained trait: the slot's own or a subtrait's. An explicit implementation must belong
    /// to the concrete owner and takes no trait type arguments; its name may differ from the slot for a checked
    /// method alias. A default's arguments may contain only closed types or the exact nominal owner's declared
    /// parameters, which concrete instance scheduling must close before execution; its name still matches its slot.
    /// Matching method spellings never suffice to establish any of these relationships. A property slot may only be
    /// filled by its same-named owned getter, never a method or property default.
    pub fn is_well_formed_trait_implementation(&self, value: &crate::body_ir::TraitImplementation) -> bool {
        let owned = declares_member(&value.owner, &value.implementation);
        let inherited = value.method == value.implementation
            || self.trait_declarations.iter().any(|owner| {
                owner.origin == value.implementation.origin && declares_member(owner, &value.implementation)
            });
        let arguments_close = if owned {
            value.type_arguments.is_empty() && self.trait_arguments_are_owner_bound(value)
        } else {
            self.trait_arguments_are_owner_bound(value)
        };
        value.method.namespace == SymbolNamespace::Member
            && (owned || value.method.declaration_name == value.implementation.declaration_name)
            && matches!(
                value.implementation.kind,
                SemanticSourceTargetKind::Method | SemanticSourceTargetKind::Property
            )
            && value.method.kind == value.implementation.kind
            && (value.method.kind != SemanticSourceTargetKind::Property
                || owned && value.method.declaration_name == value.implementation.declaration_name)
            && (self
                .nominal_declarations
                .iter()
                .any(|owner| owner.canonical == value.owner && self.is_well_formed_nominal_declaration(owner))
                || self
                    .enum_declarations
                    .iter()
                    .any(|owner| owner.canonical == value.owner && self.is_well_formed_native_enum_declaration(owner)))
            && self.trait_declarations.iter().any(|owner| {
                self.declaration_id_for_canonical(
                    owner,
                    SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Trait,
                )
                .is_some()
                    && owner.origin == value.method.origin
                    && declares_member(owner, &value.method)
                    && matches!(
                        value.method.kind,
                        SemanticSourceTargetKind::Method | SemanticSourceTargetKind::Property
                    )
            })
            && arguments_close
            && self.bodies.iter().any(|body| {
                body.canonical.as_ref() == Some(&value.implementation)
                    && self.body_has_canonical_direct_call_id(body)
                    && (owned || inherited)
            })
    }

    /// Validate slot and default arguments against the exact retained owner's parameter scope. A nominal template may
    /// refer to its declared parameters recursively; an enum has no parameter scope here. Missing or malformed
    /// owners, foreign variables, `Self`, and uninferred placeholders never authorize a template. This validates
    /// existing facts only; consumers must still instantiate every admitted owner parameter before constructing
    /// native MIR.
    pub fn trait_arguments_are_owner_bound(&self, value: &crate::body_ir::TraitImplementation) -> bool {
        if let Some(owner) = self
            .nominal_declarations
            .iter()
            .find(|owner| owner.canonical == value.owner && self.is_well_formed_nominal_declaration(owner))
        {
            return value
                .type_arguments
                .iter()
                .map(|binding| &binding.argument)
                .chain(&value.slot_arguments)
                .all(|argument| is_owner_bound_type(argument, &owner.type_parameters));
        }
        self.enum_declarations
            .iter()
            .any(|owner| owner.canonical == value.owner && self.is_well_formed_native_enum_declaration(owner))
            && value
                .type_arguments
                .iter()
                .map(|binding| &binding.argument)
                .chain(&value.slot_arguments)
                .all(is_closed_type)
    }
}
