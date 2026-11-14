//! Native Crumb row envelope writing: descriptions in their original modes, typed rows and the logical seal.
//!
//! This mirrors `row_writer.write_rows`, `descriptions.encode_entry` and the row cell writers in `cells`.

use sha2::{Digest, Sha256};

use super::columns::{self, Budget};
use super::hash::{self, Node, Value};
use super::rows::{Meta, Rows, Stored, StoredField, encode_metadata};

/// Write the decoded descriptions in their original modes, every record and any seal.
pub(super) fn write_rows(rows: &Rows) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    hash::unsigned(&mut body, rows.dictionary.len() as u128);
    for entry in &rows.dictionary {
        body.extend(encode_entry(entry)?);
    }
    hash::unsigned(&mut body, rows.records.len() as u128);
    let mut seal = b"crumb1\x00\x04".to_vec();
    hash::unsigned(&mut seal, rows.records.len() as u128);
    for record in &rows.records {
        hash::unsigned(&mut body, record.slot as u128);
        hash::frame(
            &mut body,
            &write_root(&rows.dictionary[record.slot], &record.value, &rows.dictionary)?,
        );
        let mut contribution = Vec::new();
        hash::emit_object(&record.value, &mut contribution)?;
        hash::frame(&mut seal, &contribution);
    }
    if rows.sealed {
        hash::unsigned(&mut body, rows.records.len() as u128);
        body.extend_from_slice(&Sha256::digest(&seal));
    }
    let mut output = b"CRMB\x01\x01".to_vec();
    output.push(u8::from(rows.sealed));
    hash::frame(&mut output, &body);
    Some(output)
}

/// Emit one description in its original mode with complete D metadata.
fn encode_entry(entry: &Stored) -> Option<Vec<u8>> {
    let mut budget = Budget::new(u64::MAX, u64::MAX, u64::MAX);
    let mut output = Vec::new();
    if !entry.inline {
        output.push(1);
        hash::frame(&mut output, &entry.key);
        output.extend_from_slice(&entry.digest);
        return Some(output);
    }
    let mut body = Vec::new();
    let members = if entry.record { "fields" } else { "variants" };
    hash::frame(
        &mut body,
        &metadata_bytes(&without(&entry.metadata, members)?, &mut budget)?,
    );
    if entry.record {
        encode_fields(&entry.fields, &mut body, &mut budget)?;
    } else {
        hash::unsigned(&mut body, entry.variants.len() as u128);
        for variant in &entry.variants {
            let mut payload = Vec::new();
            hash::frame(
                &mut payload,
                &metadata_bytes(&without(&variant.metadata, "fields")?, &mut budget)?,
            );
            encode_fields(&variant.fields, &mut payload, &mut budget)?;
            hash::unsigned(&mut body, variant.position as u128);
            hash::frame(&mut body, &payload);
        }
    }
    output.push(0);
    hash::frame(&mut output, &body);
    Some(output)
}

/// Write identity-ordered fields with their declaration positions.
fn encode_fields(fields: &[StoredField], output: &mut Vec<u8>, budget: &mut Budget) -> Option<()> {
    hash::unsigned(output, fields.len() as u128);
    for field in fields {
        hash::unsigned(output, field.position as u128);
        hash::frame(output, &metadata_bytes(&field.metadata, budget)?);
    }
    Some(())
}

/// The D encoding of one metadata tree.
fn metadata_bytes(value: &Meta, budget: &mut Budget) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    encode_metadata(value, &mut output, budget).ok()?;
    Some(output)
}

/// A metadata map without one named property.
fn without(value: &Meta, name: &str) -> Option<Meta> {
    let Meta::Map(entries) = value else { return None };
    Some(Meta::Map(
        entries.iter().filter(|(key, _)| key != name).cloned().collect(),
    ))
}

/// Write one root row: an enum's variant index, then the presence bitmap and indexed field cells.
fn write_root(entry: &Stored, value: &Value, dictionary: &[Stored]) -> Option<Vec<u8>> {
    match value {
        Value::Record(fields) if entry.record => write_fields(&entry.fields, fields, dictionary),
        Value::Enum(identity, fields) if !entry.record => {
            let index = entry
                .variants
                .iter()
                .position(|variant| variant.identity == *identity)?;
            let mut output = Vec::new();
            hash::unsigned(&mut output, index as u128);
            output.extend(write_fields(&entry.variants[index].fields, fields, dictionary)?);
            Some(output)
        }
        _ => None,
    }
}

