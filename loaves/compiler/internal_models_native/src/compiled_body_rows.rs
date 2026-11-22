//! Independent Crumb rows for compiler-produced checker records and Body IR.

use crate::checker_field_grammar::full_digest;
use crate::checker_record_models::{
    CheckedFamily, CodecFault, CompiledBodyDependencyRead, CompiledBodyFallback, CompiledBodyOutputs,
    CompiledBodyValue, CompiledNamespaceValue, SettledCallContract, SettledCallRead,
};
use crate::checker_records::{CHECKER_PROFILE, decode_owned_record, validate_package_roots};
use crate::crumb_rows::{ModelField, decode_rows, encode_rows, model_description, required_value};
use crate::settled_call_reads::{call_read_values, decoded_call_reads};
use CodecFault::{Limit, LimitExceeded, Malformed};
use internal_models_bindings::{Description, RowRecord, Value, ValueField};
use std::collections::HashSet;

const MAX_ROW_BYTES: i64 = 67_108_864;
const MAX_FIELDS: usize = 4096;
const MAX_METADATA_BYTES: usize = 65_536;
/// An over-long fallback reason keeps this many leading characters, at most four bytes each, under the metadata bound.
const MAX_REASON_CHARACTERS: usize = 16_000;
const LEGACY_NAMESPACE_SELECTOR: &[u8] = b"cmpns2\x00";
const ROOTS_NAMESPACE_SELECTOR: &[u8] = b"cmpns3\x00";
const CALLS_NAMESPACE_SELECTOR: &[u8] = b"cmpns4\x00";
const NAMESPACE_SELECTOR: &[u8] = b"cmpns5\x00";
const CONTRACT_NAMESPACE_SELECTOR: &[u8] = b"cmpns6\x00";
const PUBLISHED_NAMESPACE_SELECTOR: &[u8] = b"cmpns7\x00";
const SCOPED_VALUE_SELECTOR: &[u8] = b"cmpscp\x00";
const VALUE_SELECTOR: &[u8] = b"cmpval\x00";
const FALLBACK_SELECTOR: &[u8] = b"cmpfbk\x00";

/// One independently bounded compiler output row, never a whole-program transport envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompiledBodyOutput {
    Namespace(CompiledNamespaceValue),
    LegacyNamespace(String),
    Value(CompiledBodyValue),
    Fallback(CompiledBodyFallback),
}

/// Encode a successful compilation as independently bounded Crumb rows.
///
/// The returned list preserves canonical namespace-then-value-then-fallback order, but its aggregate byte length is
/// deliberately unbounded: the compiler writes and Oven reads one row file at a time.
pub fn encode_compiled_body_rows(outputs: &CompiledBodyOutputs) -> Result<Vec<Vec<u8>>, CodecFault> {
    let output_count = outputs.namespaces.len() + outputs.values.len() + outputs.fallbacks.len();
    if output_count > MAX_FIELDS {
        return Err(LimitExceeded(
            "compiled-output-row-count".into(),
            output_count as i64,
            MAX_FIELDS as i64,
        ));
    }
    let mut rows = Vec::with_capacity(output_count);
    let mut previous = String::new();
    for namespace in &outputs.namespaces {
        if namespace.module.is_empty() || namespace.module <= previous {
            return Err(Malformed);
        }
        rows.push(encode_compiled_namespace_value(namespace)?);
        previous.clone_from(&namespace.module);
    }
    previous.clear();
    for value in &outputs.values {
        let order = format!("{}\n{}", value.module, value.identity);
        if value.module.is_empty() || value.name.is_empty() || value.identity.is_empty() || order <= previous {
            return Err(Malformed);
        }
        rows.push(encode_compiled_body_value(value)?);
        previous = order;
    }
    previous.clear();
    let produced: HashSet<(&str, &str)> = outputs
        .values
        .iter()
        .map(|value| (value.module.as_str(), value.identity.as_str()))
        .collect();
    for fallback in &outputs.fallbacks {
        let identity = optional_identity(&fallback.identity);
        let order = format!("{}\n{}\n{identity}", fallback.module, fallback.name);
        if order <= previous {
            return Err(Malformed);
        }
        if !identity.is_empty() && produced.contains(&(fallback.module.as_str(), identity)) {
            return Err(Malformed);
        }
        rows.push(encode_compiled_body_fallback(fallback)?);
        previous = order;
    }
    Ok(rows)
}

