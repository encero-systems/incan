//! Checked input protocol adapters. Every serialized fact is restored from fields, with current handles used only for
//! rebinding.

use super::input_flow::{self, Reader};
use super::types::{parameter_fields, read_parameters, read_type, type_fields};
use super::{BodyLocal, Fields, NodeBindingError, Positions, span_of};
use crate::ast::Receiver;
use crate::typechecker::ownership_contracts::{InputCall, InputCallKind, InputFlow, LocalMode, Unavailable, flow};
use incan_lang::lang::types::ownership;
use std::collections::HashMap;

#[cfg(test)]
#[path = "input_restore_tests.rs"]
mod tests;

/// Persist calls, local modes and whole control-flow graphs independently of declaration ownership metadata.
pub(super) fn project(positions: &Positions, local: &BodyLocal, fields: &mut Fields) -> Result<(), NodeBindingError> {
    for (owner, decisions) in &local.protocols.explicit_input_copy {
        let mut row = vec![
            "input-copy".into(),
            positions.nodes.anchor(span_of(owner))?.path,
            "types".into(),
            "1".into(),
            positions.identity(owner)?,
        ];
        let mut entries = Vec::with_capacity(decisions.len());
        for (ty, supported) in decisions {
            let mut entry = Vec::new();
            type_fields(ty, &mut entry)?;
            let key = input_flow::canonical_key(&entry);
            entry.push(supported.to_string());
            entries.push((key, entry));
        }
        input_flow::ordered_fields(entries, &mut row)?;
        fields.push(row);
    }
    for (span, call) in &local.protocols.explicit_input_calls {
        let mut row = vec![
            "input-call".into(),
            positions.nodes.anchor(*span)?.path,
            "contract".into(),
            "1".into(),
        ];
        row.push(
            match call.kind {
                InputCallKind::Explicit => "explicit",
                InputCallKind::Ordinary => "ordinary",
                InputCallKind::Indirect => "indirect",
                // A native call is rebuilt from its checked native call on restore; its callee is no declaration.
                InputCallKind::Native => continue,
            }
            .into(),
        );
        match &call.owner {
            Some(owner) => row.extend(["some".into(), positions.identity(owner)?]),
            None => row.push("none".into()),
        }
        row.push(positions.identity(&call.callee)?);
        row.push(
            match call.receiver {
                None => "none",
                Some(Receiver::Immutable) => "shared",
                Some(Receiver::Mutable) => "exclusive",
            }
            .into(),
        );
        parameter_fields(&call.parameters, &mut row)?;
        fields.push(row);
    }
    for (binding, mode) in &local.protocols.explicit_local_modes {
        let mut row = vec![
            "input-local".into(),
            positions.nodes.anchor(span_of(binding))?.path,
            "mode".into(),
            "1".into(),
            positions.identity(binding)?,
            positions.identity(&mode.owner)?,
            mode.mode.as_str().into(),
            mode.writable.to_string(),
            positions.nodes.anchor(mode.annotation)?.path,
        ];
        type_fields(&mode.value_type, &mut row)?;
        fields.push(row);
    }
    for (owner, flow) in &local.protocols.explicit_input_flow {
        let mut row = vec![
            "input-flow".into(),
            positions.nodes.anchor(span_of(owner))?.path,
            "graph".into(),
            "2".into(),
        ];
        match flow {
            InputFlow::Checked { graph, analysis } => {
                if &graph.owner != owner || flow::analyze(graph).as_ref() != Ok(analysis) {
                    return Err(NodeBindingError::InvalidField(
                        "ownership graph analysis mismatch".into(),
                    ));
                }
                row.push("checked".into());
                input_flow::project(positions, graph, &mut row)?;
            }
            InputFlow::Unavailable(reason) => row.extend([
                "unavailable".into(),
                positions.identity(owner)?,
                positions.nodes.anchor(reason.span)?.path,
                reason.reason.clone(),
            ]),
            InputFlow::Invalid { graph, fault } => {
                if &graph.owner != owner || flow::analyze(graph).as_ref().err() != Some(fault) {
                    return Err(NodeBindingError::InvalidField(
                        "ownership graph refusal mismatch".into(),
                    ));
                }
                row.push("invalid".into());
                input_flow::project(positions, graph, &mut row)?;
                row.push(fault.0.into());
            }
        }
        fields.push(row);
    }
    Ok(())
}

