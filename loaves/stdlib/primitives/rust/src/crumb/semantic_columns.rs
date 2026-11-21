//! Native Crumb column dictionaries: exact key admission with sixteen-key restarts, and the key writer.
//!
//! This mirrors `column_key_read.read_keys` and `column_value_dictionary.build_keys`/`encode_keys`. Decoding reads
//! the literal input (or every proper prefix when the case asks for them) into keys or one typed refusal category;
//! encoding writes the case's independently authored keys. Column blocks and complete columns live in
//! `semantic_blocks.rs`, column envelopes in `semantic_envelope.rs`.

use std::cmp::Ordering;

use serde_json::Value as Json;

use super::hash::{self, Projection, Value};

/// One typed refusal category, spelled as the corpus and the Incan reader spell it.
pub(super) type Refused = &'static str;

const DEFAULT_BYTES: u64 = 67_108_864;
const DEFAULT_ELEMENTS: u64 = 4_194_304;

/// The decoder's byte and element allowance; exhausting either is `limit_exceeded`.
pub(super) struct Budget {
    bytes: u64,
    elements: u64,
    depth: u64,
    cause: Option<&'static str>,
}

impl Budget {
    /// A budget of exactly these bytes, elements and nesting depth.
    pub(super) fn new(bytes: u64, elements: u64, depth: u64) -> Self {
        Self {
            bytes,
            elements,
            depth,
            cause: None,
        }
    }

    /// Enter one nested level; exhausting the depth allowance is `limit_exceeded`.
    pub(super) fn enter(&mut self) -> Result<(), Refused> {
        let Some(depth) = self.depth.checked_sub(1) else {
            self.cause = Some("depth");
            return Err("limit_exceeded");
        };
        self.depth = depth;
        Ok(())
    }

    /// Leave one nested level.
    pub(super) fn leave(&mut self) {
        self.depth += 1;
    }

    pub(super) fn bytes(&mut self, count: u64) -> Result<(), Refused> {
        let Some(bytes) = self.bytes.checked_sub(count) else {
            self.cause = Some("bytes");
            return Err("limit_exceeded");
        };
        self.bytes = bytes;
        Ok(())
    }

    pub(super) fn elements(&mut self, count: u64) -> Result<(), Refused> {
        let Some(elements) = self.elements.checked_sub(count) else {
            self.cause = Some("elements");
            return Err("limit_exceeded");
        };
        self.elements = elements;
        Ok(())
    }

    /// The remaining nesting depth at the current cursor boundary.
    pub(super) fn depth(&self) -> u64 {
        self.depth
    }

    /// The bytes and elements still allowed.
    pub(super) fn left(&self) -> (u64, u64) {
        (self.bytes, self.elements)
    }

    /// The caller-owned resource whose charge failed, when this budget produced the refusal.
    pub(super) fn cause(&self) -> Option<&'static str> {
        self.cause
    }
}

/// The declared key type of one dictionary.
#[derive(Clone, Copy)]
pub(super) enum KeyType {
    Integer(bool, usize),
    Decimal(i64, i64),
    Text,
    Binary,
}