/// Decode one complete Crumb row without consulting or buffering any peer row.
pub fn decode_compiled_body_row(encoded: &[u8]) -> Result<CompiledBodyOutput, CodecFault> {
    if encoded.is_empty() || encoded.len() as i64 > MAX_ROW_BYTES {
        return Err(Limit);
    }
    let descriptions = vec![
        published_namespace_description(),
        namespace_description(6),
        namespace_description(5),
        namespace_description(4),
        namespace_description(3),
        namespace_description(2),
        value_description(true),
        value_description(false),
        fallback_description(),
    ];
    let rows = decode_rows(encoded, &descriptions, MAX_ROW_BYTES).map_err(|_| Malformed)?;
    let [row] = rows.as_slice() else {
        return Err(Malformed);
    };
    let key = row.key.as_slice();
    let value = &row.value;
    Ok(match key {
        PUBLISHED_NAMESPACE_SELECTOR => CompiledBodyOutput::Namespace(decode_published_namespace(value)?),
        CONTRACT_NAMESPACE_SELECTOR => CompiledBodyOutput::Namespace(decode_compiled_namespace_value(value, 6)?),
        NAMESPACE_SELECTOR => CompiledBodyOutput::Namespace(decode_compiled_namespace_value(value, 5)?),
        CALLS_NAMESPACE_SELECTOR => CompiledBodyOutput::Namespace(decode_compiled_namespace_value(value, 4)?),
        ROOTS_NAMESPACE_SELECTOR => {
            CompiledBodyOutput::LegacyNamespace(decode_compiled_namespace_value(value, 3)?.module)
        }
        LEGACY_NAMESPACE_SELECTOR => {
            CompiledBodyOutput::LegacyNamespace(decode_compiled_namespace_value(value, 2)?.module)
        }
        SCOPED_VALUE_SELECTOR => CompiledBodyOutput::Value(decode_compiled_body_value(value, true)?),
        VALUE_SELECTOR => CompiledBodyOutput::Value(decode_compiled_body_value(value, false)?),
        FALLBACK_SELECTOR => CompiledBodyOutput::Fallback(decode_compiled_body_fallback(value)?),
        _ => return Err(Malformed),
    })
}

/// Sum the raw field bytes of one namespace, refusing an oversized contract list first.
fn namespace_raw_bytes(value: &CompiledNamespaceValue) -> Result<usize, CodecFault> {
    let mut raw = value.interface.len() + value.body_ir.len() + value.module.len() + value.semantic_interface.len();
    validate_package_roots(&value.package_roots)?;
    raw += value.package_roots.iter().map(String::len).sum::<usize>();
    raw += value.member_witnesses.iter().flatten().map(String::len).sum::<usize>();
    raw += value.dependencies.iter().map(String::len).sum::<usize>();
    raw += value.lowering_inputs.iter().flatten().map(String::len).sum::<usize>();
    if let Some(contracts) = &value.call_contracts {
        if contracts.len() > MAX_FIELDS {
            return Err(Limit);
        }
        raw += contracts
            .iter()
            .map(|c| c.owner.len() + c.reader.len() + c.target.len() + c.digest.len())
            .sum::<usize>();
    }
    Ok(raw)
}

/// Strictly ascending nonempty dependency names.
fn dependency_values(dependencies: &[String]) -> Result<Vec<Value>, CodecFault> {
    let mut previous = "";
    let mut values = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        if dependency.is_empty() || dependency.as_str() <= previous {
            return Err(Malformed);
        }
        values.push(Value::Text(dependency.clone()));
        previous = dependency;
    }
    Ok(values)
}

