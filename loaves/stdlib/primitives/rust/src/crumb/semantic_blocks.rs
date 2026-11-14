//! Native Crumb column blocks: the six block encodings over at most 256 logical rows, read and written.
//!
//! This mirrors `column_block_read.read_block` and `column_block_write.write_block`: a presence bitmap for declared
//! absence, then indexed cells, frame-of-reference or zigzag-delta packed numbers, typed runs, or dictionary codes.
//! Numeric reconstruction uses 128-bit signed arithmetic; a block whose exact numbers leave that range is reported
//! unsupported rather than guessed.

use serde_json::Value as Json;

use super::columns::{self, Budget, Cursor, KeyType, Refused};
use super::hash::{self, Projection, Value};

/// One stored cell type.
#[derive(Clone)]
enum CellType {
    Integer(bool, usize),
    Decimal(i64, i64),
    Text,
    Binary,
    Boolean,
    Float(usize),
    Option(Box<CellType>),
    List(Box<CellType>),
}

/// A decoded block: its encoding identifier and every logical row value.
type Decoded = (u8, Vec<Value>);

/// Decode a column block case: one status, encoding and values preimage or refusal per execution.
pub(super) fn decode(document: &Json) -> Option<Vec<u8>> {
    let (node, absent, keys) = block_context(document)?;
    let data = columns::hex(document.get("input_hex")?.as_str()?)?;
    let rows = document.get("logical_count")?.as_i64()?;
    let lengths: Vec<usize> = if document.get("transform").is_some() {
        (0..data.len()).collect()
    } else {
        vec![data.len()]
    };
    let mut output = Vec::new();
    for length in lengths {
        let mut budget = columns::case_budget(document)?;
        match read_block(&node, absent, rows, &data[..length], keys.as_deref(), &mut budget) {
            Ok(Some((encoding, values))) => {
                output.push(0);
                hash::unsigned(&mut output, u128::from(encoding));
                hash::frame(
                    &mut output,
                    &hash::value_preimage(&Value::List(values), Projection::Object)?,
                );
            }
            Ok(None) => return None,
            Err(category) => {
                output.push(1);
                hash::frame(&mut output, category.as_bytes());
            }
        }
    }
    Some(output)
}

/// Encode a column block case's authored values with its authored encoding; a refused case writes nothing.
pub(super) fn encode(document: &Json) -> Option<Vec<u8>> {
    let (node, absent, keys) = block_context(document)?;
    let expected = document.get("expected")?;
    let Some(values) = expected.get("values") else {
        return Some(Vec::new());
    };
    let encoding = u8::try_from(expected.get("encoding")?.as_u64()?).ok()?;
    write_block(&node, absent, hash::logical_items(values)?, encoding, keys.as_deref())
}

/// Parse the declared type and absence, and build any supplied external dictionary keys.
fn block_context(document: &Json) -> Option<(CellType, char, Option<Vec<Value>>)> {
    let name = document.get("type")?.as_str()?;
    let node = cell_type(name)?;
    let absent = absence(document)?;
    let stored = stored_type(&node, absent)?;
    let mut key_type = key_type_of(&stored);
    let supplied = if let Some(values) = document.get("dictionary_values") {
        if key_type.is_none() {
            key_type = columns::key_type(values.as_array()?.first()?.get("type")?.as_str()?);
        }
        hash::logical_items(values)?
    } else if let Some(raw) = document.get("dictionary_keys_hex") {
        let mut keys = Vec::new();
        for item in raw.as_array()? {
            let bytes = columns::hex(item.as_str()?)?;
            keys.push(match name {
                "binary" => Value::Binary(bytes),
                _ => Value::Text(String::from_utf8(bytes).ok()?),
            });
        }
        keys
    } else {
        Vec::new()
    };
    if supplied.is_empty() {
        return Some((node, absent, None));
    }
    let sorted = columns::sorted_keys(key_type?, supplied)?;
    Some((node, absent, Some(sorted.into_iter().map(|(_, value)| value).collect())))
}

