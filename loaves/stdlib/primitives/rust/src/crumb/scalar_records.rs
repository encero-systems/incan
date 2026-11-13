//! Batched fixed-scalar record cells for the common real-shape row path.

use super::indexed;

/// Encoded record cells and staged exact budget receipts.
pub struct ScalarRecordBatch {
    pub kind: u8,
    pub at: i64,
    pub rows: Vec<Vec<u8>>,
    pub scalar_byte_charge: i64,
    pub layout_byte_charge: i64,
    pub layout_element_charge: i64,
}

/// Encode all required boolean and fixed-integer fields for a complete record batch.
pub fn encode_scalar_records(
    field_counts: Vec<i64>,
    kinds: Vec<u8>,
    signed: Vec<i128>,
    unsigned: Vec<u128>,
    byte_limit: i64,
    element_limit: i64,
) -> ScalarRecordBatch {
    if kinds.len() != signed.len()
        || kinds.len() != unsigned.len()
        || field_counts.iter().any(|count| *count < 0)
        || field_counts
            .iter()
            .try_fold(0usize, |total, count| total.checked_add(*count as usize))
            != Some(kinds.len())
    {
        return ScalarRecordBatch::failed(1, 0);
    }
    let mut scalar_bytes = 0i64;
    for (index, kind) in kinds.iter().copied().enumerate() {
        let Some(width) = scalar_width(kind) else {
            return ScalarRecordBatch::failed(1, index);
        };
        if !valid_value(kind, signed[index], unsigned[index]) {
            return ScalarRecordBatch::failed(2, index);
        }
        let Some(next) = scalar_bytes.checked_add(i64::from(width)) else {
            return ScalarRecordBatch::failed(1, index);
        };
        scalar_bytes = next;
    }
    if scalar_bytes > byte_limit {
        let mut failed = ScalarRecordBatch::failed(3, 0);
        failed.scalar_byte_charge = scalar_bytes;
        return failed;
    }
    let mut position = 0usize;
    let mut rows = Vec::with_capacity(field_counts.len());
    let mut layout_bytes = 0i64;
    let mut layout_elements = 0i64;
    for count in field_counts {
        let mut cells = Vec::with_capacity(count as usize);
        for _ in 0..count {
            cells.push(encode_value(kinds[position], signed[position], unsigned[position]));
            position += 1;
        }
        let total = cells.iter().map(Vec::len).sum::<usize>();
        let layout = indexed(cells, offset_width(total));
        let Some(next_bytes) = layout_bytes.checked_add(layout.byte_charge) else {
            return ScalarRecordBatch::failed(1, position);
        };
        let Some(next_elements) = layout_elements.checked_add(layout.element_charge) else {
            return ScalarRecordBatch::failed(1, position);
        };
        layout_bytes = next_bytes;
        layout_elements = next_elements;
        rows.push(layout.data);
    }
    let mut result = ScalarRecordBatch {
        kind: 0,
        at: 0,
        rows,
        scalar_byte_charge: scalar_bytes,
        layout_byte_charge: layout_bytes,
        layout_element_charge: layout_elements,
    };
    if layout_elements > element_limit {
        result.kind = 4;
        result.rows.clear();
    } else if scalar_bytes
        .checked_add(layout_bytes)
        .is_none_or(|charge| charge > byte_limit)
    {
        result.kind = 3;
        result.rows.clear();
    }
    result
}

fn scalar_width(kind: u8) -> Option<u8> {
    match kind {
        0 | 1 | 2 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 => Some(4),
        5 | 10 => Some(8),
        6 | 11 => Some(16),
        _ => None,
    }
}

fn valid_value(kind: u8, signed: i128, unsigned: u128) -> bool {
    match kind {
        0 | 1 => true,
        2 => i8::try_from(signed).is_ok(),
        3 => i16::try_from(signed).is_ok(),
        4 => i32::try_from(signed).is_ok(),
        5 => i64::try_from(signed).is_ok(),
        6 => true,
        7 => u8::try_from(unsigned).is_ok(),
        8 => u16::try_from(unsigned).is_ok(),
        9 => u32::try_from(unsigned).is_ok(),
        10 => u64::try_from(unsigned).is_ok(),
        11 => true,
        _ => false,
    }
}

fn encode_value(kind: u8, signed: i128, unsigned: u128) -> Vec<u8> {
    if kind == 0 || kind == 1 {
        return vec![kind];
    }
    let width = usize::from(scalar_width(kind).unwrap_or(0));
    let mut value = if kind <= 6 { signed as u128 } else { unsigned };
    let mut data = Vec::with_capacity(width);
    for _ in 0..width {
        data.push((value & 0xff) as u8);
        value >>= 8;
    }
    data
}

fn offset_width(length: usize) -> u8 {
    match length {
        0..=255 => 1,
        256..=65_535 => 2,
        65_536..=4_294_967_295 => 4,
        _ => 8,
    }
}

impl ScalarRecordBatch {
    fn failed(kind: u8, at: usize) -> Self {
        Self {
            kind,
            at: at as i64,
            rows: Vec::new(),
            scalar_byte_charge: 0,
            layout_byte_charge: 0,
            layout_element_charge: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::encode_scalar_records;

    #[test]
    fn batches_fixed_scalar_record_cells() {
        let result = encode_scalar_records(
            vec![3, 2],
            vec![1, 2, 8, 0, 6],
            vec![0, -128, 0, 0, i128::MIN],
            vec![0, 0, 65_535, 0, 0],
            i64::MAX,
            i64::MAX,
        );
        assert_eq!(result.kind, 0);
        assert_eq!(result.rows[0], b"\x01\x00\x01\x02\x04\x01\x80\xff\xff");
        assert_eq!(result.rows[1][0], 1);
        assert_eq!(result.rows[1].len(), 21);
        assert_eq!(result.scalar_byte_charge, 21);
    }

    #[test]
    fn refuses_ranges_and_limits_before_returning_rows() {
        assert_eq!(
            encode_scalar_records(vec![1], vec![2], vec![128], vec![0], 10, 10).kind,
            2
        );
        assert_eq!(
            encode_scalar_records(vec![1], vec![6], vec![0], vec![0], 15, 10).kind,
            3
        );
        assert_eq!(encode_scalar_records(vec![1], vec![2], vec![1], vec![0], 10, 0).kind, 4);
    }
}
