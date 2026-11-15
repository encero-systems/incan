//! Native Crumb row cells: root records and enums, the presence bitmap and cell index, and every typed cell.
//!
//! This mirrors `cell_read`, `encoding_index.read_record_index` and `scalar_read`, refusal for refusal: each
//! refusal names the envelope offset, stored field and enum variant the Incan reader names.

use super::blocks::{fixed_integer, float_cell};
use super::columns::{self, Budget};
use super::hash::{self, Node, Projection, Value};
use super::row_cursor::{CellIndex, Place, Positioned, Span, at, read_index, read_text};
use super::rows::{Stored, StoredField};

/// Decode one root record or enum row whose bytes begin `origin` bytes into the envelope.
pub(super) fn read_root(
    stored: &Stored,
    raw: &[u8],
    origin: usize,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Value> {
    let mut cursor = Span::new(raw, origin);
    if stored.record {
        let (bitmap, index) = read_record_index(&stored.fields, &mut cursor, budget)?;
        return Ok(Value::Record(read_fields(
            &stored.fields,
            &bitmap,
            &index,
            dictionary,
            budget,
        )?));
    }
    let variant = cursor.length(budget)?;
    let variant = stored
        .variants
        .get(variant)
        .ok_or_else(|| at("variant_index", origin))?;
    let identity = &variant.identity;
    let (bitmap, index) =
        read_record_index(&variant.fields, &mut cursor, budget).map_err(|refusal| refusal.in_variant(identity))?;
    let fields = read_fields(&variant.fields, &bitmap, &index, dictionary, budget)
        .map_err(|refusal| refusal.in_variant(identity))?;
    Ok(Value::Enum(identity.clone(), fields))
}

/// The presence bitmap and its envelope offset.
struct Bitmap<'a> {
    bytes: &'a [u8],
    origin: usize,
}

impl Bitmap<'_> {
    /// One eligible field's presence bit.
    fn bit(&self, position: usize) -> bool {
        (self.bytes[position / 8] >> (position % 8)) & 1 == 1
    }
}

/// Read the bitmap of absent-policy fields, then the cell index of every present field.
fn read_record_index<'a>(
    fields: &[StoredField],
    cursor: &mut Span<'a>,
    budget: &mut Budget,
) -> Positioned<(Bitmap<'a>, CellIndex<'a>)> {
    budget.elements(fields.len() as u64).place(cursor.absolute())?;
    let eligible = fields.iter().filter(|field| field.absent != 'r').count();
    let origin = cursor.absolute();
    let size = eligible.div_ceil(8);
    let bitmap = Bitmap {
        bytes: cursor.take(size, budget)?,
        origin,
    };
    if eligible % 8 != 0 && bitmap.bytes[size - 1] >> (eligible % 8) != 0 {
        return Err(at("bitmap_padding", origin + size - 1));
    }
    let present_bits = rank(&bitmap, eligible, budget)?;
    let index = read_index(cursor, fields.len() - eligible + present_bits, budget)?;
    Ok((bitmap, index))
}