/// A checked cursor over one frame of the input.
pub(super) struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(super) fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    /// The unread rest of this frame, for a reader that continues it with its own cursor.
    pub(super) fn unread(&self) -> &'a [u8] {
        &self.data[self.position..]
    }

    /// Advance past bytes another reader consumed from `unread`.
    pub(super) fn skip(&mut self, count: usize) {
        self.position += count;
    }

    pub(super) fn byte(&mut self, budget: &mut Budget) -> Result<u8, Refused> {
        let value = *self.data.get(self.position).ok_or("truncated")?;
        budget.bytes(1)?;
        self.position += 1;
        Ok(value)
    }

    pub(super) fn take(&mut self, size: usize, budget: &mut Budget) -> Result<&'a [u8], Refused> {
        if size > self.remaining() {
            return Err("truncated");
        }
        budget.bytes(size as u64)?;
        let taken = &self.data[self.position..self.position + size];
        self.position += size;
        Ok(taken)
    }

    /// Read a shortest unsigned 64-bit length.
    pub(super) fn length(&mut self, budget: &mut Budget) -> Result<usize, Refused> {
        let mut value = 0u64;
        for index in 0..10 {
            budget.elements(1)?;
            let octet = self.byte(budget)?;
            let group = u64::from(octet & 0x7f);
            if index == 9 && (group > 1 || octet >= 0x80) {
                return Err("integer_overflow");
            }
            value += group << (7 * index);
            if octet < 0x80 {
                if index > 0 && group == 0 {
                    return Err("noncanonical_integer");
                }
                return usize::try_from(value)
                    .ok()
                    .filter(|count| *count <= i64::MAX as usize)
                    .ok_or("limit_exceeded");
            }
        }
        Err("integer_overflow")
    }

    pub(super) fn frame(&mut self, budget: &mut Budget) -> Result<&'a [u8], Refused> {
        let size = self.length(budget)?;
        self.take(size, budget)
    }

    /// Read arbitrary-width shortest base-128 groups.
    pub(super) fn groups(&mut self, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
        let mut result = Vec::new();
        loop {
            budget.elements(1)?;
            let octet = self.byte(budget)?;
            let group = octet & 0x7f;
            if octet < 0x80 && !result.is_empty() && group == 0 {
                return Err("noncanonical_integer");
            }
            budget.bytes(8)?;
            result.push(group);
            if octet < 0x80 {
                return Ok(result);
            }
        }
    }

    pub(super) fn finished(&self) -> Result<(), Refused> {
        if self.remaining() != 0 {
            return Err("trailing_bytes");
        }
        Ok(())
    }
}

/// Decode a column dictionary case: one status and keys preimage or refusal per decoder execution.
pub(super) fn decode(document: &Json) -> Option<Vec<u8>> {
    match document.get("operation")?.as_str()? {
        "column_dictionary" => {}
        "column_block" => return super::blocks::decode(document),
        "column_data" => return super::blocks::decode_column(document),
        "binary_decode" => return super::envelope::decode(document),
        _ => return None,
    }
    let data = hex(document.get("input_hex")?.as_str()?)?;
    let node = key_type(document.get("type")?.as_str()?);
    let lengths: Vec<usize> = if document.get("transform").is_some() {
        (0..data.len()).collect()
    } else {
        vec![data.len()]
    };
    let mut output = Vec::new();
    for length in lengths {
        let mut budget = case_budget(document)?;
        match read_keys(node, &data[..length], &mut budget) {
            Ok(keys) => {
                output.push(0);
                hash::frame(
                    &mut output,
                    &hash::value_preimage(&Value::List(keys), Projection::Object)?,
                );
            }
            Err(category) => {
                output.push(1);
                hash::frame(&mut output, category.as_bytes());
            }
        }
    }
    Some(output)
}

/// Encode a column dictionary case's independently authored keys; a case without keys writes nothing.
pub(super) fn encode(document: &Json) -> Option<Vec<u8>> {
    match document.get("operation")?.as_str()? {
        "column_dictionary" => {}
        "column_block" => return super::blocks::encode(document),
        "column_data" => return super::blocks::encode_column(document),
        "binary_decode" => return super::envelope::encode(document),
        _ => return None,
    }
    let expected = document.get("expected")?;
    let name = document.get("type")?.as_str()?;
    let keys = if let Some(values) = expected.get("values") {
        hash::logical_items(values)?
    } else if let Some(raw) = expected.get("keys_hex") {
        let mut keys = Vec::new();
        for item in raw.as_array()? {
            let bytes = hex(item.as_str()?)?;
            keys.push(match name {
                "binary" => Value::Binary(bytes.into()),
                "string" => Value::Text(String::from_utf8(bytes).ok()?),
                _ => return None,
            });
        }
        keys
    } else {
        return Some(Vec::new());
    };
    encode_keys(key_type(name)?, keys)
}

