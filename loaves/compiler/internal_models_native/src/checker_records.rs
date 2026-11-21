//! Canonical field records for the initial free-function checker profile; native objects never enter the codec.

pub use crate::checker_field_grammar::{CanonicalField, CodecFault};
use crate::checker_field_grammar::{allocated_scope_index, decimal, full_digest, validate_field};
pub use crate::checker_record_models::{
    CheckedFamily, CheckedRecord, CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutputs,
    CompiledBodyValue, CompiledNamespaceValue, GraphBodyFields, GraphBodyValue, GraphNamespaceValue,
    GraphNamespaceView, SettledCallRead,
};
use crate::crumb_rows::{
    ModelField, admit_rows_view, decode_rows, encode_rows, model_description, operation_budget, required_value,
};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row};
use crate::hex_text::hexlify;
use crate::record_scope::MAX_CHECKER_ROW_BYTES;
use crate::settled_call_reads::{call_read_values, decoded_call_reads};
use CodecFault::{BodyChanged, Limit, LimitExceeded, Malformed, OwnerMismatch, ProfileMismatch, Unsupported};
use internal_models_bindings::{
    Description, MappedView, Projection, RowRecord, Value, binary_field_at, complete_view, description_hash,
    mapped_bytes, read_field_at, row_count, row_field_count, row_key, value_hash, view_bytes, view_len,
};
use sha2::{Digest, Sha256};

pub const CHECKER_SCHEMA: &str = "incan.checker-fields/11";
pub const CHECKER_PROFILE: &str = "incan.checker-free-functions/21";
pub(crate) const MAX_FIELDS: usize = 4096;
const CHECKER_SELECTOR: &[u8] = b"checkr\x00";
const GRAPH_BODY_SELECTOR: &[u8] = b"grbody\x00";
const GRAPH_CHECKING_SELECTOR: &[u8] = b"grchec\x00";
const PRIOR_GRAPH_NAMESPACE_SELECTOR: &[u8] = b"grnam2\x00";
const CALLS_GRAPH_NAMESPACE_SELECTOR: &[u8] = b"grnam3\x00";
const GRAPH_NAMESPACE_SELECTOR: &[u8] = b"grnam4\x00";
const MAX_PROOF_BYTES: usize = 65_536;

const FAMILIES: [&str; 31] = [
    "type",
    "type-identity",
    "identity-context",
    "binding",
    "scope",
    "binding-type",
    "reference",
    "write",
    "builtin",
    "constructor",
    "requirement",
    "parameter",
    "signature",
    "diagnostic",
    "status",
    "default",
    "final-type",
    "native-call",
    "native-value",
    "artifact-call",
    "artifact-protocol",
    "artifact-rust",
    "target",
    "identifier",
    "effect",
    "mutation",
    "ownership",
    "input-call",
    "input-local",
    "input-flow",
    "input-copy",
];

/// Encode one checked owner as one bounded Crumb row of the checker-record model.
pub fn encode_record(record: &CheckedRecord) -> Result<Vec<u8>, CodecFault> {
    let logical = record_value(record)?;
    let row = RowRecord {
        key: CHECKER_SELECTOR.to_vec(),
        value: logical,
    };
    encode_rows(&checker_description(), vec![row], MAX_CHECKER_ROW_BYTES).map_err(CodecFault::Encoding)
}

/// Decode one complete Crumb row, then bind its profile, stable owner and semantic root.
pub fn decode_record(encoded: &[u8], profile: &str, owner: &str, root: &str) -> Result<CheckedRecord, CodecFault> {
    let record = decode_owned_record(encoded, profile, owner)?;
    if record.root != root {
        return Err(BodyChanged);
    }
    Ok(record)
}

/// Decode one complete Crumb row once and bind its profile and stable owner; it keeps the root its row carries.
pub fn decode_owned_record(encoded: &[u8], profile: &str, owner: &str) -> Result<CheckedRecord, CodecFault> {
    let record = decoded_record(encoded)?;
    if record.profile != profile {
        return Err(ProfileMismatch);
    }
    if record.owner != owner {
        return Err(OwnerMismatch);
    }
    Ok(record)
}

