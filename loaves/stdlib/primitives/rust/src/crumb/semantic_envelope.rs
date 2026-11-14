//! Native Crumb column envelopes: the header, the shared description dictionary, row groups of field columns and the
//! logical seal.
//!
//! This mirrors `column_reader.read_columns` (with `column_directory` and `column_group_read`) and
//! `column_writer.write_columns`. Decoding reads the literal envelope, or every proper prefix when the case asks for
//! them; encoding writes the decoded descriptions, records and physical block choices back.

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use super::blocks::{self, CellType};
use super::columns::{self, Budget, Cursor, Refused};
use super::hash::{self, Projection, Value};
use super::row_write;
use super::rows::{self, Stored};

/// The marker for a well-formed envelope whose types or numbers lie outside the native column codec.
const UNSUPPORTED: Refused = "__unsupported__";

/// One admitted column: its block encodings and every logical value.
type Column = (Vec<u8>, Vec<Value>);

/// One maximal run of records of one description, stored as field columns.
struct Group {
    slot: usize,
    rows: usize,
    columns: Vec<Column>,
}

/// One admitted column envelope.
struct Envelope {
    dictionary: Vec<Stored>,
    groups: Vec<Group>,
    sealed: bool,
}

/// Decode a column envelope case: one status and records, or a refusal category, per decoder execution.
pub(super) fn decode(document: &Json) -> Option<Vec<u8>> {
    let (data, supplied) = inputs(document)?;
    let lengths: Vec<usize> = if document.get("transform").is_some() {
        (0..data.len()).collect()
    } else {
        vec![data.len()]
    };
    let mut output = Vec::new();
    for length in lengths {
        let mut budget = columns::case_budget(document)?;
        match read_columns(&data[..length], &supplied, require_sealed(document), &mut budget) {
            Ok(envelope) => {
                let records = records(&envelope);
                output.push(0);
                output.push(u8::from(envelope.sealed));
                hash::unsigned(&mut output, records.len() as u128);
                for (key, value) in &records {
                    hash::frame(&mut output, key);
                    hash::frame(&mut output, &hash::value_preimage(value, Projection::Object)?);
                }
            }
            Err(category) if category == UNSUPPORTED => return None,
            Err(category) => {
                output.push(1);
                hash::frame(&mut output, category.as_bytes());
            }
        }
    }
    Some(output)
}

/// Encode a column envelope case: decode the complete input, then write it back with its original choices.
pub(super) fn encode(document: &Json) -> Option<Vec<u8>> {
    let (data, supplied) = inputs(document)?;
    let mut budget = columns::case_budget(document)?;
    match read_columns(&data, &supplied, require_sealed(document), &mut budget) {
        Ok(envelope) => write_columns(&envelope),
        Err(category) if category == UNSUPPORTED => None,
        Err(_) => Some(Vec::new()),
    }
}

/// The literal input and the supplied reference descriptions.
fn inputs(document: &Json) -> Option<(Vec<u8>, Vec<rows::Meta>)> {
    let data = columns::hex(document.get("input_hex")?.as_str()?)?;
    let mut supplied = Vec::new();
    if let Some(items) = document.get("supplied_descriptions") {
        for item in items.as_array()? {
            supplied.push(rows::notation(item)?);
        }
    }
    Some((data, supplied))
}

/// Whether the case requires a sealed envelope.
fn require_sealed(document: &Json) -> bool {
    document.get("require_sealed").and_then(Json::as_bool).unwrap_or(false)
}

/// Check the header and frame, then admit the dictionary, every group and any seal.
fn read_columns(
    data: &[u8],
    supplied: &[rows::Meta],
    require_sealed: bool,
    budget: &mut Budget,
) -> Result<Envelope, Refused> {
    let mut cursor = Cursor::new(data);
    if cursor.take(4, budget)? != b"CRMB" {
        return Err("bad_magic");
    }
    budget.bytes(4)?;
    let (codec, profile) = (cursor.length(budget)?, cursor.length(budget)?);
    if codec != 2 || profile != 1 {
        return Err("unsupported_profile");
    }
    let flags = cursor.byte(budget)?;
    if flags & 254 != 0 {
        return Err("reserved_flags");
    }
    let payload = cursor.frame(budget)?;
    cursor.finished()?;
    let sealed = flags == 1;
    if require_sealed && !sealed {
        return Err("seal_required");
    }
    let mut body = Cursor::new(payload);
    let dictionary = rows::read_dictionary(&mut body, supplied, budget)?;
    let groups = read_groups(&mut body, &dictionary, budget)?;
    let envelope = Envelope {
        dictionary,
        groups,
        sealed,
    };
    if sealed {
        verify_seal(&mut body, &envelope, budget)?;
    }
    body.finished()?;
    Ok(envelope)
}