/// Parse one column type spelling, including the legacy `f64` alias.
fn cell_type(name: &str) -> Option<CellType> {
    if let Some(inner) = name.strip_prefix("option<").and_then(|rest| rest.strip_suffix('>')) {
        return Some(CellType::Option(Box::new(cell_type(inner)?)));
    }
    if let Some(inner) = name.strip_prefix("list<").and_then(|rest| rest.strip_suffix('>')) {
        return Some(CellType::List(Box::new(cell_type(inner)?)));
    }
    Some(match name {
        "boolean" => CellType::Boolean,
        "fp32" => CellType::Float(4),
        "fp64" | "f64" => CellType::Float(8),
        _ => match columns::key_type(name)? {
            KeyType::Integer(signed, width) => CellType::Integer(signed, width),
            KeyType::Decimal(precision, scale) => CellType::Decimal(precision, scale),
            KeyType::Text => CellType::Text,
            KeyType::Binary => CellType::Binary,
        },
    })
}

/// The dictionary key type of a stored cell type, when it has one.
fn key_type_of(node: &CellType) -> Option<KeyType> {
    match node {
        CellType::Integer(signed, width) => Some(KeyType::Integer(*signed, *width)),
        CellType::Decimal(precision, scale) => Some(KeyType::Decimal(*precision, *scale)),
        CellType::Text => Some(KeyType::Text),
        CellType::Binary => Some(KeyType::Binary),
        _ => None,
    }
}

/// Validate the type/absence pair and remove exactly one optional layer under declared `None` absence.
fn stored_type(node: &CellType, absent: char) -> Option<CellType> {
    match (absent, node) {
        ('r', _) => Some(node.clone()),
        ('n', CellType::Option(inner)) => Some((**inner).clone()),
        ('e', CellType::List(_)) => Some(node.clone()),
        _ => None,
    }
}

/// Admit one block and restore every logical row; `Ok(None)` marks numbers outside native arithmetic.
fn read_block(
    node: &CellType,
    absent: char,
    rows: i64,
    data: &[u8],
    keys: Option<&[Value]>,
    budget: &mut Budget,
) -> Result<Option<Decoded>, Refused> {
    let Some(stored) = stored_type(node, absent) else {
        return Ok(None);
    };
    if !(1..=256).contains(&rows) {
        return Err("block_count");
    }
    let rows = rows as usize;
    let mut cursor = Cursor::new(data);
    let bitmap = if absent == 'r' {
        &[][..]
    } else {
        cursor.take(rows.div_ceil(8), budget)?
    };
    let mut present = rows;
    if absent != 'r' {
        if rows % 8 != 0 {
            budget.bytes(1)?;
            if bitmap[bitmap.len() - 1] >> (rows % 8) != 0 {
                return Err("nonzero_padding");
            }
        }
        present = (0..rows).filter(|row| bit(bitmap, *row)).count();
    }
    let encoding = cursor.length(budget)?;
    if encoding > 5 {
        return Err("unsupported_encoding");
    }
    let encoding = encoding as u8;
    let stored_values = match stored_values(&stored, encoding, present, &mut cursor, keys, budget)? {
        Some(values) => values,
        None => return Ok(None),
    };
    let mut values = Vec::with_capacity(rows);
    let mut position = 0;
    for row in 0..rows {
        budget.elements(1)?;
        budget.bytes(16)?;
        if absent != 'r' && !bit(bitmap, row) {
            values.push(match absent {
                'n' => Value::Absent,
                _ => Value::List(Vec::new()),
            });
            continue;
        }
        let value = stored_values[position].clone();
        position += 1;
        values.push(match (absent, value) {
            ('n', value) => Value::Some(Box::new(value)),
            ('e', Value::List(items)) if items.is_empty() => return Err("redundant_absence"),
            (_, value) => value,
        });
    }
    Ok(Some((encoding, values)))
}