/// Hash every encoded record byte, including the model reference and fields excluded from semantic reuse.
pub fn content_digest(record: &CheckedRecord) -> Result<String, CodecFault> {
    Ok(format!("sha256:{}", hexlify(&Sha256::digest(encode_record(record)?))))
}

/// Hash the model description and canonical fields, excluding documentation and publication-only fields.
pub fn semantic_digest(record: &CheckedRecord) -> Result<String, CodecFault> {
    let mut budget = operation_budget(0).map_err(|_| Limit)?;
    let value = value_hash(record_value(record)?, &Projection::Canonical, &mut budget, false).map_err(|_| Malformed)?;
    let description = description_hash(&checker_description().metadata, &mut budget, false).map_err(|_| Malformed)?;
    let preimage = format!("{}\n{}", hexlify(&description.digest), hexlify(&value.digest));
    Ok(format!("sha256:{}", hexlify(&Sha256::digest(preimage.as_bytes()))))
}

/// Project one validated checker record into its exact Crumb logical model.
fn record_value(record: &CheckedRecord) -> Result<Value, CodecFault> {
    validate(record)?;
    let mut flattened = Vec::with_capacity(record.fields.iter().map(|field| 4 + field.values.len()).sum());
    for field in &record.fields {
        flattened.extend([
            Value::Text(field.family.clone()),
            Value::Text(field.node.clone()),
            Value::Text(field.selector.clone()),
            Value::Text(field.values.len().to_string()),
        ]);
        flattened.extend(field.values.iter().cloned().map(Value::Text));
    }
    Ok(Value::Record(vec![
        required_value(b"family", Value::Text(family_name(record.family).to_owned())),
        required_value(b"fields", Value::List(flattened)),
        required_value(b"owner_", Value::Text(record.owner.clone())),
        required_value(b"profil", Value::Text(record.profile.clone())),
        required_value(b"root__", Value::Text(record.root.clone())),
    ]))
}

/// Admit exactly one checker-model row and reconstruct all schema-owned fields.
fn decoded_record(encoded: &[u8]) -> Result<CheckedRecord, CodecFault> {
    if encoded.len() as i64 > MAX_CHECKER_ROW_BYTES {
        return Err(LimitExceeded(
            "checker-record-bytes".into(),
            encoded.len() as i64,
            MAX_CHECKER_ROW_BYTES,
        ));
    }
    let rows = decode_rows(encoded, &vec![checker_description()], MAX_CHECKER_ROW_BYTES).map_err(|_| Malformed)?;
    let [row] = rows.as_slice() else {
        return Err(Malformed);
    };
    if row.key != CHECKER_SELECTOR {
        return Err(Malformed);
    }
    let Value::Record(values) = &row.value else {
        return Err(Malformed);
    };
    if values.len() != 5 {
        return Err(Malformed);
    }
    let family = text_value(&values[0].value)?;
    let flattened = text_values(&values[1].value)?;
    let record = CheckedRecord {
        profile: text_value(&values[3].value)?,
        owner: text_value(&values[2].value)?,
        root: text_value(&values[4].value)?,
        family: read_family(&family)?,
        fields: decoded_fields(flattened)?,
    };
    validate(&record)?;
    Ok(record)
}

/// Restore the ordered checker-field list from the Crumb list payload.
fn decoded_fields(values: Vec<String>) -> Result<Vec<CanonicalField>, CodecFault> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < values.len() {
        if offset + 4 > values.len() {
            return Err(Malformed);
        }
        let count = decimal(&values[offset + 3])? as usize;
        if count > MAX_FIELDS || offset + 4 + count > values.len() {
            return Err(Malformed);
        }
        fields.push(CanonicalField {
            family: values[offset].clone(),
            node: values[offset + 1].clone(),
            selector: values[offset + 2].clone(),
            values: values[offset + 4..offset + 4 + count].to_vec(),
        });
        offset += 4 + count;
    }
    if fields.len() > MAX_FIELDS {
        return Err(Limit);
    }
    Ok(fields)
}

/// Require one logical Crumb string value.
fn text_value(value: &Value) -> Result<String, CodecFault> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err(Malformed),
    }
}

/// Require one logical Crumb string list without accepting nested or alternate values.
fn text_values(value: &Value) -> Result<Vec<String>, CodecFault> {
    match value {
        Value::List(items) => items.iter().map(text_value).collect(),
        _ => Err(Malformed),
    }
}

