//! Identity well-formedness of a [`BodyIrModule`]: the checks a consumer runs before it trusts a module's span-derived
//! declaration identities, retained declaration records and direct call targets.
//!
//! Lowering derives every same-module identity from a declaration's span and carries the checker's canonical identity
//! beside it. Body IR travels as data (in a published executable representation, for instance), so a consumer cannot
//! take those facts on trust: a malformed module could copy a valid identity onto an unrelated body or carry a
//! coherent-looking foreign record. These checks never mint or complete an identity; they only confirm that the
//! retained ones agree with each other and belong to this module. They answer the same question for every consumer.

use incan_lang::lang::types::collections::{self, CollectionTypeId};
use std::collections::BTreeSet;

use crate::body_ir::{
    Body, BodyIrModule, CallableParamDefault, Constant, FieldlessEnumDeclaration, LocalOrigin, NominalDeclaration,
    StaticInitializer, ValueEnumDeclaration,
};
use crate::{
    CanonicalSymbolId, CompilerNodeId, CompilerNodeKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, SymbolOrigin, canonical_module_identity,
};

/// Match an initialized method's explicit passing reference against its receiver frame's nominal type and mode.
/// Ordinary inputs must agree exactly; this does not authorize another owner or a different receiver mode.
fn decorated_parameter_matches(
    body: &Body,
    left: &crate::IncanCallableParam,
    right: &crate::body_ir::CallableParam,
) -> bool {
    if left.kind != right.kind || left.native_owned != right.native_owned {
        return false;
    }
    match body.locals.get(right.local.index()).map(|local| &local.origin) {
        Some(LocalOrigin::Receiver { mutable }) => match (&left.ty, mutable) {
            (IncanType::Ref(inner), false) | (IncanType::RefMut(inner), true) => **inner == right.ty && !left.is_mut,
            _ => false,
        },
        _ => decorated_input_type_matches(left, &right.ty) && left.is_mut == right.mutable,
    }
}