/// Check the encoding against the type, then admit the payload and read every stored value once.
fn stored_values(
    node: &CellType,
    encoding: u8,
    present: usize,
    cursor: &mut Cursor,
    keys: Option<&[Value]>,
    budget: &mut Budget,
) -> Result<Option<Vec<Value>>, Refused> {
    if present == 0 && encoding != 0 {
        return Err("empty_encoding");
    }
    let numeric = matches!(encoding, 1 | 2);
    if numeric && !matches!(node, CellType::Integer(..) | CellType::Decimal(..)) {
        return Err("encoding_type");
    }
    if matches!(encoding, 4 | 5) {
        if key_type_of(node).is_none() {
            return Err("encoding_type");
        }
        if keys.is_none() {
            return Err("missing_dictionary");
        }
    }
    match encoding {
        0 => {
            let (table, width, data) = columns::read_index(cursor, present, budget)?;
            let mut values = Vec::with_capacity(present);
            for position in 0..present {
                values.push(cell(node, selected(table, width, data, position, budget)?, budget)?);
            }
            Ok(Some(values))
        }
        1 | 2 => numeric_values(node, encoding, present, cursor, budget),
        3 | 5 => read_runs(node, encoding, present, cursor, keys, budget).map(Some),
        _ => {
            let keys = keys.ok_or("missing_dictionary")?;
            let width = cursor.length(budget)?;
            if width != bit_length((keys.len() - 1) as u128) {
                return Err("nonminimal_bits");
            }
            let payload = cursor.take(cursor.remaining(), budget)?;
            validate_pack(payload, width, present, budget)?;
            let mut values = Vec::with_capacity(present);
            for position in 0..present {
                let code = packed_value(payload, width, position).ok_or("dictionary_code")?;
                let key = usize::try_from(code)
                    .ok()
                    .and_then(|code| keys.get(code))
                    .ok_or("dictionary_code")?;
                values.push(key.clone());
            }
            Ok(Some(values))
        }
    }
}

/// Address one indexed cell through its two offsets.
fn selected<'a>(
    table: &[u8],
    width: usize,
    data: &'a [u8],
    position: usize,
    budget: &mut Budget,
) -> Result<&'a [u8], Refused> {
    let left = columns::offset_at(table, position, width, budget)?;
    let right = columns::offset_at(table, position + 1, width, budget)?;
    data.get(left..right).ok_or("invalid_offsets")
}

/// Read a frame-of-reference or zigzag-delta block: base cell, width, then exact packed residuals.
fn numeric_values(
    node: &CellType,
    encoding: u8,
    present: usize,
    cursor: &mut Cursor,
    budget: &mut Budget,
) -> Result<Option<Vec<Value>>, Refused> {
    let raw = cursor.frame(budget)?;
    let base_value = cell(node, raw, budget)?;
    let Some(base) = coefficient(&base_value) else {
        return Ok(None);
    };
    let width = cursor.length(budget)?;
    let payload = cursor.take(cursor.remaining(), budget)?;
    let count = if encoding == 2 { present - 1 } else { present };
    validate_pack(payload, width, count, budget)?;
    let mut values = Vec::with_capacity(present);
    let (mut current, mut maximum, mut has_zero) = (base, 0usize, false);
    for index in 0..present {
        budget.elements(1)?;
        let mut number = base;
        if encoding == 1 {
            let Some(residual) = packed_value(payload, width, index) else {
                return Ok(None);
            };
            maximum = maximum.max(bit_length(residual));
            has_zero |= residual == 0;
            let Some(sum) = i128::try_from(residual)
                .ok()
                .and_then(|residual| base.checked_add(residual))
            else {
                return Err("value_overflow");
            };
            number = sum;
        } else if index > 0 {
            let Some(residual) = packed_value(payload, width, index - 1) else {
                return Ok(None);
            };
            maximum = maximum.max(bit_length(residual));
            number = current.checked_add(unzigzag(residual)).ok_or("value_overflow")?;
        }
        current = number;
        values.push(from_coefficient(node, number).ok_or("value_overflow")?);
        budget.bytes(16)?;
    }
    if maximum != width {
        return Err("nonminimal_bits");
    }
    if encoding == 1 && !has_zero {
        return Err("nonminimal_base");
    }
    Ok(Some(values))
}