/// Read a strictly increasing fixed-width directory of `count + 1` offsets with a minimal width.
fn read_table(cursor: &mut Cursor, count: usize, budget: &mut Budget) -> Result<Vec<usize>, Refused> {
    let width = usize::from(cursor.byte(budget)?);
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Err("invalid_offset_width");
    }
    let size = count
        .checked_add(1)
        .and_then(|entries| entries.checked_mul(width))
        .ok_or("limit_exceeded")?;
    budget.elements(count as u64)?;
    let table = cursor.take(size, budget)?;
    let mut offsets = Vec::with_capacity(count + 1);
    for index in 0..=count {
        let current = columns::offset_at(table, index, width, budget)?;
        let previous = offsets.last().copied().unwrap_or(0);
        if (index == 0 && current != 0) || current < previous || (index > 0 && current == previous) {
            return Err("invalid_offsets");
        }
        offsets.push(current);
    }
    if width != columns::offset_width(offsets[count]) {
        return Err("nonminimal_offset_width");
    }
    Ok(offsets)
}

/// Read the row count, the group prefixes and directory, then every group's columns.
fn read_groups(cursor: &mut Cursor, dictionary: &[Stored], budget: &mut Budget) -> Result<Vec<Group>, Refused> {
    let rows = cursor.length(budget)?;
    let count = cursor.length(budget)?;
    budget.elements(rows as u64)?;
    if (rows == 0) != (count == 0) || count > rows {
        return Err("invalid_offsets");
    }
    let prefixes = read_table(cursor, count, budget)?;
    if prefixes[count] != rows {
        return Err("invalid_offsets");
    }
    let directory = read_table(cursor, count, budget)?;
    let data = cursor.take(directory[count], budget)?;
    let mut groups: Vec<Group> = Vec::with_capacity(count);
    for index in 0..count {
        let raw = &data[directory[index]..directory[index + 1]];
        let group = read_group(raw, prefixes[index + 1] - prefixes[index], dictionary, budget)?;
        if groups.last().is_some_and(|previous| previous.slot == group.slot) {
            return Err("nonmaximal_group");
        }
        budget.bytes(64)?;
        groups.push(group);
    }
    Ok(groups)
}

/// Read one group: its description slot and one complete column per field, or one root enum column.
fn read_group(raw: &[u8], rows: usize, dictionary: &[Stored], budget: &mut Budget) -> Result<Group, Refused> {
    let mut cursor = Cursor::new(raw);
    let slot = cursor.length(budget)?;
    let entry = dictionary.get(slot).ok_or("description_index")?;
    let count = if entry.record { entry.fields.len() } else { 1 };
    let (table, width, data) = columns::read_index(&mut cursor, count, budget)?;
    let mut group_columns = Vec::with_capacity(count);
    for position in 0..count {
        budget.elements(1)?;
        let left = columns::offset_at(table, position, width, budget)?;
        let right = columns::offset_at(table, position + 1, width, budget)?;
        let raw_column = data.get(left..right).ok_or("invalid_offsets")?;
        let (node, absent) = column_type(entry, position, budget)?;
        match blocks::read_column(&node, absent, rows, raw_column, dictionary, budget)? {
            Some(column) => group_columns.push(column),
            None => return Err(UNSUPPORTED),
        }
    }
    Ok(Group {
        slot,
        rows,
        columns: group_columns,
    })
}

/// The stored cell type and absence of one group column.
fn column_type(entry: &Stored, position: usize, budget: &mut Budget) -> Result<(CellType, char), Refused> {
    if !entry.record {
        let selector = rows::parse_key(&entry.key, false, budget)?;
        return Ok((blocks::from_node(&selector).ok_or(UNSUPPORTED)?, 'r'));
    }
    let field = &entry.fields[position];
    Ok((blocks::from_node(&field.storage).ok_or(UNSUPPORTED)?, field.absent))
}

