//! Position-free executable node and binding projections for the initial free-function record profile.
//!
//! Checked source signatures own declaration identity. The existing syntax inventory owns normalized roots and
//! structural paths; this adapter only binds those paths to the current revision. It never checks a body.

use std::collections::BTreeMap;

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};

use super::TypeChecker;
use super::diagnostic_values::OwnerSyntax;
use crate::ast::{Declaration, Program, Span};

/// A compiler-owned node identity, with no revision-local offsets or symbol-table indices.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SemanticNodeAnchor {
    /// The early checked source signature, including the enclosing declaration context when required.
    pub(crate) owner: StableDeclarationId,
    /// Normalized executable owner's syntax, retaining semantic ordering and expansion identity.
    pub(crate) root: String,
    /// Field, variant and element roles in the compiler's syntax inventory.
    pub(crate) path: String,
}

/// Why the initial canonical profile cannot represent or bind an input. These are capabilities, not diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeBindingError {
    /// A canonical field does not satisfy its selector, ordering, or native reference binding contract.
    #[error("invalid canonical field: {0}")]
    InvalidField(String),
    /// A declaration has no checked source signature or is not in this captured revision.
    #[error("missing checked source owner")]
    MissingOwner,
    /// Methods, generated ranges, joined ranges and opaque nested executable owners await their own profiles.
    #[error("unsupported canonical capability: {0}")]
    Unsupported(&'static str),
    /// A test-only native round trip differed from the complete independent cold oracle.
    #[error("native round-trip difference: {0}")]
    OracleDifference(String),
    /// The selected owner differs from the persisted owner, including an overload/signature change.
    #[error("canonical owner mismatch")]
    WrongOwner,
    /// A semantic body edit invalidated the persisted structural paths.
    #[error("canonical body root changed")]
    ChangedBody,
    /// A malformed or no-longer-present structural path must not select an approximate range.
    #[error("canonical node reference is missing")]
    MissingNode,
}

/// Activation-owned positions for exactly one free function; dropping this value releases every native coordinate.
pub(crate) struct SemanticNodeBindings {
    owner: StableDeclarationId,
    syntax: OwnerSyntax,
}

impl SemanticNodeBindings {
    /// Borrow the checker's early source signature and inventory one selected function, without reading other bodies.
    pub(crate) fn new(checker: &TypeChecker, program: &Program, owner: Span) -> Result<Self, NodeBindingError> {
        Self::with_profile(checker, program, owner, false)
    }

    /// Declaration/default anchors retain checked headers and default expressions, never a statement-body input.
    pub(crate) fn declaration(checker: &TypeChecker, program: &Program, owner: Span) -> Result<Self, NodeBindingError> {
        Self::with_profile(checker, program, owner, true)
    }

    /// Bind the selected profile through the same early checked signature and structural inventory authority.
    fn with_profile(
        checker: &TypeChecker,
        program: &Program,
        owner: Span,
        header_only: bool,
    ) -> Result<Self, NodeBindingError> {
        let declaration = program
            .declarations
            .iter()
            .find(|declaration| declaration.span == owner)
            .ok_or(NodeBindingError::MissingOwner)?;
        let Declaration::Function(function) = &declaration.node else {
            return Err(NodeBindingError::Unsupported("free-function owner"));
        };
        if !function.decorators.is_empty()
            || !function.surface_modifiers.is_empty()
            || !function.type_params.is_empty()
            || function.visibility != crate::ast::Visibility::Private
            || declaration.span.expansion.is_some()
            || !declaration.required_features.is_empty()
            || program.rust_module_path.is_some()
        {
            return Err(NodeBindingError::Unsupported(
                "modified, generic, public, or generated owner",
            ));
        }
        if function.body.first().is_some_and(|statement| matches!(&statement.node,
            crate::ast::Statement::Expr(expression) if matches!(&expression.node, crate::ast::Expr::Literal(crate::ast::Literal::String(_))))) {
            return Err(NodeBindingError::Unsupported("function docstring projection"));
        }
        let canonical = checker
            .declaration_view()
            .function(owner)
            .and_then(|binding| binding.identity.as_ref())
            .ok_or(NodeBindingError::MissingOwner)?;
        let owner = checker
            .declaration_view()
            .source_identity(canonical)
            .cloned()
            .ok_or(NodeBindingError::MissingOwner)?;
        let syntax = if header_only {
            OwnerSyntax::of_header(declaration)
        } else {
            OwnerSyntax::of(declaration)
        }
        .map_err(|_| NodeBindingError::Unsupported("syntax inventory"))?;
        Ok(Self { owner, syntax })
    }