/// Encode one complete namespace Crumb row without interpreting compiler-owned payloads or witnesses.
fn encode_compiled_namespace_value(value: &CompiledNamespaceValue) -> Result<Vec<u8>, CodecFault> {
    if !value.published_record.is_empty() {
        return encode_published_namespace(value);
    }
    if value.module.is_empty()
        || value.interface.is_empty()
        || value.body_ir.is_empty()
        || value.dependencies.len() > MAX_FIELDS
        || value.lowering_inputs.len() > MAX_FIELDS
    {
        return Err(Malformed);
    }
    if !matches!(value.semantic_interface.len(), 0 | 32) || value.member_witnesses.len() > MAX_FIELDS {
        return Err(Malformed);
    }
    require_row_room(namespace_raw_bytes(value)?)?;
    let dependencies = dependency_values(&value.dependencies)?;
    let mut lowering_inputs = Vec::with_capacity(value.lowering_inputs.len() * 2);
    let mut previous = "";
    for input in &value.lowering_inputs {
        let [identity, witness] = input.as_slice() else {
            return Err(Malformed);
        };
        if identity.is_empty() || witness.is_empty() || identity.as_str() <= previous {
            return Err(Malformed);
        }
        lowering_inputs.extend([Value::Text(identity.clone()), Value::Text(witness.clone())]);
        previous = identity;
    }
    let members = member_values(&value.member_witnesses)?;
    let mut fields = vec![
        required_value(b"bodyir", Value::Binary(value.body_ir.clone())),
        required_value(b"deps__", Value::List(dependencies)),
        required_value(b"iface_", Value::Binary(value.interface.clone())),
        required_value(b"inputs", Value::List(lowering_inputs)),
        required_value(b"member", Value::List(members)),
        required_value(b"module", Value::Text(value.module.clone())),
        required_value(
            b"roots_",
            Value::List(value.package_roots.iter().cloned().map(Value::Text).collect()),
        ),
        required_value(b"semant", Value::Binary(value.semantic_interface.clone())),
        required_value(b"wreads", Value::List(call_read_values(&value.call_reads)?)),
    ];
    if value.identity_proof.is_empty() {
        if value.call_contracts.is_some() {
            return Err(Malformed);
        }
        return encoded_row(&namespace_description(4), record(CALLS_NAMESPACE_SELECTOR, fields));
    }
    if value.identity_proof.len() > MAX_METADATA_BYTES {
        return Err(Limit);
    }
    fields.push(required_value(b"zproof", Value::Binary(value.identity_proof.clone())));
    if let Some(contracts) = &value.call_contracts {
        fields.push(required_value(b"zsupp_", Value::List(contract_values(contracts)?)));
        return encoded_row(&namespace_description(6), record(CONTRACT_NAMESPACE_SELECTOR, fields));
    }
    encoded_row(&namespace_description(5), record(NAMESPACE_SELECTOR, fields))
}

fn record(key: &[u8], fields: Vec<ValueField>) -> RowRecord {
    RowRecord {
        key: key.to_vec(),
        value: Value::Record(fields),
    }
}

fn published_namespace_description() -> Description {
    model_description(
        PUBLISHED_NAMESPACE_SELECTOR,
        "incan.checker-compiled-namespace/7",
        &[
            ModelField::new(b"deps__", "dependencies", "list<string>"),
            ModelField::new(b"member", "members", "list<string>"),
            ModelField::new(b"module", "module", "string"),
            ModelField::new(b"record", "namespace_record", "binary"),
            ModelField::new(b"semant", "semantic_interface", "binary"),
            ModelField::new(b"wreads", "call_reads", "list<tuple<string,string,string>>"),
            ModelField::new(b"zsupp_", "contracts", "list<tuple<string,string,string,string>>"),
        ],
    )
}

/// Pass the durable record through unchanged alongside the facts Oven uses to publish it.
fn encode_published_namespace(value: &CompiledNamespaceValue) -> Result<Vec<u8>, CodecFault> {
    if value.module.is_empty()
        || value.semantic_interface.len() != 32
        || value.published_record.len() as i64 > MAX_ROW_BYTES
    {
        return Err(Malformed);
    }
    if value.interface.len()
        + value.body_ir.len()
        + value.lowering_inputs.len()
        + value.package_roots.len()
        + value.identity_proof.len()
        != 0
    {
        return Err(Malformed);
    }
    if value.member_witnesses.len() > MAX_FIELDS || value.dependencies.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let dependencies = dependency_values(&value.dependencies)?;
    let Some(contracts) = &value.call_contracts else {
        return Err(Malformed);
    };
    if contracts.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let fields = vec![
        required_value(b"deps__", Value::List(dependencies)),
        required_value(b"member", Value::List(member_values(&value.member_witnesses)?)),
        required_value(b"module", Value::Text(value.module.clone())),
        required_value(b"record", Value::Binary(value.published_record.clone())),
        required_value(b"semant", Value::Binary(value.semantic_interface.clone())),
        required_value(b"wreads", Value::List(call_read_values(&value.call_reads)?)),
        required_value(b"zsupp_", Value::List(contract_values(contracts)?)),
    ];
    encoded_row(
        &published_namespace_description(),
        record(PUBLISHED_NAMESPACE_SELECTOR, fields),
    )
}

