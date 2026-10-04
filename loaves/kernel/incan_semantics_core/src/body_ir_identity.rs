//! Identity well-formedness of a [`BodyIrModule`]: the checks a consumer runs before it trusts a module's span-derived
//! declaration identities, retained declaration records and direct call targets.
//!
//! Lowering derives every same-module identity from a declaration's span and carries the checker's canonical identity
//! beside it. Body IR travels as data (in a published executable representation, for instance), so a consumer cannot
//! take those facts on trust: a malformed module could copy a valid identity onto an unrelated body or carry a
//! coherent-looking foreign record. These checks never mint or complete an identity; they only confirm that the
//! retained ones agree with each other and belong to this module. They answer the same question for every consumer.

use std::collections::BTreeSet;

use crate::body_ir::{Body, BodyIrModule, FieldlessEnumDeclaration, NominalDeclaration, ValueEnumDeclaration};
use crate::{
    CanonicalSymbolId, CompilerNodeId, CompilerNodeKind, SemanticSourceTargetKind, SymbolNamespace,
    canonical_module_identity,
};

impl BodyIrModule {
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

    /// Whether a free function's body retains precisely the identity lowering derives for its own declaration: its
    /// span-derived direct-call id and a canonical function identity of this module that names the same declaration.
    pub fn body_has_canonical_direct_call_id(&self, body: &Body) -> bool {
        body.direct_call_id == CompilerNodeId::declaration_span(self.module_id.path(), body.span.start, body.span.end)
            && body.canonical.as_ref().is_some_and(|canonical| {
                canonical.declaration_name == body.name
                    && self.declaration_id_for_canonical(
                        canonical,
                        SymbolNamespace::OrdinaryLexical,
                        SemanticSourceTargetKind::Function,
                    ) == Some(body.direct_call_id.clone())
            })
    }

    /// Whether a retained plain-model layout agrees with its checked canonical identities.
    pub fn is_well_formed_nominal_declaration(&self, declaration: &NominalDeclaration) -> bool {
        self.declaration_id_for_canonical(
            &declaration.canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Model,
        ) == Some(declaration.direct_declaration_id.clone())
            && declaration.canonical.declaration_name == declaration.name
            && declaration.fields.len() == declaration.field_identities.len()
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
                    && variant_canonical.declaration_name == variant_name
            })
    }
}
