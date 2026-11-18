//! Bounded transport for graph-selected checked owners and editable namespaces.

use crate::CodecFault::{self, BodyChanged, Limit, Malformed};
use crate::checker_records::{CHECKER_PROFILE, CheckedFamily, CheckedRecord, MAX_FIELDS, decode_owned_record};
use crate::crumb_rows::{Rows, Writer};
use crate::row_shapes::{GRAPH_BODY, GRAPH_NAMESPACE};

pub(crate) const MAX_TRANSPORT: i64 = 67_108_864;

/// One graph-selected owner's checker values and opaque kernel-owned executable frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphBodyValue {
    pub identity: String,
    pub local: Vec<u8>,
    pub r#final: Vec<u8>,
    pub body_ir: Vec<u8>,
}
/// One validated graph-selected owner projected into native checker fields plus its opaque Body IR frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphBodyFields {
    pub identity: String,
    pub local: Vec<Vec<String>>,
    pub final_fields: Vec<Vec<String>>,
    pub body_ir: Vec<u8>,
}
/// One graph-selected editable namespace with its interface, Body IR and authenticated lowering witnesses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNamespaceValue {
    pub module: String,
    pub interface: Vec<u8>,
    pub body_ir: Vec<u8>,
    pub lowering_inputs: Vec<Vec<String>>,
}

/// Require nonempty identity and witness pairs in strictly increasing identity order.
pub(crate) fn valid_pairs(inputs: &[Vec<String>]) -> Result<(), CodecFault> {
    let mut previous = "";
    for input in inputs {
        let [identity, witness] = input.as_slice() else {
            return Err(Malformed);
        };
        if identity.as_str() <= previous || witness.is_empty() {
            return Err(Malformed);
        }
        previous = identity;
    }
    Ok(())
}
/// Move alternating identity and witness strings from the wire list into pairs.
pub(crate) fn read_pairs(tokens: Vec<String>) -> Result<Vec<Vec<String>>, CodecFault> {
    if tokens.len() % 2 != 0 {
        return Err(Malformed);
    }
    let mut inputs = Vec::with_capacity(tokens.len() / 2);
    let mut it = tokens.into_iter();
    while let Some(identity) = it.next() {
        inputs.push(vec![identity, it.next().ok_or(Malformed)?]);
    }
    valid_pairs(&inputs)?;
    Ok(inputs)
}
/// Write authenticated identity and witness pairs as a flat text list.
pub(crate) fn write_pairs(out: &mut Writer, inputs: &[Vec<String>]) -> Result<(), CodecFault> {
    out.list(b"inputs", inputs.len() * 2)?;
    for s in inputs.iter().flatten() {
        out.text_node(s.to_owned());
    }
    Ok(())
}
/// Authenticate a local and a final record under their shared owner and semantic root.
fn checked_pair(
    identity: &str,
    local: &[u8],
    settled: &[u8],
    body_ir: &[u8],
) -> Result<(CheckedRecord, CheckedRecord), CodecFault> {
    if body_ir.is_empty() {
        return Err(Malformed);
    }
    let local = decode_owned_record(local, CHECKER_PROFILE, identity)?;
    let settled = decode_owned_record(settled, CHECKER_PROFILE, identity)?;
    if local.family != CheckedFamily::Local || settled.family != CheckedFamily::Final {
        return Err(Malformed);
    }
    if local.root != settled.root {
        return Err(BodyChanged);
    }
    Ok((local, settled))
}
/// Move the decoded checker strings directly into their native projection.
fn into_fields(record: CheckedRecord) -> Vec<Vec<String>> {
    let mut rows = Vec::with_capacity(1 + record.fields.len());
    rows.push(vec![
        "header".into(),
        record.owner,
        record.root,
        record.family.name().into(),
        record.profile,
    ]);
    for field in record.fields {
        let mut row = Vec::with_capacity(3 + field.values.len());
        row.extend([field.family, field.node, field.selector]);
        row.extend(field.values);
        rows.push(row);
    }
    rows
}