/// Read typed or dictionary-code runs with positive lengths and no equal adjacent runs.
fn read_runs(
    node: &CellType,
    encoding: u8,
    present: usize,
    cursor: &mut Cursor,
    keys: Option<&[Value]>,
    budget: &mut Budget,
) -> Result<Vec<Value>, Refused> {
    let count = cursor.length(budget)?;
    if count < 1 || count > present {
        return Err("run_length");
    }
    let (mut total, mut previous_raw, mut previous_code) = (0usize, &[][..], None);
    let mut result = Vec::with_capacity(present);
    for index in 0..count {
        let length = cursor.length(budget)?;
        if length < 1 || length > present - total {
            return Err("run_length");
        }
        let value = if encoding == 3 {
            let raw = cursor.frame(budget)?;
            if index > 0 && raw == previous_raw {
                return Err("adjacent_runs");
            }
            previous_raw = raw;
            cell(node, raw, budget)?
        } else {
            let code = cursor.length(budget)?;
            if previous_code == Some(code) {
                return Err("adjacent_runs");
            }
            previous_code = Some(code);
            keys.ok_or("missing_dictionary")?
                .get(code)
                .ok_or("dictionary_code")?
                .clone()
        };
        budget.elements(length as u64)?;
        for _ in 0..length {
            budget.bytes(16)?;
            result.push(value.clone());
        }
        total += length;
    }
    if total != present {
        return Err("run_length");
    }
    cursor.finished()?;
    Ok(result)
}

/// Check the exact packed length and zero final padding before any packed value is read.
fn validate_pack(payload: &[u8], width: usize, count: usize, budget: &mut Budget) -> Result<(), Refused> {
    budget.elements(width as u64)?;
    let bits = width.checked_mul(count).ok_or("limit_exceeded")?;
    if bits.div_ceil(8) != payload.len() {
        return Err("truncated");
    }
    budget.elements(count as u64)?;
    if bits % 8 != 0 {
        budget.bytes(1)?;
        if payload[payload.len() - 1] >> (bits % 8) != 0 {
            return Err("nonzero_padding");
        }
    }
    Ok(())
}

/// Read one low-first packed value of `width` bits; wider than 128 bits is outside native arithmetic.
fn packed_value(payload: &[u8], width: usize, position: usize) -> Option<u128> {
    if width > 128 {
        return None;
    }
    let start = width * position;
    let mut value = 0u128;
    for bit in 0..width {
        let source = start + bit;
        value |= u128::from((payload.get(source / 8)? >> (source % 8)) & 1) << bit;
    }
    Some(value)
}

/// Exact magnitude width; zero needs no bits.
fn bit_length(value: u128) -> usize {
    128 - value.leading_zeros() as usize
}

/// Recover a signed delta from its zigzag magnitude.
fn unzigzag(value: u128) -> i128 {
    if value & 1 == 0 {
        (value >> 1) as i128
    } else {
        -((value >> 1) as i128) - 1
    }
}

/// Map a signed delta to its zigzag magnitude.
fn zigzag(value: i128) -> u128 {
    if value >= 0 {
        (value as u128) << 1
    } else {
        ((value.unsigned_abs() - 1) << 1) + 1
    }
}

/// Whether one eligible presence bit is set.
fn bit(bitmap: &[u8], position: usize) -> bool {
    (bitmap[position / 8] >> (position % 8)) & 1 == 1
}

/// The exact coefficient of one stored numeric value, when it fits native signed arithmetic.
fn coefficient(value: &Value) -> Option<i128> {
    match value {
        Value::Integer(true, magnitude) => 0i128.checked_sub_unsigned(*magnitude),
        Value::Integer(false, magnitude) => i128::try_from(*magnitude).ok(),
        Value::Decimal(coefficient, _) => Some(*coefficient),
        _ => None,
    }
}