/// Corpus ceilings tighten the decoder's one real allowance.
pub(super) fn case_budget(document: &Json) -> Option<Budget> {
    let mut budget = Budget {
        bytes: DEFAULT_BYTES,
        elements: DEFAULT_ELEMENTS,
        depth: 128,
        cause: None,
    };
    if let Some(limits) = document.get("limits") {
        for key in ["elements", "records", "dictionary_entries", "bits"] {
            if let Some(value) = limits.get(key) {
                budget.elements = budget.elements.min(value.as_u64()?);
            }
        }
        if let Some(value) = limits.get("dictionary_bytes") {
            budget.bytes = value.as_u64()?;
        }
    }
    Some(budget)
}

/// Parse the dictionary key types; floats and other types are not dictionary types.
pub(super) fn key_type(name: &str) -> Option<KeyType> {
    if let Some(inner) = name.strip_prefix("decimal<").and_then(|rest| rest.strip_suffix('>')) {
        let (precision, scale) = inner.split_once(',')?;
        return Some(KeyType::Decimal(
            precision.trim().parse().ok()?,
            scale.trim().parse().ok()?,
        ));
    }
    match name {
        "string" => Some(KeyType::Text),
        "binary" => Some(KeyType::Binary),
        _ => {
            let signed = name.starts_with('i');
            if !signed && !name.starts_with('u') {
                return None;
            }
            let bits: usize = name.get(1..)?.parse().ok()?;
            matches!(bits, 8 | 16 | 32 | 64 | 128).then_some(KeyType::Integer(signed, bits / 8))
        }
    }
}

/// Validate every key and exact restart boundary, returning the keys in code order.
pub(super) fn read_keys(node: Option<KeyType>, data: &[u8], budget: &mut Budget) -> Result<Vec<Value>, Refused> {
    let node = node.ok_or("encoding_type")?;
    let mut cursor = Cursor::new(data);
    let count = cursor.length(budget)?;
    if count == 0 {
        return Err("dictionary_membership");
    }
    budget.elements(count as u64)?;
    let encoded = cursor.frame(budget)?;
    cursor.finished()?;
    let mut inner = Cursor::new(encoded);
    let chunks = count.div_ceil(16);
    let (table, width, entries_data) = read_index(&mut inner, chunks, budget)?;
    let mut entries = Cursor::new(entries_data);
    let mut keys: Vec<Value> = Vec::with_capacity(count);
    for index in 0..count {
        budget.elements(1)?;
        if index % 16 == 0 && entries.position != offset_at(table, index / 16, width, budget)? {
            return Err("restart_offset");
        }
        let value = next_key(node, index, keys.last(), &mut entries, budget)?;
        if let Some(previous) = keys.last()
            && hash::compare(previous, &value, Projection::Object).ok_or("invalid_cell")? != Ordering::Less
        {
            return Err("dictionary_order");
        }
        keys.push(value);
    }
    entries.finished()?;
    Ok(keys)
}

/// Check the restart table's width, bounds, order, completeness and minimality.
pub(super) fn read_index<'a>(
    cursor: &mut Cursor<'a>,
    count: usize,
    budget: &mut Budget,
) -> Result<(&'a [u8], usize, &'a [u8]), Refused> {
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
    let data = cursor.take(cursor.remaining(), budget)?;
    let mut previous = 0;
    for index in 0..=count {
        let current = offset_at(table, index, width, budget)?;
        if (index == 0 && current != 0) || current < previous || current > data.len() {
            return Err("invalid_offsets");
        }
        previous = current;
    }
    if previous != data.len() {
        return Err("invalid_offsets");
    }
    if width != offset_width(previous) {
        return Err("nonminimal_offset_width");
    }
    Ok((table, width, data))
}