/// Authenticate a single unframed graph owner and expose its native checker fields.
pub fn decode_graph_body_page(
    identity: &str,
    local: &[u8],
    settled: &[u8],
    body_ir: &[u8],
) -> Result<GraphBodyFields, CodecFault> {
    let (local, settled) = checked_pair(identity, local, settled, body_ir)?;
    Ok(GraphBodyFields {
        identity: identity.to_owned(),
        local: into_fields(local),
        final_fields: into_fields(settled),
        body_ir: body_ir.to_vec(),
    })
}
/// Write graph-selected body pairs in canonical owner order.
pub fn encode_graph_body_values(values: &[GraphBodyValue]) -> Result<Vec<u8>, CodecFault> {
    if values.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut out = Writer::new(&GRAPH_BODY, MAX_TRANSPORT)?;
    let mut previous = "";
    for v in values {
        if v.identity.as_str() <= previous {
            return Err(Malformed);
        }
        checked_pair(&v.identity, &v.local, &v.r#final, &v.body_ir)?;
        out.record();
        out.binary(b"bodyir", &v.body_ir);
        out.binary(b"final_", &v.r#final);
        out.text(b"ident_", &v.identity);
        out.binary(b"local_", &v.local);
        previous = &v.identity;
    }
    out.finish()
}

/// Restore and authenticate every owner from a single graph selection.
pub fn decode_graph_body_values(encoded: &[u8]) -> Result<Vec<GraphBodyFields>, CodecFault> {
    let mut rows = Rows::admit(encoded, &[&GRAPH_BODY], MAX_TRANSPORT)?;
    if rows.len() > MAX_FIELDS as i64 {
        rows.remaining_reads(0)?;
        return Err(Limit);
    }
    let mut selected: Vec<GraphBodyFields> = Vec::with_capacity(rows.len() as usize);
    for i in 0..rows.len() {
        let (key, mut fields) = rows.row(i)?;
        if key != GRAPH_BODY.selector {
            return Err(Malformed);
        }
        fields.count(4)?;
        let body_ir = fields.binary()?;
        let settled = fields.binary()?;
        let identity = fields.text()?;
        let local = fields.binary()?;
        if identity.is_empty() || selected.last().is_some_and(|v| v.identity >= identity) {
            return Err(Malformed);
        }
        let (local, settled) = match checked_pair(&identity, &local, &settled, &body_ir) {
            Ok(pair) => pair,
            Err(Malformed) => return Err(Malformed),
            Err(reason) => {
                rows.remaining_reads(i + 1)?;
                return Err(reason);
            }
        };
        selected.push(GraphBodyFields {
            identity,
            local: into_fields(local),
            final_fields: into_fields(settled),
            body_ir,
        });
    }
    Ok(selected)
}

/// Encode one graph-selected namespace and its lowering witnesses.
pub fn encode_graph_namespace_value(v: &GraphNamespaceValue) -> Result<Vec<u8>, CodecFault> {
    if v.module.is_empty() || v.interface.is_empty() || v.body_ir.is_empty() || v.lowering_inputs.len() > MAX_FIELDS {
        return Err(Malformed);
    }
    valid_pairs(&v.lowering_inputs)?;
    let mut out = Writer::new(&GRAPH_NAMESPACE, MAX_TRANSPORT)?;
    out.record();
    out.binary(b"bodyir", &v.body_ir);
    out.binary(b"iface_", &v.interface);
    write_pairs(&mut out, &v.lowering_inputs)?;
    out.text(b"module", &v.module);
    out.finish()
}

/// Admit a namespace row and restore the checked interface and opaque body bytes.
pub fn decode_graph_namespace_value(encoded: &[u8]) -> Result<GraphNamespaceValue, CodecFault> {
    let mut rows = Rows::admit(encoded, &[&GRAPH_NAMESPACE], MAX_TRANSPORT)?;
    if rows.len() != 1 {
        return Err(Malformed);
    }
    let (key, mut fields) = rows.row(0)?;
    if key != GRAPH_NAMESPACE.selector {
        return Err(Malformed);
    }
    fields.count(4)?;
    let body_ir = fields.binary()?;
    let interface = fields.binary()?;
    let lowering_inputs = read_pairs(fields.texts()?)?;
    let module = fields.text()?;
    if module.is_empty() || interface.is_empty() || body_ir.is_empty() {
        return Err(Malformed);
    }
    Ok(GraphNamespaceValue {
        module,
        interface,
        body_ir,
        lowering_inputs,
    })
}