/// Require each retained rest frame to contain exactly the source signature's checked element type.
fn decorated_input_type_matches(parameter: &crate::IncanCallableParam, packed: &IncanType) -> bool {
    match parameter.kind {
        crate::IncanCallableParamKind::Normal => parameter.ty == *packed,
        crate::IncanCallableParamKind::RestPositional => matches!(packed,
            IncanType::Generic { base, args } if base == "List" && args.as_slice() == [parameter.ty.clone()]),
        crate::IncanCallableParamKind::RestKeyword => matches!(packed,
            IncanType::Generic { base, args } if base == "Dict"
                && args.as_slice() == [IncanType::Primitive(IncanPrimitiveType::Str), parameter.ty.clone()]),
    }
}

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
    /// lifetime. Only module replacements acquire initialized storage; originals never replace ordinary lookup.
    /// An authenticated compiled wrapper contributes its declaration frame; its implementation remains in its Loaf.
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
            return binding.initializer.is_none() && self.is_well_formed_invocation_binding(binding);
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
            && !original.is_async
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
    /// The same formal binders must govern both templates; another source declaration cannot provide either body.
    fn is_well_formed_generic_decorated_binding(&self, original: &Body, initializer: &Body) -> bool {
        let Some(identity) = &original.canonical else {
            return false;
        };
        if identity.kind != SemanticSourceTargetKind::Function
            || original.type_parameters.is_empty()
            || original.type_parameters.iter().any(String::is_empty)
            || original.type_parameters.iter().collect::<BTreeSet<_>>().len() != original.type_parameters.len()
            || original.params.iter().any(|parameter| {
                crate::body_ir::decorated_signature_uses_parameters(&parameter.ty, &original.type_parameters)
            })
            || crate::body_ir::decorated_signature_uses_parameters(&original.return_type, &original.type_parameters)
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
            && wrapper.type_parameters == original.type_parameters
            && params.len() == wrapper.params.len()
            && params
                .iter()
                .zip(&wrapper.params)
                .all(|(left, right)| decorated_parameter_matches(wrapper, left, right))
            && **return_type == wrapper.return_type
    }

    /// Require one source generic frame and its visible wrapper, with identical declared binders and no storage.
    /// Only signatures using those binders have per-call semantics; closed signatures require the separate cache
    /// policy.
    fn is_well_formed_invocation_binding(&self, binding: &crate::body_ir::DecoratedBinding) -> bool {
        let original = &binding.original_body;
        let Some(identity) = original.canonical.as_ref() else {
            return false;
        };
        if identity.kind != SemanticSourceTargetKind::Function
            || original.type_parameters.is_empty()
            || original.type_parameters.iter().any(String::is_empty)
            || original.type_parameters.iter().collect::<BTreeSet<_>>().len() != original.type_parameters.len()
            || original.is_async
            || original.is_generator()
            || !self.body_has_canonical_direct_call_id(original)
            || self
                .static_declarations
                .iter()
                .any(|storage| &storage.canonical == identity)
        {
            return false;
        }
        let uses_parameters =
            original.params.iter().any(|parameter| {
                crate::body_ir::decorated_signature_uses_parameters(&parameter.ty, &original.type_parameters)
            }) || crate::body_ir::decorated_signature_uses_parameters(&original.return_type, &original.type_parameters);
        uses_parameters
            && self.callable_declaration_frames().any(|wrapper| {
                wrapper.canonical.as_ref() == Some(identity)
                    && wrapper.decl_id == original.decl_id
                    && self.body_has_canonical_direct_call_id(wrapper)
                    && wrapper.type_parameters == original.type_parameters
                    && wrapper.params == original.params
                    && wrapper.return_type == original.return_type
                    && !wrapper.is_async
            })
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

    /// Whether a retained layout agrees with its canonical identities and physical declaration scope. Native layouts
    /// either match a fixed carrier or extend an opaque skeleton with public metadata-selected fields, whose exact
    /// native types rustc verifies. A linked Loaf's item must encode its canonical declaration. A selected native
    /// equality witness must be a member of its model; lowering separately validates that body's executable ABI.
    pub fn is_well_formed_nominal_declaration(&self, declaration: &NominalDeclaration) -> bool {
        if declaration.trait_adoptions.iter().enumerate().any(|(index, adoption)| {
            adoption.trait_identity.kind != SemanticSourceTargetKind::Trait
                || adoption.trait_identity.namespace != SymbolNamespace::OrdinaryLexical
                || adoption.trait_identity.declaration_name.is_empty()
                || adoption.trait_identity.scope_discriminant.is_some()
                || adoption.methods.as_ref().is_some_and(|methods| {
                    methods.iter().enumerate().any(|(index, method)| {
                        method.kind != SemanticSourceTargetKind::Method
                            || method.namespace != SymbolNamespace::Member
                            || method.scope_discriminant.is_some()
                            || methods[..index].contains(method)
                    })
                })
                || adoption
                    .primitive_adapter
                    .as_ref()
                    .is_some_and(|adapter| !adapter.is_well_formed_for(declaration, adoption))
                || declaration.trait_adoptions[..index].contains(adoption)
        }) {
            return false;
        }
        if let Some(metadata) = &declaration.reflection_metadata {
            let Some(slots) = &declaration.reflection_fields else {
                return false;
            };
            if slots.len() != metadata.len()
                || slots
                    .iter()
                    .zip(metadata)
                    .any(|(slot, record)| declaration.fields.get(*slot) != Some(&record.name))
            {
                return false;
            }
        }
        if !declaration.native_item.is_empty()
            && crate::body_ir::native_nominal_item(&declaration.canonical)
                .or_else(|| crate::body_ir::loaf_item_path(&declaration.canonical))
                .as_deref()
                != Some(declaration.native_item.as_str())
        {
            return false;
        }
        if matches!(
            declaration.canonical.origin,
            crate::SymbolOrigin::Builtin | crate::SymbolOrigin::RustCrate(_)
        ) {
            let exact = crate::body_ir::native_carrier_nominal(&declaration.canonical).or_else(|| {
                crate::body_ir::native_opaque_nominal(&declaration.canonical, declaration.type_parameter_count)
            });
            return (exact.as_ref() == Some(declaration) || self.is_well_formed_native_struct_layout(declaration))
                && crate::body_ir::native_nominal_module(&declaration.canonical).as_deref()
                    == Some(self.module_id.path());
        }
        if !declaration.native_type_arguments.is_empty() {
            return false;
        }
        if declaration.display_method.as_ref().is_some_and(|method| {
            method.kind != SemanticSourceTargetKind::Method || method.namespace != SymbolNamespace::Member
        }) {
            return false;
        }
        if declaration.comparison_equality.as_ref().is_some_and(|method| {
            !matches!(
                declaration.canonical.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
            ) || method.kind != SemanticSourceTargetKind::Method
                || method.namespace != SymbolNamespace::Member
                || method.origin != declaration.canonical.origin
                || !declares_member(&declaration.canonical, method)
        }) {
            return false;
        }
        if declaration.type_parameters.len() != declaration.type_parameter_count
            || declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
        }
        if declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
            return self.declaration_id_for_canonical(
                &declaration.canonical,
                SymbolNamespace::OrdinaryLexical,
                SemanticSourceTargetKind::Newtype,
            ) == Some(declaration.direct_declaration_id.clone())
                && declaration.canonical.declaration_name == declaration.name
                && declaration.fields == ["0"]
                && declaration.field_identities == [declaration.canonical.clone()]
                && declaration.field_types.len() == 1
                && declaration.field_public == [true]
                && declaration.field_aliases.iter().all(Option::is_none)
                && !declaration.has_field_defaults
                && declaration.class_inheritance.is_none()
                && declaration.method_partial_defaults.is_empty()
                && Self::has_well_formed_newtype_string_conversion(declaration)
                && self.has_well_formed_newtype_deserialize(declaration);
        }
        if declaration.newtype_deserialize != crate::body_ir::NewtypeDeserialize::Unselected
            || declaration.newtype_string_conversion != crate::body_ir::NewtypeStringConversion::Unselected
        {
            return false;
        }
        if !self.are_well_formed_method_partial_defaults(declaration) {
            return false;
        }
        if !self.class_inheritance_is_well_formed(declaration) {
            return false;
        }

        matches!(
            declaration.canonical.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
        ) && self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            declaration.canonical.kind.clone(),
        ) == Some(declaration.direct_declaration_id.clone())
            && declaration.canonical.declaration_name == declaration.name
            && declaration.fields.len() == declaration.field_identities.len()
            && declaration.fields.len() == declaration.field_types.len()
            && declaration.fields.len() == declaration.field_public.len()
            && (declaration.field_aliases.is_empty() || declaration.fields.len() == declaration.field_aliases.len())
            && declaration.reflection_fields.as_ref().is_none_or(|slots| {
                slots.iter().all(|slot| *slot < declaration.fields.len())
                    && slots.windows(2).all(|pair| pair[0] < pair[1])
            })
            && declaration.fields.iter().collect::<BTreeSet<_>>().len() == declaration.fields.len()
            && declaration.field_identities.iter().collect::<BTreeSet<_>>().len() == declaration.field_identities.len()
            && declaration
                .fields
                .iter()
                .zip(&declaration.field_identities)
                .enumerate()
                .all(|(index, (name, identity))| {
                    identity.namespace == SymbolNamespace::Member
                        && identity.kind == SemanticSourceTargetKind::Field
                        && identity.scope_discriminant.is_none()
                        && identity.origin == declaration.canonical.origin
                        && (declares_member(&declaration.canonical, identity)
                            || self.inherited_field_matches(declaration, index))
                        && identity.declaration_name == *name
                })
    }

    /// Validate the identity shape of a retained public native struct layout. Ordered public fields, native binders
    /// and a Copy capability may extend the opaque declaration; rustc verifies fields, types and the Copy
    /// implementation.
    fn is_well_formed_native_struct_layout(&self, declaration: &NominalDeclaration) -> bool {
        if !matches!(declaration.canonical.origin, crate::SymbolOrigin::RustCrate(_))
            || (declaration.fields.is_empty() && declaration.derives != ["Copy"])
            || declaration.fields.len() != declaration.field_types.len()
            || declaration.fields.len() != declaration.field_identities.len()
            || declaration.field_public != vec![true; declaration.fields.len()]
            || declaration.fields.iter().any(String::is_empty)
            || declaration.fields.iter().collect::<BTreeSet<_>>().len() != declaration.fields.len()
            || declaration.type_parameters.len() != declaration.type_parameter_count
            || declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
        }
        for (name, field) in declaration.fields.iter().zip(&declaration.field_identities) {
            let mut expected = declaration.canonical.clone();
            expected.namespace = SymbolNamespace::Member;
            expected.kind = SemanticSourceTargetKind::Field;
            expected.declaration_name = name.clone();
            if *field != expected {
                return false;
            }
        }
        let Some(mut opaque) =
            crate::body_ir::native_opaque_nominal(&declaration.canonical, declaration.type_parameter_count)
        else {
            return false;
        };
        opaque.fields = declaration.fields.clone();
        opaque.field_types = declaration.field_types.clone();
        opaque.field_identities = declaration.field_identities.clone();
        opaque.field_public = declaration.field_public.clone();
        opaque.type_parameters = declaration.type_parameters.clone();
        if declaration.derives == ["Copy"] {
            opaque.derives = declaration.derives.clone();
        }
        opaque == *declaration
    }

    /// Validate a bounded local class edge and complete ancestor-slot mapping without recursively validating bodies.
    /// Original bodies are validated separately; nearest dispatch is checked against their retained owner spans.
    fn class_inheritance_is_well_formed(&self, declaration: &NominalDeclaration) -> bool {
        let Some(inheritance) = &declaration.class_inheritance else {
            return true;
        };
        if declaration.canonical.kind != SemanticSourceTargetKind::Class
            || declaration.type_parameter_count != 0
            || declaration.has_field_defaults
            || !declaration.method_partial_defaults.is_empty()
        {
            return false;
        }
        let Some(chain) = self.class_owner_chain(declaration) else {
            return false;
        };
        let parent = chain[1];
        if declaration.fields.len() < parent.fields.len()
            || !parent.fields.iter().enumerate().all(|(index, name)| {
                declaration.fields.get(index) == Some(name)
                    && declaration.field_identities.get(index) == parent.field_identities.get(index)
                    && declaration.field_types.get(index) == parent.field_types.get(index)
                    && declaration.field_public.get(index) == parent.field_public.get(index)
            })
        {
            return false;
        }
        let ancestors = &chain[1..];
        let slots: BTreeSet<_> = self
            .bodies
            .iter()
            .filter_map(|body| body.canonical.as_ref())
            .filter(|method| {
                method.kind == SemanticSourceTargetKind::Method
                    && ancestors.iter().any(|owner| declares_member(&owner.canonical, method))
            })
            .collect();
        if inheritance.methods.len() != slots.len() {
            return false;
        }
        let mut recorded = BTreeSet::new();
        inheritance.methods.iter().all(|record| {
            slots.contains(&record.method)
                && recorded.insert(&record.method)
                && self.nearest_class_body_identity(&chain, &record.method) == Some(&record.implementation)
                && !self.decorated_bindings.iter().any(|binding| {
                    binding.original_body.canonical.as_ref() == Some(&record.method)
                        || binding.original_body.canonical.as_ref() == Some(&record.implementation)
                })
        })
    }

    /// Resolve a local class's concrete-to-oldest owner chain, rejecting cycles, unretained parents and open layouts.
    fn class_owner_chain<'a>(&'a self, declaration: &'a NominalDeclaration) -> Option<Vec<&'a NominalDeclaration>> {
        let mut chain = vec![declaration];
        let mut seen = BTreeSet::from([&declaration.canonical]);
        let mut current = declaration;
        while let Some(inheritance) = &current.class_inheritance {
            let parent = self
                .nominal_declarations
                .iter()
                .find(|owner| owner.canonical == inheritance.parent)?;
            if !seen.insert(&parent.canonical)
                || parent.canonical.kind != SemanticSourceTargetKind::Class
                || parent.type_parameter_count != 0
                || parent.canonical.origin != declaration.canonical.origin
                || self.declaration_id_for_canonical(
                    &parent.canonical,
                    SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Class,
                ) != Some(parent.direct_declaration_id.clone())
            {
                return None;
            }
            chain.push(parent);
            current = parent;
        }
        Some(chain)
    }

    /// Check an inherited field at its exact parent-first position, preserving identity, type and visibility.
    fn inherited_field_matches(&self, declaration: &NominalDeclaration, index: usize) -> bool {
        let Some(chain) = self.class_owner_chain(declaration) else {
            return false;
        };
        declaration.class_inheritance.is_some()
            && chain[1..].iter().any(|parent| {
                parent.field_identities.get(index) == declaration.field_identities.get(index)
                    && parent.fields.get(index) == declaration.fields.get(index)
                    && parent.field_types.get(index) == declaration.field_types.get(index)
                    && parent.field_public.get(index) == declaration.field_public.get(index)
            })
    }

    /// Select the nearest retained original method body for one ancestor slot, without minting or rewriting identities.
    fn nearest_class_body_identity<'a>(
        &'a self,
        chain: &[&NominalDeclaration],
        slot: &CanonicalSymbolId,
    ) -> Option<&'a CanonicalSymbolId> {
        for owner in chain {
            for body in &self.bodies {
                let Some(method) = &body.canonical else {
                    continue;
                };
                if method.kind == SemanticSourceTargetKind::Method
                    && method.declaration_name == slot.declaration_name
                    && method.origin == owner.canonical.origin
                    && declares_member(&owner.canonical, method)
                {
                    return Some(method);
                }
            }
        }
        None
    }

    /// Validate reconstruction authority against the exact attached owner and its unique checked source Body.
    /// Missing selection never authorizes Deserialize. Checked hooks retain no duplicate signature or binder facts;
    /// their existing frame must carry one owned underlying parameter and the exact owner/ValidationError Result.
    ///
    /// Nongeneric project owners use their identity-checked carrier key, matching the hook frame after projection.
    fn has_well_formed_newtype_deserialize(&self, declaration: &NominalDeclaration) -> bool {
        use crate::body_ir::{CallableParamDefault, NewtypeDeserialize};
        let deserialize = declaration.derives.iter().any(|derive| derive == "serde::Deserialize");
        let hook = match &declaration.newtype_deserialize {
            NewtypeDeserialize::Unselected => return !deserialize,
            NewtypeDeserialize::Ordinary => return deserialize,
            NewtypeDeserialize::Checked(hook) if deserialize => hook,
            NewtypeDeserialize::Checked(_) => return false,
        };
        if hook.kind != SemanticSourceTargetKind::Method
            || hook.namespace != SymbolNamespace::Member
            || hook.origin != declaration.canonical.origin
            || !declares_member(&declaration.canonical, hook)
        {
            return false;
        }
        let Some(body) = self.body_for_canonical_target(hook) else {
            return false;
        };
        let [parameter] = body.params.as_slice() else {
            return false;
        };
        let [underlying] = declaration.field_types.as_slice() else {
            return false;
        };
        // This check is nonrecursive: body_has_canonical_direct_call_id validates the owner's layout in turn.
        if self.declaration_id_for_canonical(hook, SymbolNamespace::Member, SemanticSourceTargetKind::Method)
            != Some(body.direct_call_id.clone())
            || body.direct_call_id != CompilerNodeId::declaration_at(self.module_id.path(), body.span)
            || body.name != hook.declaration_name
            || body.is_async
            || body.is_generator()
            || body.type_parameters != declaration.type_parameters
            || parameter.mutable
            || !matches!(parameter.default, CallableParamDefault::Required)
            || &parameter.ty != underlying
        {
            return false;
        }
        let owner = if declaration.type_parameters.is_empty() {
            if underlying != &IncanType::Primitive(IncanPrimitiveType::Str) {
                return false;
            }
            IncanType::Named(declaration.carrier_name())
        } else {
            let [name] = declaration.type_parameters.as_slice() else {
                return false;
            };
            if underlying != &IncanType::TypeVar(name.clone()) {
                return false;
            }
            IncanType::Generic {
                base: declaration.template_carrier_name(),
                args: vec![underlying.clone()],
            }
        };
        body.return_type
            == IncanType::Generic {
                base: "Result".to_owned(),
                args: vec![
                    owner,
                    IncanType::NativeBuiltin(crate::RuntimeBuiltinType::ValidationError),
                ],
            }
    }

    /// Check retained storage consistency without inferring support or activation from a raw tuple layout.
    /// The producer's automatic selection attests source-only exclusions that physical layout cannot recover.
    /// Qualified carrier keys must encode the exact retained inner identity; a matching source name alone cannot
    /// substitute another owner.
    fn has_well_formed_newtype_string_conversion(declaration: &NominalDeclaration) -> bool {
        use crate::body_ir::NewtypeStringConversion;
        if declaration.newtype_string_conversion == NewtypeStringConversion::Unselected {
            return true;
        }
        declaration.type_parameters.is_empty()
            && declaration.field_default_body.is_none()
            && !matches!(
                declaration.newtype_deserialize,
                crate::body_ir::NewtypeDeserialize::Checked(_)
            )
            && match declaration.field_types.as_slice() {
                [IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Str)] => true,
                [IncanType::Named(name)] => declaration.named_type_identities.get(name).is_some_and(|identity| {
                    identity.kind == SemanticSourceTargetKind::Newtype
                        && (identity.declaration_name == *name
                            || crate::body_ir::nominal_carrier_key(identity).as_ref() == Some(name))
                }),
                _ => false,
            }
    }

    /// Whether each method-partial frame of a model or class is a uniquely named binding inside its owner whose frame
    /// is the target method's own surface: the checked target is a method of this owner, and the frame carries that
    /// method's identity, name, span and direct call id, with the owner's receiver first.
    fn are_well_formed_method_partial_defaults(&self, declaration: &NominalDeclaration) -> bool {
        let partials = &declaration.method_partial_defaults;
        partials
            .iter()
            .map(|binding| &binding.name)
            .collect::<BTreeSet<_>>()
            .len()
            == partials.len()
            && partials.iter().all(|binding| {
                !binding.name.is_empty()
                    && binding.span.start < binding.span.end
                    && binding.span.start >= declaration.canonical.declaration_span.start
                    && binding.span.end <= declaration.canonical.declaration_span.end
                    && binding.target.kind == SemanticSourceTargetKind::Method
                    && declares_member(&declaration.canonical, &binding.target)
                    && binding.frame.canonical.as_ref() == Some(&binding.target)
                    && binding.frame.name == binding.target.declaration_name
                    && self.declaration_id_for_canonical(
                        &binding.target,
                        SymbolNamespace::Member,
                        SemanticSourceTargetKind::Method,
                    ) == Some(binding.frame.direct_call_id.clone())
                    && binding
                        .frame
                        .params
                        .first()
                        .is_some_and(|receiver| receiver.ty == nominal_receiver_type(declaration))
            })
    }

    /// Whether a retained fieldless enum is bound to this module, owner and members alike.
    pub fn is_well_formed_fieldless_enum_declaration(&self, declaration: &FieldlessEnumDeclaration) -> bool {
        self.is_well_formed_enum_declaration(
            &declaration.direct_declaration_id,
            &declaration.canonical,
            &declaration.name,
            &declaration.variants,
            |variant| {
                (
                    &variant.direct_declaration_id,
                    &variant.canonical,
                    variant.name.as_str(),
                )
            },
        )
    }

    /// Whether a retained value enum is bound to this module, owner and members alike.
    pub fn is_well_formed_value_enum_declaration(&self, declaration: &ValueEnumDeclaration) -> bool {
        self.is_well_formed_enum_declaration(
            &declaration.direct_declaration_id,
            &declaration.canonical,
            &declaration.name,
            &declaration.variants,
            |variant| {
                (
                    &variant.direct_declaration_id,
                    &variant.canonical,
                    variant.name.as_str(),
                )
            },
        )
    }

    /// One retained enum owner and member registry, validated without recovering any identity from a spelling.
    fn is_well_formed_enum_declaration<T>(
        &self,
        direct_declaration_id: &CompilerNodeId,
        canonical: &CanonicalSymbolId,
        name: &str,
        variants: &[T],
        variant_facts: impl Fn(&T) -> (&CompilerNodeId, &CanonicalSymbolId, &str),
    ) -> bool {
        self.declaration_id_for_canonical(
            canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Enum,
        ) == Some(direct_declaration_id.clone())
            && canonical.declaration_name == name
            && variants
                .iter()
                .map(|variant| variant_facts(variant).0)
                .collect::<BTreeSet<_>>()
                .len()
                == variants.len()
            && variants
                .iter()
                .map(|variant| variant_facts(variant).2)
                .collect::<BTreeSet<_>>()
                .len()
                == variants.len()
            && variants.iter().all(|variant| {
                let (direct_variant_id, variant_canonical, variant_name) = variant_facts(variant);
                self.declaration_id_for_canonical(
                    variant_canonical,
                    SymbolNamespace::Member,
                    SemanticSourceTargetKind::Variant,
                ) == Some(direct_variant_id.clone())
                    && variant_canonical.origin == canonical.origin
                    && declares_member(canonical, variant_canonical)
                    && variant_canonical.declaration_name == variant_name
            })
    }
}

