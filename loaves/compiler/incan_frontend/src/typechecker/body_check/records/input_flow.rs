//! Ownership graph primitive adapter for the Incan record grammar. Source handles are rebound, never replayed as
//! offsets.

use super::types::next;
use super::{NodeBindingError, Positions, Span};
use crate::typechecker::ownership_contracts::flow::{
    Access, Event, Graph, InputEvaluation, Invocation, InvocationArgument, Loan, LoanId, LoanKind, Node,
    OperandDependency, OperationInputs, OwnedInitialization, Permission, Place, Projection, Root,
};
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};
use std::collections::BTreeMap;

/// Append a source anchor selected by the authenticated callable's structural binding map.
fn anchor(positions: &Positions, span: HirSourceSpan) -> Result<String, NodeBindingError> {
    positions
        .nodes
        .anchor(Span {
            start: span.start,
            end: span.end,
            expansion: span.expansion,
        })
        .map(|anchor| anchor.path)
}

/// Encode a checked place root without replacing temporaries by invented declarations.
fn root(positions: &Positions, value: &Root, fields: &mut Vec<String>) -> Result<(), NodeBindingError> {
    match value {
        Root::Binding(identity) => fields.extend(["binding".into(), positions.identity(identity)?]),
        Root::Temporary {
            owner,
            expression,
            invocation,
        } => fields.extend([
            "temporary".into(),
            positions.identity(owner)?,
            anchor(positions, *expression)?,
            anchor(positions, *invocation)?,
        ]),
    }
    Ok(())
}

/// Encode every field or evaluated element projection in source order.
fn place(positions: &Positions, value: &Place, fields: &mut Vec<String>) -> Result<(), NodeBindingError> {
    root(positions, &value.root, fields)?;
    fields.push(value.projection.len().to_string());
    for item in &value.projection {
        match item {
            Projection::Field(identity) => fields.extend(["field".into(), positions.identity(identity)?]),
            Projection::Element(span) => fields.extend(["element".into(), anchor(positions, *span)?]),
        }
    }
    Ok(())
}

/// Preserve the distinction between absent dependencies and graph-local loan identifiers.
fn loan_option(value: Option<LoanId>, fields: &mut Vec<String>) {
    match value {
        Some(loan) => fields.extend(["some".into(), loan.0.to_string()]),
        None => fields.push("none".into()),
    }
}

/// Retain exact producer-linked evaluation boundaries and source access; restoration never rediscovers them.
fn input_evaluation(value: &InputEvaluation, fields: &mut Vec<String>) {
    fields.extend([value.start_node.to_string(), value.end_node.to_string()]);
    match value.source_node {
        Some(node) => fields.extend(["some".into(), node.to_string()]),
        None => fields.push("none".into()),
    }
}

/// Frame map keys unambiguously using UTF-8 byte lengths, independently of current native identity ordering.
pub(super) fn canonical_key(values: &[String]) -> String {
    values.iter().map(|value| format!("{}:{value}", value.len())).collect()
}

/// Emit map entries in the same portable order that the Incan grammar and native rebinder require.
pub(super) fn ordered_fields(
    mut entries: Vec<(String, Vec<String>)>,
    fields: &mut Vec<String>,
) -> Result<(), NodeBindingError> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    fields.push(entries.len().to_string());
    let mut previous = String::new();
    for (key, row) in entries {
        if key <= previous {
            return Err(NodeBindingError::InvalidField("duplicate ownership map key".into()));
        }
        previous = key;
        fields.extend(row);
    }
    Ok(())
}

/// Require exact canonical order before current handles normalize the persisted map into native collections.
pub(super) fn ordered_key(previous: &mut String, before: &[String], remaining: usize) -> Result<(), NodeBindingError> {
    let consumed = before
        .len()
        .checked_sub(remaining)
        .ok_or(NodeBindingError::MissingNode)?;
    let key = canonical_key(&before[..consumed]);
    if key <= *previous {
        return Err(NodeBindingError::InvalidField(
            "noncanonical ownership map order".into(),
        ));
    }
    *previous = key;
    Ok(())
}

