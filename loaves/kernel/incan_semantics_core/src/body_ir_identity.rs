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

impl BodyIrModule {
    /// Prove a literal static belongs to this module and its initializer matches its exact retained carrier.
    pub fn is_well_formed_static_declaration(&self, declaration: &crate::body_ir::StaticDeclaration) -> bool {
        self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Static,
        )
        .is_some()
            && match &declaration.initial {
                StaticInitializer::Literal(value) => literal_static_carrier_matches(&declaration.ty, value),
                StaticInitializer::List(values) => {
                    let IncanType::Generic { base, args } = &declaration.ty else {
                        return false;
                    };
                    let [element] = args.as_slice() else {
                        return false;
                    };
                    collections::from_str(base) == Some(CollectionTypeId::List)
                        && matches!(
                            element,
                            IncanType::Primitive(
                                IncanPrimitiveType::Int
                                    | IncanPrimitiveType::Float
                                    | IncanPrimitiveType::Bool
                                    | IncanPrimitiveType::Str
                            )
                        )
                        && values
                            .iter()
                            .all(|value| literal_static_carrier_matches(element, value))
                }
            }
    }

    /// Whether a normal enum layout belongs to this module and every variant identity belongs to its declared owner.
    pub fn is_well_formed_native_enum_declaration(&self, declaration: &crate::body_ir::EnumDeclaration) -> bool {
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

    /// Whether a function or retained nominal method has its own canonical declaration identity and span-derived
    /// direct-call id. Methods must belong to a retained nominal or trait owner whose span contains them; an instance
    /// receiver must match that owner.
    pub fn body_has_canonical_direct_call_id(&self, body: &Body) -> bool {
        body.direct_call_id == CompilerNodeId::declaration_span(self.module_id.path(), body.span.start, body.span.end)
            && body.canonical.as_ref().is_some_and(|canonical| {
                let namespace = match canonical.kind {
                    SemanticSourceTargetKind::Function => SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Method => SymbolNamespace::Member,
                    _ => return false,
                };
                canonical.declaration_name == body.name
                    && self.declaration_id_for_canonical(canonical, namespace, canonical.kind.clone())
                        == Some(body.direct_call_id.clone())
                    && (canonical.kind == SemanticSourceTargetKind::Function
                        || self.nominal_declarations.iter().any(|owner| {
                            self.is_well_formed_nominal_declaration(owner)
                                && canonical.origin == owner.canonical.origin
                                && declares_member(&owner.canonical, canonical)
                                && body.locals.first().is_none_or(|receiver| {
                                    !matches!(receiver.origin, LocalOrigin::Receiver { .. })
                                        || receiver.ty == nominal_receiver_type(owner)
                                })
                        })
                        || self.trait_declarations.iter().any(|owner| {
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

    /// Validate a concrete trait slot against retained physical owners and implementation bodies.
    ///
    /// A default refers back to its own trait slot. An explicit implementation must belong to the concrete owner;
    /// matching method spellings never suffice to establish either relationship.
    pub fn is_well_formed_trait_implementation(&self, value: &crate::body_ir::TraitImplementation) -> bool {
        value.method.namespace == SymbolNamespace::Member
            && value.method.declaration_name == value.implementation.declaration_name
            && value.implementation.kind == SemanticSourceTargetKind::Method
            && self
                .nominal_declarations
                .iter()
                .any(|owner| owner.canonical == value.owner && self.is_well_formed_nominal_declaration(owner))
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
            && self.bodies.iter().any(|body| {
                body.canonical.as_ref() == Some(&value.implementation)
                    && self.body_has_canonical_direct_call_id(body)
                    && (value.method == value.implementation || declares_member(&value.owner, &value.implementation))
            })
    }

    /// Whether a retained model, class, or newtype layout agrees with its checked canonical identities.
    pub fn is_well_formed_nominal_declaration(&self, declaration: &NominalDeclaration) -> bool {
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
                && declaration.type_parameter_count == 0
                && !declaration.has_field_defaults;
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

/// Validate a literal payload against its exact primitive storage or list element carrier.
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
