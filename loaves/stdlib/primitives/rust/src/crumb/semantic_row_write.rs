//! Native Crumb row envelope writing: descriptions in their original modes, typed rows and the logical seal.
//!
//! This mirrors `row_writer.write_rows`, `descriptions.encode_entry` and the row cell writers in `cells`.

use sha2::{Digest, Sha256};

use super::columns::{self, Budget};
use super::hash::{self, Node, Value};
use super::rows::{Meta, Rows, Stored, StoredField, encode_metadata};

type Refused = &'static str;

/// Encode one admitted description with the writer contract's exact bounded-copy charges.
pub(super) fn encode_entry_charged(entry: &Stored, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    let mut output = ChargedWriter::new();
    if !entry.inline {
        output.octet(1, budget)?;
        output.frame(&entry.key, budget)?;
        output.append(&entry.digest, budget)?;
        return output.finish(budget);
    }
    let members = if entry.record { "fields" } else { "variants" };
    let header = without_charged(&entry.metadata, members, budget)?;
    let mut body = ChargedWriter::new();
    let header_bytes = metadata_bytes_charged(&header, budget)?;
    body.frame(&header_bytes, budget)?;
    if entry.record {
        encode_stored_fields_charged(&entry.fields, &mut body, budget)?;
    } else {
        body.count(entry.variants.len(), budget)?;
        for variant in &entry.variants {
            let header = without_charged(&variant.metadata, "fields", budget)?;
            let mut payload = ChargedWriter::new();
            let header_bytes = metadata_bytes_charged(&header, budget)?;
            payload.frame(&header_bytes, budget)?;
            encode_stored_fields_charged(&variant.fields, &mut payload, budget)?;
            body.count(variant.position, budget)?;
            let payload = payload.finish(budget)?;
            body.frame(&payload, budget)?;
        }
    }
    output.octet(0, budget)?;
    let body = body.finish(budget)?;
    output.frame(&body, budget)?;
    output.finish(budget)
}

/// Validate and encode one root value against its stored description under the writer's allowance.
pub(super) fn write_root_charged(
    entry: &Stored,
    value: &Value,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match (entry.record, value) {
        (true, Value::Record(fields)) => write_fields_charged(&entry.fields, fields, dictionary, budget),
        (false, Value::Enum(identity, fields)) => {
            let mut selected = None;
            for (index, variant) in entry.variants.iter().enumerate() {
                budget.elements(1)?;
                if variant.identity == *identity {
                    selected = Some((index, &variant.fields));
                    break;
                }
            }
            let (index, shape) = selected.ok_or("invalid_cell")?;
            let mut output = ChargedWriter::new();
            output.count(index, budget)?;
            let encoded = write_fields_charged(shape, fields, dictionary, budget)?;
            output.append(&encoded, budget)?;
            output.finish(budget)
        }
        _ => Err("invalid_cell"),
    };
    budget.leave();
    result
}

fn encode_stored_fields_charged(
    fields: &[StoredField],
    output: &mut ChargedWriter,
    budget: &mut Budget,
) -> Result<(), Refused> {
    output.count(fields.len(), budget)?;
    for field in fields {
        output.count(field.position, budget)?;
        let metadata = metadata_bytes_charged(&field.metadata, budget)?;
        output.frame(&metadata, budget)?;
    }
    Ok(())
}

fn metadata_bytes_charged(value: &Meta, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    let mut output = Vec::new();
    encode_metadata(value, &mut output, budget)?;
    budget.bytes(output.len() as u64)?;
    Ok(output)
}

fn without_charged(value: &Meta, name: &str, budget: &mut Budget) -> Result<Meta, Refused> {
    let Meta::Map(entries) = value else {
        return Err("invalid_description");
    };
    let mut retained = Vec::new();
    for (key, child) in entries {
        budget.elements(1)?;
        if key != name {
            (0..4).try_for_each(|_| budget.bytes(key.len() as u64))?;
            charge_meta_copy(child, budget)?;
            retained.push((key.clone(), child.clone()));
        }
    }
    Ok(Meta::Map(retained))
}

fn charge_meta_copy(value: &Meta, budget: &mut Budget) -> Result<(), Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match value {
        Meta::Null | Meta::Boolean(_) => budget.bytes(1),
        Meta::Integer(_) => budget.bytes(16),
        Meta::Text(text) | Meta::Symbol(text) => (0..4).try_for_each(|_| budget.bytes(text.len() as u64)),
        Meta::Binary(data) => budget.bytes(data.len() as u64),
        Meta::List(items) => items.iter().try_for_each(|item| charge_meta_copy(item, budget)),
        Meta::Map(entries) => entries.iter().try_for_each(|(key, item)| {
            budget.elements(1)?;
            (0..4).try_for_each(|_| budget.bytes(key.len() as u64))?;
            charge_meta_copy(item, budget)
        }),
    };
    budget.leave();
    result
}