/// Convert the complete graph to bounded record primitives; liveness is recomputed from these exact facts on restore.
pub(super) fn project(positions: &Positions, graph: &Graph, fields: &mut Vec<String>) -> Result<(), NodeBindingError> {
    fields.extend([positions.identity(&graph.owner)?, graph.entry.to_string()]);
    let mut permissions = Vec::new();
    for (storage, permission) in &graph.permissions {
        let mut row = Vec::new();
        root(positions, storage, &mut row)?;
        let key = canonical_key(&row);
        row.extend([
            permission.exclusive.to_string(),
            permission.writable.to_string(),
            permission.transferable.to_string(),
        ]);
        permissions.push((key, row));
    }
    ordered_fields(permissions, fields)?;
    fields.push(graph.loans.len().to_string());
    for loan in &graph.loans {
        place(positions, &loan.source, fields)?;
        fields.push(
            match loan.kind {
                LoanKind::Shared => "shared",
                LoanKind::Exclusive => "exclusive",
            }
            .into(),
        );
        fields.push(loan.writable.to_string());
        loan_option(loan.parent, fields);
        fields.push(anchor(positions, loan.span)?);
    }
    let mut locals = Vec::new();
    for (local, loan) in &graph.local_loans {
        let mut row = vec![positions.identity(local)?];
        let key = canonical_key(&row);
        row.push(loan.0.to_string());
        locals.push((key, row));
    }
    ordered_fields(locals, fields)?;
    let mut owned = Vec::new();
    for (destination, initializer) in &graph.owned_locals {
        let mut row = vec![positions.identity(destination)?];
        let key = canonical_key(&row);
        row.push(anchor(positions, initializer.initializer)?);
        match &initializer.source {
            Some(source) => {
                row.push("some".into());
                place(positions, source, &mut row)?;
            }
            None => row.push("none".into()),
        }
        match initializer.access_node {
            Some(node) => row.extend(["some".into(), node.to_string()]),
            None => row.push("none".into()),
        }
        owned.push((key, row));
    }
    ordered_fields(owned, fields)?;
    let mut operations = Vec::new();
    for (span, operation) in &graph.operations {
        let mut row = vec![anchor(positions, *span)?];
        let key = canonical_key(&row);
        row.push(operation.completion_node.to_string());
        row.push(operation.operands.len().to_string());
        for operand in &operation.operands {
            row.push(anchor(positions, operand.operand)?);
            place(positions, &operand.source, &mut row)?;
            row.push(operand.loan.0.to_string());
            row.push(operand.final_access.to_string());
        }
        operations.push((key, row));
    }
    ordered_fields(operations, fields)?;
    let mut invocations = Vec::new();
    for (span, invocation) in &graph.invocations {
        let mut row = vec![anchor(positions, *span)?];
        let key = canonical_key(&row);
        row.push(positions.identity(&invocation.callee)?);
        loan_option(invocation.receiver, &mut row);
        row.push(invocation.arguments.len().to_string());
        for argument in &invocation.arguments {
            row.extend([
                anchor(positions, argument.span)?,
                argument.slot.to_string(),
                argument.written_position.to_string(),
                match argument.access {
                    IncanInputAccess::Inferred => "inferred",
                    IncanInputAccess::Shared => "shared",
                    IncanInputAccess::Exclusive => "exclusive",
                    IncanInputAccess::Owned => "owned",
                }
                .into(),
            ]);
            loan_option(argument.loan, &mut row);
            match &argument.source {
                Some(source) => {
                    row.push("some".into());
                    place(positions, source, &mut row)?;
                }
                None => row.push("none".into()),
            }
            input_evaluation(&argument.evaluation, &mut row);
        }
        match &invocation.receiver_evaluation {
            Some(evaluation) => {
                row.push("some".into());
                input_evaluation(evaluation, &mut row);
            }
            None => row.push("none".into()),
        }
        row.push(invocation.completion_node.to_string());
        row.push(invocation.final_accesses.len().to_string());
        row.extend(invocation.final_accesses.iter().map(usize::to_string));
        invocations.push((key, row));
    }
    ordered_fields(invocations, fields)?;
    fields.push(graph.nodes.len().to_string());
    for node in &graph.nodes {
        match &node.event {
            Event::Nothing => fields.push("nothing".into()),
            Event::Borrow(loan) => fields.extend(["borrow".into(), loan.0.to_string()]),
            Event::Access {
                place: source,
                access,
                through,
            } => {
                fields.push("access".into());
                place(positions, source, fields)?;
                fields.push(
                    match access {
                        Access::Read => "read",
                        Access::Write => "write",
                        Access::Move => "move",
                        Access::Replace => "replace",
                    }
                    .into(),
                );
                loan_option(*through, fields);
            }
        }
        fields.push(node.successors.len().to_string());
        fields.extend(node.successors.iter().map(usize::to_string));
        fields.push(anchor(positions, node.span)?);
    }
    Ok(())
}