/// Group a flattened witness list into triples and validate them.
fn member_triples(flattened: Vec<String>) -> Result<Vec<Vec<String>>, CodecFault> {
    let members: Vec<Vec<String>> = flattened.chunks(3).map(<[String]>::to_vec).collect();
    member_values(&members)?;
    Ok(members)
}

/// Restore publishing metadata without copying the record's Body IR into a second object.
fn decode_published_namespace(value: &Value) -> Result<CompiledNamespaceValue, CodecFault> {
    let fields = record_fields(value, 7)?;
    let dependencies = text_values(&fields[0].value)?;
    let flattened = text_values(&fields[1].value)?;
    if dependencies.len() > MAX_FIELDS || flattened.len() % 3 != 0 || flattened.len() / 3 > MAX_FIELDS {
        return Err(Limit);
    }
    dependency_values(&dependencies)?;
    let members = member_triples(flattened)?;
    let module = text_value(&fields[2].value)?;
    let published = binary_value(&fields[3].value)?;
    let semantic = binary_value(&fields[4].value)?;
    if module.is_empty() || published.is_empty() || semantic.len() != 32 {
        return Err(Malformed);
    }
    Ok(CompiledNamespaceValue {
        module,
        interface: Vec::new(),
        body_ir: Vec::new(),
        dependencies,
        lowering_inputs: Vec::new(),
        package_roots: Vec::new(),
        call_reads: decoded_call_reads(&fields[5].value)?,
        semantic_interface: semantic,
        member_witnesses: members,
        call_contracts: Some(read_contracts(&fields[6].value)?),
        identity_proof: Vec::new(),
        published_record: published,
    })
}

/// The order key and wire tokens of one dependency read.
fn dependency_order(read: &CompiledBodyDependencyRead) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        optional_identity(&read.package),
        read.module,
        read.name,
        read.identity,
        read.reference_subject
    )
}

/// Refuse an incomplete or out-of-order dependency read.
fn admit_read(read: &CompiledBodyDependencyRead, previous: &mut String) -> Result<(), CodecFault> {
    let order = dependency_order(read);
    if read.module.is_empty()
        || read.name.is_empty()
        || read.identity.is_empty()
        || read.reference_subject.is_empty()
        || order <= *previous
    {
        return Err(Malformed);
    }
    *previous = order;
    Ok(())
}