/// Read every field cell in identity order, naming the stored field of any refusal inside it.
fn read_fields(
    fields: &[StoredField],
    bitmap: &Bitmap,
    index: &CellIndex,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Vec<hash::Field>> {
    let (mut result, mut eligible, mut position) = (Vec::with_capacity(fields.len()), 0, 0);
    for field in fields {
        budget.elements(1).place(bitmap.origin)?;
        budget.bytes(6).place(bitmap.origin)?;
        let mut present = true;
        if field.absent != 'r' {
            present = bitmap.bit(eligible);
            eligible += 1;
        }
        let value = if present {
            let (view, origin) = index.selected(position, budget)?;
            position += 1;
            field_cell(field, view, origin, dictionary, budget).map_err(|refusal| refusal.in_field(&field.identity))?
        } else {
            absent_value(field, bitmap.origin)?
        };
        result.push(hash::Field {
            identity: field.identity.clone(),
            canonical: field.canonical,
            absent: field.absent,
            value,
        });
    }
    Ok(result)
}

/// One present field cell: the presence bit replaces one outer option tag; present empties are redundant.
fn field_cell(
    field: &StoredField,
    view: &[u8],
    origin: usize,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Value> {
    if field.absent == 'n' {
        let Node::Container(kind, arguments) = &field.storage else {
            return Err(at("invalid_description", origin));
        };
        if kind != "option" {
            return Err(at("invalid_description", origin));
        }
        return Ok(Value::Some(Box::new(cell(
            &arguments[0],
            view,
            origin,
            dictionary,
            budget,
        )?)));
    }
    let value = cell(&field.storage, view, origin, dictionary, budget)?;
    if field.absent == 'e' {
        match &value {
            Value::List(items) | Value::Set(items) if items.is_empty() => return Err(at("redundant_presence", origin)),
            Value::Map(entries) if entries.is_empty() => return Err(at("redundant_presence", origin)),
            Value::List(_) | Value::Set(_) | Value::Map(_) => {}
            _ => return Err(at("invalid_description", origin)),
        }
    }
    Ok(value)
}

/// Decode one typed cell under the shared depth allowance.
fn cell(node: &Node, view: &[u8], origin: usize, dictionary: &[Stored], budget: &mut Budget) -> Positioned<Value> {
    budget.elements(1).place(origin)?;
    budget.enter().place(origin)?;
    let result = cell_inner(node, view, origin, dictionary, budget);
    budget.leave();
    result
}

/// Interpret one cell only through its admitted written selector.
fn cell_inner(
    node: &Node,
    view: &[u8],
    origin: usize,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Value> {
    match node {
        Node::Scalar(kind) => scalar(kind, view, origin, budget),
        Node::Decimal(precision, scale) => decimal(*precision, *scale, view, origin, budget),
        Node::Reference(_) => scalar("reference", view, origin, budget),
        Node::Nominal(..) => {
            let key = hash::closed_selector_charged(node, budget).map_err(|kind| {
                at(
                    if kind == "limit_exceeded" {
                        kind
                    } else {
                        "invalid_selector"
                    },
                    origin,
                )
            })?;
            budget.elements(1).place(origin)?;
            budget.bytes(2 * key.len() as u64).place(origin)?;
            let entry = dictionary
                .iter()
                .find(|entry| entry.key == key)
                .ok_or_else(|| at("invalid_selector", origin))?;
            read_root(entry, view, origin, dictionary, budget)
        }
        Node::Container(kind, arguments) => container(kind, arguments, view, origin, dictionary, budget),
        _ => Err(at("invalid_selector", origin)),
    }
}

/// Decode one scalar cell of its admitted width and type.
fn scalar(kind: &str, view: &[u8], origin: usize, budget: &mut Budget) -> Positioned<Value> {
    if let Some(columns::KeyType::Integer(signed, width)) = columns::key_type(kind) {
        if view.len() != width {
            return Err(at("invalid_cell", origin));
        }
        budget.bytes(width as u64).place(origin)?;
        return Ok(fixed_integer(view, signed));
    }
    match kind {
        "fp32" => {
            budget.bytes(8).place(origin)?;
            return float_cell(view, 4).place(origin);
        }
        "fp64" => {
            budget.bytes(16).place(origin)?;
            return float_cell(view, 8).place(origin);
        }
        _ => {}
    }
    let mut cursor = Span::new(view, origin);
    let value = match kind {
        "boolean" => {
            if view.len() != 1 || cursor.byte(budget)? > 1 {
                return Err(at("invalid_cell", origin));
            }
            Value::Boolean(view[0] == 1)
        }
        "string" => Value::Text(read_text(&mut cursor, budget)?),
        "binary" => {
            let raw = cursor.frame(budget)?;
            budget.bytes(raw.len() as u64).place(origin)?;
            Value::Binary(raw.to_vec())
        }
        "secret_string" | "secret_bytes" => {
            if view.len() != 1 || cursor.byte(budget)? != 0 {
                return Err(at("invalid_cell", origin));
            }
            Value::Withheld
        }
        "reference" => {
            let target_view = cursor.frame(budget)?;
            budget.bytes(target_view.len() as u64).place(origin)?;
            let target = target_view.to_vec();
            let identity = cursor.frame(budget)?;
            budget.bytes(identity.len() as u64).place(origin)?;
            Value::Reference(target, identity.to_vec())
        }
        _ => return Err(at("invalid_cell", origin)),
    };
    if cursor.remaining() != 0 {
        return Err(at("invalid_cell", cursor.absolute()));
    }
    Ok(value)
}

/// Decode one decimal coefficient already at its declared scale, refusing excess precision.
fn decimal(precision: i64, scale: i64, view: &[u8], origin: usize, budget: &mut Budget) -> Positioned<Value> {
    let mut cursor = Span::new(view, origin);
    let groups = cursor.groups(budget)?;
    let coefficient = columns::unzigzag(&groups).ok_or_else(|| at("integer_overflow", origin))?;
    if columns::digits(coefficient) > precision || cursor.remaining() != 0 {
        return Err(at("invalid_cell", origin));
    }
    Ok(Value::Decimal(coefficient, scale))
}

/// Decode option, union, list, tuple, set and map cells with their tag, count and order checks.
fn container(
    kind: &str,
    arguments: &[Node],
    view: &[u8],
    origin: usize,
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Value> {
    let mut cursor = Span::new(view, origin);
    if kind == "option" {
        let value = match cursor.byte(budget)? {
            0 => Value::Absent,
            1 => {
                let child = cursor.frame(budget)?;
                let child_origin = cursor.absolute() - child.len();
                Value::Some(Box::new(cell(&arguments[0], child, child_origin, dictionary, budget)?))
            }
            _ => return Err(at("invalid_cell", origin)),
        };
        if cursor.remaining() != 0 {
            return Err(at("invalid_cell", cursor.absolute()));
        }
        return Ok(value);
    }
    if kind == "union" {
        return union(arguments, &mut cursor, dictionary, budget);
    }
    let count = cursor.length(budget)?;
    budget.elements(count as u64).place(origin)?;
    if kind == "tuple" && count != arguments.len() {
        return Err(at("arity_mismatch", origin));
    }
    let mut slots = count;
    if kind == "map" {
        if count > (i64::MAX as usize - 1) / 2 {
            return Err(at("limit_exceeded", origin));
        }
        slots = count * 2;
    }
    let index = read_index(&mut cursor, slots, budget)?;
    if kind == "map" {
        return map(arguments, &index, dictionary, budget);
    }
    let mut values: Vec<Value> = Vec::with_capacity(count);
    for position in 0..count {
        let (child, child_origin) = index.selected(position, budget)?;
        let element = if kind == "tuple" {
            &arguments[position]
        } else {
            &arguments[0]
        };
        let value = cell(element, child, child_origin, dictionary, budget)?;
        if kind == "set"
            && let Some(previous) = values.last()
        {
            checked_order(previous, &value, child_origin)?;
        }
        values.push(value);
    }
    match kind {
        "list" => Ok(Value::List(values)),
        "tuple" => Ok(Value::Tuple(values)),
        "set" => Ok(Value::Set(values)),
        _ => Err(at("invalid_selector", origin)),
    }
}

/// Read a map's key and value cells pair by pair, refusing each key not strictly after the one before it.
fn map(arguments: &[Node], index: &CellIndex, dictionary: &[Stored], budget: &mut Budget) -> Positioned<Value> {
    let mut entries: Vec<(Value, Value)> = Vec::with_capacity(index.count() / 2);
    for position in 0..index.count() / 2 {
        let (key_view, key_origin) = index.selected(position * 2, budget)?;
        let (value_view, value_origin) = index.selected(position * 2 + 1, budget)?;
        let key = cell(&arguments[0], key_view, key_origin, dictionary, budget)?;
        let value = cell(&arguments[1], value_view, value_origin, dictionary, budget)?;
        if let Some((previous, _)) = entries.last() {
            checked_order(previous, &key, key_origin)?;
        }
        entries.push((key, value));
    }
    Ok(Value::Map(entries))
}

/// Select a union member by its rank in the written selector order, then read the rest of the cell as that member.
fn union(arguments: &[Node], cursor: &mut Span, dictionary: &[Stored], budget: &mut Budget) -> Positioned<Value> {
    let offset = cursor.absolute();
    let requested = cursor.length(budget)?;
    if requested >= arguments.len() {
        return Err(at("union_index", offset));
    }
    for candidate in arguments {
        let candidate_key = hash::selector_key(candidate).ok_or_else(|| at("invalid_cell", offset))?;
        let mut rank = 0;
        for other in arguments {
            if hash::selector_key(other).ok_or_else(|| at("invalid_cell", offset))? < candidate_key {
                rank += 1;
            }
        }
        if rank == requested {
            let origin = cursor.absolute();
            let rest = cursor.rest(budget)?;
            let value = cell(candidate, rest, origin, dictionary, budget)?;
            return Ok(Value::Union(candidate.clone(), Box::new(value)));
        }
    }
    Err(at("union_index", offset))
}

/// Refuse a duplicate or descending stored key before any canonical projection could erase it.
fn checked_order(previous: &Value, current: &Value, origin: usize) -> Positioned<()> {
    match hash::compare(previous, current, Projection::Object).ok_or_else(|| at("invalid_cell", origin))? {
        std::cmp::Ordering::Equal => Err(at("duplicate_key", origin)),
        std::cmp::Ordering::Greater => Err(at("collection_order", origin)),
        std::cmp::Ordering::Less => Ok(()),
    }
}

/// One stored field of an admitted row, selected by identity: its cell bytes and envelope offset, or `None` for
/// declared absence, and the enum variant it belongs to.
pub(super) struct Selected<'a> {
    pub(super) field: &'a StoredField,
    pub(super) cell: Option<(&'a [u8], usize)>,
    bitmap_origin: usize,
    variant: Option<&'a [u8]>,
}

/// Select one field of an admitted row by identity, as `row_projection.project_view` does. The admission already
/// charged the row's bitmap and index, so they are read again outside `budget`; the selection itself is charged. A
/// refusal inside an enum names its variant.
pub(super) fn select_field<'a>(
    stored: &'a Stored,
    raw: &'a [u8],
    origin: usize,
    identity: &[u8],
    budget: &mut Budget,
) -> Positioned<Selected<'a>> {
    let mut admitted = Budget::new(u64::MAX, u64::MAX, u64::MAX);
    let mut cursor = Span::new(raw, origin);
    if stored.record {
        return select_in(&stored.fields, None, &mut cursor, identity, &mut admitted, budget);
    }
    let variant = cursor.length(&mut admitted)?;
    let variant = stored
        .variants
        .get(variant)
        .ok_or_else(|| at("variant_index", origin))?;
    select_in(
        &variant.fields,
        Some(&variant.identity),
        &mut cursor,
        identity,
        &mut admitted,
        budget,
    )
    .map_err(|refusal| refusal.in_variant(&variant.identity))
}

/// Find the field by binary search over identity order, then address its cell through its presence rank.
fn select_in<'a>(
    fields: &'a [StoredField],
    variant: Option<&'a [u8]>,
    cursor: &mut Span<'a>,
    identity: &[u8],
    admitted: &mut Budget,
    budget: &mut Budget,
) -> Positioned<Selected<'a>> {
    let (bitmap, index) = read_record_index(fields, cursor, admitted)?;
    let position = field_index(fields, identity, bitmap.origin, budget)?;
    let field = &fields[position];
    let required_before = fields[..position].iter().filter(|field| field.absent == 'r').count();
    let eligible_before = position - required_before;
    let mut selected = Selected {
        field,
        cell: None,
        bitmap_origin: bitmap.origin,
        variant,
    };
    if field.absent != 'r' && !bitmap.bit(eligible_before) {
        return Ok(selected);
    }
    let preceding = rank(&bitmap, eligible_before, budget)?;
    selected.cell = Some(index.selected(required_before + preceding, budget)?);
    Ok(selected)
}

