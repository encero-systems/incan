//! Accessors and lowering inputs of checked body views and Body IR baseline inputs.

use super::*;

impl CheckedBodyView {
    /// Revision-local declaration span selecting this immutable local result, never a persistent semantic key.
    pub(crate) fn owner(&self) -> Span {
        self.local.owner
    }

    /// Settled local expression, ownership, destination and control-flow facts. Shared declaration inputs remain with
    /// their own producer and are not recreated or copied into this view.
    pub(crate) fn expressions(&self) -> &ExpressionArtifacts {
        &self.local.expressions
    }

    /// Local canonical references and write-target identities used by Body IR construction.
    pub(crate) fn references(&self) -> &ReferenceArtifacts {
        &self.local.references
    }

    /// Local checked calls, substitutions, argument carriers and constructor decisions.
    pub(crate) fn calls(&self) -> &CallArtifacts {
        &self.local.calls
    }

    /// Local checked iteration, indexing and operator protocol facts.
    pub(crate) fn protocols(&self) -> &ProtocolArtifacts {
        &self.local.protocols
    }

    /// Local Rust-call and coercion facts; shared Rust declaration inputs stay with their declaration views.
    pub(crate) fn rust(&self) -> &RustInteropArtifacts {
        &self.local.rust
    }

    /// Local trait-dispatch and result-carrier decisions, excluding shared trait declarations.
    pub(crate) fn traits(&self) -> &TraitArtifacts {
        &self.local.traits
    }

    /// Local verified C-boundary uses; declared native binding facts remain separate inputs.
    pub(crate) fn c_abi(&self) -> &CAbiInteropArtifacts {
        &self.local.c_abi
    }

    /// Read a checked sum-iterator item carrier owned by this body, without consulting another body or a module map.
    pub(crate) fn iterator_sum_item(&self, expression: Span) -> Option<&ResolvedType> {
        self.local.iterator_sum_items.get(&expression)
    }

    /// Read a nested declaration's canonical identity from this owner's checked lexical facts.
    pub(crate) fn nested_declaration_identity(
        &self,
        declaration: Span,
    ) -> Option<&incan_semantics_core::CanonicalSymbolId> {
        self.local.nested.get(&declaration)
    }

    /// Whether local checking or deferred settlement refused this owner. Successful checking of another owner does
    /// not turn a refused result into usable Body IR input. A false result does not establish whole-module admission;
    /// declaration and module refusals remain independent inputs. Local warning and lint messages do not refuse an
    /// owner.
    pub(crate) fn is_refused(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|value| !matches!(value.kind(), ErrorKind::Warning | ErrorKind::Lint))
            || !self.mutation.is_empty()
            || !self.dictionary.is_empty()
            || !self.open.is_empty()
    }
}

impl BodyIrBaselineInputs {
    /// Locate a default's exact checked writer by its expression key, not by a preset binding's signature span or
    /// an inferred source-range enclosure. Ambiguous ownership is unavailable, never an arbitrary chosen writer.
    pub(crate) fn default_owner(&self, expression: Span) -> Option<&CheckedBodyView> {
        let mut selected = self
            .declarations
            .values()
            .filter(|declaration| declaration.expressions().expr_types.contains_key(&expression));
        let owner = selected.next()?;
        if selected.next().is_some() {
            return None;
        }
        Some(owner)
    }

    /// Locate two settled owners by their revision-local source handle. Persistent source identity and snapshot
    /// authentication belong to the scoped record source, not to this compatibility baseline container.
    pub(crate) fn owners(&self, span: Span) -> Option<(&CheckedBodyView, &CheckedBodyView)> {
        let body = self.bodies.get(&span)?;
        let declaration = self.declarations.get(&span)?;
        Some((body, declaration))
    }
}

impl CheckedBodyView {
    /// Read the references a body builder needs from its two settled owners. Parameter writes belong to declaration
    /// preparation, while statement references belong to the body. Reuse the existing reference join in its original
    /// declaration-before-statement order; no checker, settlement, diagnostics or other body's facts are consulted.
    pub(crate) fn reference_inputs(&self, declaration: &Self) -> Result<ReferenceArtifacts, CheckedBodyInputError> {
        if self.local.kind == LocalOwnerKind::Declaration || declaration.local.kind != LocalOwnerKind::Declaration {
            return Err(CheckedBodyInputError::WrongOwnerRole);
        }
        if self.owner() != declaration.owner() {
            return Err(CheckedBodyInputError::DifferentOwner);
        }
        if self.is_refused() || declaration.is_refused() {
            return Err(CheckedBodyInputError::RefusedOwner);
        }
        let mut references = declaration.references().clone();
        append_references(&mut references, self.references().clone());
        Ok(references)
    }

    /// Assemble this lexical body's local inputs over caller-supplied earlier facets. The caller owns the selection
    /// of those facets; this operation never reads another body or reconstructs shared declaration facts. Declaration
    /// expressions precede statement facts, and open-value group indices are relocated exactly as in publication.
    pub(crate) fn append_lowering_inputs(
        &self,
        declaration: &Self,
        target: &mut TypeCheckInfo,
    ) -> Result<(), CheckedBodyInputError> {
        let references = self.reference_inputs(declaration)?;
        for owner in [declaration, self] {
            owner.append_native_inputs(target);
        }
        append_references(&mut target.references, references);
        Ok(())
    }

    /// Read a previously settled default/declaration projection over already selected earlier facets. A callee's
    /// statement body is never needed to retain its residual defaults or declaration-time lexical closures.
    pub(crate) fn append_declaration_inputs(&self, target: &mut TypeCheckInfo) -> Result<(), CheckedBodyInputError> {
        if self.local.kind != LocalOwnerKind::Declaration {
            return Err(CheckedBodyInputError::WrongOwnerRole);
        }
        if self.is_refused() {
            return Err(CheckedBodyInputError::RefusedOwner);
        }
        self.append_native_inputs(target);
        append_references(&mut target.references, self.references().clone());
        Ok(())
    }

    /// Copy one owner's native local families through the existing exhaustive joins. This is a read projection,
    /// not another checker, settlement, shared-input collector or witness implementation.
    pub(super) fn append_native_inputs(&self, target: &mut TypeCheckInfo) {
        append_local_expressions(&mut target.expressions, self.expressions().clone());
        append_calls(&mut target.calls, self.calls().clone());
        append_protocols(&mut target.protocols, self.protocols().clone());
        append_rust(&mut target.rust, self.rust().clone());
        append_traits(&mut target.traits, self.traits().clone());
        append_c_abi(&mut target.c_abi, self.c_abi().clone());
        target
            .declarations
            .nested_declaration_identities
            .extend(self.local.nested.clone());
        target
            .body_ir_iterator_sum_items
            .extend(self.local.iterator_sum_items.clone());
    }
}