/// Every record of the envelope in order, with its description selector.
fn records(envelope: &Envelope) -> Vec<(Vec<u8>, Value)> {
    let mut result = Vec::new();
    for group in &envelope.groups {
        let entry = &envelope.dictionary[group.slot];
        for position in 0..group.rows {
            let value = if entry.record {
                let fields = entry
                    .fields
                    .iter()
                    .zip(&group.columns)
                    .map(|(field, column)| hash::Field {
                        identity: field.identity.clone(),
                        canonical: field.canonical,
                        absent: field.absent,
                        value: column.1[position].clone(),
                    })
                    .collect();
                Value::Record(fields)
            } else {
                group.columns[0].1[position].clone()
            };
            result.push((entry.key.clone(), value));
        }
    }
    result
}

/// Hash logical record order, independently of physical choices, and compare the trailer.
fn verify_seal(cursor: &mut Cursor, envelope: &Envelope, budget: &mut Budget) -> Result<(), Refused> {
    let total: usize = envelope.groups.iter().map(|group| group.rows).sum();
    if cursor.length(budget)? != total {
        return Err("seal_count");
    }
    let expected = cursor.take(32, budget)?;
    if seal(envelope).ok_or("invalid_cell")?.as_slice() != expected {
        return Err("seal_mismatch");
    }
    Ok(())
}

/// The logical seal over every record's object contribution.
fn seal(envelope: &Envelope) -> Option<Vec<u8>> {
    let records = records(envelope);
    let mut preimage = b"crumb1\x00\x04".to_vec();
    hash::unsigned(&mut preimage, records.len() as u128);
    for (_, value) in &records {
        let mut contribution = Vec::new();
        hash::emit_object(value, &mut contribution)?;
        hash::frame(&mut preimage, &contribution);
    }
    Some(Sha256::digest(&preimage).to_vec())
}

/// Write the dictionary, prefix table, group directory and columns, and any seal, with the original choices.
fn write_columns(envelope: &Envelope) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    hash::unsigned(&mut body, envelope.dictionary.len() as u128);
    for entry in &envelope.dictionary {
        body.extend(row_write::encode_entry(entry)?);
    }
    let total: usize = envelope.groups.iter().map(|group| group.rows).sum();
    hash::unsigned(&mut body, total as u128);
    hash::unsigned(&mut body, envelope.groups.len() as u128);
    let width = columns::offset_width(total);
    body.push(width as u8);
    let mut end = 0usize;
    body.extend_from_slice(&0u64.to_le_bytes()[..width]);
    let mut groups = Vec::with_capacity(envelope.groups.len());
    for group in &envelope.groups {
        end += group.rows;
        body.extend_from_slice(&(end as u64).to_le_bytes()[..width]);
        groups.push(write_group(group, envelope)?);
    }
    body.extend(columns::indexed(&groups));
    if envelope.sealed {
        hash::unsigned(&mut body, total as u128);
        body.extend(seal(envelope)?);
    }
    let mut output = b"CRMB\x02\x01".to_vec();
    output.push(u8::from(envelope.sealed));
    hash::frame(&mut output, &body);
    Some(output)
}

/// Write one group: its description slot and indexed columns with their original block encodings.
fn write_group(group: &Group, envelope: &Envelope) -> Option<Vec<u8>> {
    let entry = &envelope.dictionary[group.slot];
    let mut budget = Budget::new(u64::MAX, u64::MAX, u64::MAX);
    let mut cells = Vec::with_capacity(group.columns.len());
    for (position, (encodings, values)) in group.columns.iter().enumerate() {
        let (node, absent) = column_type(entry, position, &mut budget).ok()?;
        cells.push(blocks::write_column(
            &node,
            absent,
            values.clone(),
            encodings,
            &envelope.dictionary,
        )?);
    }
    let mut output = Vec::new();
    hash::unsigned(&mut output, group.slot as u128);
    output.extend(columns::indexed(&cells));
    Some(output)
}