/// The stored field with exactly this six-octet identity; any other identity is `missing_field`.
fn field_index(fields: &[StoredField], identity: &[u8], origin: usize, budget: &mut Budget) -> Positioned<usize> {
    if identity.len() != 6 {
        return Err(at("missing_field", origin));
    }
    budget.bytes(12).place(origin)?;
    let (mut low, mut high) = (0, fields.len());
    while low < high {
        budget.elements(1).place(origin)?;
        budget.bytes(12).place(origin)?;
        let middle = low + (high - low) / 2;
        match fields[middle].identity.as_slice().cmp(identity) {
            std::cmp::Ordering::Equal => return Ok(middle),
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
        }
    }
    Err(at("missing_field", origin))
}

/// Count the present eligible fields before one, charging each whole bitmap byte it scans.
fn rank(bitmap: &Bitmap, before: usize, budget: &mut Budget) -> Positioned<usize> {
    if before / 8 > bitmap.bytes.len() || (before / 8 == bitmap.bytes.len() && before % 8 != 0) {
        return Err(at("invalid_offsets", bitmap.origin));
    }
    let mut count = 0;
    for index in 0..before / 8 {
        budget.elements(1).place(bitmap.origin + index)?;
        count += bitmap.bytes[index].count_ones() as usize;
    }
    if before % 8 != 0 {
        count += (bitmap.bytes[before / 8] & ((1u8 << (before % 8)) - 1)).count_ones() as usize;
    }
    Ok(count)
}