/// Encode one authenticated producer triple as a Crumb row with opaque checker and Body IR fields.
fn encode_compiled_body_value(value: &CompiledBodyValue) -> Result<Vec<u8>, CodecFault> {
    if value.module.is_empty() || value.name.is_empty() || value.identity.is_empty() {
        return Err(Malformed);
    }
    validate_compiled_body_value(value)?;
    let mut raw = value.declaration.len()
        + value.local.len()
        + value.r#final.len()
        + value.body_ir.len()
        + value.module.len()
        + value.name.len()
        + value.identity.len();
    raw += value.scope_names.iter().map(String::len).sum::<usize>();
    for read in &value.dependency_reads {
        raw += optional_identity(&read.package).len()
            + read.module.len()
            + read.name.len()
            + read.identity.len()
            + read.reference_subject.len();
    }
    require_row_room(raw)?;
    let mut dependency_reads = Vec::with_capacity(value.dependency_reads.len() * 6);
    let mut previous = String::new();
    for read in &value.dependency_reads {
        admit_read(read, &mut previous)?;
        let package = optional_identity(&read.package);
        let marker = if package.is_empty() { "none" } else { "some" };
        dependency_reads.extend([
            Value::Text(marker.into()),
            Value::Text(package.to_owned()),
            Value::Text(read.module.clone()),
            Value::Text(read.name.clone()),
            Value::Text(read.identity.clone()),
            Value::Text(read.reference_subject.clone()),
        ]);
    }
    let mut fields = vec![
        required_value(b"bodyir", Value::Binary(value.body_ir.clone())),
        required_value(b"declar", Value::Binary(value.declaration.clone())),
        required_value(b"deps__", Value::List(dependency_reads)),
        required_value(b"final_", Value::Binary(value.r#final.clone())),
        required_value(b"ident_", Value::Text(value.identity.clone())),
        required_value(b"local_", Value::Binary(value.local.clone())),
        required_value(b"module", Value::Text(value.module.clone())),
        required_value(b"name__", Value::Text(value.name.clone())),
    ];
    if !value.scope_names.is_empty() {
        validate_scope_names(&value.name, &value.scope_names, &value.dependency_reads)?;
        fields.push(required_value(
            b"zscope",
            Value::List(value.scope_names.iter().cloned().map(Value::Text).collect()),
        ));
        return encoded_row(&value_description(true), record(SCOPED_VALUE_SELECTOR, fields));
    }
    encoded_row(&value_description(false), record(VALUE_SELECTOR, fields))
}

/// Encode one explicit full-source decision as a Crumb row independently of eligible owner payloads.
fn encode_compiled_body_fallback(fallback: &CompiledBodyFallback) -> Result<Vec<u8>, CodecFault> {
    if fallback.module.is_empty() || fallback.name.is_empty() || fallback.reason.is_empty() {
        return Err(Malformed);
    }
    let identity = encoded_optional_identity(&fallback.identity)?;
    let fields = vec![
        required_value(b"ident_", identity),
        required_value(b"module", Value::Text(fallback.module.clone())),
        required_value(b"name__", Value::Text(fallback.name.clone())),
        required_value(b"reason", Value::Text(bounded_reason(&fallback.reason))),
    ];
    encoded_row(&fallback_description(), record(FALLBACK_SELECTOR, fields))
}

/// Keep one fallback reason inside the metadata bound.
///
/// A reason is diagnostic text the compiler renders from a refusal, so its length follows the refused owner. One
/// over-long reason keeps its start and states its full size; it never refuses the other owners' rows.
fn bounded_reason(reason: &str) -> String {
    let size = reason.len();
    if size <= MAX_METADATA_BYTES {
        return reason.to_owned();
    }
    let kept: String = reason.chars().take(MAX_REASON_CHARACTERS).collect();
    format!("{kept} [reason cut from {size} bytes]")
}

/// Restore one value row after checking its exact fields and flattened dependency ordering.
fn decode_compiled_body_value(value: &Value, scoped: bool) -> Result<CompiledBodyValue, CodecFault> {
    let fields = record_fields(value, if scoped { 9 } else { 8 })?;
    let flattened = text_values(&fields[2].value)?;
    if flattened.len() % 6 != 0 || flattened.len() / 6 > MAX_FIELDS {
        return Err(Malformed);
    }
    let mut dependency_reads = Vec::with_capacity(flattened.len() / 6);
    let mut previous = String::new();
    for cells in flattened.chunks_exact(6) {
        let read = CompiledBodyDependencyRead {
            package: decoded_package(&cells[0], &cells[1])?,
            module: cells[2].clone(),
            name: cells[3].clone(),
            identity: cells[4].clone(),
            reference_subject: cells[5].clone(),
        };
        admit_read(&read, &mut previous)?;
        dependency_reads.push(read);
    }
    let mut scope_names = Vec::new();
    if scoped {
        scope_names = text_values(&fields[8].value)?;
        validate_scope_names(&text_value(&fields[7].value)?, &scope_names, &dependency_reads)?;
    }
    let restored = CompiledBodyValue {
        module: text_value(&fields[6].value)?,
        name: text_value(&fields[7].value)?,
        identity: text_value(&fields[4].value)?,
        declaration: binary_value(&fields[1].value)?,
        local: binary_value(&fields[5].value)?,
        r#final: binary_value(&fields[3].value)?,
        body_ir: binary_value(&fields[0].value)?,
        dependency_reads,
        scope_names,
    };
    if restored.module.is_empty() || restored.name.is_empty() || restored.identity.is_empty() {
        return Err(Malformed);
    }
    validate_compiled_body_value(&restored)?;
    Ok(restored)
}

/// Join each current emitted symbol to a distinct stable declaration and its checked lowering witness.
fn member_values(members: &[Vec<String>]) -> Result<Vec<Value>, CodecFault> {
    let mut values = Vec::with_capacity(members.len() * 3);
    let mut owners = HashSet::new();
    let mut previous = "";
    for entry in members {
        let [symbol, owner, witness] = entry.as_slice() else {
            return Err(Malformed);
        };
        if symbol.as_str() <= previous || owner.is_empty() || witness.is_empty() || !owners.insert(owner.as_str()) {
            return Err(Malformed);
        }
        previous = symbol;
        values.extend([
            Value::Text(symbol.clone()),
            Value::Text(owner.clone()),
            Value::Text(witness.clone()),
        ]);
    }
    Ok(values)
}

fn contract_values(contracts: &[SettledCallContract]) -> Result<Vec<Value>, CodecFault> {
    let mut values = Vec::with_capacity(contracts.len());
    let mut previous = "";
    for contract in contracts {
        if contract.target.as_str() <= previous
            || (contract.reader.is_empty() && !contract.owner.is_empty())
            || !full_digest(&contract.digest)
        {
            return Err(Malformed);
        }
        values.push(Value::Tuple(vec![
            Value::Text(contract.target.clone()),
            Value::Text(contract.digest.clone()),
            Value::Text(contract.reader.clone()),
            Value::Text(contract.owner.clone()),
        ]));
        previous = &contract.target;
    }
    Ok(values)
}

fn read_contracts(value: &Value) -> Result<Vec<SettledCallContract>, CodecFault> {
    let Value::List(entries) = value else {
        return Err(Malformed);
    };
    if entries.len() > MAX_FIELDS {
        return Err(Limit);
    }
    let mut contracts = Vec::with_capacity(entries.len());
    let mut previous = String::new();
    for entry in entries {
        let Value::Tuple(cells) = entry else {
            return Err(Malformed);
        };
        if cells.len() != 4 {
            return Err(Malformed);
        }
        let target = text_value(&cells[0])?;
        let digest = text_value(&cells[1])?;
        let reader = text_value(&cells[2])?;
        let owner = text_value(&cells[3])?;
        if target <= previous || (reader.is_empty() && !owner.is_empty()) || !full_digest(&digest) {
            return Err(Malformed);
        }
        previous.clone_from(&target);
        contracts.push(SettledCallContract {
            target,
            digest,
            reader,
            owner,
        });
    }
    Ok(contracts)
}

/// Restore one bounded namespace value, imported modules and canonical lowering witnesses.
fn decode_compiled_namespace_value(value: &Value, version: i64) -> Result<CompiledNamespaceValue, CodecFault> {
    let semantic_index = if version == 2 { 6 } else { 7 };
    let count = semantic_index + 1 + usize::from(version >= 4) + usize::from(version >= 5) + usize::from(version >= 6);
    let fields = record_fields(value, count)?;
    let dependencies = text_values(&fields[1].value)?;
    if dependencies.len() > MAX_FIELDS {
        return Err(Malformed);
    }
    dependency_values(&dependencies)?;
    let flattened = text_values(&fields[3].value)?;
    if flattened.len() % 2 != 0 || flattened.len() / 2 > MAX_FIELDS {
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
    let flattened_members = text_values(&fields[4].value)?;
    if flattened_members.len() % 3 != 0 || flattened_members.len() / 3 > MAX_FIELDS {
        return Err(Malformed);
    }
    let members = member_triples(flattened_members)?;
    let mut roots = Vec::new();
    if version >= 3 {
        roots = text_values(&fields[6].value)?;
        validate_package_roots(&roots)?;
    }
    let semantic = binary_value(&fields[semantic_index].value)?;
    if !matches!(semantic.len(), 0 | 32) {
        return Err(Malformed);
    }
    let call_reads: Vec<SettledCallRead> = if version >= 4 {
        decoded_call_reads(&fields[8].value)?
    } else {
        Vec::new()
    };
    let mut proof = Vec::new();
    if version >= 5 {
        proof = binary_value(&fields[9].value)?;
        if proof.is_empty() || proof.len() > MAX_METADATA_BYTES {
            return Err(Malformed);
        }
    }
    let contracts = if version >= 6 {
        Some(read_contracts(&fields[10].value)?)
    } else {
        None
    };
    let restored = CompiledNamespaceValue {
        call_contracts: contracts,
        identity_proof: proof,
        call_reads,
        module: text_value(&fields[5].value)?,
        member_witnesses: members,
        package_roots: roots,
        semantic_interface: semantic,
        interface: binary_value(&fields[2].value)?,
        body_ir: binary_value(&fields[0].value)?,
        dependencies,
        lowering_inputs,
        published_record: Vec::new(),
    };
    if restored.module.is_empty() || restored.interface.is_empty() || restored.body_ir.is_empty() {
        return Err(Malformed);
    }
    Ok(restored)
}

/// Restore one explicit full-source row and retain identity absence as its own state.
fn decode_compiled_body_fallback(value: &Value) -> Result<CompiledBodyFallback, CodecFault> {
    let fields = record_fields(value, 4)?;
    let fallback = CompiledBodyFallback {
        module: text_value(&fields[1].value)?,
        name: text_value(&fields[2].value)?,
        identity: optional_text_value(&fields[0].value)?,
        reason: text_value(&fields[3].value)?,
    };
    if fallback.module.is_empty()
        || fallback.name.is_empty()
        || fallback.reason.is_empty()
        || fallback.reason.len() > MAX_METADATA_BYTES
    {
        return Err(Malformed);
    }
    Ok(fallback)
}

/// Encode one Crumb row whose file boundary lets a reader bound allocation before decoding it.
fn encoded_row(description: &Description, row: RowRecord) -> Result<Vec<u8>, CodecFault> {
    let encoded = encode_rows(description, vec![row], MAX_ROW_BYTES * 2).map_err(|_| Limit)?;
    if encoded.len() as i64 > MAX_ROW_BYTES {
        return Err(LimitExceeded(
            "compiled-output-row-bytes".into(),
            encoded.len() as i64,
            MAX_ROW_BYTES,
        ));
    }
    Ok(encoded)
}

/// Refuse raw fields that already exceed the row before Crumb adds its model and row structure.
fn require_row_room(raw_bytes: usize) -> Result<(), CodecFault> {
    if raw_bytes as i64 > MAX_ROW_BYTES {
        return Err(LimitExceeded(
            "compiled-output-row-bytes".into(),
            raw_bytes as i64,
            MAX_ROW_BYTES,
        ));
    }
    Ok(())
}

/// Require one logical Crumb record with its complete declared field set.
fn record_fields(value: &Value, expected: usize) -> Result<&Vec<ValueField>, CodecFault> {
    match value {
        Value::Record(values) if values.len() == expected => Ok(values),
        _ => Err(Malformed),
    }
}

/// Require one logical Crumb string value.
fn text_value(value: &Value) -> Result<String, CodecFault> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err(Malformed),
    }
}

/// Require one logical Crumb byte-string value.
fn binary_value(value: &Value) -> Result<Vec<u8>, CodecFault> {
    match value {
        Value::Binary(data) => Ok(data.clone()),
        _ => Err(Malformed),
    }
}

/// Require one logical Crumb string list without accepting alternate element values.
fn text_values(value: &Value) -> Result<Vec<String>, CodecFault> {
    match value {
        Value::List(items) => items.iter().map(text_value).collect(),
        _ => Err(Malformed),
    }
}

/// Decode one exact Crumb option string without an empty-string absence sentinel.
fn optional_text_value(value: &Value) -> Result<Option<String>, CodecFault> {
    match value {
        Value::NoneValue => Ok(None),
        Value::Some(items) => {
            let [item] = items.as_slice() else {
                return Err(Malformed);
            };
            let text = text_value(item)?;
            if text.is_empty() {
                return Err(Malformed);
            }
            Ok(Some(text))
        }
        _ => Err(Malformed),
    }
}

/// Encode one fallback identity as an exact Crumb option without admitting an empty value.
fn encoded_optional_identity(identity: &Option<String>) -> Result<Value, CodecFault> {
    match identity {
        Some(value) if value.is_empty() => Err(Malformed),
        Some(value) => Ok(Value::Some(vec![Value::Text(value.clone())])),
        None => Ok(Value::NoneValue),
    }
}

/// Restore the explicit package-presence marker used by one flattened dependency read.
fn decoded_package(marker: &str, value: &str) -> Result<Option<String>, CodecFault> {
    match (marker, value.is_empty()) {
        ("none", true) => Ok(None),
        ("some", false) => Ok(Some(value.to_owned())),
        _ => Err(Malformed),
    }
}

/// Encode identity absence only for an explicit fallback; stable value records never admit this sentinel.
fn optional_identity(identity: &Option<String>) -> &str {
    identity.as_deref().unwrap_or("")
}

/// Each version adds package roots, settled call reads, or the physical identity proof in field order.
fn namespace_description(version: i64) -> Description {
    let mut fields = vec![
        ModelField::new(b"bodyir", "body_ir", "binary"),
        ModelField::new(b"deps__", "dependencies", "list<string>"),
        ModelField::new(b"iface_", "interface", "binary"),
        ModelField::new(b"inputs", "lowering_inputs", "list<string>"),
        ModelField::new(b"member", "member_witnesses", "list<string>"),
        ModelField::new(b"module", "module", "string"),
    ];
    if version >= 3 {
        fields.push(ModelField::new(b"roots_", "package_roots", "list<string>"));
    }
    fields.push(ModelField::new(b"semant", "semantic_interface", "binary"));
    if version == 2 {
        return model_description(LEGACY_NAMESPACE_SELECTOR, "incan.checker-compiled-namespace/2", &fields);
    }
    if version == 3 {
        return model_description(ROOTS_NAMESPACE_SELECTOR, "incan.checker-compiled-namespace/3", &fields);
    }
    fields.push(ModelField::new(
        b"wreads",
        "call_reads",
        "list<tuple<string,string,string>>",
    ));
    if version == 4 {
        return model_description(CALLS_NAMESPACE_SELECTOR, "incan.checker-compiled-namespace/4", &fields);
    }
    fields.push(ModelField::new(b"zproof", "identity_proof", "binary"));
    if version == 5 {
        return model_description(NAMESPACE_SELECTOR, "incan.checker-compiled-namespace/5", &fields);
    }
    fields.push(ModelField::new(
        b"zsupp_",
        "settled_contracts",
        "list<tuple<string,string,string,string>>",
    ));
    model_description(
        CONTRACT_NAMESPACE_SELECTOR,
        "incan.checker-compiled-namespace/6",
        &fields,
    )
}

/// Require the owner and every exact declaration read to be covered by the proved name scope.
fn validate_scope_names(owner: &str, names: &[String], reads: &[CompiledBodyDependencyRead]) -> Result<(), CodecFault> {
    if names.len() > MAX_FIELDS {
        return Err(Malformed);
    }
    let mut previous = "";
    let mut found = false;
    for name in names {
        if name == owner {
            found = true;
        }
        if name.as_str() <= previous
            || name.chars().count() > MAX_METADATA_BYTES
            || name.starts_with(|c: char| c.is_ascii_digit())
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(Malformed);
        }
        previous = name;
    }
    if !found || reads.iter().any(|read| names.binary_search(&read.name).is_err()) {
        return Err(Malformed);
    }
    Ok(())
}