fn write_fields_charged(
    shape: &[StoredField],
    fields: &[hash::Field],
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    if shape.len() != fields.len() {
        return Err("invalid_cell");
    }
    let mut seen = std::collections::BTreeSet::new();
    for field in fields {
        budget.elements(1)?;
        budget.bytes(12)?;
        if field.identity.len() != 6 || !seen.insert(field.identity.as_slice()) {
            return Err("invalid_cell");
        }
    }
    let mut bitmap = Vec::new();
    let mut cells = Vec::new();
    let mut eligible = 0usize;
    for field in shape {
        budget.elements(1)?;
        budget.bytes(12)?;
        let source = fields
            .iter()
            .find(|candidate| candidate.identity == field.identity)
            .ok_or("invalid_cell")?;
        if source.canonical != field.canonical || source.absent != field.absent {
            return Err("invalid_cell");
        }
        let absent = absent(field, &source.value);
        if field.absent != 'r' {
            if eligible.is_multiple_of(8) {
                budget.bytes(1)?;
                bitmap.push(0u8);
            }
            if !absent {
                bitmap[eligible / 8] |= 1 << (eligible % 8);
            }
            eligible += 1;
        }
        if !absent {
            budget.elements(1)?;
            cells.push(write_field_cell(field, &source.value, dictionary, budget)?);
        }
    }
    let indexed = indexed_charged(cells, budget)?;
    let mut output = ChargedWriter::new();
    output.append(&bitmap, budget)?;
    output.append(&indexed, budget)?;
    output.finish(budget)
}

fn absent(field: &StoredField, value: &Value) -> bool {
    match (field.absent, &field.storage, value) {
        ('n', _, Value::Absent) => true,
        ('e', Node::Container(kind, _), Value::List(items) | Value::Set(items)) if kind != "map" => items.is_empty(),
        ('e', Node::Container(kind, _), Value::Map(items)) if kind == "map" => items.is_empty(),
        _ => false,
    }
}

fn write_field_cell(
    field: &StoredField,
    value: &Value,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    if field.absent == 'n' {
        let (Node::Container(kind, arguments), Value::Some(inner)) = (&field.storage, value) else {
            return Err("invalid_cell");
        };
        if kind != "option" || arguments.len() != 1 {
            return Err("invalid_cell");
        }
        return write_cell_charged(&arguments[0], inner, dictionary, budget);
    }
    write_cell_charged(&field.storage, value, dictionary, budget)
}

fn write_cell_charged(
    node: &Node,
    value: &Value,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match node {
        Node::Scalar(kind) => write_scalar_charged(kind, value, budget),
        Node::Reference(arguments) if matches!(arguments.as_ref(), Node::Scalar(_)) => {
            write_scalar_charged("reference", value, budget)
        }
        Node::Reference(_) => write_scalar_charged("reference", value, budget),
        Node::Decimal(precision, scale) => write_decimal_charged(*precision, *scale, value, budget),
        Node::Nominal(..) => {
            let key = hash::closed_selector_charged(node, budget)?;
            let index = dictionary_slot(dictionary, &key, budget)?;
            write_root_charged(&dictionary[index], value, dictionary, budget)
        }
        Node::Container(kind, arguments) => write_container_charged(kind, arguments, value, dictionary, budget),
        _ => Err("invalid_cell"),
    };
    budget.leave();
    result
}

fn dictionary_slot(dictionary: &[Stored], key: &[u8], budget: &mut Budget) -> Result<usize, Refused> {
    budget.elements(1)?;
    budget.bytes(2 * key.len() as u64)?;
    dictionary
        .iter()
        .position(|entry| entry.key == key)
        .ok_or("invalid_cell")
}

fn write_scalar_charged(kind: &str, value: &Value, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    if let Some(key_type @ columns::KeyType::Integer(..)) = columns::key_type(kind) {
        let data = columns::key_cell(key_type, value).ok_or("invalid_cell")?;
        budget.bytes(data.len() as u64)?;
        return Ok(data);
    }
    match (kind, value) {
        ("boolean", Value::Boolean(flag)) => {
            budget.bytes(1)?;
            Ok(vec![u8::from(*flag)])
        }
        ("fp32", Value::Float(bits)) => float_cell(bits, 32, budget),
        ("fp64", Value::Float(bits)) => float_cell(bits, 64, budget),
        ("string", Value::Text(text)) => {
            (0..4).try_for_each(|_| budget.bytes(text.len() as u64))?;
            frame_charged(text.as_bytes(), budget)
        }
        ("binary", Value::Binary(data)) => frame_charged(data, budget),
        ("secret_string" | "secret_bytes", Value::Withheld) => {
            budget.bytes(1)?;
            Ok(vec![0])
        }
        ("reference", Value::Reference(target, identity)) => {
            let mut output = ChargedWriter::new();
            output.frame(target, budget)?;
            output.frame(identity, budget)?;
            output.finish(budget)
        }
        _ => Err("invalid_cell"),
    }
}