/// Declare the complete checker-record model while keeping its repeated wire header referenced.
fn checker_description() -> Description {
    model_description(
        CHECKER_SELECTOR,
        CHECKER_SCHEMA,
        &[
            ModelField::new(b"family", "family", "string"),
            ModelField::new(b"fields", "fields", "list<string>"),
            ModelField::new(b"owner_", "owner", "string"),
            ModelField::new(b"profil", "profile", "string"),
            ModelField::new(b"root__", "root", "string"),
        ],
    )
}

/// Require one logical Crumb byte-string value.
fn binary_value(value: &Value) -> Result<Vec<u8>, CodecFault> {
    match value {
        Value::Binary(data) => Ok(data.clone()),
        _ => Err(Malformed),
    }
}

/// Describe one graph-selected checked owner while retaining Body IR as opaque kernel bytes.
fn graph_body_description() -> Description {
    model_description(
        GRAPH_BODY_SELECTOR,
        "incan.checker-graph-body/1",
        &[
            ModelField::new(b"bodyir", "body_ir", "binary"),
            ModelField::new(b"final_", "final", "binary"),
            ModelField::new(b"ident_", "identity", "string"),
            ModelField::new(b"local_", "local", "binary"),
        ],
    )
}

/// A settled checking pair permits Body IR production from the current graph revision.
fn graph_checking_description() -> Description {
    model_description(
        GRAPH_CHECKING_SELECTOR,
        "incan.checker-graph-pair/1",
        &[
            ModelField::new(b"final_", "final", "binary"),
            ModelField::new(b"ident_", "identity", "string"),
            ModelField::new(b"local_", "local", "binary"),
        ],
    )
}

/// Read older graph rows without granting current executable-call admission. The reference default version is 4.
fn graph_namespace_description(version: i64) -> Description {
    let mut fields = vec![ModelField::new(b"bodyir", "body_ir", "binary")];
    if version >= 3 {
        fields.push(ModelField::new(
            b"calls_",
            "call_reads",
            "list<tuple<string,string,string>>",
        ));
    }
    fields.extend([
        ModelField::new(b"iface_", "interface", "binary"),
        ModelField::new(b"inputs", "lowering_inputs", "list<string>"),
        ModelField::new(b"module", "module", "string"),
        ModelField::new(b"roots_", "package_roots", "list<string>"),
    ]);
    match version {
        2 => model_description(
            PRIOR_GRAPH_NAMESPACE_SELECTOR,
            "incan.checker-graph-namespace/2",
            &fields,
        ),
        3 => model_description(
            CALLS_GRAPH_NAMESPACE_SELECTOR,
            "incan.checker-graph-namespace/3",
            &fields,
        ),
        _ => {
            fields.push(ModelField::new(b"zproof", "identity_proof", "binary"));
            model_description(GRAPH_NAMESPACE_SELECTOR, "incan.checker-graph-namespace/4", &fields)
        }
    }
}

/// Reject unknown capabilities and duplicate/unsorted fields; an empty supported family remains explicit.
pub(crate) fn validate(record: &CheckedRecord) -> Result<(), CodecFault> {
    if record.profile != CHECKER_PROFILE {
        return Err(ProfileMismatch);
    }
    if record.owner.is_empty() || !full_digest(&record.root) {
        return Err(Malformed);
    }
    if record.fields.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut allocation_count = -1;
    let mut largest_binding_scope = -1;
    let mut previous: (&str, &str, &str) = ("", "", "");
    for field in &record.fields {
        // Field names are versioned schema vocabulary, never native field offsets or Debug spellings.
        if !FAMILIES.contains(&field.family.as_str()) {
            return Err(Unsupported(field.family.clone()));
        }
        let (family, node, selector) = (field.family.as_str(), field.node.as_str(), field.selector.as_str());
        if family < previous.0
            || (family == previous.0 && node < previous.1)
            || (family == previous.0 && node == previous.1 && selector <= previous.2)
            || field.values.len() > MAX_FIELDS
        {
            return Err(Malformed);
        }
        validate_field(field)?;
        if family == "scope" {
            if record.family == CheckedFamily::Declaration {
                return Err(Malformed);
            }
            allocation_count = allocated_scope_index(field)?;
        } else if family == "binding" && field.values.len() == 2 {
            largest_binding_scope = largest_binding_scope.max(allocated_scope_index(field)?);
        }
        previous = (family, node, selector);
    }
    if largest_binding_scope > allocation_count {
        return Err(Malformed);
    }
    Ok(())
}