/// Primitive cursor; bounds are checked before allocation and handles resolve only in the current owner's map.
pub(super) struct Reader<'a> {
    pub positions: &'a Positions,
    pub values: std::slice::Iter<'a, String>,
}

impl Reader<'_> {
    /// Read an exact grammar token, rejecting truncation.
    pub fn token(&mut self) -> Result<&str, NodeBindingError> {
        next(&mut self.values)
    }
    /// Decode canonical decimal syntax independently of machine parse permissiveness.
    pub fn number(&mut self) -> Result<usize, NodeBindingError> {
        let text = self.token()?;
        let number = text.parse::<usize>().map_err(|_| NodeBindingError::MissingNode)?;
        if number.to_string() != text {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(number)
    }
    /// Bound collection lengths by the remaining record fields before constructing native containers.
    pub fn count(&mut self) -> Result<usize, NodeBindingError> {
        let size = self.number()?;
        if size > self.values.len() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(size)
    }
    /// Decode a literal permission bit; no string truthiness is permitted.
    pub fn boolean(&mut self) -> Result<bool, NodeBindingError> {
        match self.token()? {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(NodeBindingError::MissingNode),
        }
    }
    /// Rebind a canonical declaration rather than recovering identity from its display name.
    pub fn identity(&mut self) -> Result<CanonicalSymbolId, NodeBindingError> {
        self.positions.canonical(next(&mut self.values)?)
    }
    /// Resolve a source anchor against current syntax, including expansion provenance.
    pub fn span(&mut self) -> Result<Span, NodeBindingError> {
        self.positions.span(next(&mut self.values)?)
    }
    /// Convert an anchored current span to the graph's shared semantic span representation.
    fn semantic_span(&mut self) -> Result<HirSourceSpan, NodeBindingError> {
        let span = self.span()?;
        Ok(HirSourceSpan {
            start: span.start,
            end: span.end,
            expansion: span.expansion,
        })
    }
    /// Preserve missing loan dependencies independently of zero-valued loan identifiers.
    fn loan_option(&mut self) -> Result<Option<LoanId>, NodeBindingError> {
        match self.token()? {
            "none" => Ok(None),
            "some" => Ok(Some(LoanId(self.number()?))),
            _ => Err(NodeBindingError::MissingNode),
        }
    }
    /// Reconstruct only the two admitted storage-root forms.
    fn root(&mut self) -> Result<Root, NodeBindingError> {
        match self.token()? {
            "binding" => Ok(Root::Binding(self.identity()?)),
            "temporary" => Ok(Root::Temporary {
                owner: self.identity()?,
                expression: self.semantic_span()?,
                invocation: self.semantic_span()?,
            }),
            _ => Err(NodeBindingError::MissingNode),
        }
    }
    /// Reconstruct an exact place including dynamic element ancestry.
    fn place(&mut self) -> Result<Place, NodeBindingError> {
        let root = self.root()?;
        let size = self.count()?;
        let mut projection = Vec::new();
        for _ in 0..size {
            projection.push(match self.token()? {
                "field" => Projection::Field(self.identity()?),
                "element" => Projection::Element(self.semantic_span()?),
                _ => return Err(NodeBindingError::MissingNode),
            });
        }
        Ok(Place { root, projection })
    }
    /// Restore explicit graph-local links without substituting current analysis or source-span guesses.
    fn input_evaluation(&mut self) -> Result<InputEvaluation, NodeBindingError> {
        let start_node = self.number()?;
        let end_node = self.number()?;
        let source_node = match self.token()? {
            "none" => None,
            "some" => Some(self.number()?),
            _ => return Err(NodeBindingError::MissingNode),
        };
        Ok(InputEvaluation {
            start_node,
            end_node,
            source_node,
        })
    }

    /// Restore the full graph without consulting checked current-state payloads. The caller must run analysis.
    pub fn graph(&mut self) -> Result<Graph, NodeBindingError> {
        let owner = self.identity()?;
        let entry = self.number()?;
        let mut permissions = BTreeMap::new();
        let mut previous = String::new();
        for _ in 0..self.count()? {
            let before = self.values.as_slice();
            let root = self.root()?;
            ordered_key(&mut previous, before, self.values.len())?;
            let permission = Permission {
                exclusive: self.boolean()?,
                writable: self.boolean()?,
                transferable: self.boolean()?,
            };
            if permissions.insert(root, permission).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        let mut loans = Vec::new();
        for _ in 0..self.count()? {
            let source = self.place()?;
            let kind = match self.token()? {
                "shared" => LoanKind::Shared,
                "exclusive" => LoanKind::Exclusive,
                _ => return Err(NodeBindingError::MissingNode),
            };
            loans.push(Loan {
                source,
                kind,
                writable: self.boolean()?,
                parent: self.loan_option()?,
                span: self.semantic_span()?,
            });
        }
        let mut local_loans = BTreeMap::new();
        let mut previous = String::new();
        for _ in 0..self.count()? {
            let before = self.values.as_slice();
            let local = self.identity()?;
            ordered_key(&mut previous, before, self.values.len())?;
            let loan = LoanId(self.number()?);
            if local_loans.insert(local, loan).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
        }
        let mut owned_locals = BTreeMap::new();
        let mut previous = String::new();
        for _ in 0..self.count()? {
            let before = self.values.as_slice();
            let destination = self.identity()?;
            ordered_key(&mut previous, before, self.values.len())?;
            let initializer = self.semantic_span()?;
            let source = match self.token()? {
                "none" => None,
                "some" => Some(self.place()?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            let access_node = match self.token()? {
                "none" => None,
                "some" => Some(self.number()?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            if owned_locals
                .insert(
                    destination,
                    OwnedInitialization {
                        initializer,
                        source,
                        access_node,
                    },
                )
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        let mut operations = BTreeMap::new();
        let mut previous = String::new();
        for _ in 0..self.count()? {
            let before = self.values.as_slice();
            let operation = self.semantic_span()?;
            ordered_key(&mut previous, before, self.values.len())?;
            let completion_node = self.number()?;
            let mut operands = Vec::new();
            for _ in 0..self.count()? {
                operands.push(OperandDependency {
                    operand: self.semantic_span()?,
                    source: self.place()?,
                    loan: LoanId(self.number()?),
                    final_access: self.number()?,
                });
            }
            if operations
                .insert(
                    operation,
                    OperationInputs {
                        completion_node,
                        operands,
                    },
                )
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        let mut invocations = BTreeMap::new();
        let mut previous = String::new();
        for _ in 0..self.count()? {
            let before = self.values.as_slice();
            let span = self.semantic_span()?;
            ordered_key(&mut previous, before, self.values.len())?;
            let callee = self.identity()?;
            let receiver = self.loan_option()?;
            let mut arguments = Vec::new();
            for _ in 0..self.count()? {
                let span = self.semantic_span()?;
                let slot = self.number()?;
                let written_position = self.number()?;
                let access = match self.token()? {
                    "inferred" => IncanInputAccess::Inferred,
                    "shared" => IncanInputAccess::Shared,
                    "exclusive" => IncanInputAccess::Exclusive,
                    "owned" => IncanInputAccess::Owned,
                    _ => return Err(NodeBindingError::MissingNode),
                };
                let loan = self.loan_option()?;
                let source = match self.token()? {
                    "none" => None,
                    "some" => Some(self.place()?),
                    _ => return Err(NodeBindingError::MissingNode),
                };
                arguments.push(InvocationArgument {
                    span,
                    slot,
                    written_position,
                    access,
                    loan,
                    source,
                    evaluation: self.input_evaluation()?,
                });
            }
            let receiver_evaluation = match self.token()? {
                "none" => None,
                "some" => Some(self.input_evaluation()?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            let completion_node = self.number()?;
            let mut final_accesses = Vec::new();
            for _ in 0..self.count()? {
                final_accesses.push(self.number()?);
            }
            if invocations
                .insert(
                    span,
                    Invocation {
                        callee,
                        receiver,
                        arguments,
                        receiver_evaluation,
                        completion_node,
                        final_accesses,
                    },
                )
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        let mut nodes = Vec::new();
        for _ in 0..self.count()? {
            let event = match self.token()? {
                "nothing" => Event::Nothing,
                "borrow" => Event::Borrow(LoanId(self.number()?)),
                "access" => {
                    let place = self.place()?;
                    let access = match self.token()? {
                        "read" => Access::Read,
                        "write" => Access::Write,
                        "move" => Access::Move,
                        "replace" => Access::Replace,
                        _ => return Err(NodeBindingError::MissingNode),
                    };
                    Event::Access {
                        place,
                        access,
                        through: self.loan_option()?,
                    }
                }
                _ => return Err(NodeBindingError::MissingNode),
            };
            let mut successors = Vec::new();
            for _ in 0..self.count()? {
                successors.push(self.number()?);
            }
            nodes.push(Node {
                event,
                successors,
                span: self.semantic_span()?,
            });
        }
        Ok(Graph {
            owner,
            entry,
            permissions,
            loans,
            local_loans,
            owned_locals,
            operations,
            invocations,
            nodes,
        })
    }
    /// Require complete consumption, preventing hidden trailing facts from being ignored.
    pub fn finish(&mut self) -> Result<(), NodeBindingError> {
        if self.values.next().is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_key, ordered_fields, ordered_key};

    /// Map keys count UTF-8 bytes and frame embedded separators; readers require the producer's exact ordering.
    #[test]
    fn unicode_map_keys_use_unambiguous_byte_frames() -> Result<(), Box<dyn std::error::Error>> {
        let wide = vec!["é".into(), "a:b".into()];
        let narrow = vec!["z".into(), "a:b".into()];
        assert_eq!(canonical_key(&wide), "2:é3:a:b");
        assert_eq!(canonical_key(&narrow), "1:z3:a:b");
        assert_ne!(
            canonical_key(&["a".into(), "b".into()]),
            canonical_key(&["a1:b".into()])
        );
        let mut fields = Vec::new();
        ordered_fields(
            vec![
                (canonical_key(&wide), wide.clone()),
                (canonical_key(&narrow), narrow.clone()),
            ],
            &mut fields,
        )?;
        assert_eq!(fields, vec!["2", "z", "a:b", "é", "a:b"]);
        let mut previous = String::new();
        ordered_key(&mut previous, &narrow, 0)?;
        ordered_key(&mut previous, &wide, 0)?;
        assert!(ordered_key(&mut previous, &wide, 0).is_err());
        assert!(ordered_key(&mut previous, &narrow, 0).is_err());
        Ok(())
    }
}