/// Return the declaration's open receiver type in its retained parameter order, never a concrete inferred use.
fn nominal_receiver_type(owner: &NominalDeclaration) -> crate::IncanType {
    if owner.type_parameters.is_empty() {
        crate::IncanType::Named(owner.carrier_name())
    } else {
        crate::IncanType::Generic {
            base: owner.template_carrier_name(),
            args: owner
                .type_parameters
                .iter()
                .cloned()
                .map(crate::IncanType::TypeVar)
                .collect(),
        }
    }
}

/// Whether a static initializer's literals match its storage carrier: a primitive, or a list, dictionary or set of
/// primitives whose hashed keys and members exclude floating point, as legacy hashed collections do. A computed
/// initializer is a closed body with no parameters that returns exactly the storage carrier.
fn static_initializer_matches_parts(ty: &IncanType, initial: &StaticInitializer) -> bool {
    match initial {
        StaticInitializer::Computed(body) => {
            body.params.is_empty()
                && body.param_locals.is_empty()
                && body.type_parameters.is_empty()
                && !body.is_async
                && body.return_type == *ty
        }
        StaticInitializer::Literal(value) => literal_static_carrier_matches(ty, value),
        StaticInitializer::List(values) => match collection_arguments(ty, CollectionTypeId::List) {
            Some([element]) => {
                primitive_static_carrier(element)
                    && values
                        .iter()
                        .all(|value| literal_static_carrier_matches(element, value))
            }
            _ => false,
        },
        StaticInitializer::Dict(entries) => match collection_arguments(ty, CollectionTypeId::Dict) {
            Some([key, value]) => {
                hashed_static_carrier(key)
                    && primitive_static_carrier(value)
                    && entries.iter().all(|(entry_key, entry_value)| {
                        literal_static_carrier_matches(key, entry_key)
                            && literal_static_carrier_matches(value, entry_value)
                    })
            }
            _ => false,
        },
        StaticInitializer::Set(members) => match collection_arguments(ty, CollectionTypeId::Set) {
            Some([element]) => {
                hashed_static_carrier(element)
                    && members
                        .iter()
                        .all(|member| literal_static_carrier_matches(element, member))
            }
            _ => false,
        },
    }
}