/// A compiled owner, optionally with the names proved sufficient by the producing frontend.
fn value_description(scoped: bool) -> Description {
    let mut fields = vec![
        ModelField::new(b"bodyir", "body_ir", "binary"),
        ModelField::new(b"declar", "declaration", "binary"),
        ModelField::new(b"deps__", "dependency_reads", "list<string>"),
        ModelField::new(b"final_", "final", "binary"),
        ModelField::new(b"ident_", "identity", "string"),
        ModelField::new(b"local_", "local", "binary"),
        ModelField::new(b"module", "module", "string"),
        ModelField::new(b"name__", "name", "string"),
    ];
    if scoped {
        fields.push(ModelField::new(b"zscope", "scope_names", "list<string>"));
        return model_description(SCOPED_VALUE_SELECTOR, "incan.checker-compiled-value/2", &fields);
    }
    model_description(VALUE_SELECTOR, "incan.checker-compiled-value/1", &fields)
}

/// Describe one explicit full-source fallback without encoding absence as an empty string.
fn fallback_description() -> Description {
    model_description(
        FALLBACK_SELECTOR,
        "incan.checker-compiled-fallback/1",
        &[
            ModelField::new(b"ident_", "identity", "option<string>"),
            ModelField::new(b"module", "module", "string"),
            ModelField::new(b"name__", "name", "string"),
            ModelField::new(b"reason", "reason", "string"),
        ],
    )
}

/// Require one declaration and a matching local/final pair under the compiler-owned stable identity.
fn validate_compiled_body_value(value: &CompiledBodyValue) -> Result<(), CodecFault> {
    // Each checker record is decoded once, through its authenticated semantic root and compiler-owned identity.
    let declaration = decode_owned_record(&value.declaration, CHECKER_PROFILE, &value.identity)?;
    let local = decode_owned_record(&value.local, CHECKER_PROFILE, &value.identity)?;
    let settled = decode_owned_record(&value.r#final, CHECKER_PROFILE, &value.identity)?;
    if declaration.family != CheckedFamily::Declaration
        || local.family != CheckedFamily::Local
        || settled.family != CheckedFamily::Final
        || local.root != settled.root
        || local.owner != declaration.owner
        || value.body_ir.is_empty()
    {
        return Err(Malformed);
    }
    Ok(())
}