/// Read one little-endian offset.
pub(super) fn offset_at(table: &[u8], index: usize, width: usize, budget: &mut Budget) -> Result<usize, Refused> {
    let start = index.checked_mul(width).ok_or("invalid_offsets")?;
    let bytes = table.get(start..start + width).ok_or("invalid_offsets")?;
    budget.bytes(width as u64)?;
    budget.elements(1)?;
    let value = bytes
        .iter()
        .rev()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    usize::try_from(value).map_err(|_| "invalid_offsets")
}

/// The smallest permitted offset width for a final offset.
pub(super) fn offset_width(length: usize) -> usize {
    match length {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    }
}

/// Read one key: an absolute restart, a positive numeric difference, or a maximal raw prefix and suffix.
fn next_key(
    node: KeyType,
    index: usize,
    previous: Option<&Value>,
    cursor: &mut Cursor,
    budget: &mut Budget,
) -> Result<Value, Refused> {
    if let KeyType::Integer(..) | KeyType::Decimal(..) = node {
        let raw = cursor.frame(budget)?;
        if index.is_multiple_of(16) {
            return numeric_cell(node, raw, budget);
        }
        let difference = positive_difference(raw, budget)?;
        let before = previous.ok_or("invalid_cell")?;
        return add_difference(node, before, difference).ok_or("value_overflow");
    }
    let prefix = cursor.length(budget)?;
    if index == 0 && prefix != 0 {
        return Err("invalid_prefix");
    }
    if index.is_multiple_of(16) && prefix != 0 {
        return Err("restart_prefix");
    }
    let before: Vec<u8> = match (index % 16, previous) {
        (0, _) | (_, None) => Vec::new(),
        (_, Some(Value::Binary(raw))) => raw.as_ref().to_vec(),
        (_, Some(Value::Text(text))) => text.as_bytes().to_vec(),
        _ => return Err("encoding_type"),
    };
    if prefix > before.len() {
        return Err("invalid_prefix");
    }
    let suffix = cursor.frame(budget)?;
    budget.bytes((prefix + suffix.len()) as u64)?;
    let mut raw = before[..prefix].to_vec();
    raw.extend_from_slice(suffix);
    if !index.is_multiple_of(16) {
        let longest = before
            .iter()
            .zip(&raw)
            .take_while(|(left, right)| left == right)
            .count();
        if longest != prefix {
            return Err("nonmaximal_prefix");
        }
    }
    match node {
        KeyType::Binary => Ok(Value::Binary(raw.into())),
        _ => String::from_utf8(raw).map(Value::Text).map_err(|_| "invalid_utf8"),
    }
}

/// Decode one restart cell: fixed little-endian integers or a Z-encoded decimal coefficient.
fn numeric_cell(node: KeyType, raw: &[u8], budget: &mut Budget) -> Result<Value, Refused> {
    match node {
        KeyType::Integer(signed, width) => {
            if raw.len() != width {
                return Err("invalid_cell");
            }
            budget.bytes(width as u64)?;
            let mut value = raw
                .iter()
                .rev()
                .fold(0u128, |value, byte| (value << 8) | u128::from(*byte));
            if !signed || raw[width - 1] < 0x80 {
                return Ok(Value::Integer(false, value));
            }
            if width < 16 {
                value = (1u128 << (width * 8)) - value;
            } else {
                value = value.wrapping_neg();
            }
            Ok(Value::Integer(true, value))
        }
        KeyType::Decimal(precision, scale) => {
            let mut cursor = Cursor::new(raw);
            let groups = cursor.groups(budget)?;
            let coefficient = unzigzag(&groups).ok_or("integer_overflow")?;
            if digits(coefficient) > precision || cursor.remaining() != 0 {
                return Err("invalid_cell");
            }
            Ok(Value::Decimal(coefficient, scale))
        }
        _ => Err("encoding_type"),
    }
}

