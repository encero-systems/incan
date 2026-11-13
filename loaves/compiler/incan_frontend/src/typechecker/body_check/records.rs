//! Selected owner projections for the Incan checker-record codec. This is a bootstrap conversion, not a store.

mod diagnostics;
mod function_calls;
mod input_flow;
mod inputs;
mod mutation;
mod ownership;
mod types;
use diagnostics::{project_diagnostics, read_diagnostic};
mod unsupported;

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::canonical_nodes::{NodeBindingError, SemanticNodeAnchor, SemanticNodeBindings};
use super::super::diagnostic_values::DiagnosticPositions;
use super::{BodyCheckInput, BodyLocal, CheckedBodyView, LocalOwnerKind, TypeChecker};
use crate::ast::{Program, Span};

use incan_semantics_core::stable_identity::StableDeclarationId;
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
    let mut fields = local_fields(&positions, local)?;
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
    /// Complete one owner trace with the checker-record outcome decided after all producer boundaries settle.
    pub(crate) fn record_body_check_outcome(&self, owner: Span, outcome: &'static str) {
        if let Some(span) = self.body_check_traces.get(&owner) {
            span.record("outcome", outcome);
        }
    }

    /// Report whether this activation restored the selected local body instead of checking statements afresh.
    pub(crate) fn restored_body_record(&self, owner: Span) -> bool {
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

    /// Restore one graph-selected final value against current compiler identities and syntax, then enqueue it for the
    /// ordinary checked-body settlement join. A refusal leaves the caller on fresh body checking and publishes no
    /// selected value.
    pub(in crate::typechecker) fn reuse_graph_body_record(
        &mut self,
        program: &Program,
        owner: Span,
    ) -> Result<bool, NodeBindingError> {
        let identity = Positions::new(self, program, owner, false)?
            .nodes
            .owner()
            .render_compact();
        let Some(selected) = self.graph_body_records.get(&identity)? else {
            return Ok(false);
        };
        // The checker owns selected rows after this boundary so its publication view cannot outlive the invocation
        // transport. This is the one deliberate copy from immutable transport into checker-owned selected output.
        let local_fields = selected.local.clone();
        let final_fields = selected.final_value.clone();
        let positions = Positions::for_decoded_local(self, program, owner, &local_fields)?;
        let restored = restore(&positions, program, owner, &final_fields)?;
        if restored.binding.is_some() || !restored.defaults.is_empty() {
            return Err(NodeBindingError::InvalidField(
                "body record carried declaration output".into(),
            ));
        }
        let canonical = self
            .declaration_view()
            .function(owner)
            .and_then(|binding| binding.identity.as_ref())
            .cloned()
            .ok_or(NodeBindingError::MissingOwner)?;
        let reused_body = selected
            .body_ir
            .as_ref()
            .filter(|frame| {
                frame.identity == canonical.render_compact() && frame.body.name == canonical.declaration_name
            })
            .map(|frame| frame.body.clone());
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
        if let Some(body) = reused_body {
            self.type_info.reused_callable_bodies.insert(owner, body);
        }
        Ok(true)
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
        if self.declaration_view().function(owner) != Some(&binding)
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
    nodes: SemanticNodeBindings,
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
        let mut identities = checker
            .type_info
            .declarations
            .source_identities
            .iter()
            .map(|(canonical, stable)| (stable.render_compact(), canonical.clone()))
            .collect::<BTreeMap<_, _>>();
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
        let mut type_identities: BTreeMap<String, CanonicalSymbolId> = checker
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
            .collect();
        for identity in checker.public_library_type_identities.values() {
            let Some(canonical) = &identity.canonical else {
                continue;
            };
            if let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) {
                type_identities.insert(stable.render_compact(), canonical.clone());
            }
        }
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
        let declaration_bindings = program
            .declarations
            .iter()
            .find(|declaration| declaration.span == owner)
            .and_then(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) => Some(function),
                _ => None,
            })
            .into_iter()
            .flat_map(|function| &function.params)
            .map(|parameter| {
                (0..checker.symbols.all_symbols().len())
                    .filter_map(|index| checker.symbols.identity_of(index))
                    .find(|canonical| {
                        canonical.kind == SemanticSourceTargetKind::Parameter
                            && canonical.origin == checker.symbols.declaration_origin()
                            && canonical.declaration_span == parameter.span.into()
                    })
                    .cloned()
                    .unwrap_or_else(|| CanonicalSymbolId {
                        namespace: SymbolNamespace::OrdinaryLexical,
                        origin: checker.symbols.declaration_origin(),
                        declaration_name: parameter.node.name.clone(),
                        kind: SemanticSourceTargetKind::Parameter,
                        scope_discriminant: Some(ScopeDiscriminant(checker.symbols.next_scope_index())),
                        declaration_span: parameter.span.into(),
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
            nodes,
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
        Ok(positions)
    }

    /// Bind opaque nested identities from one authenticated local value to this revision's structural declaration
    /// nodes. Module declarations and parameters retain their ordinary collected identities; only body-local
    /// declarations receive activation-private handles, which are mapped back to their persisted stable identities.
    fn for_decoded_local(
        checker: &TypeChecker,
        program: &Program,
        owner: Span,
        fields: &Fields,
    ) -> Result<Self, NodeBindingError> {
        let mut positions = Self::new(checker, program, owner, false)?;
        for row in fields {
            if row.len() != 4 || row[0] != "binding" || row[2] != "declaration" {
                continue;
            }
            let span = positions.span(&row[1])?;
            let canonical = CanonicalSymbolId {
                namespace: SymbolNamespace::OrdinaryLexical,
                origin: checker.symbols.declaration_origin(),
                declaration_name: row[3].clone(),
                kind: SemanticSourceTargetKind::Local,
                scope_discriminant: Some(ScopeDiscriminant(checker.symbols.next_scope_index())),
                declaration_span: span.into(),
            };
            if positions.identities.insert(row[3].clone(), canonical).is_some() {
                return Err(NodeBindingError::InvalidField(
                    "duplicate nested declaration identity".into(),
                ));
            }
        }
        Ok(positions)
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

/// Rebuild one pending local producer value for its settlement activation. The value carries no finalized status,
/// diagnostics or types; those remain owned by the settlement operation entered after this function returns.
fn restore_pending(positions: &Positions, owner: Span, fields: &Fields) -> Result<BodyLocal, NodeBindingError> {
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
    let mut local = BodyLocal {
        owner,
        kind: LocalOwnerKind::Body,
        ..BodyLocal::default()
    };
    let mut record_identities = BTreeMap::new();
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
        restore_local(positions, &mut local, &mut record_identities, row).map_err(|error| {
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

/// Select complete supported expression maps. Their native field names are schema selectors, not memory offsets.
fn project_expressions(positions: &Positions, local: &BodyLocal, fields: &mut Fields) -> Result<(), NodeBindingError> {
    let expressions = &local.expressions;
    for (selector, values) in [
        ("expr", &expressions.expr_types),
        ("body-expr", &expressions.body_ir_expr_types),
        ("assignment", &expressions.assignment_binding_types),
        ("destination", &expressions.value_destination_types),
    ] {
        for (span, ty) in values {
            let mut row = vec!["type".into(), positions.nodes.anchor(*span)?.path, selector.into()];
            type_fields(ty, &mut row)?;
            fields.push(row);
        }
    }
    for (binding, ty) in &expressions.body_ir_binding_types {
        let mut row = vec![
            "binding-type".into(),
            positions.nodes.anchor(span_of(binding))?.path,
            "body".into(),
            positions.identity(binding)?,
        ];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    for (binding, requirement) in &expressions.body_ir_parameter_requirements {
        if binding.kind != incan_semantics_core::SemanticSourceTargetKind::Parameter {
            return Err(NodeBindingError::WrongOwner);
        }
        let mut row = vec![
            "binding-type".into(),
            positions.nodes.anchor(span_of(binding))?.path,
            "parameter-requirement".into(),
            positions.identity(binding)?,
        ];
        match requirement {
            Some(ty) => {
                row.push("some".into());
                type_fields(ty, &mut row)?;
            }
            None => row.push("none".into()),
        }
        fields.push(row);
    }
    for (span, kind) in &expressions.ident_kinds {
        let tag = match kind {
            super::super::type_info::IdentKind::Value => "value",
            _ => return Err(NodeBindingError::Unsupported("identifier kind")),
        };
        fields.push(vec![
            "identifier".into(),
            positions.nodes.anchor(*span)?.path,
            "kind".into(),
            tag.into(),
        ]);
    }
    for (span, target) in &expressions.source_targets {
        let mut row = vec![
            "target".into(),
            positions.nodes.anchor(*span)?.path,
            "source".into(),
            target.name.clone(),
            target.kind.clone(),
        ];
        row.extend(target.module_path.iter().cloned());
        fields.push(row);
    }
    for (span, identities) in &expressions.expression_type_identities {
        for (path, identity) in identities {
            fields.push(vec![
                "type-identity".into(),
                positions.nodes.anchor(*span)?.path,
                type_identity_path(path),
                positions.type_identity(identity)?,
            ]);
        }
    }
    Ok(())
}

/// Encode one nested type-component path as a canonical field selector; `root` names the expression type itself.
fn type_identity_path(path: &[usize]) -> String {
    if path.is_empty() {
        return "root".into();
    }
    path.iter().map(usize::to_string).collect::<Vec<_>>().join(".")
}

/// Decode the exact canonical component path emitted by [`type_identity_path`].
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

/// Recreate every supported local map directly from the selected field's typed primitives.
fn restore_local(
    positions: &Positions,
    local: &mut BodyLocal,
    record_identities: &mut BTreeMap<String, CanonicalSymbolId>,
    row: &[String],
) -> Result<(), NodeBindingError> {
    if matches!(
        row[0].as_str(),
        "input-call" | "input-local" | "input-flow" | "input-copy"
    ) {
        return inputs::restore(positions, local, row);
    }
    let span = positions.span(&row[1])?;
    let mut values = row[3..].iter();
    match row[0].as_str() {
        "type" => {
            let ty = read_type(&mut values)?;
            let map = match row[2].as_str() {
                "expr" => &mut local.expressions.expr_types,
                "body-expr" => &mut local.expressions.body_ir_expr_types,
                "assignment" => &mut local.expressions.assignment_binding_types,
                "destination" => &mut local.expressions.value_destination_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, ty);
        }
        "binding-type" if row[2] == "body" => {
            let binding = positions.canonical(next(&mut values)?)?;
            if span_of(&binding) != span {
                return Err(NodeBindingError::WrongOwner);
            }
            if local
                .expressions
                .body_ir_binding_types
                .insert(binding, read_type(&mut values)?)
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "binding-type" if row[2] == "parameter-requirement" => {
            let binding = positions.canonical(next(&mut values)?)?;
            if span_of(&binding) != span || binding.kind != incan_semantics_core::SemanticSourceTargetKind::Parameter {
                return Err(NodeBindingError::WrongOwner);
            }
            let requirement = match next(&mut values)? {
                "none" => None,
                "some" => Some(read_type(&mut values)?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            if local.expressions.body_ir_parameter_requirements.contains_key(&binding) {
                return Err(NodeBindingError::MissingNode);
            }
            local
                .expressions
                .body_ir_parameter_requirements
                .insert(binding, requirement);
        }
        "parameter" => {
            local
                .references
                .resolved_write_types
                .insert((span, row[2].clone()), read_type(&mut values)?);
        }
        "reference" if row[2] == "read" => {
            let identity = next(&mut values)?;
            let canonical = restored_identity(positions, record_identities, identity)?;
            local.references.resolved_identities.insert(span, canonical);
        }
        "write" => {
            local
                .references
                .resolved_write_identities
                .insert((span, row[2].clone()), positions.canonical(next(&mut values)?)?);
        }
        "binding" if row[2] == "declaration" => {
            local.nested.insert(span, positions.canonical(next(&mut values)?)?);
        }
        "identifier" if row[2] == "kind" && next(&mut values)? == "value" => {
            local
                .expressions
                .ident_kinds
                .insert(span, super::super::type_info::IdentKind::Value);
        }
        "type-identity" if row.len() == 4 => {
            let path = read_type_identity_path(&row[2])?;
            let identity = positions.canonical_type(next(&mut values)?)?;
            if local
                .expressions
                .expression_type_identities
                .entry(span)
                .or_default()
                .insert(path, identity)
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "requirement" if row[2] == "constructor-field" => {
            let field = positions.canonical(next(&mut values)?)?;
            let owner = positions.canonical_type(next(&mut values)?)?;
            if field.kind != SemanticSourceTargetKind::Field
                || !matches!(
                    owner.kind,
                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                )
                || local
                    .calls
                    .body_ir_constructor_field_requirements
                    .insert(span, (field, owner))
                    .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        "builtin" if row[2] == "call" => {
            let builtin =
                incan_lang::lang::builtins::from_str(next(&mut values)?).ok_or(NodeBindingError::MissingNode)?;
            local.calls.resolved_builtin_calls.insert(span, builtin);
        }
        "builtin" if row[2] == "method" => {
            let builtin = incan_lang::lang::builtins::method_from_parts(next(&mut values)?, next(&mut values)?)
                .ok_or(NodeBindingError::MissingNode)?;
            retain_builtin_method_identity(record_identities, builtin)?;
            local.calls.resolved_builtin_method_calls.insert(span, builtin);
        }
        "signature" if matches!(row[2].as_str(), "monomorphs" | "type-arguments") => {
            let count = next(&mut values)?
                .parse::<usize>()
                .map_err(|_| NodeBindingError::MissingNode)?;
            if count > values.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let arguments = (0..count)
                .map(|_| read_type(&mut values))
                .collect::<Result<Vec<_>, _>>()?;
            let map = if row[2] == "monomorphs" {
                &mut local.calls.call_site_monomorph_type_args
            } else {
                &mut local.calls.body_ir_call_type_arguments
            };
            map.insert(span, arguments);
        }
        "signature" => {
            let parameters = read_parameters(&mut values)?;
            let map = match row[2].as_str() {
                "call" => &mut local.calls.call_site_callable_params,
                "body-call" => &mut local.calls.body_ir_call_parameter_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, parameters);
        }
        "target" if row[2] == "source" => {
            let name = next(&mut values)?.to_owned();
            let kind = next(&mut values)?.to_owned();
            let module_path = values.by_ref().cloned().collect();
            local.expressions.source_targets.insert(
                span,
                super::super::type_info::SourceTargetInfo {
                    name,
                    kind,
                    module_path,
                },
            );
        }
        _ => return Err(NodeBindingError::Unsupported("canonical field")),
    }
    if values.next().is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
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