/// The type arguments of a checked builtin collection type of the expected kind.
fn collection_arguments(ty: &IncanType, expected: CollectionTypeId) -> Option<&[IncanType]> {
    match ty {
        IncanType::Generic { base, args } if collections::from_str(base) == Some(expected) => Some(args.as_slice()),
        _ => None,
    }
}

/// A primitive storage carrier a static literal may hold, even when an initializer has no element to validate.
fn primitive_static_carrier(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool | IncanPrimitiveType::Str
        )
    )
}

/// A primitive hash key or set member: floating point lacks the Eq and Hash legacy hashed collections require.
fn hashed_static_carrier(ty: &IncanType) -> bool {
    primitive_static_carrier(ty) && !matches!(ty, IncanType::Primitive(IncanPrimitiveType::Float))
}

/// Validate a literal payload against its exact primitive storage or collection element carrier.
fn literal_static_carrier_matches(ty: &IncanType, value: &Constant) -> bool {
    matches!(
        (ty, value),
        (IncanType::Primitive(IncanPrimitiveType::Int), Constant::Int(_))
            | (IncanType::Primitive(IncanPrimitiveType::Float), Constant::Float(_))
            | (IncanType::Primitive(IncanPrimitiveType::Bool), Constant::Bool(_))
            | (IncanType::Primitive(IncanPrimitiveType::Str), Constant::Str(_))
    )
}

