//! Selected owner projections for the Incan checker-record codec. This is a bootstrap conversion, not a store.

mod artifact_admission;
mod call_artifacts_generated;
mod diagnostics;
mod function_calls;
mod imported_members;
mod input_flow;
mod inputs;
mod local_restore;
mod mutation;
mod native_types;
mod ownership;
mod protocol_artifacts_generated;
mod rust_artifact_admission;
mod scope_layout;
pub(in crate::typechecker) use scope_layout::CallableScopeLayout;
mod rust_artifacts_generated;
mod types;
use diagnostics::{project_diagnostics, read_diagnostic};
use local_restore::restore_local;
mod unsupported;

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::canonical_nodes::{NodeBindingError, SemanticNodeAnchor, SemanticNodeBindings, callable_params};
use super::super::diagnostic_values::DiagnosticPositions;
use super::{BodyCheckInput, BodyLocal, CheckedBodyView, LocalOwnerKind, TypeChecker};
use crate::ast::{Program, Span};

use incan_semantics_core::stable_identity::StableDeclarationContext;
use incan_semantics_core::stable_identity::{StableDeclarationId, StableDeclarationLocation};
use incan_semantics_core::{CanonicalSymbolId, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace};
use types::{next, parameter_fields, read_parameters, read_type, type_fields};

mod fixtures;
mod projections;

pub(crate) use fixtures::*;
pub(crate) use projections::*;

/// The native boundary carries only field primitives. The Incan schema owns their versioned encoding.
pub(crate) type Fields = Vec<Vec<String>>;

/// Emit one pending record from the direct body-check boundary. The native adapter carries only owner/root/profile
/// metadata and canonical field primitives; the Incan schema assigns and validates the closed family tag.
fn produce_local(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    local: &BodyLocal,
) -> Result<Fields, NodeBindingError> {
    let positions = Positions::for_local(checker, program, owner, local)?;
    if positions.scopes.is_none() {
        return Err(NodeBindingError::Unsupported("callable scope allocation"));
    }
    let mut fields = local_fields(&positions, local)?;
    for (kind, slots, writer) in [
        ("error", &local.diagnostic_slots, &checker.errors),
        ("warning", &local.warning_slots, &checker.warnings),
    ] {
        for (index, slot) in slots.iter().enumerate() {
            let value = writer.get(*slot).ok_or(NodeBindingError::MissingNode)?;
            fields.push(diagnostics::diagnostic_row(&positions, value, kind, index)?);
        }
    }
    fields.push(vec!["status".into(), String::new(), "pending".into(), "true".into()]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            crate::checker_records::GRAPH_BODY_RECORD_PROFILE.into(),
        ],
    );
    Ok(fields)
}

impl TypeChecker {
    /// Return how many physical callable owners entered the body-check scope in the current test activation.
    #[cfg(test)]
    pub(crate) fn body_check_trace_count(&self) -> usize {
        self.body_check_traces.len()
    }

    /// Complete one owner trace with the checker-record outcome decided after all producer boundaries settle.
    pub(crate) fn record_body_check_outcome(&self, owner: Span, outcome: &'static str) {
        if let Some(span) = self.body_check_traces.get(&owner) {
            span.record("outcome", outcome);
        }
    }

    /// Report whether this activation restored the selected local body instead of checking statements afresh.
    pub fn restored_body_record(&self, owner: Span) -> bool {
        self.selected_local_records.contains_key(&owner)
    }

    /// Publish the local record emitted by one just-completed callable producer. The index is returned by that exact
    /// invocation, so this boundary never searches the settled body inventory or observes a sibling owner.
    pub(in crate::typechecker) fn produce_local_record(&mut self, program: &Program, owner: Span, index: usize) {
        let produced = {
            let Some(BodyCheckInput::Local(local)) = self.body_checks.get(index) else {
                tracing::debug!(target: "incan::invocation", ?owner, index, "local record output is missing");
                return;
            };
            produce_local(self, program, owner, local)
        };
        match produced {
            Ok(fields) => {
                self.produced_local_records.insert(owner, fields);
                self.check_work.local_record_evaluations += 1;
            }
            Err(error) => {
                tracing::debug!(target: "incan::invocation", ?owner, ?error, "local record production refused");
                self.local_record_refusals.insert(owner, error);
            }
        }
    }