/// Write the presence bitmap and the cells of every present field in identity order.
fn write_fields(shape: &[StoredField], fields: &[hash::Field], dictionary: &[Stored]) -> Option<Vec<u8>> {
    let eligible = shape.iter().filter(|field| field.absent != 'r').count();
    let mut bitmap = vec![0u8; eligible.div_ceil(8)];
    let (mut cells, mut bit) = (Vec::new(), 0);
    for field in shape {
        let value = &fields.iter().find(|stored| stored.identity == field.identity)?.value;
        let present = match (field.absent, value) {
            ('n', Value::Absent) => false,
            ('e', Value::List(items) | Value::Set(items)) => !items.is_empty(),
            ('e', Value::Map(entries)) => !entries.is_empty(),
            _ => true,
        };
        if field.absent != 'r' {
            if present {
                bitmap[bit / 8] |= 1 << (bit % 8);
            }
            bit += 1;
        }
        if !present {
            continue;
        }
        cells.push(match (field.absent, &field.storage, value) {
            ('n', Node::Container(_, arguments), Value::Some(inner)) => write_cell(&arguments[0], inner, dictionary)?,
            ('n', ..) => return None,
            _ => write_cell(&field.storage, value, dictionary)?,
        });
    }
    let mut output = bitmap;
    output.extend(columns::indexed(&cells));
    Some(output)
}

/// Write one canonical typed cell.
fn write_cell(node: &Node, value: &Value, dictionary: &[Stored]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    match (node, value) {
        (Node::Scalar(kind), _) => return write_scalar(kind, value),
        (Node::Reference(_), _) => return write_scalar("reference", value),
        (Node::Decimal(..), Value::Decimal(coefficient, _)) => {
            hash::zigzag(&mut output, coefficient.unsigned_abs(), *coefficient < 0)
        }
        (Node::Nominal(..), _) => {
            let key = hash::closed_selector(node)?;
            let entry = dictionary.iter().find(|entry| entry.key == key)?;
            return write_root(entry, value, dictionary);
        }
        (Node::Container(kind, _), Value::Absent) if kind == "option" => output.push(0),
        (Node::Container(kind, arguments), Value::Some(inner)) if kind == "option" => {
            output.push(1);
            hash::frame(&mut output, &write_cell(&arguments[0], inner, dictionary)?);
        }
        (Node::Container(kind, arguments), Value::Union(member, inner)) if kind == "union" => {
            let member_key = hash::selector_key(member)?;
            let rank = arguments
                .iter()
                .filter(|other| hash::selector_key(other).is_some_and(|key| key < member_key))
                .count();
            hash::unsigned(&mut output, rank as u128);
            output.extend(write_cell(member, inner, dictionary)?);
        }
        (Node::Container(kind, arguments), Value::List(items) | Value::Tuple(items) | Value::Set(items)) => {
            let mut cells = Vec::with_capacity(items.len());
            for (position, item) in items.iter().enumerate() {
                let element = if kind == "tuple" {
                    arguments.get(position)?
                } else {
                    arguments.first()?
                };
                cells.push(write_cell(element, item, dictionary)?);
            }
            hash::unsigned(&mut output, items.len() as u128);
            output.extend(columns::indexed(&cells));
        }
        (Node::Container(_, arguments), Value::Map(entries)) => {
            let mut cells = Vec::with_capacity(entries.len() * 2);
            for (key, item) in entries {
                cells.push(write_cell(arguments.first()?, key, dictionary)?);
                cells.push(write_cell(arguments.get(1)?, item, dictionary)?);
            }
            hash::unsigned(&mut output, entries.len() as u128);
            output.extend(columns::indexed(&cells));
        }
        _ => return None,
    }
    Some(output)
}

/// Write one scalar cell.
fn write_scalar(kind: &str, value: &Value) -> Option<Vec<u8>> {
    if let Some(key_type @ columns::KeyType::Integer(..)) = columns::key_type(kind) {
        return columns::key_cell(key_type, value);
    }
    let mut output = Vec::new();
    match (kind, value) {
        ("boolean", Value::Boolean(flag)) => output.push(u8::from(*flag)),
        ("fp32", Value::Float(bits)) => {
            let wide = f64::from_bits(u64::from_be_bytes(bits.as_slice().try_into().ok()?));
            let narrow = if wide.is_nan() {
                f32::from_bits(0x7fc0_0000)
            } else {
                wide as f32
            };
            output.extend_from_slice(&narrow.to_bits().to_le_bytes());
        }
        ("fp64", Value::Float(bits)) => output.extend(bits.iter().rev()),
        ("string", Value::Text(text)) => hash::frame(&mut output, text.as_bytes()),
        ("binary", Value::Binary(data)) => hash::frame(&mut output, data),
        ("secret_string" | "secret_bytes", Value::Withheld) => output.push(0),
        ("reference", Value::Reference(target, identity)) => {
            hash::frame(&mut output, target);
            hash::frame(&mut output, identity);
        }
        _ => return None,
    }
    Some(output)
}