impl crate::body_ir::PrimitiveTraitAdapter {
    /// Return the exact source carrier admitted for an adapter family. Checker retention and publication validation
    /// share this projection so a registered family cannot receive inconsistent payload-field authority.
    pub fn registered_receiver_type(
        family: incan_lang::lang::trait_capabilities::TraitCapabilityType,
    ) -> Option<IncanType> {
        use crate::IncanPrimitiveType;
        use incan_lang::lang::trait_capabilities::TraitCapabilityType;
        match family {
            TraitCapabilityType::Str => Some(IncanType::Primitive(IncanPrimitiveType::Str)),
            TraitCapabilityType::Int => Some(IncanType::Primitive(IncanPrimitiveType::Int)),
            TraitCapabilityType::Bool => Some(IncanType::Primitive(IncanPrimitiveType::Bool)),
            TraitCapabilityType::Numeric(numeric) => Some(IncanType::Primitive(IncanPrimitiveType::Numeric(numeric))),
            TraitCapabilityType::Decimal => Some(IncanType::Decimal {
                precision: 38,
                scale: 0,
            }),
            _ => None,
        }
    }

    /// Validate the registered source owner, its sole payload field, exact trait and owned decoder identity.
    /// Executable consumers must additionally validate the retained method bodies and decoder result signature.
    pub fn is_well_formed_for(&self, owner: &NominalDeclaration, adoption: &crate::body_ir::TraitAdoption) -> bool {
        let SymbolOrigin::Package { library, module_path } = &owner.canonical.origin else {
            return false;
        };
        let SymbolOrigin::Package {
            library: trait_library,
            module_path: trait_module,
        } = &adoption.trait_identity.origin
        else {
            return false;
        };
        let registered = incan_lang::lang::trait_capabilities::PRIMITIVE_SOURCE_ADAPTERS
            .iter()
            .any(|specification| {
                specification.matches_owner(library, module_path, &owner.canonical.declaration_name)
                    && specification.matches_trait(
                        trait_library,
                        trait_module,
                        &adoption.trait_identity.declaration_name,
                    )
                    && Self::registered_receiver_type(specification.receiver).as_ref() == Some(&self.receiver_type)
                    && self.payload_field.declaration_name == specification.payload_field
                    && self.decode_slot.declaration_name == specification.decode_slot
                    && self.decoded_value.declaration_name == specification.decoded_value
            });
        registered
            && owner.canonical.kind == SemanticSourceTargetKind::Model
            && owner.type_parameters.is_empty()
            && owner.field_types.as_slice() == [self.receiver_type.clone()]
            && owner.field_identities.as_slice() == [self.payload_field.clone()]
            && owner.field_public.as_slice() == [true]
            && adoption.arguments.is_empty()
            && adoption.methods.is_some()
            && adoption
                .methods
                .as_ref()
                .is_some_and(|methods| methods.contains(&self.decode_slot))
            && self.decode_slot.kind == SemanticSourceTargetKind::Method
            && self.decode_slot.namespace == SymbolNamespace::Member
            && self.decode_slot.origin == adoption.trait_identity.origin
            && declares_member(&adoption.trait_identity, &self.decode_slot)
            && self.decoded_value.kind == SemanticSourceTargetKind::Method
            && self.decoded_value.namespace == SymbolNamespace::Member
            && self.decoded_value.scope_discriminant.is_none()
            && self.decoded_value.origin == owner.canonical.origin
            && declares_member(&owner.canonical, &self.decoded_value)
    }
}