fn float_cell(bits: &[u8], target: i64, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    if bits.len() != 8 {
        return Err("invalid_cell");
    }
    budget.bytes(8)?;
    let wide = f64::from_bits(u64::from_be_bytes(bits.try_into().map_err(|_| "invalid_cell")?));
    if target == 64 {
        budget.bytes(8)?;
        return Ok(wide.to_bits().to_le_bytes().to_vec());
    }
    let narrow = wide as f32;
    if !wide.is_nan() && f64::from(narrow).to_bits() != wide.to_bits() {
        return Err("invalid_cell");
    }
    let data = if wide.is_nan() {
        0x7fc0_0000u32
    } else {
        narrow.to_bits()
    }
    .to_le_bytes();
    let mut output = ChargedWriter::new();
    for byte in data {
        output.octet(byte, budget)?;
    }
    output.finish(budget)
}

fn write_decimal_charged(precision: i64, scale: i64, value: &Value, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    let Value::Decimal(coefficient, original_scale) = value else {
        return Err("invalid_cell");
    };
    if precision <= 0 || precision > 38 || scale < 0 || scale > precision {
        return Err("invalid_cell");
    }
    let (mut coefficient, normalized_scale) = normalize_decimal(*coefficient, *original_scale)?;
    let digits = decimal_digits(coefficient);
    if digits > precision || normalized_scale < scale - (precision - digits) || normalized_scale > scale {
        return Err("invalid_cell");
    }
    let extra = scale - normalized_scale;
    budget.elements(extra as u64)?;
    for _ in 0..extra {
        coefficient = coefficient.checked_mul(10).ok_or("invalid_cell")?;
    }
    let encoded = crate::crumb::encode_native_signed(coefficient);
    charge_encoded(&encoded, budget)?;
    Ok(encoded.data)
}

fn normalize_decimal(mut coefficient: i128, mut scale: i64) -> Result<(i128, i64), Refused> {
    if scale < 0 || scale > 38 {
        return Err("invalid_cell");
    }
    while scale > 0 && coefficient % 10 == 0 {
        coefficient /= 10;
        scale -= 1;
    }
    Ok((coefficient, scale))
}