/// Narrow a coefficient to the declared carrier and recheck its stored range.
fn from_coefficient(node: &CellType, number: i128) -> Option<Value> {
    match node {
        CellType::Integer(signed, width) => {
            let (negative, magnitude) = (number < 0, number.unsigned_abs());
            columns::in_range(*signed, *width, negative, magnitude).then_some(Value::Integer(negative, magnitude))
        }
        CellType::Decimal(precision, scale) => {
            (columns::digits(number) <= *precision).then_some(Value::Decimal(number, *scale))
        }
        _ => None,
    }
}

/// Decode one typed cell that must consume its complete frame.
fn cell(node: &CellType, raw: &[u8], budget: &mut Budget) -> Result<Value, Refused> {
    budget.elements(1)?;
    let mut cursor = Cursor::new(raw);
    let value = match node {
        CellType::Integer(signed, width) => {
            if raw.len() != *width {
                return Err("invalid_cell");
            }
            budget.bytes(*width as u64)?;
            return Ok(fixed_integer(raw, *signed));
        }
        CellType::Boolean => {
            if raw.len() != 1 || raw[0] > 1 {
                return Err("invalid_cell");
            }
            return Ok(Value::Boolean(raw[0] == 1));
        }
        CellType::Float(width) => return float_cell(raw, *width),
        CellType::Text => {
            let data = cursor.frame(budget)?;
            Value::Text(String::from_utf8(data.to_vec()).map_err(|_| "invalid_utf8")?)
        }
        CellType::Binary => Value::Binary(cursor.frame(budget)?.to_vec()),
        CellType::Decimal(precision, scale) => {
            let groups = cursor.groups(budget)?;
            let coefficient = columns::unzigzag(&groups).ok_or("integer_overflow")?;
            if columns::digits(coefficient) > *precision {
                return Err("invalid_cell");
            }
            Value::Decimal(coefficient, *scale)
        }
        CellType::Option(inner) => match cursor.byte(budget)? {
            0 => Value::Absent,
            1 => Value::Some(Box::new(cell(inner, cursor.frame(budget)?, budget)?)),
            _ => return Err("invalid_cell"),
        },
        CellType::List(inner) => {
            let count = cursor.length(budget)?;
            budget.elements(count as u64)?;
            let (table, width, data) = columns::read_index(&mut cursor, count, budget)?;
            let mut items = Vec::with_capacity(count);
            for position in 0..count {
                items.push(cell(inner, selected(table, width, data, position, budget)?, budget)?);
            }
            Value::List(items)
        }
    };
    if cursor.remaining() != 0 {
        return Err("invalid_cell");
    }
    Ok(value)
}

/// Decode fixed little-endian two's-complement or unsigned bytes.
fn fixed_integer(raw: &[u8], signed: bool) -> Value {
    let value = raw
        .iter()
        .rev()
        .fold(0u128, |value, byte| (value << 8) | u128::from(*byte));
    if !signed || raw[raw.len() - 1] < 0x80 {
        return Value::Integer(false, value);
    }
    let magnitude = if raw.len() < 16 {
        (1u128 << (raw.len() * 8)) - value
    } else {
        value.wrapping_neg()
    };
    Value::Integer(true, magnitude)
}

/// Decode a little-endian IEEE cell, refusing any NaN that is not the canonical quiet NaN.
fn float_cell(raw: &[u8], width: usize) -> Result<Value, Refused> {
    if raw.len() != width {
        return Err("invalid_cell");
    }
    let mut bits = raw.to_vec();
    bits.reverse();
    let wide = if width == 4 {
        let value = f32::from_bits(u32::from_be_bytes([bits[0], bits[1], bits[2], bits[3]]));
        if value.is_nan() && value.to_bits() != 0x7fc0_0000 {
            return Err("noncanonical_nan");
        }
        f64::from(value)
    } else {
        let mut array = [0u8; 8];
        array.copy_from_slice(&bits);
        let value = f64::from_bits(u64::from_be_bytes(array));
        if value.is_nan() && value.to_bits() != 0x7ff8_0000_0000_0000 {
            return Err("noncanonical_nan");
        }
        value
    };
    let normalized = if wide.is_nan() {
        0x7ff8_0000_0000_0000u64
    } else {
        wide.to_bits()
    };
    Ok(Value::Float(normalized.to_be_bytes().to_vec()))
}