/// Whether `member` is declared inside `owner`: its declaration site lies within the owner's.
///
/// A canonical identity's origin names the module, not the declaration, so two declarations of one module with a
/// same-named member (`A.value` and `B.value`) give member identities that agree on everything else. The declaration
/// site is the one fact that tells them apart.
fn declares_member(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}

/// Validate the implicit immutable getter receiver independently of the member's owning nominal layout.
/// This rejects forged static, mutable, generic-method or ordinary-parameter property bodies before scheduling.
fn is_getter_frame(body: &Body, canonical: &CanonicalSymbolId) -> bool {
    let Some(receiver) = body.locals.first() else {
        return false;
    };
    let [parameter] = body.params.as_slice() else {
        return false;
    };
    let Some(identity) = receiver.identity.as_ref() else {
        return false;
    };
    !body.is_async
        && body.type_parameters.is_empty()
        && body.param_locals.as_slice() == [receiver.id]
        && matches!(receiver.origin, LocalOrigin::Receiver { mutable: false })
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.kind == SemanticSourceTargetKind::Receiver
        && identity.origin == canonical.origin
        && identity.declaration_name == "self"
        && identity.declaration_span == body.span
        && parameter.local == receiver.id
        && parameter.ty == receiver.ty
        && parameter.span == body.span
        && receiver.span == body.span
        && parameter.name == "self"
        && !parameter.mutable
        && matches!(parameter.default, CallableParamDefault::Required)
}