/// The persistent stage tag is independent of compiler enum discriminants.
fn family_name(family: CheckedFamily) -> &'static str {
    match family {
        CheckedFamily::Declaration => "declaration",
        CheckedFamily::Local => "local",
        CheckedFamily::Final => "final",
    }
}

/// Unknown stage tags require a new admitted codec profile.
fn read_family(value: &str) -> Result<CheckedFamily, CodecFault> {
    match value {
        "declaration" => Ok(CheckedFamily::Declaration),
        "local" => Ok(CheckedFamily::Local),
        "final" => Ok(CheckedFamily::Final),
        _ => Err(Unsupported(value.to_owned())),
    }
}

/// Split native rows of at least four primitives into canonical fields.
fn native_rows(rows: &[Vec<String>]) -> Result<Vec<CanonicalField>, CodecFault> {
    rows.iter()
        .map(|row| {
            if row.len() < 4 {
                return Err(Malformed);
            }
            Ok(CanonicalField {
                family: row[0].clone(),
                node: row[1].clone(),
                selector: row[2].clone(),
                values: row[3..].to_vec(),
            })
        })
        .collect()
}

/// Convert native field primitives into the schema; this boundary never accepts native dump bytes.
pub fn from_native_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    let Some(header) = rows.first() else {
        return Err(Malformed);
    };
    if header.len() != 5 || header[0] != "header" {
        return Err(Malformed);
    }
    let fields = native_rows(&rows[1..])?;
    let record = CheckedRecord {
        profile: header[4].clone(),
        owner: header[1].clone(),
        root: header[2].clone(),
        family: read_family(&header[3])?,
        fields,
    };
    validate(&record)?;
    Ok(record)
}

/// Assign the schema-owned pending family to native owner/root/profile metadata and canonical field primitives.
pub fn from_native_local_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    from_family_fields(rows, CheckedFamily::Local)
}

/// Assign the schema-owned settled family to native owner/root/profile metadata and canonical field primitives.
pub fn from_native_final_fields(rows: &[Vec<String>]) -> Result<CheckedRecord, CodecFault> {
    from_family_fields(rows, CheckedFamily::Final)
}

/// Attach or verify one family on fresh producer rows or a decoded graph value retained for re-publication.
fn from_family_fields(rows: &[Vec<String>], family: CheckedFamily) -> Result<CheckedRecord, CodecFault> {
    let Some(header) = rows.first() else {
        return Err(Malformed);
    };
    if header.is_empty() || header[0] != "header" {
        return Err(Malformed);
    }
    let profile = match header.len() {
        4 => header[3].clone(),
        5 if header[3] == family_name(family) => header[4].clone(),
        _ => return Err(Malformed),
    };
    let fields = native_rows(&rows[1..])?;
    let record = CheckedRecord {
        profile,
        owner: header[1].clone(),
        root: header[2].clone(),
        family,
        fields,
    };
    validate(&record)?;
    Ok(record)
}

/// Project decoded canonical fields for the narrow rebinding adapter, retaining every field and ordered value.
pub fn native_fields(record: &CheckedRecord) -> Result<Vec<Vec<String>>, CodecFault> {
    validate(record)?;
    let mut rows = vec![vec![
        "header".to_owned(),
        record.owner.clone(),
        record.root.clone(),
        family_name(record.family).to_owned(),
        record.profile.clone(),
    ]];
    for field in &record.fields {
        let mut row = vec![field.family.clone(), field.node.clone(), field.selector.clone()];
        row.extend(field.values.iter().cloned());
        rows.push(row);
    }
    Ok(rows)
}

/// Restore the scalar capture producer's original implicit local/final header after validating its typed family.
pub fn callable_checker_fields(record: &CheckedRecord) -> Result<Vec<Vec<String>>, CodecFault> {
    let mut rows = native_fields(record)?;
    if record.family != CheckedFamily::Declaration {
        rows[0] = vec![
            "header".to_owned(),
            record.owner.clone(),
            record.root.clone(),
            record.profile.clone(),
        ];
    }
    Ok(rows)
}

