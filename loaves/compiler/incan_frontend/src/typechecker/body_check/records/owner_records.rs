//! The type checker's per-owner record accessors: produced, available and refused declaration, local and final
//! records, retained records, and the physical inputs and digests of a selected graph body.

use super::*;

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
            .or_else(|| self.selected_local_records.get(&owner).map(Arc::as_ref))
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

    /// A staged graph owner needs its current bound locals to authenticate and republish its executable frame.
    pub(crate) fn graph_selects_body(&self, identity: &StableDeclarationId) -> bool {
        self.graph_body_records.selects_owner(identity)
    }

    /// Retain one selected local value for republishing without treating selection as producer execution.
    pub(crate) fn retain_local_record(&mut self, owner: Span, fields: Fields) {
        self.selected_local_records.insert(owner, Arc::new(fields));
    }

    /// Borrow the final record emitted directly by this activation's settlement producer.
    pub(crate) fn produced_final_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_final_records.get(&owner)
    }

    /// Borrow either the final value evaluated in this activation or a producer-validated settled predecessor.
    pub(crate) fn available_final_record(&self, owner: Span) -> Option<&Fields> {
        self.produced_final_records
            .get(&owner)
            .or_else(|| self.selected_final_records.get(&owner).map(Arc::as_ref))
    }

    /// Stage one scalar product's settled value until its body passes the current physical-scope check.
    pub(crate) fn retain_callable_final_record(&mut self, owner: Span, fields: Fields) {
        self.callable_final_record_reuse.insert(owner, fields);
    }

    /// Publish the staged final value only after the matching scalar body is accepted for this activation.
    pub(in crate::typechecker) fn select_callable_final_record(&mut self, owner: Span) {
        if let Some(fields) = self.callable_final_record_reuse.remove(&owner) {
            self.selected_final_records.insert(owner, Arc::new(fields));
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
        self.graph_body_inputs(program, owner, false).map(|value| value.0)
    }

    /// Borrow the physical proof already computed during admission for this check's completed owner.
    /// The caller must be observing the checked program in the same activation.
    pub fn completed_graph_body_digest(&self, owner: Span) -> Option<[u8; 32]> {
        self.completed_graph_body_digests.get(&owner).copied()
    }

    /// The same native proof and its optional syntax-path-ordered coordinate map, built from one checkout of the facts.
    pub fn graph_body_inputs(
        &self,
        program: &Program,
        owner: Span,
        coordinates: bool,
    ) -> Option<([u8; 32], Vec<(u32, u32)>)> {
        let local = &self.checked_body(owner)?.local;
        let records = [
            self.available_declaration_record(owner)?,
            self.available_local_record(owner)?,
            self.available_final_record(owner)?,
        ];
        let positions = Positions::for_local(self, program, owner, local).ok()?;
        let digest = positions.physical_inputs_in(&self.type_info, local, records, None)?;
        let nodes = positions.nodes.physical_nodes();
        let geometry =
            if coordinates && nodes.len() <= incan_semantics_core::executable_representation::MAX_OWNER_COORDINATES {
                nodes
                    .values()
                    .map(|span| {
                        if span.expansion.is_some() {
                            return None;
                        }
                        Some((u32::try_from(span.start).ok()?, u32::try_from(span.end).ok()?))
                    })
                    .collect::<Option<Vec<_>>>()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        Some((digest, geometry))
    }

    /// Wider executable forms use the exact unmodified native guard, without carrying unused coordinates.
    pub fn graph_body_has_scalar_placement(body: &incan_semantics_core::body_ir::Body) -> bool {
        placement::scalar_frame(body)
    }

    /// Check the exact current owner before a ranked activation enters this function's statements.
    pub(in crate::typechecker) fn permits_ranked_body(&self, program: Option<&Program>, owner: Span) -> bool {
        if self.graph_body_records.demanded_owner().is_none() {
            return true;
        }
        let identity = if self.graph_body_records.ranked_owner().is_some() {
            program
                .and_then(|program| SemanticNodeBindings::new(self, program, owner).ok())
                .map(|nodes| nodes.owner().render_compact())
        } else {
            // The regular checker already entered the callable's declared signature. Its stable identity
            // names this statement entry without walking the owner's syntax a second time.
            let declarations = self.declaration_view();
            declarations
                .function(owner)
                .and_then(|binding| binding.identity.as_ref())
                .and_then(|canonical| declarations.source_identity(canonical))
                .map(StableDeclarationId::render_compact)
        };
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
        // The invocation and the checker share the same authenticated row fields without copying the selected facts.
        let local_fields = selected.local;
        let final_fields = selected.final_value;
        let positions = Positions::for_decoded_local(self, program, owner, nodes, &local_fields, true)?;
        if !final_fields
            .iter()
            .any(|row| row.first().is_some_and(|tag| tag == "scope"))
        {
            return Err(NodeBindingError::InvalidField(
                "final callable scope allocation is missing".into(),
            ));
        }
        let restored = restore(self, &positions, program, owner, &final_fields)?;
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
                || (frame.coordinates.is_empty()
                    && (frame.body.canonical.as_ref() != Some(&canonical) || frame.body.span != owner))
                || frame.body.name != canonical.declaration_name
            {
                return None;
            }
            Some((
                owner,
                frame.physical_inputs?,
                frame.body.clone(),
                frame.coordinates.clone(),
            ))
        });
        let mut diagnostic_positions = DiagnosticPositions::new(program, self.symbols.declaration_origin());
        self.reuse_checked_body(Arc::new(restored.view), &mut diagnostic_positions)
            .map_err(|_| NodeBindingError::ChangedBody)?;
        self.type_info.rust.checked_layouts.extend(restored.rust_layouts);
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
        for (owner, expected, mut body, coordinates) in std::mem::take(&mut self.pending_graph_frames) {
            let Some(local) = self.checked_body(owner).map(|view| &view.local) else {
                continue;
            };
            let (Some(declaration), Some(local_fields), Some(final_value)) = (
                self.available_declaration_record(owner),
                self.available_local_record(owner),
                self.available_final_record(owner),
            ) else {
                continue;
            };
            let Ok(positions) = Positions::for_local(self, program, owner, local) else {
                continue;
            };
            let rows = [declaration, local_fields, final_value];
            let current = positions.physical_inputs_in(&self.type_info, local, rows, None);
            let exact = body.span == owner
                && body.canonical.as_ref()
                    == self
                        .declaration_view()
                        .callable(owner)
                        .and_then(|v| v.identity.as_ref())
                && current == Some(expected);
            if exact
                || (!coordinates.is_empty()
                    && placement::relocate(
                        &positions,
                        &self.type_info,
                        local,
                        rows,
                        expected,
                        &coordinates,
                        &mut body,
                    )
                    .is_some())
            {
                self.type_info.reused_callable_bodies.insert(owner, body);
            }
            if let Some(digest) = current {
                self.completed_graph_body_digests.insert(owner, digest);
            }
        }
    }
}
