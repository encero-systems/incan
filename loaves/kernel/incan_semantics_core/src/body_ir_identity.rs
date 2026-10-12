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
    Body, BodyIrModule, Constant, FieldlessEnumDeclaration, LocalOrigin, NominalDeclaration, StaticInitializer,
    ValueEnumDeclaration,
};
use crate::{
    CanonicalSymbolId, CompilerNodeId, CompilerNodeKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, canonical_module_identity,
};

/// Match an initialized method's explicit passing reference against its receiver frame's nominal type and mode.
/// Ordinary inputs must agree exactly; this does not authorize another owner or a different receiver mode.
fn decorated_parameter_matches(
    body: &Body,
    left: &crate::IncanCallableParam,
    right: &crate::body_ir::CallableParam,
) -> bool {
    match body.locals.get(right.local.index()).map(|local| &local.origin) {
        Some(LocalOrigin::Receiver { mutable }) => match (&left.ty, mutable) {
            (IncanType::Ref(inner), false) | (IncanType::RefMut(inner), true) => **inner == right.ty && !left.is_mut,
            _ => false,
        },
        _ => left.ty == right.ty && left.is_mut == right.mutable,
    }
}

impl BodyIrModule {
    /// Prove a static belongs to this module and its initializer matches its exact retained carrier. A computed
    /// initializer's body must carry the static's own canonical identity and declaration id, so a module cannot attach
    /// another declaration's body to it.
    pub fn is_well_formed_static_declaration(&self, declaration: &crate::body_ir::StaticDeclaration) -> bool {
        if matches!(
            declaration.canonical.kind,
            SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Method
        ) {
            return self.decorated_bindings.iter().any(|binding| {
                binding.original_body.canonical.as_ref() == Some(&declaration.canonical)
                    && self.is_well_formed_decorated_binding(binding)
            });
        }
        let Some(declaration_id) = self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Static,
        ) else {
            return false;
        };
        if let StaticInitializer::Computed(body) = &declaration.initial
            && (body.canonical.as_ref() != Some(&declaration.canonical) || body.decl_id != declaration_id)
        {
            return false;
        }
        static_initializer_matches(&declaration.ty, &declaration.initial)
    }

    /// Prove an RFC 036 original and its replacement belong to the same physical source declaration and that the
    /// replacement initializer returns its exact checked callable carrier. Originals never replace ordinary lookup.
    pub fn is_well_formed_decorated_binding(&self, binding: &crate::body_ir::DecoratedBinding) -> bool {
        let original = &binding.original_body;
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
            && self.bodies.iter().any(|body| {
                body.canonical == original.canonical
                    && matches!(&replacement.ty, IncanType::Function { params, return_type }
                        if params.len() == body.params.len()
                            && params.iter().zip(&body.params).all(|(left, right)| decorated_parameter_matches(body, left, right))
                            && **return_type == body.return_type)
            })
            && static_initializer_matches(&replacement.ty, &replacement.initial)
    }

    /// Whether a normal enum layout belongs to this module and every variant identity belongs to its declared owner.
    pub fn is_well_formed_native_enum_declaration(&self, declaration: &crate::body_ir::EnumDeclaration) -> bool {
        if declaration.type_parameters.iter().any(String::is_empty)
            || declaration.type_parameters.iter().collect::<BTreeSet<_>>().len() != declaration.type_parameters.len()
        {
            return false;
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
                        == CompilerNodeId::declaration_span(
                            self.module_id.path(),
                            variant.canonical.declaration_span.start,
                            variant.canonical.declaration_span.end,
                        )
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
        .then(|| {
            CompilerNodeId::declaration_span(
                self.module_id.path(),
                identity.declaration_span.start,
                identity.declaration_span.end,
            )
        })
    }

    /// Whether a function, source Partial forwarding body, or retained nominal method or computed-property getter has
    /// its own canonical declaration identity and span-derived direct-call id. Methods and getters must belong to a
    /// retained nominal, normal enum or trait owner whose span contains them; an instance receiver must match that
    /// owner.
    pub fn body_has_canonical_direct_call_id(&self, body: &Body) -> bool {
        body.direct_call_id == CompilerNodeId::declaration_span(self.module_id.path(), body.span.start, body.span.end)
            && body.canonical.as_ref().is_some_and(|canonical| {
                let namespace = match canonical.kind {
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial => {
                        SymbolNamespace::OrdinaryLexical
                    }
                    SemanticSourceTargetKind::Method | SemanticSourceTargetKind::Property => SymbolNamespace::Member,
                    _ => return false,
                };
                canonical.declaration_name == body.name
                    && self.declaration_id_for_canonical(canonical, namespace, canonical.kind.clone())
                        == Some(body.direct_call_id.clone())
                    && (matches!(
                        canonical.kind,
                        SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial
                    ) || self.nominal_declarations.iter().any(|owner| {
                        self.is_well_formed_nominal_declaration(owner)
                            && canonical.origin == owner.canonical.origin
                            && declares_member(&owner.canonical, canonical)
                            && body.locals.first().is_none_or(|receiver| {
                                !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                    || receiver.ty == nominal_receiver_type(owner)
                            })
                    }) || self.enum_declarations.iter().any(|owner| {
                        self.is_well_formed_native_enum_declaration(owner)
                            && canonical.origin == owner.canonical.origin
                            && declares_member(&owner.canonical, canonical)
                            && body.locals.first().is_none_or(|receiver| {
                                !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                    || receiver.ty == crate::IncanType::Named(owner.name.clone())
                            })
                    }) || self.trait_declarations.iter().any(|owner| {
                        self.declaration_id_for_canonical(
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
    /// method alias. A default's type arguments must be closed types and its name must still match its slot.
    /// Matching method spellings never suffice to establish any of these relationships.
    pub fn is_well_formed_trait_implementation(&self, value: &crate::body_ir::TraitImplementation) -> bool {
        let owned = declares_member(&value.owner, &value.implementation);
        let inherited = value.method == value.implementation
            || self.trait_declarations.iter().any(|owner| {
                owner.origin == value.implementation.origin && declares_member(owner, &value.implementation)
            });
        let arguments_close = if owned {
            value.type_arguments.is_empty()
        } else {
            value
                .type_arguments
                .iter()
                .all(|binding| is_closed_type(&binding.argument))
        };
        value.method.namespace == SymbolNamespace::Member
            && (owned || value.method.declaration_name == value.implementation.declaration_name)
            && value.implementation.kind == SemanticSourceTargetKind::Method
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
                    && value.method.kind == SemanticSourceTargetKind::Method
            })
            && arguments_close
            && self.bodies.iter().any(|body| {
                body.canonical.as_ref() == Some(&value.implementation)
                    && self.body_has_canonical_direct_call_id(body)
                    && (owned || inherited)
            })
    }

    /// Whether a retained model, class, or newtype layout agrees with its checked canonical identities.
    pub fn is_well_formed_nominal_declaration(&self, declaration: &NominalDeclaration) -> bool {
        if declaration.display_method.as_ref().is_some_and(|method| {
            method.kind != SemanticSourceTargetKind::Method || method.namespace != SymbolNamespace::Member
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
                && declaration.method_partial_defaults.is_empty();
        }
        if !self.are_well_formed_method_partial_defaults(declaration) {
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
            && declaration.fields.iter().collect::<BTreeSet<_>>().len() == declaration.fields.len()
            && declaration.field_identities.iter().collect::<BTreeSet<_>>().len() == declaration.field_identities.len()
            && declaration
                .fields
                .iter()
                .zip(&declaration.field_identities)
                .all(|(name, identity)| {
                    identity.namespace == SymbolNamespace::Member
                        && identity.kind == SemanticSourceTargetKind::Field
                        && identity.scope_discriminant.is_none()
                        && identity.origin == declaration.canonical.origin
                        && declares_member(&declaration.canonical, identity)
                        && identity.declaration_name == *name
                })
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
        crate::IncanType::Named(owner.name.clone())
    } else {
        crate::IncanType::Generic {
            base: owner.name.clone(),
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
fn static_initializer_matches(ty: &IncanType, initial: &StaticInitializer) -> bool {
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

/// Whether `member` is declared inside `owner`: its declaration site lies within the owner's.
///
/// A canonical identity's origin names the module, not the declaration, so two declarations of one module with a
/// same-named member (`A.value` and `B.value`) give member identities that agree on everything else. The declaration
/// site is the one fact that tells them apart.
fn declares_member(owner: &CanonicalSymbolId, member: &CanonicalSymbolId) -> bool {
    owner.declaration_span.start <= member.declaration_span.start
        && member.declaration_span.end <= owner.declaration_span.end
}

/// Whether a checked type names no open type: no type variable, `Self`, or uninferred placeholder at any depth.
///
/// A trait default instantiated for a concrete adopter must receive closed type arguments; an open one would leave
/// the default generic after specialization.
fn is_closed_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::TypeVar(_) | IncanType::SelfType | IncanType::Infer | IncanType::Unknown => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(is_closed_type),
        IncanType::Function { params, return_type } => {
            params.iter().all(|param| is_closed_type(&param.ty)) && is_closed_type(return_type)
        }
        IncanType::TypeToken(inner) | IncanType::Ref(inner) | IncanType::RefMut(inner) => is_closed_type(inner),
        IncanType::Never
        | IncanType::Primitive(_)
        | IncanType::Named(_)
        | IncanType::Decimal { .. }
        | IncanType::RustInteropPath(_) => true,
    }
}