    /// The checked source identity is available before local checking and is independent of body coordinates.
    pub(crate) fn owner(&self) -> &StableDeclarationId {
        &self.owner
    }

    /// The normalized syntax root is semantic data; all coordinates remain in the activation-owned inventory.
    pub(crate) fn root(&self) -> &str {
        &self.syntax.root
    }

    /// Project an exact syntax range. Unverifiable relative offsets and detached/foreign ranges are unsupported.
    pub(crate) fn anchor(&self, span: Span) -> Result<SemanticNodeAnchor, NodeBindingError> {
        let path = self
            .syntax
            .by_span
            .get(&span)
            .ok_or(NodeBindingError::Unsupported("non-node source range"))?;
        Ok(SemanticNodeAnchor {
            owner: self.owner.clone(),
            root: self.syntax.root.clone(),
            path: path.clone(),
        })
    }

    /// Rebind only after both the checked owner and normalized body match; never use persisted source coordinates.
    pub(crate) fn resolve(&self, anchor: &SemanticNodeAnchor) -> Result<Span, NodeBindingError> {
        if anchor.owner != self.owner {
            return Err(NodeBindingError::WrongOwner);
        }
        if anchor.root != self.syntax.root {
            return Err(NodeBindingError::ChangedBody);
        }
        self.syntax
            .by_path
            .get(&anchor.path)
            .copied()
            .ok_or(NodeBindingError::MissingNode)
    }

    /// Give nested references compiler-owned owner context and structural collision ordinals. The caller supplies
    /// only this owner's declarations, not a whole-module scope table; every declaration must have an exact node.
    pub(crate) fn binding_identities<'a>(
        &self,
        declarations: impl IntoIterator<Item = &'a CanonicalSymbolId>,
    ) -> Result<BTreeMap<CanonicalSymbolId, StableDeclarationId>, NodeBindingError> {
        let mut groups = BTreeMap::new();
        for canonical in declarations {
            if !matches!(
                canonical.kind,
                incan_semantics_core::SemanticSourceTargetKind::Local
                    | incan_semantics_core::SemanticSourceTargetKind::Parameter
            ) {
                return Err(NodeBindingError::Unsupported("nested declaration kind"));
            }
            if canonical.origin != self.owner.origin {
                return Err(NodeBindingError::Unsupported("foreign lexical binding"));
            }
            let span = Span {
                start: canonical.declaration_span.start,
                end: canonical.declaration_span.end,
                expansion: canonical.declaration_span.expansion,
            };
            let anchor = self.anchor(span)?;
            groups
                .entry((
                    canonical.namespace,
                    canonical.kind.clone(),
                    canonical.declaration_name.clone(),
                ))
                .or_insert_with(BTreeMap::new)
                .insert(anchor.path, canonical);
        }
        let mut identities = BTreeMap::new();
        for declarations in groups.values() {
            for (ordinal, canonical) in declarations.values().enumerate() {
                let binding_ordinal =
                    u32::try_from(ordinal).map_err(|_| NodeBindingError::Unsupported("binding ordinal"))?;
                let context = StableDeclarationContext {
                    owner: self.owner.clone(),
                    binding_ordinal,
                };
                let identity = StableDeclarationId::from_canonical(canonical, None, Some(context))
                    .ok_or(NodeBindingError::MissingOwner)?;
                identities.insert((*canonical).clone(), identity);
            }
        }
        Ok(identities)
    }
}