    /// Borrow the declaration record emitted by this activation's declaration producer.
    pub(crate) fn produced_declaration_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_declaration_records.get(&owner)
    }

    /// Borrow either the declaration evaluated in this activation or an independently validated selected predecessor.
    pub(crate) fn available_declaration_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_declaration_records
            .get(&owner)
            .or_else(|| self.selected_declaration_records.get(&owner))
    }

    /// Clone the explicit refusal returned by this activation's direct declaration producer.
    pub(crate) fn declaration_record_refusal(&self, owner: Span) -> Option<NodeBindingError> {
        self.declaration_record_refusals.get(&owner).cloned()
    }

    /// Borrow the local record emitted directly by this activation's body-check producer.
    pub(crate) fn produced_local_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_local_records.get(&owner)
    }

    /// Borrow either the value evaluated in this activation or an independently validated selected predecessor.
    pub(crate) fn available_local_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_local_records
            .get(&owner)
            .or_else(|| self.selected_local_records.get(&owner))
    }

    /// Clone the explicit refusal returned by this activation's direct local producer.
    pub(crate) fn local_record_refusal(&self, owner: Span) -> Option<NodeBindingError> {
        self.local_record_refusals.get(&owner).cloned()
    }

    /// Name every nonempty artifact field that prevents this checked owner from crossing the reusable-record boundary.
    pub(crate) fn body_artifact_refusals(&self, owner: Span) -> Vec<&'static str> {
        let mut refusals = Vec::new();
        if let Some(view) = self.checked_declaration(owner) {
            refusals.extend(unsupported::artifact_refusals(view));
        }
        if let Some(view) = self.checked_body(owner) {
            refusals.extend(unsupported::artifact_refusals(view));
        }
        refusals.sort_unstable();
        refusals.dedup();
        refusals
    }

    /// Retain one selected local value for republishing without treating selection as producer execution.
    pub(crate) fn retain_local_record(&mut self, owner: Span, fields: Fields) {
        self.selected_local_records.insert(owner, fields);
    }

    /// Borrow the final record emitted directly by this activation's settlement producer.
    pub(crate) fn produced_final_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_final_records.get(&owner)
    }

    /// Borrow either the final value evaluated in this activation or a producer-validated settled predecessor.
    pub(crate) fn available_final_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_final_records
            .get(&owner)
            .or_else(|| self.selected_final_records.get(&owner))
    }

    /// Stage one scalar product's settled value until its body passes the current physical-scope check.
    pub(crate) fn retain_callable_final_record(&mut self, owner: Span, fields: Fields) {
        self.callable_final_record_reuse.insert(owner, fields);
    }

    /// Publish the staged final value only after the matching scalar body is accepted for this activation.
    pub(in crate::typechecker) fn select_callable_final_record(&mut self, owner: Span) {
        if let Some(fields) = self.callable_final_record_reuse.remove(&owner) {
            self.selected_final_records.insert(owner, fields);
        }
    }

    /// Clone the explicit refusal returned by this activation's direct final producer.
    pub(crate) fn final_record_refusal(&self, owner: Span) -> Option<NodeBindingError> {
        self.final_record_refusals.get(&owner).cloned()
    }

    /// Retain one producer-validated declaration record until its source-order owner consumes it.
    pub(crate) fn retain_declaration_record(&mut self, owner: Span, fields: Fields) {
        self.callable_declaration_reuse.insert(owner, fields);
    }

    /// Report whether the current activation selected a declaration value for this exact source owner.
    pub(crate) fn has_declaration_record(&self, owner: Span) -> bool {
        self.callable_declaration_reuse.contains_key(&owner)
    }

    /// Fingerprint the current callable coordinates and checked native bindings before publishing its executable frame.
    pub fn graph_body_physical_inputs(&self, program: &Program, owner: Span) -> Option<[u8; 32]> {
        let local = &self.checked_body(owner)?.local;
        Positions::for_local(self, program, owner, local)
            .ok()?
            .physical_inputs(local)
    }

    /// Check the exact current owner before a ranked activation enters this function's statements.
    pub(in crate::typechecker) fn permits_ranked_body(&self, program: Option<&Program>, owner: Span) -> bool {
        if self.graph_body_records.ranked_owner().is_none() {
            return true;
        }
        let identity = program
            .and_then(|program| SemanticNodeBindings::new(self, program, owner).ok())
            .map(|nodes| nodes.owner().render_compact());
        self.graph_body_records.enter_ranked_owner(identity.as_deref())
    }

    /// Restore one graph-selected final value against current compiler identities and syntax, then enqueue it for the
    /// ordinary checked-body settlement join. A refusal leaves the caller on fresh body checking and publishes no
    /// selected value.
    pub(in crate::typechecker) fn reuse_graph_body_record(
        &mut self,
        program: &Program,
        owner: Span,
    ) -> Result<bool, NodeBindingError> {
        let nodes = SemanticNodeBindings::new(self, program, owner)?;
        let identity = nodes.owner().render_compact();
        let Some(selected) = self.graph_body_records.get(&identity)? else {
            return Ok(false);
        };
        // The checker owns selected rows after this boundary so its publication view cannot outlive the invocation
        // transport. This is the one deliberate copy from immutable transport into checker-owned selected output.
        let local_fields = selected.local.clone();
        let final_fields = selected.final_value.clone();
        let positions = Positions::for_decoded_local(self, program, owner, nodes, &local_fields, true)?;
        if !final_fields
            .iter()
            .any(|row| row.first().is_some_and(|tag| tag == "scope"))
        {
            return Err(NodeBindingError::InvalidField(
                "final callable scope allocation is missing".into(),
            ));
        }
        let restored = restore(&positions, program, owner, &final_fields)?;
        if restored.binding.is_some() || !restored.defaults.is_empty() {
            return Err(NodeBindingError::InvalidField(
                "body record carried declaration output".into(),
            ));
        }
        let canonical = self
            .declaration_view()
            .callable(owner)
            .and_then(|binding| binding.identity.as_ref())
            .cloned()
            .ok_or(NodeBindingError::MissingOwner)?;
        let frame = selected.body_ir.as_ref().and_then(|frame| {
            if frame.identity != identity
                || frame.body.canonical.as_ref() != Some(&canonical)
                || frame.body.span != owner
                || frame.body.name != canonical.declaration_name
            {
                return None;
            }
            Some((owner, frame.physical_inputs?, frame.body.clone()))
        });
        let mut diagnostic_positions = DiagnosticPositions::new(program, self.symbols.declaration_origin());
        self.reuse_checked_body(Arc::new(restored.view), &mut diagnostic_positions)
            .map_err(|_| NodeBindingError::ChangedBody)?;
        match restored.ownership {
            Some(contract) => {
                self.type_info
                    .declarations
                    .ownership_contracts
                    .insert(canonical.clone(), contract);
            }
            None => {
                self.type_info.declarations.ownership_contracts.remove(&canonical);
            }
        }
        self.type_info
            .expressions
            .body_ir_expr_types
            .extend(restored.final_types.expressions);
        self.type_info
            .expressions
            .body_ir_binding_types
            .extend(restored.final_types.bindings);
        self.selected_local_records.insert(owner, local_fields);
        self.selected_final_records.insert(owner, final_fields);
        if let Some(layout) = positions.scopes {
            layout.install(&mut self.symbols);
            self.callable_scope_layouts.insert(owner, layout);
        }
        if let Some(frame) = frame {
            self.pending_graph_frames.push(frame);
        }
        Ok(true)
    }

    /// Compare in the same completed module phase that produced the frame. Earlier callables have not seen the
    /// declaration and scope identities of their later siblings during statement admission.
    pub(in crate::typechecker) fn finish_graph_body_frames(&mut self, program: &Program) {
        for (owner, expected, body) in std::mem::take(&mut self.pending_graph_frames) {
            if self.graph_body_physical_inputs(program, owner) == Some(expected) {
                self.type_info.reused_callable_bodies.insert(owner, body);
            }
        }
    }

    /// Restore one selected declaration value through current coordinates, publish its own final facts and enqueue
    /// it for ordinary settlement. This adapter performs no declaration validation or default checking.
    pub(super) fn reuse_declaration_record(
        &mut self,
        program: &Program,
        owner: Span,
    ) -> Result<bool, NodeBindingError> {
        let Some(fields) = self.callable_declaration_reuse.remove(&owner) else {
            return Ok(false);
        };
        let positions = Positions::new(self, program, owner, true)?;
        let restored = restore(&positions, program, owner, &fields)?;
        let binding = restored.binding.ok_or(NodeBindingError::MissingOwner)?;
        if self.declaration_view().callable(owner) != Some(&binding)
            || restored.defaults != default_writers(program, owner)?
        {
            return Err(NodeBindingError::BaselineDifference("declaration inputs".into()));
        }
        let identity = binding.identity.as_ref().ok_or(NodeBindingError::MissingOwner)?;
        let mut diagnostic_positions = DiagnosticPositions::new(program, self.symbols.declaration_origin());
        self.reuse_checked_declaration(Arc::new(restored.view), &mut diagnostic_positions)
            .map_err(|_| NodeBindingError::ChangedBody)?;
        match restored.ownership {
            Some(contract) => {
                self.type_info
                    .declarations
                    .ownership_contracts
                    .insert(identity.clone(), contract);
            }
            None => {
                self.type_info.declarations.ownership_contracts.remove(identity);
            }
        }
        self.type_info
            .expressions
            .body_ir_expr_types
            .extend(restored.final_types.expressions);
        self.type_info
            .expressions
            .body_ir_binding_types
            .extend(restored.final_types.bindings);
        self.selected_declaration_records.insert(owner, fields);
        Ok(true)
    }
}