/// Write one block: presence bitmap, encoding identifier and the encoding's exact payload.
fn write_block(
    node: &CellType,
    absent: char,
    values: Vec<Value>,
    encoding: u8,
    keys: Option<&[Value]>,
) -> Option<Vec<u8>> {
    if values.is_empty() || values.len() > 256 {
        return None;
    }
    let stored_node = stored_type(node, absent)?;
    let mut bitmap: Vec<u8> = Vec::new();
    let (mut stored, mut cells) = (Vec::new(), Vec::new());
    for (index, value) in values.into_iter().enumerate() {
        if absent != 'r' && index % 8 == 0 {
            bitmap.push(0);
        }
        let Some(value) = present(value, absent)? else {
            continue;
        };
        cells.push(write_cell(&stored_node, &value)?);
        stored.push(value);
        if absent != 'r' {
            *bitmap.last_mut()? |= 1 << (index % 8);
        }
    }
    if stored.is_empty() && encoding != 0 {
        return None;
    }
    let mut output = bitmap;
    hash::unsigned(&mut output, u128::from(encoding));
    output.extend(payload(&stored_node, &stored, &cells, encoding, keys)?);
    Some(output)
}

/// Return a stored value, or none for the declared absent value.
fn present(value: Value, absent: char) -> Option<Option<Value>> {
    match (absent, value) {
        ('n', Value::Absent) => Some(None),
        ('n', Value::Some(inner)) => Some(Some(*inner)),
        ('n', _) => None,
        ('e', Value::List(items)) if items.is_empty() => Some(None),
        (_, value) => Some(Some(value)),
    }
}

/// The declared encoding's exact payload.
fn payload(
    node: &CellType,
    values: &[Value],
    cells: &[Vec<u8>],
    encoding: u8,
    keys: Option<&[Value]>,
) -> Option<Vec<u8>> {
    match encoding {
        0 => Some(columns::indexed(cells)),
        3 => Some(runs(cells.iter().map(|cell| (cell.clone(), None)).collect())),
        4 | 5 => {
            let keys = keys?;
            let key_type = key_type_of(node)?;
            let key_cells: Vec<Vec<u8>> = keys
                .iter()
                .map(|key| columns::key_cell(key_type, key))
                .collect::<Option<_>>()?;
            let mut codes = Vec::with_capacity(values.len());
            for value in values {
                let cell = columns::key_cell(key_type, value)?;
                codes.push(key_cells.iter().position(|key| *key == cell)? as u128);
            }
            if encoding == 5 {
                return Some(runs(codes.iter().map(|code| (Vec::new(), Some(*code))).collect()));
            }
            let width = bit_length((keys.len() - 1) as u128);
            let mut output = Vec::new();
            hash::unsigned(&mut output, width as u128);
            output.extend(pack(width, &codes)?);
            Some(output)
        }
        1 | 2 => numeric_payload(values, cells, encoding == 2),
        _ => None,
    }
}

/// Frame-of-reference from the first minimum, or zigzag deltas from the first value, at the minimum width.
fn numeric_payload(values: &[Value], cells: &[Vec<u8>], delta: bool) -> Option<Vec<u8>> {
    let numbers: Vec<i128> = values.iter().map(coefficient).collect::<Option<_>>()?;
    let mut base = 0;
    if !delta {
        for index in 1..numbers.len() {
            if numbers[index] < numbers[base] {
                base = index;
            }
        }
    }
    let mut residuals = Vec::with_capacity(numbers.len());
    for index in 0..numbers.len() {
        if delta && index == 0 {
            continue;
        }
        residuals.push(if delta {
            zigzag(numbers[index].checked_sub(numbers[index - 1])?)
        } else {
            u128::try_from(numbers[index].checked_sub(numbers[base])?).ok()?
        });
    }
    let width = residuals
        .iter()
        .map(|residual| bit_length(*residual))
        .max()
        .unwrap_or(0);
    let mut output = Vec::new();
    hash::frame(&mut output, &cells[base]);
    hash::unsigned(&mut output, width as u128);
    output.extend(pack(width, &residuals)?);
    Some(output)
}