/// Encode graph-selected body pairs independently of the unit description and callable-body product cache.
///
/// Each pair is decoded before framing, so an owner mismatch, changed body root or crossed local/final family
/// cannot enter the transport. Strict owner order gives equivalent selections one canonical byte representation.
pub fn encode_graph_body_values(values: &[GraphBodyValue]) -> Result<Vec<u8>, CodecFault> {
    if values.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut rows = Vec::with_capacity(values.len());
    let mut previous = "";
    for value in values {
        if value.identity.is_empty() || value.identity.as_str() <= previous {
            return Err(Malformed);
        }
        validate_graph_body_value(value)?;
        rows.push(RowRecord {
            key: GRAPH_BODY_SELECTOR.to_vec(),
            value: Value::Record(vec![
                required_value(b"bodyir", Value::Binary(value.body_ir.clone())),
                required_value(b"final_", Value::Binary(value.r#final.clone())),
                required_value(b"ident_", Value::Text(value.identity.clone())),
                required_value(b"local_", Value::Binary(value.local.clone())),
            ]),
        });
        previous = &value.identity;
    }
    encode_rows(&graph_body_description(), rows, MAX_CHECKER_ROW_BYTES).map_err(|_| Limit)
}

/// Publish completed checking work as one typed input for an independent Body IR evaluation.
pub fn encode_graph_checking_pair(identity: &str, local: &[u8], r#final: &[u8]) -> Result<Vec<u8>, CodecFault> {
    checked_pair(identity, local, r#final)?;
    encode_graph_row(
        GRAPH_CHECKING_SELECTOR,
        &graph_checking_description(),
        vec![
            required_value(b"final_", Value::Binary(r#final.to_vec())),
            required_value(b"ident_", Value::Text(identity.to_owned())),
            required_value(b"local_", Value::Binary(local.to_vec())),
        ],
    )
    .map_err(CodecFault::Encoding)
}

/// Accept only a complete graph body or one separately typed settled pair ready for Body IR.
pub fn decode_graph_checking_inputs(encoded: &[u8]) -> Result<Vec<GraphBodyFields>, CodecFault> {
    if let Ok(complete) = decode_graph_body_values(encoded) {
        return Ok(complete);
    }
    let fields =
        decode_graph_row(encoded, GRAPH_CHECKING_SELECTOR, &graph_checking_description(), 3).map_err(|_| Malformed)?;
    let identity = text_value(&fields[1].value)?;
    let [local, settled] = checked_pair(
        &identity,
        &binary_value(&fields[2].value)?,
        &binary_value(&fields[0].value)?,
    )?;
    Ok(vec![GraphBodyFields {
        identity,
        local: native_fields(&local)?,
        final_fields: native_fields(&settled)?,
        body_ir: Vec::new(),
    }])
}

/// Decode and authenticate every selected body pair before projecting its canonical native field primitives.
pub fn decode_graph_body_values(encoded: &[u8]) -> Result<Vec<GraphBodyFields>, CodecFault> {
    let rows = decode_rows(encoded, &vec![graph_body_description()], MAX_CHECKER_ROW_BYTES).map_err(|_| Malformed)?;
    if rows.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut selected = Vec::with_capacity(rows.len());
    let mut previous = String::new();
    for row in rows {
        if row.key != GRAPH_BODY_SELECTOR {
            return Err(Malformed);
        }
        let Value::Record(fields) = &row.value else {
            return Err(Malformed);
        };
        if fields.len() != 4 {
            return Err(Malformed);
        }
        let value = GraphBodyValue {
            identity: text_value(&fields[2].value)?,
            local: binary_value(&fields[3].value)?,
            r#final: binary_value(&fields[1].value)?,
            body_ir: binary_value(&fields[0].value)?,
        };
        if value.identity.is_empty() || value.identity <= previous {
            return Err(Malformed);
        }
        selected.push(decode_graph_body_page(
            &value.identity,
            &value.local,
            &value.r#final,
            &value.body_ir,
        )?);
        previous = value.identity;
    }
    Ok(selected)
}

/// Authenticate one already unframed owner page while the native framing workaround remains removable.
pub fn decode_graph_body_page(
    identity: &str,
    local: &[u8],
    r#final: &[u8],
    body_ir: &[u8],
) -> Result<GraphBodyFields, CodecFault> {
    if body_ir.is_empty() {
        return Err(Malformed);
    }
    let [pending, settled] = checked_pair(identity, local, r#final)?;
    Ok(GraphBodyFields {
        identity: identity.to_owned(),
        local: native_fields(&pending)?,
        final_fields: native_fields(&settled)?,
        body_ir: body_ir.to_vec(),
    })
}

/// A namespace publishes the complete, sorted set of package roots observed by its checker.
pub fn validate_package_roots(roots: &[String]) -> Result<(), CodecFault> {
    if roots.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut previous = "";
    for root in roots {
        if root.as_str() <= previous {
            return Err(Malformed);
        }
        previous = root;
    }
    Ok(())
}

/// Encode one checked namespace as one Crumb row while its compiler-owned payloads stay opaque.
pub fn encode_graph_namespace_value(value: &GraphNamespaceValue) -> Result<Vec<u8>, CodecFault> {
    if !value.call_reads_known
        || value.module.is_empty()
        || value.interface.is_empty()
        || value.body_ir.is_empty()
        || value.lowering_inputs.len() > MAX_FIELDS
    {
        return Err(Malformed);
    }
    validate_package_roots(&value.package_roots)?;
    let mut flattened = Vec::with_capacity(value.lowering_inputs.len() * 2);
    let mut previous = "";
    for input in &value.lowering_inputs {
        let [identity, witness] = input.as_slice() else {
            return Err(Malformed);
        };
        if identity.is_empty() || witness.is_empty() || identity.as_str() <= previous {
            return Err(Malformed);
        }
        flattened.extend([Value::Text(identity.clone()), Value::Text(witness.clone())]);
        previous = identity;
    }
    let mut fields = vec![
        required_value(b"bodyir", Value::Binary(value.body_ir.clone())),
        required_value(b"calls_", Value::List(call_read_values(&value.call_reads)?)),
        required_value(b"iface_", Value::Binary(value.interface.clone())),
        required_value(b"inputs", Value::List(flattened)),
        required_value(b"module", Value::Text(value.module.clone())),
        required_value(
            b"roots_",
            Value::List(value.package_roots.iter().cloned().map(Value::Text).collect()),
        ),
    ];
    let (mut version, mut key) = (3, CALLS_GRAPH_NAMESPACE_SELECTOR);
    if value.identity_proof.len() > MAX_PROOF_BYTES {
        return Err(Limit);
    }
    if !value.identity_proof.is_empty() {
        (version, key) = (4, GRAPH_NAMESPACE_SELECTOR);
        fields.push(required_value(b"zproof", Value::Binary(value.identity_proof.clone())));
    }
    let record = RowRecord {
        key: key.to_vec(),
        value: Value::Record(fields),
    };
    encode_rows(
        &graph_namespace_description(version),
        vec![record],
        MAX_CHECKER_ROW_BYTES,
    )
    .map_err(|_| Limit)
}

/// Materialize one validated namespace when the caller requires independently owned payloads.
pub fn decode_graph_namespace_value(encoded: &[u8]) -> Result<GraphNamespaceValue, CodecFault> {
    let value = decode_owned_namespace_view(encoded)?;
    Ok(GraphNamespaceValue {
        module: value.module,
        interface: view_bytes(&value.interface),
        identity_proof: value.identity_proof,
        body_ir: view_bytes(&value.body_ir),
        lowering_inputs: value.lowering_inputs,
        package_roots: value.package_roots,
        call_reads: value.call_reads,
        call_reads_known: value.call_reads_known,
    })
}

/// Admit owned transport bytes while keeping their namespace fields anchored in the same envelope.
pub fn decode_owned_namespace_view(encoded: &[u8]) -> Result<GraphNamespaceView, CodecFault> {
    let owner = mapped_bytes(&encoded.to_vec()).map_err(|_| Malformed)?;
    let view = complete_view(&owner).ok_or(Malformed)?;
    decode_graph_namespace_view(&view)
}

/// Validate the envelope and materialize only the namespace's small logical metadata.
pub fn decode_graph_namespace_view(view: &MappedView) -> Result<GraphNamespaceView, CodecFault> {
    let mut budget = operation_budget(MAX_CHECKER_ROW_BYTES * 4).map_err(|_| Malformed)?;
    let descriptions = vec![
        graph_namespace_description(4),
        graph_namespace_description(3),
        graph_namespace_description(2),
    ];
    let rows = admit_rows_view(view, &descriptions, MAX_CHECKER_ROW_BYTES, &mut budget).map_err(|_| Malformed)?;
    if row_count(&rows) != 1 {
        return Err(Malformed);
    }
    let key = row_key(&rows, 0);
    let has_proof = key == GRAPH_NAMESPACE_SELECTOR;
    let current = has_proof || key == CALLS_GRAPH_NAMESPACE_SELECTOR;
    if !current && key != PRIOR_GRAPH_NAMESPACE_SELECTOR {
        return Err(Malformed);
    }
    let width = 5 + i64::from(current) + i64::from(has_proof);
    if row_field_count(&rows, 0) != width {
        return Err(Malformed);
    }
    let binary = |budget: &mut _, identity: &[u8]| binary_field_at(&rows, 0, &identity.to_vec(), budget);
    let field = |budget: &mut _, identity: &[u8]| read_field_at(&rows, 0, &identity.to_vec(), budget);
    let mut proof = Vec::new();
    if has_proof {
        let proof_view = binary(&mut budget, b"zproof").map_err(|_| Malformed)?;
        if view_len(&proof_view) == 0 || view_len(&proof_view) > MAX_PROOF_BYTES as i64 {
            return Err(Malformed);
        }
        proof = view_bytes(&proof_view);
    }
    let body_ir = binary(&mut budget, b"bodyir").map_err(|_| Malformed)?;
    let interface = binary(&mut budget, b"iface_").map_err(|_| Malformed)?;
    let mut call_reads = Vec::new();
    if current {
        let calls = field(&mut budget, b"calls_").map_err(|_| Malformed)?;
        call_reads = decoded_call_reads(&calls)?;
    }
    let flattened = text_values(&field(&mut budget, b"inputs").map_err(|_| Malformed)?)?;
    let module = text_value(&field(&mut budget, b"module").map_err(|_| Malformed)?)?;
    let roots = text_values(&field(&mut budget, b"roots_").map_err(|_| Malformed)?)?;
    validate_package_roots(&roots)?;
    if module.is_empty() || view_len(&interface) == 0 || view_len(&body_ir) == 0 || flattened.len() % 2 != 0 {
        return Err(Malformed);
    }
    let mut lowering_inputs = Vec::with_capacity(flattened.len() / 2);
    let mut previous = "";
    for pair in flattened.chunks_exact(2) {
        let (identity, witness) = (&pair[0], &pair[1]);
        if identity.is_empty() || witness.is_empty() || identity.as_str() <= previous {
            return Err(Malformed);
        }
        lowering_inputs.push(vec![identity.clone(), witness.clone()]);
        previous = identity;
    }
    Ok(GraphNamespaceView {
        module,
        interface,
        identity_proof: proof,
        body_ir,
        lowering_inputs,
        package_roots: roots,
        call_reads,
        call_reads_known: current,
    })
}

/// Read one validated checker record's position-free semantic root for its logical record key.
pub fn record_root(encoded: &[u8]) -> Result<String, CodecFault> {
    Ok(decoded_record(encoded)?.root)
}

/// Require a complete checked body transport to supply both the checking pair and executable frame.
fn validate_graph_body_value(value: &GraphBodyValue) -> Result<[CheckedRecord; 2], CodecFault> {
    if value.body_ir.is_empty() {
        return Err(Malformed);
    }
    checked_pair(&value.identity, &value.local, &value.r#final)
}

/// Bind both independently produced stages to one checked identity and semantic syntax root.
fn checked_pair(identity: &str, pending: &[u8], settled: &[u8]) -> Result<[CheckedRecord; 2], CodecFault> {
    let local = decode_owned_record(pending, CHECKER_PROFILE, identity)?;
    let settled = decode_owned_record(settled, CHECKER_PROFILE, identity)?;
    if local.family != CheckedFamily::Local || settled.family != CheckedFamily::Final {
        return Err(Malformed);
    }
    if local.root != settled.root {
        return Err(BodyChanged);
    }
    Ok([local, settled])
}