/// Current coordinates and canonical activation handles, keyed by persisted semantic identity. No type, call or
/// diagnostic payload is retained here. The test sink may build these from a separate fresh checker baseline.
struct Positions {
    scopes: Option<CallableScopeLayout>,
    nodes: SemanticNodeBindings,
    owner: Span,
    identities: BTreeMap<String, CanonicalSymbolId>,
    caller: Option<super::super::type_info::CBindingRawCallOwner>,
    type_identities: BTreeMap<String, CanonicalSymbolId>,
}

impl Positions {
    /// Select exactly one owner's syntax and current declaration/binding handles; never take semantic body tables.
    fn new(checker: &TypeChecker, program: &Program, owner: Span, declaration: bool) -> Result<Self, NodeBindingError> {
        let nodes = if declaration {
            SemanticNodeBindings::declaration(checker, program, owner)?
        } else {
            SemanticNodeBindings::new(checker, program, owner)?
        };
        Self::from_nodes(checker, program, owner, declaration, nodes)
    }

    /// Bind the compiler's current declaration and type identities only after an owner page is selected.
    fn from_nodes(
        checker: &TypeChecker,
        program: &Program,
        owner: Span,
        declaration: bool,
        nodes: SemanticNodeBindings,
    ) -> Result<Self, NodeBindingError> {
        let mut identities = checker
            .type_info
            .declarations
            .source_identities
            .iter()
            .map(|(canonical, stable)| (stable.render_compact(), canonical.clone()))
            .collect::<BTreeMap<_, _>>();
        if let Some(binding) = checker.declaration_view().method(owner)
            && let Some(canonical) = &binding.identity
        {
            identities.insert(nodes.owner().render_compact(), canonical.clone());
        }
        // Imported package and sibling-module bindings are authenticated by their declaration records. Retain those
        // exact canonical identities alongside builtins so a body record can name the dependency declaration it read.
        for index in 0..checker.symbols.all_symbols().len() {
            let Some(canonical) = checker.symbols.identity_of(index) else {
                continue;
            };
            if !matches!(
                canonical.origin,
                incan_semantics_core::SymbolOrigin::Builtin
                    | incan_semantics_core::SymbolOrigin::Module(_)
                    | incan_semantics_core::SymbolOrigin::Package { .. }
            ) {
                continue;
            }
            if let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) {
                identities.insert(stable.render_compact(), canonical.clone());
            }
        }
        // Restore imported member references against the current physical declaration in the checked nominal table.
        identities.extend(imported_members::identities(checker));
        let type_identities = admitted_type_identities(checker);
        let nested = checker
            .checked_body(owner)
            .into_iter()
            .chain(checker.checked_declaration(owner))
            .filter(|view| !declaration || view.local.kind == LocalOwnerKind::Declaration)
            .flat_map(|view| view.local.nested.values());
        let raw = checker
            .body_checks
            .iter()
            .map(|input| input.local())
            .filter(|local| local.owner == owner && (!declaration || local.kind == LocalOwnerKind::Declaration))
            .flat_map(|local| local.nested.values());
        // Declaration records are restored before the function scope is entered. Mint the current activation's
        // parameter handles from the symbol table's next scope index so stored nested identities bind without
        // running annotation/default checking or creating the scope early.
        let declaration_bindings = callable_params(program, owner)?
            .iter()
            .map(|parameter| {
                (0..checker.symbols.all_symbols().len())
                    .filter_map(|index| checker.symbols.identity_of(index))
                    .find(|canonical| {
                        canonical.kind == SemanticSourceTargetKind::Parameter
                            && canonical.origin == checker.symbols.declaration_origin()
                            && canonical.declaration_span == parameter.span
                    })
                    .cloned()
                    .unwrap_or_else(|| CanonicalSymbolId {
                        namespace: SymbolNamespace::OrdinaryLexical,
                        origin: checker.symbols.declaration_origin(),
                        declaration_name: parameter.node.name.clone(),
                        kind: SemanticSourceTargetKind::Parameter,
                        scope_discriminant: Some(ScopeDiscriminant(checker.symbols.next_scope_index())),
                        declaration_span: parameter.span,
                    })
            })
            .collect::<Vec<_>>();
        // Real checked locals, when present, replace the pre-scope parameter handles under the same stable key.
        for (canonical, stable) in nodes.binding_identities(declaration_bindings.iter().chain(nested).chain(raw))? {
            identities.insert(stable.render_compact(), canonical);
        }
        let caller = program.declarations.iter().find_map(|declaration| {
            if declaration.span != owner {
                return None;
            }
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return None;
            };
            Some(super::super::type_info::CBindingRawCallOwner {
                name: function.name.clone(),
                visibility: function.visibility,
                declaration_span: owner,
            })
        });
        Ok(Self {
            scopes: if declaration {
                None
            } else {
                checker.callable_scope_layouts.get(&owner).cloned()
            },
            nodes,
            owner,
            identities,
            caller,
            type_identities,
        })
    }

    /// Bind one fresh local output's lexical identities without publishing it into the checker's pending queue.
    fn for_local(
        checker: &TypeChecker,
        program: &Program,
        owner: Span,
        local: &BodyLocal,
    ) -> Result<Self, NodeBindingError> {
        let mut positions = Self::new(checker, program, owner, false)?;
        for (canonical, stable) in positions.nodes.binding_identities(local.nested.values())? {
            positions.identities.insert(stable.render_compact(), canonical);
        }
        for canonical in local
            .expressions
            .expression_type_identities
            .values()
            .flat_map(|identities| identities.values())
        {
            if let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) {
                positions
                    .type_identities
                    .insert(stable.render_compact(), canonical.clone());
            }
        }
        Ok(positions)
    }

    /// Bind opaque nested identities from one authenticated local value to this revision's structural declaration
    /// nodes. Module declarations and parameters retain their ordinary collected identities; only body-local
    /// declarations receive activation-private handles, which are mapped back to their persisted stable identities.
    fn for_decoded_local(
        checker: &TypeChecker,
        program: &Program,
        owner: Span,
        nodes: SemanticNodeBindings,
        fields: &Fields,
        active: bool,
    ) -> Result<Self, NodeBindingError> {
        let mut positions = Self::from_nodes(checker, program, owner, false, nodes)?;
        imported_members::bind_transitive(checker, &mut positions, fields)?;
        let layout = CallableScopeLayout::decode(&checker.symbols, fields, active)?
            .ok_or_else(|| NodeBindingError::InvalidField("callable scope allocation is missing".into()))?;
        let mut nested = Vec::new();
        for row in fields {
            if row.len() < 3 || row[0] != "binding" || row[2] != "declaration" {
                continue;
            }
            let span = positions.span(&row[1])?;
            let (kind, name) = persisted_nested_binding(positions.nodes.owner(), &row[3])?;
            let canonical = CanonicalSymbolId {
                namespace: SymbolNamespace::OrdinaryLexical,
                origin: checker.symbols.declaration_origin(),
                declaration_name: name,
                kind,
                scope_discriminant: Some(ScopeDiscriminant(
                    layout.binding_scope(
                        row.get(4)
                            .ok_or_else(|| NodeBindingError::InvalidField("binding has no scope ordinal".into()))?,
                    )?,
                )),
                declaration_span: span,
            };
            nested.push((&row[3], canonical));
        }
        positions.scopes = Some(layout);
        // The same structural grouping as the writer authenticates each kind, owner and collision ordinal.
        let bound = positions
            .nodes
            .binding_identities(nested.iter().map(|(_, symbol)| symbol))?;
        for (persisted, canonical) in nested {
            if bound
                .get(&canonical)
                .is_none_or(|stable| stable.render_compact() != *persisted)
            {
                return Err(NodeBindingError::InvalidField("nested declaration identity".into()));
            }
            if positions.identities.insert(persisted.clone(), canonical).is_some() {
                return Err(NodeBindingError::InvalidField(
                    "duplicate nested declaration identity".into(),
                ));
            }
        }
        Ok(positions)
    }

    /// A position-free checker result can remain reusable after its native executable references have moved.
    /// Bind the native frame to the current owner's entire syntax geometry, checked type providers, and the exact
    /// reference, nested, and write handles settled for that owner. Hash ordered typed bytes, including scope IDs.
    fn physical_inputs(&self, local: &BodyLocal) -> Option<[u8; 32]> {
        use sha2::{Digest, Sha256};
        let references = local.references.resolved_identities.iter().collect::<BTreeMap<_, _>>();
        let writes = local
            .references
            .resolved_write_identities
            .iter()
            .collect::<BTreeMap<_, _>>();
        let types = local
            .expressions
            .expression_type_identities
            .iter()
            .collect::<BTreeMap<_, _>>();
        let contexts = local
            .references
            .stable_declaration_contexts
            .iter()
            .map(|(span, context)| (span, (&context.owner, context.binding_ordinal)))
            .collect::<BTreeMap<_, _>>();
        let bytes = postcard::to_allocvec(&(
            self.nodes.root(),
            self.nodes.physical_nodes(),
            &self.identities,
            &self.type_identities,
            &local.nested,
            references,
            writes,
            types,
            contexts,
        ))
        .ok()?;
        Some(Sha256::digest(bytes).into())
    }

    /// Only a checked declaration or an explicitly admitted builtin can become a persistent reference.
    fn identity(&self, canonical: &CanonicalSymbolId) -> Result<String, NodeBindingError> {
        if let Some((stable, _)) = self.identities.iter().find(|(_, current)| *current == canonical) {
            return Ok(stable.clone());
        }
        if canonical.origin == incan_semantics_core::SymbolOrigin::Builtin {
            return StableDeclarationId::from_canonical(canonical, None, None)
                .map(|stable| stable.render_compact())
                .ok_or(NodeBindingError::MissingOwner);
        }
        Err(NodeBindingError::Unsupported("unadmitted declaration reference"))
    }

    /// Resolve a checked expression-type leaf through an admitted source/package identity or one imported Rust type.
    fn type_identity(&self, canonical: &CanonicalSymbolId) -> Result<String, NodeBindingError> {
        if let Some((stable, _)) = self
            .identities
            .iter()
            .chain(&self.type_identities)
            .find(|(_, current)| *current == canonical)
        {
            return Ok(stable.clone());
        }
        Err(NodeBindingError::Unsupported("unadmitted expression type identity"))
    }

    /// Project one checked member through its exact declaring nominal instead of guessing from the member spelling.
    fn contextual_identity(
        &self,
        canonical: &CanonicalSymbolId,
        context: &incan_semantics_core::CanonicalStableDeclarationContext,
    ) -> Result<String, NodeBindingError> {
        stable_member_identity(canonical, &context.owner, context.binding_ordinal)
    }

    /// Current scope indices and spans are supplied by this activation's binding map, never parsed from a key.
    fn canonical(&self, identity: &str) -> Result<CanonicalSymbolId, NodeBindingError> {
        self.identities
            .get(identity)
            .cloned()
            .ok_or(NodeBindingError::MissingNode)
    }

    /// Rebind one record-carried type identity without admitting it as a callable or value reference.
    fn canonical_type(&self, identity: &str) -> Result<CanonicalSymbolId, NodeBindingError> {
        self.identities
            .get(identity)
            .or_else(|| self.type_identities.get(identity))
            .cloned()
            .ok_or(NodeBindingError::MissingNode)
    }

    /// Bind a structural path only under the already authenticated selected owner/root.
    fn span(&self, path: &str) -> Result<Span, NodeBindingError> {
        self.nodes.resolve(&SemanticNodeAnchor {
            owner: self.nodes.owner().clone(),
            root: self.nodes.root().to_owned(),
            path: path.to_owned(),
        })
    }

    /// Return the selected owner's canonical structural path for owner-wide artifact rows.
    fn owner_path(&self) -> Result<String, NodeBindingError> {
        Ok(self.nodes.anchor(self.owner)?.path)
    }
}