fn decimal_digits(coefficient: i128) -> i64 {
    let mut value = coefficient.unsigned_abs();
    let mut digits = 0;
    while value != 0 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn write_container_charged(
    kind: &str,
    arguments: &[Node],
    value: &Value,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    if kind == "option" {
        let mut output = ChargedWriter::new();
        match value {
            Value::Absent => output.octet(0, budget)?,
            Value::Some(inner) if arguments.len() == 1 => {
                output.octet(1, budget)?;
                let cell = write_cell_charged(&arguments[0], inner, dictionary, budget)?;
                output.frame(&cell, budget)?;
            }
            _ => return Err("invalid_cell"),
        }
        return output.finish(budget);
    }
    if kind == "union" {
        return write_union_charged(arguments, value, dictionary, budget);
    }
    let (items, map) = match (kind, value) {
        ("list", Value::List(items)) => (ordered_values(items, false)?, None),
        ("set", Value::Set(items)) => (ordered_values(items, true)?, None),
        ("tuple", Value::Tuple(items)) if items.len() == arguments.len() => ((0..items.len()).collect(), None),
        ("map", Value::Map(entries)) => (Vec::new(), Some(ordered_entries(entries)?)),
        _ => return Err("invalid_cell"),
    };
    let count = map.as_ref().map_or(items.len(), Vec::len);
    let mut output = ChargedWriter::new();
    output.count(count, budget)?;
    let mut cells = Vec::new();
    if let Some(order) = map {
        let Value::Map(entries) = value else { unreachable!() };
        if arguments.len() != 2 {
            return Err("invalid_cell");
        }
        for index in order {
            cells.push(write_cell_charged(
                &arguments[0],
                &entries[index].0,
                dictionary,
                budget,
            )?);
            cells.push(write_cell_charged(
                &arguments[1],
                &entries[index].1,
                dictionary,
                budget,
            )?);
        }
    } else {
        let values = match value {
            Value::List(values) | Value::Set(values) | Value::Tuple(values) => values,
            _ => unreachable!(),
        };
        for index in items {
            let node = if kind == "tuple" {
                arguments.get(index)
            } else {
                arguments.first()
            }
            .ok_or("invalid_cell")?;
            cells.push(write_cell_charged(node, &values[index], dictionary, budget)?);
        }
    }
    let indexed = indexed_charged(cells, budget)?;
    output.append(&indexed, budget)?;
    output.finish(budget)
}

fn write_union_charged(
    arguments: &[Node],
    value: &Value,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Result<Vec<u8>, Refused> {
    let Value::Union(member, inner) = value else {
        return Err("invalid_cell");
    };
    let selected = hash::selector_key(member).ok_or("invalid_cell")?;
    let mut found = false;
    let mut rank = 0usize;
    for candidate in arguments {
        budget.elements(1)?;
        let key = hash::selector_key(candidate).ok_or("invalid_cell")?;
        if key == selected {
            found = true;
        }
        if key < selected {
            rank += 1;
        }
    }
    if !found {
        return Err("invalid_cell");
    }
    let mut output = ChargedWriter::new();
    output.count(rank, budget)?;
    let cell = write_cell_charged(member, inner, dictionary, budget)?;
    output.append(&cell, budget)?;
    output.finish(budget)
}

fn ordered_values(values: &[Value], sort: bool) -> Result<Vec<usize>, Refused> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    if sort {
        order.sort_by(|left, right| {
            hash::compare(&values[*left], &values[*right], hash::Projection::Object)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if order.windows(2).any(|pair| {
            hash::compare(&values[pair[0]], &values[pair[1]], hash::Projection::Object)
                == Some(std::cmp::Ordering::Equal)
        }) {
            return Err("invalid_cell");
        }
    }
    Ok(order)
}

fn ordered_entries(entries: &[(Value, Value)]) -> Result<Vec<usize>, Refused> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|left, right| {
        hash::compare(&entries[*left].0, &entries[*right].0, hash::Projection::Object)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if order.windows(2).any(|pair| {
        hash::compare(&entries[pair[0]].0, &entries[pair[1]].0, hash::Projection::Object)
            == Some(std::cmp::Ordering::Equal)
    }) {
        return Err("invalid_cell");
    }
    Ok(order)
}

fn indexed_charged(cells: Vec<Vec<u8>>, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    let elements = u64::try_from(cells.len()).map_err(|_| "limit_exceeded")?;
    let data = columns::indexed(&cells);
    let copied_bytes = u64::try_from(data.len())
        .map_err(|_| "limit_exceeded")?
        .checked_mul(2)
        .ok_or("limit_exceeded")?;
    budget.bytes(copied_bytes)?;
    budget.elements(elements)?;
    Ok(data)
}

fn frame_charged(data: &[u8], budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    let encoded = crate::crumb::frame(data.to_vec());
    charge_encoded(&encoded, budget)?;
    Ok(encoded.data)
}

fn charge_encoded(encoded: &crate::crumb::Encoded, budget: &mut Budget) -> Result<(), Refused> {
    budget.bytes(encoded.byte_charge)?;
    budget.elements(encoded.element_charge)
}

struct ChargedWriter {
    data: Vec<u8>,
}

impl ChargedWriter {
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    fn append(&mut self, value: &[u8], budget: &mut Budget) -> Result<(), Refused> {
        self.data.len().checked_add(value.len()).ok_or("limit_exceeded")?;
        budget.bytes(value.len() as u64)?;
        self.data.extend_from_slice(value);
        Ok(())
    }

    fn octet(&mut self, value: u8, budget: &mut Budget) -> Result<(), Refused> {
        budget.bytes(1)?;
        self.data.push(value);
        Ok(())
    }

    fn count(&mut self, value: usize, budget: &mut Budget) -> Result<(), Refused> {
        let encoded = crate::crumb::encode_native_unsigned(value as u128);
        charge_encoded(&encoded, budget)?;
        self.append(&encoded.data, budget)
    }

    fn frame(&mut self, value: &[u8], budget: &mut Budget) -> Result<(), Refused> {
        self.count(value.len(), budget)?;
        self.append(value, budget)
    }

    fn finish(self, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
        budget.bytes(self.data.len() as u64)?;
        Ok(self.data)
    }
}

/// Write the decoded descriptions in their original modes, every record and any seal.
pub(super) fn write_rows(rows: &Rows) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    hash::unsigned(&mut body, rows.dictionary.len() as u128);
    for entry in rows.dictionary.iter() {
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
pub(super) fn encode_entry(entry: &Stored) -> Option<Vec<u8>> {
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
pub(super) fn write_root(entry: &Stored, value: &Value, dictionary: &[Stored]) -> Option<Vec<u8>> {
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