/// Decode one complete ownership protocol row. Neither missing owners nor duplicate rows can introduce authority.
pub(super) fn restore(positions: &Positions, local: &mut BodyLocal, row: &[String]) -> Result<(), NodeBindingError> {
    let anchor = positions.span(&row[1])?;
    let schema = match row.get(3).map(String::as_str) {
        Some("1") => 1,
        Some("2") => 2,
        _ => return Err(NodeBindingError::Unsupported("input record schema")),
    };
    let mut reader = Reader {
        positions,
        values: row[4..].iter(),
    };
    match (row[0].as_str(), row[2].as_str()) {
        ("input-copy", "types") => {
            let owner = reader.identity()?;
            if span_of(&owner) != anchor || local.protocols.explicit_input_copy.contains_key(&owner) {
                return Err(NodeBindingError::WrongOwner);
            }
            let mut decisions = HashMap::new();
            let mut previous = String::new();
            for _ in 0..reader.count()? {
                let before = reader.values.as_slice();
                let ty = read_type(&mut reader.values)?;
                let mut canonical = Vec::new();
                type_fields(&ty, &mut canonical)?;
                let consumed = before.len() - reader.values.len();
                if canonical.as_slice() != &before[..consumed] {
                    return Err(NodeBindingError::InvalidField("noncanonical checked Copy type".into()));
                }
                input_flow::ordered_key(&mut previous, before, reader.values.len())?;
                let supported = reader.boolean()?;
                if decisions.insert(ty, supported).is_some() {
                    return Err(NodeBindingError::InvalidField("duplicate checked Copy type".into()));
                }
            }
            reader.finish()?;
            local.protocols.explicit_input_copy.insert(owner, decisions);
        }
        ("input-call", "contract") => {
            let kind = match reader.token()? {
                "explicit" => InputCallKind::Explicit,
                "ordinary" => InputCallKind::Ordinary,
                "indirect" => InputCallKind::Indirect,
                _ => return Err(NodeBindingError::MissingNode),
            };
            let owner = match reader.token()? {
                "none" => None,
                "some" => Some(reader.identity()?),
                _ => return Err(NodeBindingError::MissingNode),
            };
            let callee = reader.identity()?;
            let receiver = match reader.token()? {
                "none" => None,
                "shared" => Some(Receiver::Immutable),
                "exclusive" => Some(Receiver::Mutable),
                _ => return Err(NodeBindingError::MissingNode),
            };
            let parameters = read_parameters(&mut reader.values)?;
            if local
                .protocols
                .explicit_input_calls
                .insert(
                    anchor,
                    InputCall {
                        site: anchor,
                        kind,
                        owner,
                        callee,
                        receiver,
                        parameters,
                    },
                )
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        ("input-local", "mode") => {
            let binding = reader.identity()?;
            if span_of(&binding) != anchor {
                return Err(NodeBindingError::WrongOwner);
            }
            let owner = reader.identity()?;
            let name = reader.token()?;
            let mode = ownership::from_str(name)
                .filter(|mode| mode.as_str() == name)
                .ok_or(NodeBindingError::MissingNode)?;
            let writable = reader.boolean()?;
            let annotation = reader.span()?;
            let value_type = read_type(&mut reader.values)?;
            if local
                .protocols
                .explicit_local_modes
                .insert(
                    binding,
                    LocalMode {
                        owner,
                        mode,
                        value_type,
                        writable,
                        annotation,
                    },
                )
                .is_some()
            {
                return Err(NodeBindingError::MissingNode);
            }
        }
        ("input-flow", "graph") => {
            let (owner, flow) = match reader.token()? {
                "unavailable" => {
                    let owner = reader.identity()?;
                    let span = reader.span()?;
                    let reason = reader.token()?.to_owned();
                    if reason.is_empty() {
                        return Err(NodeBindingError::MissingNode);
                    }
                    (owner, InputFlow::Unavailable(Unavailable { span, reason }))
                }
                state @ ("checked" | "invalid") => {
                    let checked = state == "checked";
                    let graph = reader.graph(schema)?;
                    let owner = graph.owner.clone();
                    let analysis = flow::analyze(&graph);
                    let flow = if checked {
                        InputFlow::Checked {
                            graph,
                            analysis: analysis.map_err(|fault| NodeBindingError::InvalidField(fault.0.into()))?,
                        }
                    } else {
                        let fault = analysis.err().ok_or(NodeBindingError::MissingNode)?;
                        if reader.token()? != fault.0 {
                            return Err(NodeBindingError::MissingNode);
                        }
                        InputFlow::Invalid { graph, fault }
                    };
                    (owner, flow)
                }
                _ => return Err(NodeBindingError::MissingNode),
            };
            if span_of(&owner) != anchor || local.protocols.explicit_input_flow.insert(owner, flow).is_some() {
                return Err(NodeBindingError::WrongOwner);
            }
        }
        _ => return Err(NodeBindingError::MissingNode),
    }
    reader.finish()
}
