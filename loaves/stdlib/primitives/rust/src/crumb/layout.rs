//! Batched row and column envelope layout with exact logical budget receipts.

use super::{Encoded, encode_native_unsigned, indexed};

/// One complete envelope and the exact logical work charged by its Incan contract.
pub struct EnvelopeLayout {
    pub kind: u8,
    pub data: Vec<u8>,
    pub byte_charge: i64,
    pub element_charge: i64,
}

/// Lay out a complete row envelope after descriptions and typed record cells are admitted.
pub fn row_envelope(
    descriptions: Vec<Vec<u8>>,
    slots: Vec<i64>,
    cells: Vec<Vec<u8>>,
    sealed: bool,
    seal: Vec<u8>,
) -> EnvelopeLayout {
    if slots.len() != cells.len() || slots.iter().any(|slot| *slot < 0) || (sealed && seal.len() != 32) {
        return EnvelopeLayout::failed(1);
    }
    let mut body = ChargedWriter::new();
    if !body.count(descriptions.len()) {
        return EnvelopeLayout::failed(2);
    }
    for description in descriptions {
        if !body.append(&description) {
            return EnvelopeLayout::failed(2);
        }
    }
    let record_count = cells.len();
    if !body.count(record_count) {
        return EnvelopeLayout::failed(2);
    }
    for (slot, cell) in slots.into_iter().zip(cells) {
        if !body.count(slot as usize) || !body.frame(&cell) {
            return EnvelopeLayout::failed(2);
        }
    }
    if sealed && (!body.count(record_count) || !body.append(&seal)) {
        return EnvelopeLayout::failed(2);
    }
    finish_envelope(body, b"CRMB\x01\x01", sealed)
}

/// Lay out a complete column envelope after descriptions and physical groups are admitted.
pub fn column_envelope(
    descriptions: Vec<Vec<u8>>,
    records: i64,
    group_ends: Vec<i64>,
    groups: Vec<Vec<u8>>,
    sealed: bool,
    seal: Vec<u8>,
) -> EnvelopeLayout {
    if records < 0
        || group_ends.len() != groups.len()
        || group_ends.last().copied().unwrap_or(0) != records
        || group_ends.iter().any(|end| *end < 0)
        || group_ends.windows(2).any(|pair| pair[0] >= pair[1])
        || (sealed && seal.len() != 32)
    {
        return EnvelopeLayout::failed(1);
    }
    let mut body = ChargedWriter::new();
    if !body.count(descriptions.len()) {
        return EnvelopeLayout::failed(2);
    }
    for description in descriptions {
        if !body.append(&description) {
            return EnvelopeLayout::failed(2);
        }
    }
    if !body.count(records as usize) || !body.count(groups.len()) {
        return EnvelopeLayout::failed(2);
    }
    let width = offset_width(records as usize);
    if !body.push(width) || !body.offset(0, width) {
        return EnvelopeLayout::failed(2);
    }
    for end in group_ends {
        if !body.offset(end as usize, width) {
            return EnvelopeLayout::failed(2);
        }
    }
    let Some(group_bytes) = groups
        .iter()
        .try_fold(0usize, |total, group| total.checked_add(group.len()))
    else {
        return EnvelopeLayout::failed(2);
    };
    let layout = indexed(groups, offset_width(group_bytes));
    if !body.charge(layout.byte_charge, layout.element_charge) || !body.append(&layout.data) {
        return EnvelopeLayout::failed(2);
    }
    if sealed && (!body.count(records as usize) || !body.append(&seal)) {
        return EnvelopeLayout::failed(2);
    }
    finish_envelope(body, b"CRMB\x02\x01", sealed)
}

fn finish_envelope(mut body: ChargedWriter, magic: &[u8], sealed: bool) -> EnvelopeLayout {
    let Some(payload) = body.finish() else {
        return EnvelopeLayout::failed(2);
    };
    let mut envelope = ChargedWriter::new();
    if !envelope.append(magic) || !envelope.push(u8::from(sealed)) || !envelope.frame(&payload) {
        return EnvelopeLayout::failed(2);
    }
    let Some(data) = envelope.finish() else {
        return EnvelopeLayout::failed(2);
    };
    let Some(byte_charge) = body.byte_charge.checked_add(envelope.byte_charge) else {
        return EnvelopeLayout::failed(2);
    };
    let Some(element_charge) = body.element_charge.checked_add(envelope.element_charge) else {
        return EnvelopeLayout::failed(2);
    };
    EnvelopeLayout {
        kind: 0,
        data,
        byte_charge,
        element_charge,
    }
}