/// Combine adjacent identical cells or codes into positive-length runs.
fn runs(items: Vec<(Vec<u8>, Option<u128>)>) -> Vec<u8> {
    let mut ends = Vec::new();
    for index in 1..items.len() {
        if items[index] != items[index - 1] {
            ends.push(index);
        }
    }
    ends.push(items.len());
    let mut output = Vec::new();
    hash::unsigned(&mut output, ends.len() as u128);
    let mut start = 0;
    for end in ends {
        hash::unsigned(&mut output, (end - start) as u128);
        match &items[start] {
            (_, Some(code)) => hash::unsigned(&mut output, *code),
            (cell, None) => hash::frame(&mut output, cell),
        }
        start = end;
    }
    output
}

/// Pack values low-first at exactly `width` bits each, with zero final padding.
fn pack(width: usize, values: &[u128]) -> Option<Vec<u8>> {
    let mut output = vec![0u8; (width * values.len()).div_ceil(8)];
    for (index, value) in values.iter().enumerate() {
        if bit_length(*value) > width {
            return None;
        }
        for bit in 0..width {
            if (value >> bit) & 1 == 1 {
                let target = index * width + bit;
                output[target / 8] |= 1 << (target % 8);
            }
        }
    }
    Some(output)
}

/// Write one canonical typed cell.
fn write_cell(node: &CellType, value: &Value) -> Option<Vec<u8>> {
    match (node, value) {
        (CellType::Option(_), Value::Absent) => Some(vec![0]),
        (CellType::Option(inner), Value::Some(item)) => {
            let mut output = vec![1];
            hash::frame(&mut output, &write_cell(inner, item)?);
            Some(output)
        }
        (CellType::List(inner), Value::List(items)) => {
            let cells: Vec<Vec<u8>> = items
                .iter()
                .map(|item| write_cell(inner, item))
                .collect::<Option<_>>()?;
            let mut output = Vec::new();
            hash::unsigned(&mut output, cells.len() as u128);
            output.extend(columns::indexed(&cells));
            Some(output)
        }
        (CellType::Boolean, Value::Boolean(flag)) => Some(vec![u8::from(*flag)]),
        _ => columns::key_cell(key_type_of(node)?, value),
    }
}

/// Decode a complete column case: the shared dictionary, block directory and every block, then membership.
pub(super) fn decode_column(document: &Json) -> Option<Vec<u8>> {
    let node = cell_type(document.get("type")?.as_str()?)?;
    let absent = absence(document)?;
    let data = columns::hex(document.get("input_hex")?.as_str()?)?;
    let rows = usize::try_from(document.get("logical_count")?.as_u64()?).ok()?;
    let mut budget = columns::case_budget(document)?;
    let mut output = Vec::new();
    match read_column(&node, absent, rows, &data, &mut budget) {
        Ok(Some((encodings, values))) => {
            output.push(0);
            hash::unsigned(&mut output, encodings.len() as u128);
            for encoding in encodings {
                hash::unsigned(&mut output, u128::from(encoding));
            }
            hash::frame(
                &mut output,
                &hash::value_preimage(&Value::List(values), Projection::Object)?,
            );
        }
        Ok(None) => return None,
        Err(category) => {
            output.push(1);
            hash::frame(&mut output, category.as_bytes());
        }
    }
    Some(output)
}