/// Recover a signed coefficient from little-endian base-128 Z groups.
pub(super) fn unzigzag(groups: &[u8]) -> Option<i128> {
    let mut value = 0u128;
    for (index, group) in groups.iter().enumerate() {
        let shifted = u128::from(*group).checked_shl(7 * index as u32)?;
        if index > 0 && shifted >> (7 * index) != u128::from(*group) {
            return None;
        }
        value |= shifted;
    }
    let magnitude = value >> 1;
    if value & 1 == 0 {
        i128::try_from(magnitude).ok()
    } else {
        Some(-i128::try_from(magnitude).ok()? - 1)
    }
}

/// Count decimal magnitude digits; zero has one digit.
pub(super) fn digits(coefficient: i128) -> i64 {
    let mut remaining = coefficient.unsigned_abs();
    let mut count = 0;
    while remaining != 0 {
        remaining /= 10;
        count += 1;
    }
    count.max(1)
}

/// Read a shortest positive little-endian magnitude, which may be wider than any stored value.
fn positive_difference(raw: &[u8], budget: &mut Budget) -> Result<Option<u128>, Refused> {
    if raw.last().is_none_or(|last| *last == 0) {
        return Err("nonminimal_delta");
    }
    budget.elements(raw.len() as u64)?;
    budget.bytes(raw.len() as u64)?;
    if raw.len() > 16 {
        return Ok(None);
    }
    Ok(Some(
        raw.iter()
            .rev()
            .fold(0u128, |value, byte| (value << 8) | u128::from(*byte)),
    ))
}

/// Add a positive difference to the previous key and recheck the declared range.
fn add_difference(node: KeyType, before: &Value, difference: Option<u128>) -> Option<Value> {
    let difference = difference?;
    match (node, before) {
        (KeyType::Integer(signed, width), Value::Integer(negative, magnitude)) => {
            let (negative, magnitude) = if *negative {
                if difference >= *magnitude {
                    (false, difference - magnitude)
                } else {
                    (true, magnitude - difference)
                }
            } else {
                (false, magnitude.checked_add(difference)?)
            };
            in_range(signed, width, negative, magnitude).then_some(Value::Integer(negative, magnitude))
        }
        (KeyType::Decimal(precision, scale), Value::Decimal(coefficient, _)) => {
            let sum = coefficient.checked_add(i128::try_from(difference).ok()?)?;
            (digits(sum) <= precision).then_some(Value::Decimal(sum, scale))
        }
        _ => None,
    }
}

/// Whether a signed magnitude fits one fixed integer width and signedness.
pub(super) fn in_range(signed: bool, width: usize, negative: bool, magnitude: u128) -> bool {
    let bits = width * 8;
    match (signed, negative) {
        (false, true) => magnitude == 0,
        (false, false) => bits == 128 || magnitude < (1u128 << bits),
        (true, false) => magnitude < (1u128 << (bits - 1)),
        (true, true) => magnitude <= (1u128 << (bits - 1)),
    }
}

/// Deduplicate keys by their canonical cells and sort them by stored comparison.
pub(super) fn sorted_keys(node: KeyType, values: Vec<Value>) -> Option<Vec<(Vec<u8>, Value)>> {
    let mut distinct: Vec<(Vec<u8>, Value)> = Vec::with_capacity(values.len());
    for value in values {
        let cell = key_cell(node, &value)?;
        if !distinct.iter().any(|(seen, _)| *seen == cell) {
            distinct.push((cell, value));
        }
    }
    if distinct.is_empty() {
        return None;
    }
    let mut failed = false;
    distinct.sort_by(|left, right| {
        hash::compare(&left.1, &right.1, Projection::Object).unwrap_or_else(|| {
            failed = true;
            Ordering::Equal
        })
    });
    (!failed).then_some(distinct)
}