fn offset_width(length: usize) -> u8 {
    match length {
        0..=255 => 1,
        256..=65_535 => 2,
        65_536..=4_294_967_295 => 4,
        _ => 8,
    }
}

impl EnvelopeLayout {
    fn failed(kind: u8) -> Self {
        Self {
            kind,
            data: Vec::new(),
            byte_charge: 0,
            element_charge: 0,
        }
    }
}

struct ChargedWriter {
    data: Vec<u8>,
    byte_charge: i64,
    element_charge: i64,
}

impl ChargedWriter {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            byte_charge: 0,
            element_charge: 0,
        }
    }

    fn charge(&mut self, bytes: i64, elements: i64) -> bool {
        let Some(byte_charge) = self.byte_charge.checked_add(bytes) else {
            return false;
        };
        let Some(element_charge) = self.element_charge.checked_add(elements) else {
            return false;
        };
        self.byte_charge = byte_charge;
        self.element_charge = element_charge;
        true
    }

    fn append(&mut self, value: &[u8]) -> bool {
        let Ok(charge) = i64::try_from(value.len()) else {
            return false;
        };
        if self.data.len().checked_add(value.len()).is_none() || !self.charge(charge, 0) {
            return false;
        }
        self.data.extend_from_slice(value);
        true
    }

    fn push(&mut self, value: u8) -> bool {
        if !self.charge(1, 0) {
            return false;
        }
        self.data.push(value);
        true
    }

    fn count(&mut self, value: usize) -> bool {
        let encoded = encode_native_unsigned(value as u128);
        self.encoded(&encoded) && self.append(&encoded.data)
    }

    fn encoded(&mut self, value: &Encoded) -> bool {
        let Ok(bytes) = i64::try_from(value.byte_charge) else {
            return false;
        };
        let Ok(elements) = i64::try_from(value.element_charge) else {
            return false;
        };
        self.charge(bytes, elements)
    }

    fn frame(&mut self, value: &[u8]) -> bool {
        self.count(value.len()) && self.append(value)
    }

    fn offset(&mut self, mut value: usize, width: u8) -> bool {
        for _ in 0..width {
            if !self.push((value & 0xff) as u8) {
                return false;
            }
            value >>= 8;
        }
        value == 0
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        let charge = i64::try_from(self.data.len()).ok()?;
        if !self.charge(charge, 0) {
            return None;
        }
        Some(self.data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{column_envelope, row_envelope};

    #[test]
    fn row_layout_batches_the_complete_envelope() {
        let result = row_envelope(
            vec![b"description".to_vec()],
            vec![0, 0],
            vec![b"a".to_vec(), b"bc".to_vec()],
            false,
            Vec::new(),
        );
        assert_eq!(result.kind, 0);
        assert_eq!(
            result.data,
            b"CRMB\x01\x01\x00\x14\x01description\x02\x00\x01a\x00\x02bc"
        );
        assert_eq!((result.byte_charge, result.element_charge), (103, 18));
    }

    #[test]
    fn column_layout_batches_prefixes_and_groups() {
        let result = column_envelope(
            Vec::new(),
            3,
            vec![2, 3],
            vec![b"ab".to_vec(), b"c".to_vec()],
            false,
            Vec::new(),
        );
        assert_eq!(result.kind, 0);
        assert_eq!(
            result.data,
            b"CRMB\x02\x01\x00\x0e\x00\x03\x02\x01\x00\x02\x03\x01\x00\x02\x03abc"
        );
        assert!(result.byte_charge > 0);
        assert!(result.element_charge > 0);
    }

    #[test]
    fn layouts_refuse_mismatched_batch_dimensions() {
        assert_eq!(row_envelope(Vec::new(), vec![0], Vec::new(), false, Vec::new()).kind, 1);
        assert_eq!(
            column_envelope(Vec::new(), 2, vec![1], vec![Vec::new()], false, Vec::new()).kind,
            1
        );
    }
}