/// Encode a complete column case's authored values with its authored block encodings.
pub(super) fn encode_column(document: &Json) -> Option<Vec<u8>> {
    let node = cell_type(document.get("type")?.as_str()?)?;
    let absent = absence(document)?;
    let expected = document.get("expected")?;
    let Some(values) = expected.get("values") else {
        return Some(Vec::new());
    };
    let values = hash::logical_items(values)?;
    let encodings: Vec<u8> = expected
        .get("encodings")?
        .as_array()?
        .iter()
        .map(|item| item.as_u64().and_then(|value| u8::try_from(value).ok()))
        .collect::<Option<_>>()?;
    if encodings.len() != values.len().div_ceil(256) || values.is_empty() {
        return None;
    }
    let stored_node = stored_type(&node, absent)?;
    let mut output = Vec::new();
    let mut keys: Option<Vec<Value>> = None;
    if encodings.iter().any(|encoding| matches!(encoding, 4 | 5)) {
        let mut stored = Vec::new();
        for value in &values {
            if let Some(item) = present(value.clone(), absent)? {
                stored.push(item);
            }
        }
        let key_type = key_type_of(&stored_node)?;
        let sorted = columns::sorted_keys(key_type, stored)?;
        let written = columns::encode_keys(key_type, sorted.iter().map(|(_, value)| value.clone()).collect())?;
        output.push(1);
        hash::frame(&mut output, &written);
        keys = Some(sorted.into_iter().map(|(_, value)| value).collect());
    } else {
        output.push(0);
    }
    let mut blocks = Vec::with_capacity(encodings.len());
    for (index, chunk) in values.chunks(256).enumerate() {
        blocks.push(write_block(
            &node,
            absent,
            chunk.to_vec(),
            encodings[index],
            keys.as_deref(),
        )?);
    }
    output.extend(columns::indexed(&blocks));
    Some(output)
}

/// The declared absence policy, required when the case names none.
fn absence(document: &Json) -> Option<char> {
    match document.get("absent").and_then(Json::as_str).unwrap_or("required") {
        "required" => Some('r'),
        "none" => Some('n'),
        "empty" => Some('e'),
        _ => None,
    }
}

/// Admit a complete column and require its dictionary to be used by codes and fully referenced.
fn read_column(
    node: &CellType,
    absent: char,
    rows: usize,
    data: &[u8],
    budget: &mut Budget,
) -> Result<Option<(Vec<u8>, Vec<Value>)>, Refused> {
    if rows < 1 {
        return Err("invalid_cell");
    }
    let count = rows.div_ceil(256);
    let Some(stored) = stored_type(node, absent) else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(data);
    let mode = cursor.byte(budget)?;
    if mode > 1 {
        return Err("unsupported_encoding");
    }
    let key_type = key_type_of(&stored);
    let keys = if mode == 1 {
        Some(columns::read_keys(key_type, cursor.frame(budget)?, budget)?)
    } else {
        None
    };
    let (table, width, directory) = columns::read_index(&mut cursor, count, budget)?;
    let (mut encodings, mut values, mut used) = (Vec::with_capacity(count), Vec::with_capacity(rows), Vec::new());
    for index in 0..count {
        budget.elements(1)?;
        let raw = selected(table, width, directory, index, budget)?;
        if raw.is_empty() {
            return Err("invalid_offsets");
        }
        let length = (rows - index * 256).min(256) as i64;
        let Some((encoding, block)) = read_block(node, absent, length, raw, keys.as_deref(), budget)? else {
            return Ok(None);
        };
        if mode == 1 {
            for value in &block {
                budget.elements(1)?;
                if let Some(item) = present(value.clone(), absent).ok_or("invalid_cell")? {
                    let identity = columns::key_cell(key_type.ok_or("encoding_type")?, &item).ok_or("invalid_cell")?;
                    if !used.contains(&identity) {
                        used.push(identity);
                    }
                }
            }
        }
        budget.bytes(128)?;
        encodings.push(encoding);
        values.extend(block);
    }
    if let Some(keys) = keys {
        if !encodings.iter().any(|encoding| matches!(encoding, 4 | 5)) {
            return Err("unused_dictionary");
        }
        if used.len() != keys.len() {
            return Err("dictionary_membership");
        }
        for key in &keys {
            let identity = columns::key_cell(key_type.ok_or("encoding_type")?, key).ok_or("invalid_cell")?;
            if !used.contains(&identity) {
                return Err("dictionary_membership");
            }
        }
    }
    Ok(Some((encodings, values)))
}