/// Materialize a selected field's value, as `row_projection.project_value` does: a present cell is read under
/// `budget` and names the requested field in a refusal, and declared absence restores its stored absent value.
pub(super) fn read_selected(
    selected: &Selected,
    identity: &[u8],
    dictionary: &[Stored],
    budget: &mut Budget,
) -> Positioned<Value> {
    let value = match selected.cell {
        Some((view, origin)) => {
            field_cell(selected.field, view, origin, dictionary, budget).map_err(|refusal| refusal.in_field(identity))
        }
        None => absent_value(selected.field, selected.bitmap_origin),
    };
    match selected.variant {
        Some(variant) => value.map_err(|refusal| refusal.in_variant(variant)),
        None => value,
    }
}

/// A field's stored absence: none for an option, an empty collection for an empty policy.
fn absent_value(field: &StoredField, origin: usize) -> Positioned<Value> {
    match (field.absent, &field.storage) {
        ('n', _) => Ok(Value::Absent),
        ('e', Node::Container(kind, _)) if kind == "list" => Ok(Value::List(Vec::new())),
        ('e', Node::Container(kind, _)) if kind == "set" => Ok(Value::Set(Vec::new())),
        ('e', Node::Container(kind, _)) if kind == "map" => Ok(Value::Map(Vec::new())),
        _ => Err(at("invalid_description", origin)),
    }
}