/// Recover the lexical spelling and kind for a decoded binding. The caller verifies the complete stable identity
/// against the writer's current structural binding order before installing the handle.
fn persisted_nested_binding(
    owner: &StableDeclarationId,
    identity: &str,
) -> Result<(SemanticSourceTargetKind, String), NodeBindingError> {
    let refused = || NodeBindingError::InvalidField("nested declaration identity".into());
    let (tag, _) = identity.split_once(':').ok_or_else(refused)?;
    let kind = SemanticSourceTargetKind::from_kind_str(tag);
    let template = StableDeclarationId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: owner.origin.clone(),
        declaration_name: String::new(),
        kind,
        location: StableDeclarationLocation::ModuleLevel,
        signature: None,
    };
    let (name, _) = identity
        .strip_prefix(&template.render_compact())
        .and_then(|rest| rest.split_once("#in("))
        .ok_or_else(refused)?;
    Ok((template.kind, name.to_owned()))
}

/// Collect every active nominal owner that the checker already authenticated independently of one lexical spelling.
fn admitted_type_identities(checker: &TypeChecker) -> BTreeMap<String, CanonicalSymbolId> {
    let mut identities = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter_map(|(index, _)| checker.symbols.identity_of(index))
        .filter(|canonical| {
            canonical.scope_discriminant.is_none()
                && ((canonical.origin == checker.symbols.declaration_origin()
                    && matches!(
                        canonical.kind,
                        SemanticSourceTargetKind::Model
                            | SemanticSourceTargetKind::Class
                            | SemanticSourceTargetKind::Newtype
                            | SemanticSourceTargetKind::Rusttype
                            | SemanticSourceTargetKind::Enum
                            | SemanticSourceTargetKind::TypeAlias
                            | SemanticSourceTargetKind::Trait
                    ))
                    || (canonical.kind == SemanticSourceTargetKind::RustItem
                        && matches!(canonical.origin, incan_semantics_core::SymbolOrigin::RustCrate(_))))
        })
        .filter_map(|canonical| {
            StableDeclarationId::from_canonical(canonical, None, None)
                .map(|stable| (stable.render_compact(), canonical.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let active = checker
        .type_info
        .declarations
        .named_type_identities
        .values()
        .chain(checker.type_info.declarations.body_ir_native_type_identities.values())
        .chain(
            checker
                .type_info
                .expressions
                .expression_type_identities
                .values()
                .flat_map(|identities| identities.values()),
        )
        .chain(
            checker
                .public_library_type_identities
                .values()
                .filter_map(|identity| identity.canonical.as_ref()),
        );
    for canonical in active {
        if let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) {
            identities.insert(stable.render_compact(), canonical.clone());
        }
    }
    identities
}

/// Rebuild a selected owner solely from canonical fields and current position/identity bindings. The independently
/// checked current baseline supplies coordinates/handles only; no native fact payload is copied from it.
fn restore(
    positions: &Positions,
    program: &Program,
    owner: Span,
    fields: &[Vec<String>],
) -> Result<RestoredOwner, NodeBindingError> {
    let header = fields
        .first()
        .filter(|header| header.len() == 5)
        .ok_or(NodeBindingError::MissingOwner)?;
    if header[0] != "header" || header[4] != crate::checker_records::GRAPH_BODY_RECORD_PROFILE {
        return Err(NodeBindingError::Unsupported("record profile"));
    }
    if header[1] != positions.nodes.owner().render_compact() {
        return Err(NodeBindingError::WrongOwner);
    }
    if header[2] != positions.nodes.root() {
        return Err(NodeBindingError::ChangedBody);
    }
    let kind = match header[3].as_str() {
        "declaration" => LocalOwnerKind::Declaration,
        "final" => LocalOwnerKind::Body,
        _ => return Err(NodeBindingError::Unsupported("record stage")),
    };
    let mut rebound = Rebinding {
        local: BodyLocal {
            owner,
            kind,
            ..BodyLocal::default()
        },
        ..Rebinding::default()
    };
    let mut previous: Option<&[String]> = None;
    for row in &fields[1..] {
        if row.len() < 4 || previous.is_some_and(|previous| previous[..3] >= row[..3]) {
            return Err(NodeBindingError::InvalidField(
                "canonical ordering or field width".into(),
            ));
        }
        previous = Some(row);
        rebound.field(program, positions, header, row).map_err(|error| {
            if error == NodeBindingError::MissingNode {
                NodeBindingError::InvalidField(format!("family={} node={} selector={}", row[0], row[1], row[2]))
            } else {
                error
            }
        })?;
    }
    // Native input calls are not recorded; rebuild them from the restored native calls, as the body proof built them.
    if let Some(identity) = rebound.local.protocols.explicit_input_flow.keys().next().cloned() {
        super::super::ownership_contracts::native::retain_native_input_calls(
            &mut rebound.local.protocols,
            &rebound.local.rust,
            &identity,
            None,
        );
    }
    function_calls::restore(positions, &mut rebound.local)?;
    rebound.finish(&header[3])
}

/// Rebuild one pending owner and rebind its recorded local diagnostic values to this settlement's writers.
fn restore_pending(
    checker: &mut TypeChecker,
    program: &Program,
    positions: &Positions,
    owner: Span,
    fields: &Fields,
) -> Result<BodyLocal, NodeBindingError> {
    let header = fields
        .first()
        .filter(|header| header.len() == 5)
        .ok_or(NodeBindingError::MissingOwner)?;
    if header[0] != "header"
        || header[1] != positions.nodes.owner().render_compact()
        || header[2] != positions.nodes.root()
        || header[3] != "local"
        || header[4] != crate::checker_records::GRAPH_BODY_RECORD_PROFILE
    {
        return Err(NodeBindingError::Unsupported("local record header"));
    }
    let mut restored = Rebinding::default();
    restored.local.owner = owner;
    restored.local.kind = LocalOwnerKind::Body;
    let mut previous: Option<&[String]> = None;
    let mut pending = false;
    for row in &fields[1..] {
        if row.len() < 4 || previous.is_some_and(|previous| previous[..3] >= row[..3]) {
            return Err(NodeBindingError::InvalidField(
                "canonical ordering or field width".into(),
            ));
        }
        previous = Some(row);
        if row.len() == 4 && row[0] == "status" && row[1].is_empty() && row[2] == "pending" && row[3] == "true" {
            if pending {
                return Err(NodeBindingError::InvalidField("duplicate pending status".into()));
            }
            pending = true;
            continue;
        }
        if row[0] == "diagnostic" {
            restored.diagnostic(program, positions, row)?;
            continue;
        }
        restore_local(positions, &mut restored.local, &mut restored.record_identities, row).map_err(|error| {
            if error == NodeBindingError::MissingNode {
                NodeBindingError::InvalidField(format!("family={} node={} selector={}", row[0], row[1], row[2]))
            } else {
                error
            }
        })?;
    }
    if !pending {
        return Err(NodeBindingError::InvalidField("missing pending status".into()));
    }
    let mut local = restored.local;
    let mut current = DiagnosticPositions::new(program, checker.symbols.declaration_origin());
    for (kind, values, slots, writer) in [
        (
            "error",
            restored.diagnostics,
            &mut local.diagnostic_slots,
            &mut checker.errors,
        ),
        (
            "warning",
            restored.warnings,
            &mut local.warning_slots,
            &mut checker.warnings,
        ),
    ] {
        for value in ordered_diagnostics(values, kind)? {
            let diagnostic = current.render(&value).map_err(|_| NodeBindingError::MissingNode)?;
            slots.push(writer.len());
            writer.push(diagnostic);
        }
    }
    local.mutation.restore_flow_graphs(&mut local.protocols);
    function_calls::restore(positions, &mut local)?;
    Ok(local)
}

/// Rebinding owns decoded native scratch only for this activation; persistent bytes and source lifetimes stay outside.
#[derive(Default)]
struct Rebinding {
    local: BodyLocal,
    /// Compiler-owned identities authenticated by preceding builtin method fields.
    record_identities: BTreeMap<String, CanonicalSymbolId>,
    diagnostics: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    warnings: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    mutation: super::super::ownership_contracts::mutation::RecordedMutation,
    status: Option<bool>,
    binding: Option<super::super::type_info::FunctionBindingInfo>,
    defaults: BTreeMap<Span, (Span, String)>,
    final_types: FinalTypes,
    ownership: Option<crate::ownership_metadata::CheckedOwnershipContract>,
}

impl Rebinding {
    /// Dispatch exactly one admitted field grammar. Unrecognized selectors never fall back to native source facts.
    fn field(
        &mut self,
        program: &Program,
        positions: &Positions,
        header: &[String],
        row: &[String],
    ) -> Result<(), NodeBindingError> {
        match row[0].as_str() {
            "status" if row[1].is_empty() && row[2] == "refused" && row.len() == 4 => {
                self.status = Some(row[3].parse::<bool>().map_err(|_| NodeBindingError::MissingNode)?)
            }
            "effect" if row[1].is_empty() && row[3] == "checked" && row.len() == 4 => {
                self.mutation.bodies.insert(positions.canonical(&row[2])?);
            }
            "mutation" => mutation::restore(positions, &mut self.mutation, row)?,
            "ownership" if row[2] == "contract" => {
                if self.ownership.is_some() || row[1] != positions.nodes.anchor(self.local.owner)?.path {
                    return Err(NodeBindingError::MissingNode);
                }
                self.ownership = Some(ownership::read(positions, &row[3..])?);
            }
            "signature" if row[2] == "declaration" => self.signature(positions, header, row)?,
            "default" if row.len() == 4 && row[3] == header[1] => {
                let span = positions.span(&row[1])?;
                if self.defaults.insert(span, (self.local.owner, row[2].clone())).is_some() {
                    return Err(NodeBindingError::MissingNode);
                }
            }
            "final-type" => self.final_type(positions, row)?,
            "native-call" if row[1].is_empty() && row[2] == "empty" && row.len() == 4 && row[3] == "true" => {}
            "diagnostic" => self.diagnostic(program, positions, row)?,
            _ => restore_local(positions, &mut self.local, &mut self.record_identities, row)?,
        }
        Ok(())
    }

    /// Restore declaration signatures from their canonical parameter/type primitives, not from the current binding.
    fn signature(&mut self, positions: &Positions, header: &[String], row: &[String]) -> Result<(), NodeBindingError> {
        if self.binding.is_some()
            || header[3] != "declaration"
            || row[1] != positions.nodes.anchor(self.local.owner)?.path
        {
            return Err(NodeBindingError::MissingNode);
        }
        let mut values = row[3..].iter();
        let is_async = next(&mut values)?
            .parse::<bool>()
            .map_err(|_| NodeBindingError::MissingNode)?;
        let params = read_parameters(&mut values)?;
        let return_type = read_type(&mut values)?;
        if values.next().is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        self.binding = Some(super::super::type_info::FunctionBindingInfo {
            is_async,
            params,
            return_type,
            identity: Some(positions.canonical(&header[1])?),
        });
        Ok(())
    }

    /// Finalized own expression/binding values are separate from pending local checks and their original snapshots.
    fn final_type(&mut self, positions: &Positions, row: &[String]) -> Result<(), NodeBindingError> {
        let mut values = row[3..].iter();
        match row[2].as_str() {
            "expression" => {
                self.final_types
                    .expressions
                    .insert(positions.span(&row[1])?, read_type(&mut values)?);
            }
            "binding" if !row[1].is_empty() => {
                let canonical = positions.canonical(&row[1])?;
                self.final_types.bindings.insert(canonical, read_type(&mut values)?);
            }
            "empty" if row[1].is_empty() && row.len() == 4 && row[3] == "true" => {
                let _ = next(&mut values)?;
            }
            _ => return Err(NodeBindingError::MissingNode),
        }
        if values.next().is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(())
    }

    /// Retain the legacy diagnostic order independently of canonical field sorting by structural path.
    fn diagnostic(&mut self, program: &Program, positions: &Positions, row: &[String]) -> Result<(), NodeBindingError> {
        let value = read_diagnostic(program, positions, row)?;
        let map = match row[2].split_once(':').map(|(kind, _)| kind) {
            Some("error") => &mut self.diagnostics,
            Some("warning") => &mut self.warnings,
            _ => return Err(NodeBindingError::MissingNode),
        };
        if map.insert(row[2].clone(), value).is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(())
    }

    /// Finish one independently settled owner; pending producer values are never decoded by this adapter.
    fn finish(mut self, stage: &str) -> Result<RestoredOwner, NodeBindingError> {
        self.local.mutation =
            super::super::ownership_contracts::mutation::MutationLocal::from_record_projection(self.mutation);
        self.local.mutation.restore_flow_graphs(&mut self.local.protocols);
        let diagnostics = ordered_diagnostics(self.diagnostics, "error")?;
        let warnings = ordered_diagnostics(self.warnings, "warning")?;
        let view = CheckedBodyView {
            local: Arc::new(self.local),
            effect_reads: Default::default(),
            diagnostics,
            warnings,
            mutation: Vec::new(),
            dictionary: Vec::new(),
            open: Vec::new(),
        };
        if self.status != Some(view.is_refused()) || (stage == "declaration") != self.binding.is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(RestoredOwner {
            view,
            binding: self.binding,
            defaults: self.defaults,
            final_types: self.final_types,
            ownership: self.ownership,
        })
    }
}

/// Require contiguous canonical diagnostic indices before dropping their invocation-independent order keys.
fn ordered_diagnostics(
    values: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    kind: &str,
) -> Result<Vec<super::super::diagnostic_values::AnchoredDiagnostic>, NodeBindingError> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, (key, value))| {
            if key != format!("{kind}:{index:08}") {
                return Err(NodeBindingError::MissingNode);
            }
            Ok(value)
        })
        .collect()
}