/// Write sorted keys with restarts every sixteen keys and a minimal offset directory.
pub(super) fn encode_keys(node: KeyType, values: Vec<Value>) -> Option<Vec<u8>> {
    let distinct = sorted_keys(node, values)?;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    for (index, (cell, value)) in distinct.iter().enumerate() {
        if index % 16 == 0 {
            chunks.push(Vec::new());
        }
        let chunk = chunks.last_mut()?;
        let previous = (index % 16 != 0).then(|| &distinct[index - 1]);
        match (node, previous) {
            (KeyType::Integer(..) | KeyType::Decimal(..), None) => hash::frame(chunk, cell),
            (KeyType::Integer(..) | KeyType::Decimal(..), Some((_, before))) => {
                hash::frame(chunk, &difference_bytes(before, value)?)
            }
            (_, before) => {
                let raw = raw_key(value)?;
                let prefix = before.map_or(0, |(_, before)| {
                    let before = raw_key(before).unwrap_or_default();
                    before
                        .iter()
                        .zip(&raw)
                        .take_while(|(left, right)| left == right)
                        .count()
                });
                hash::unsigned(chunk, prefix as u128);
                hash::frame(chunk, &raw[prefix..]);
            }
        }
    }
    let mut output = Vec::new();
    hash::unsigned(&mut output, distinct.len() as u128);
    hash::frame(&mut output, &indexed(&chunks));
    Some(output)
}

/// Lay out a width byte, little-endian offsets and the concatenated cells.
pub(super) fn indexed(cells: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = cells.iter().map(Vec::len).sum();
    let width = offset_width(total);
    let mut output = vec![width as u8];
    let mut position = 0usize;
    output.extend_from_slice(&(position as u64).to_le_bytes()[..width]);
    for cell in cells {
        position += cell.len();
        output.extend_from_slice(&(position as u64).to_le_bytes()[..width]);
    }
    for cell in cells {
        output.extend_from_slice(cell);
    }
    output
}

/// The canonical cell bytes that identify one key: fixed integers, Z decimals or framed raw keys.
pub(super) fn key_cell(node: KeyType, value: &Value) -> Option<Vec<u8>> {
    match (node, value) {
        (KeyType::Integer(signed, width), Value::Integer(negative, magnitude)) => {
            if !in_range(signed, width, *negative, *magnitude) {
                return None;
            }
            let bits = if *negative {
                magnitude.wrapping_neg()
            } else {
                *magnitude
            };
            Some(bits.to_le_bytes()[..width].to_vec())
        }
        (KeyType::Decimal(precision, scale), Value::Decimal(coefficient, original)) => {
            if *original != scale || digits(*coefficient) > precision {
                return None;
            }
            let mut cell = Vec::new();
            hash::zigzag(&mut cell, coefficient.unsigned_abs(), *coefficient < 0);
            Some(cell)
        }
        (KeyType::Text | KeyType::Binary, _) => {
            let mut cell = Vec::new();
            hash::frame(&mut cell, &raw_key(value)?);
            Some(cell)
        }
        _ => None,
    }
}

/// Raw UTF-8 or binary octets used by prefix compression.
pub(super) fn raw_key(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Text(text) => Some(text.as_bytes().to_vec()),
        Value::Binary(data) => Some(data.as_ref().to_vec()),
        _ => None,
    }
}

/// The shortest little-endian magnitude of a strictly positive difference between sorted keys.
fn difference_bytes(before: &Value, value: &Value) -> Option<Vec<u8>> {
    let difference = match (before, value) {
        (Value::Integer(before_negative, before), Value::Integer(negative, magnitude)) => {
            match (before_negative, negative) {
                (false, false) => magnitude.checked_sub(*before)?,
                (true, false) => magnitude.checked_add(*before)?,
                (true, true) => before.checked_sub(*magnitude)?,
                (false, true) => return None,
            }
        }
        (Value::Decimal(before, _), Value::Decimal(coefficient, _)) => {
            u128::try_from(coefficient.checked_sub(*before)?).ok()?
        }
        _ => return None,
    };
    if difference == 0 {
        return None;
    }
    let bytes = difference.to_le_bytes();
    let used = 16 - (difference.leading_zeros() as usize / 8);
    Some(bytes[..used].to_vec())
}

/// Decode strict hexadecimal octets.
pub(super) fn hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect()
}