/// Whether a checked type names no open type: no type variable, `Self`, or uninferred placeholder at any depth.
///
/// A trait default instantiated for a concrete adopter must receive closed type arguments; an open one would leave
/// the default generic after specialization.
fn is_closed_type(ty: &IncanType) -> bool {
    is_owner_bound_type(ty, &[])
}

/// Validate one argument tree in a retained owner's declared parameter scope, without resolving names or inferring
/// missing arguments. An empty scope preserves the closed-type contract used outside nominal templates.
fn is_owner_bound_type(ty: &IncanType, parameters: &[String]) -> bool {
    match ty {
        IncanType::TypeVar(name) => parameters.iter().any(|parameter| parameter == name),
        IncanType::SelfType | IncanType::Infer | IncanType::Unknown | IncanType::Unsupported(_) => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
            args.iter().all(|argument| is_owner_bound_type(argument, parameters))
        }
        IncanType::Function { params, return_type } => {
            params.iter().all(|param| is_owner_bound_type(&param.ty, parameters))
                && is_owner_bound_type(return_type, parameters)
        }
        IncanType::TypeToken(inner)
        | IncanType::Ref(inner)
        | IncanType::RefMut(inner)
        | IncanType::NativeArray { element: inner, .. } => is_owner_bound_type(inner, parameters),
        IncanType::Never
        | IncanType::Primitive(_)
        | IncanType::Named(_)
        | IncanType::Decimal { .. }
        | IncanType::RustInteropPath(_)
        | IncanType::NativeTraitObject { .. }
        | IncanType::NativeBuiltin(_)
        | IncanType::NativeCChar => true,
        IncanType::NativePointer { pointee, .. } => is_owner_bound_type(pointee, parameters),
    }
}