/// Decode the exact canonical component path emitted by the expression-record projection.
fn read_type_identity_path(path: &str) -> Result<Vec<usize>, NodeBindingError> {
    if path == "root" {
        return Ok(Vec::new());
    }
    let components = path.split('.').collect::<Vec<_>>();
    if components.is_empty() || components.len() > 32 {
        return Err(NodeBindingError::MissingNode);
    }
    components
        .into_iter()
        .map(|component| {
            let index = component.parse::<usize>().map_err(|_| NodeBindingError::MissingNode)?;
            if component != index.to_string() {
                return Err(NodeBindingError::MissingNode);
            }
            Ok(index)
        })
        .collect()
}

/// Decode one canonical nonnegative integer without accepting padded or signed spellings.
fn read_number(values: &mut std::slice::Iter<'_, String>) -> Result<usize, NodeBindingError> {
    let text = next(values)?;
    let number = text.parse::<usize>().map_err(|_| NodeBindingError::MissingNode)?;
    if number.to_string() != text {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(number)
}

/// Restore one member reference's exact declaring nominal before its later reference row consumes the identity.
fn restore_identity_context(
    positions: &Positions,
    local: &mut BodyLocal,
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    span: Span,
    values: &mut std::slice::Iter<'_, String>,
) -> Result<(), NodeBindingError> {
    let identity = next(values)?.to_owned();
    let owner = positions.canonical_type(next(values)?)?;
    let binding_ordinal = u32::try_from(read_number(values)?).map_err(|_| NodeBindingError::MissingNode)?;
    let kind_text = next(values)?;
    let kind = SemanticSourceTargetKind::from_kind_str(kind_text);
    if kind.as_str() != kind_text
        || !matches!(
            kind,
            SemanticSourceTargetKind::Field
                | SemanticSourceTargetKind::Method
                | SemanticSourceTargetKind::Property
                | SemanticSourceTargetKind::Variant
        )
    {
        return Err(NodeBindingError::MissingNode);
    }
    let name = next(values)?;
    let canonical = match positions.canonical(&identity) {
        Ok(member) => member,
        Err(error) if matches!(owner.origin, incan_semantics_core::SymbolOrigin::Package { .. }) => {
            // A package declaration uses its own anchor. Only a checked generated member belongs at the owner.
            let generated = owner.generated_member(name, kind.clone());
            if local.calls.compiler_generated_member_identities.get(&generated) != Some(&owner) {
                return Err(error);
            }
            generated
        }
        Err(_) => owner.generated_member(name, kind.clone()),
    };
    if canonical.declaration_name != name
        || canonical.kind != kind
        || stable_member_identity(&canonical, &owner, binding_ordinal)? != identity
        || record_identities
            .get(&identity)
            .is_some_and(|previous| previous != &canonical)
        || local
            .references
            .stable_declaration_contexts
            .insert(
                span,
                incan_semantics_core::CanonicalStableDeclarationContext { owner, binding_ordinal },
            )
            .is_some()
    {
        return Err(NodeBindingError::MissingNode);
    }
    record_identities.insert(identity, canonical);
    Ok(())
}

/// Bind one member to its checked enclosing declaration and collision-local ordinal.
fn stable_member_identity(
    member: &CanonicalSymbolId,
    owner: &CanonicalSymbolId,
    binding_ordinal: u32,
) -> Result<String, NodeBindingError> {
    let owner = StableDeclarationId::from_canonical(owner, None, None).ok_or(NodeBindingError::MissingOwner)?;
    StableDeclarationId::from_canonical(member, None, Some(StableDeclarationContext { owner, binding_ordinal }))
        .map(|identity| identity.render_compact())
        .ok_or(NodeBindingError::MissingOwner)
}

/// Resolve an ordinary activation identity or one admitted by an earlier builtin method field in the same record.
fn restored_identity(
    positions: &Positions,
    record_identities: &BTreeMap<String, CanonicalSymbolId>,
    identity: &str,
) -> Result<CanonicalSymbolId, NodeBindingError> {
    positions
        .canonical(identity)
        .or_else(|error| record_identities.get(identity).cloned().ok_or(error))
}

/// Admit a registry-validated builtin method identity for reference fields that follow it in canonical order.
fn retain_builtin_method_identity(
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    builtin: incan_lang::lang::builtins::BuiltinMethodId,
) -> Result<(), NodeBindingError> {
    let canonical = crate::symbols::canonical_builtin_member_identity(builtin.owner(), builtin.member());
    retain_registry_identity(record_identities, canonical)
}

/// A checked builtin call can name a registry target without a physical lexical symbol.
fn retain_registry_identity(
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    canonical: CanonicalSymbolId,
) -> Result<(), NodeBindingError> {
    let stable = StableDeclarationId::from_canonical(&canonical, None, None)
        .ok_or(NodeBindingError::MissingNode)?
        .render_compact();
    match record_identities.get(&stable) {
        Some(previous) if previous != &canonical => return Err(NodeBindingError::WrongOwner),
        Some(_) => {}
        None => {
            record_identities.insert(stable, canonical);
        }
    }
    Ok(())
}

/// The complete native value is an independent baseline only; every sorted map, sequence and scalar remains visible.
fn compare_native(expected: &impl std::fmt::Debug, restored: &impl std::fmt::Debug) -> Result<(), NodeBindingError> {
    let before = super::baseline_compare::canonical_debug(&format!("{expected:?}"));
    let after = super::baseline_compare::canonical_debug(&format!("{restored:?}"));
    if before != after {
        return Err(NodeBindingError::BaselineDifference(format!(
            "expected {before}; actual {after}"
        )));
    }
    Ok(())
}

/// Reconstructed semantic native values and default-writer references, separate from the current position bindings.
struct RestoredOwner {
    view: CheckedBodyView,
    binding: Option<super::super::type_info::FunctionBindingInfo>,
    defaults: BTreeMap<Span, (Span, String)>,
    final_types: FinalTypes,
    ownership: Option<crate::ownership_metadata::CheckedOwnershipContract>,
}

/// Selected final module refinements for one body's own expression/reference domains. These are distinct from the
/// pending local facts and never include a sibling's payload.
#[derive(Debug, Default, PartialEq)]
struct FinalTypes {
    expressions: BTreeMap<Span, crate::symbols::ResolvedType>,
    bindings: BTreeMap<CanonicalSymbolId, crate::symbols::ResolvedType>,
}
