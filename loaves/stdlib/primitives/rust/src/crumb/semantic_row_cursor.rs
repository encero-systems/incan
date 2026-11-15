//! Positioned refusals for native Crumb row envelopes, and the checked cursor and cell index that produce them.
//!
//! A refusal carries the category, the envelope offset and the enclosing field and variant identities exactly as
//! `encoding_errors.BinaryError` reports them; `Span` mirrors `encoding_cursor.EncodingCursor` and the index helpers
//! mirror `encoding_index`, so each refusal lands at the byte the Incan reader names.

use super::columns::{Budget, Refused};

/// One row refusal: its category, the deepest known envelope offset, and the stored field and enum variant it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Refusal {
    pub(super) category: Refused,
    pub(super) offset: usize,
    pub(super) field: Option<Vec<u8>>,
    pub(super) variant: Option<Vec<u8>>,
}

/// A row reader result whose refusal names its envelope position.
pub(super) type Positioned<T> = Result<T, Refusal>;

/// A refusal at a known envelope offset, outside any stored member.
pub(super) fn at(category: Refused, offset: usize) -> Refusal {
    Refusal {
        category,
        offset,
        field: None,
        variant: None,
    }
}

impl Refusal {
    /// Keep a deeper field identity; otherwise name the failing outer stored field.
    pub(super) fn in_field(mut self, identity: &[u8]) -> Self {
        if self.field.is_none() {
            self.field = Some(identity.to_vec());
        }
        self
    }

    /// Keep a deeper variant identity; otherwise name the enum payload that failed.
    pub(super) fn in_variant(mut self, identity: &[u8]) -> Self {
        if self.variant.is_none() {
            self.variant = Some(identity.to_vec());
        }
        self
    }
}

/// Attach an envelope offset to a category-only result, such as a budget charge or a selector check.
pub(super) trait Place<T> {
    fn place(self, offset: usize) -> Positioned<T>;
}

impl<T> Place<T> for Result<T, Refused> {
    fn place(self, offset: usize) -> Positioned<T> {
        self.map_err(|category| at(category, offset))
    }
}

/// A checked cursor over one frame that knows the frame's absolute offset in the envelope.
#[derive(Clone, Copy)]
pub(super) struct Span<'a> {
    data: &'a [u8],
    origin: usize,
    position: usize,
}

impl<'a> Span<'a> {
    /// A cursor at the start of `data`, which begins `origin` bytes into the envelope.
    pub(super) fn new(data: &'a [u8], origin: usize) -> Self {
        Self {
            data,
            origin,
            position: 0,
        }
    }

    /// The current byte position in the envelope.
    pub(super) fn absolute(&self) -> usize {
        self.origin + self.position
    }

    /// The bytes left in this frame.
    pub(super) fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    /// The bytes consumed from this frame.
    pub(super) fn consumed(&self) -> usize {
        self.position
    }

    /// The envelope offset one past this frame, where a truncated take is reported.
    fn end(&self) -> usize {
        self.origin + self.data.len()
    }

    /// Charge and read one byte; a missing byte is `truncated` at its own position.
    pub(super) fn byte(&mut self, budget: &mut Budget) -> Positioned<u8> {
        let offset = self.absolute();
        let value = *self.data.get(self.position).ok_or_else(|| at("truncated", offset))?;
        budget.bytes(1).place(offset)?;
        self.position += 1;
        Ok(value)
    }

    /// Retain `size` bytes; a range past the frame is `truncated` at the frame's end.
    pub(super) fn take(&mut self, size: usize, budget: &mut Budget) -> Positioned<&'a [u8]> {
        let offset = self.absolute();
        if size > self.remaining() {
            return Err(at("truncated", self.end()));
        }
        budget.bytes(size as u64).place(offset)?;
        let taken = &self.data[self.position..self.position + size];
        self.position += size;
        Ok(taken)
    }

    /// Retain the rest of the frame.
    pub(super) fn rest(&mut self, budget: &mut Budget) -> Positioned<&'a [u8]> {
        self.take(self.remaining(), budget)
    }

    /// Read a shortest unsigned 64-bit length; its encoding refusals are reported at its first byte.
    pub(super) fn length(&mut self, budget: &mut Budget) -> Positioned<usize> {
        let start = self.absolute();
        let mut value = 0u64;
        for index in 0..10 {
            budget.elements(1).place(start)?;
            let octet = self.byte(budget)?;
            let group = u64::from(octet & 0x7f);
            if index == 9 && (group > 1 || octet >= 0x80) {
                return Err(at("integer_overflow", start));
            }
            value += group << (7 * index);
            if octet < 0x80 {
                if index > 0 && group == 0 {
                    return Err(at("noncanonical_integer", start));
                }
                return usize::try_from(value)
                    .ok()
                    .filter(|count| *count <= i64::MAX as usize)
                    .ok_or_else(|| at("limit_exceeded", start));
            }
        }
        Err(at("integer_overflow", start))
    }

    /// Read a length, then retain exactly that frame.
    pub(super) fn frame(&mut self, budget: &mut Budget) -> Positioned<&'a [u8]> {
        let size = self.length(budget)?;
        self.take(size, budget)
    }

    /// Read arbitrary-width shortest base-128 groups, reported at their first byte.
    pub(super) fn groups(&mut self, budget: &mut Budget) -> Positioned<Vec<u8>> {
        let start = self.absolute();
        let mut result = Vec::new();
        loop {
            budget.elements(1).place(start)?;
            let octet = self.byte(budget)?;
            let group = octet & 0x7f;
            if octet < 0x80 && !result.is_empty() && group == 0 {
                return Err(at("noncanonical_integer", start));
            }
            budget.bytes(8).place(start)?;
            result.push(group);
            if octet < 0x80 {
                return Ok(result);
            }
        }
    }

    /// Require this frame to be consumed exactly.
    pub(super) fn finished(&self) -> Positioned<()> {
        if self.remaining() != 0 {
            return Err(at("trailing_bytes", self.absolute()));
        }
        Ok(())
    }
}

/// A child count that the remaining bytes could hold; an impossible count is `truncated` at the frame's end.
pub(super) fn bounded_count(cursor: &mut Span, budget: &mut Budget) -> Positioned<usize> {
    let offset = cursor.absolute();
    let count = cursor.length(budget)?;
    budget.elements(count as u64).place(offset)?;
    if count > cursor.remaining() {
        return Err(at("truncated", cursor.absolute() + cursor.remaining()));
    }
    Ok(count)
}

/// Read a framed exact UTF-8 string; invalid UTF-8 is reported at the frame's length.
pub(super) fn read_text(cursor: &mut Span, budget: &mut Budget) -> Positioned<String> {
    let offset = cursor.absolute();
    let raw = cursor.frame(budget)?;
    budget.bytes(2 * raw.len() as u64).place(offset)?;
    String::from_utf8(raw.to_vec()).map_err(|_| at("invalid_utf8", offset))
}

/// One validated offset table and its cell data, with both anchors in the envelope.
#[derive(Clone, Copy)]
pub(super) struct CellIndex<'a> {
    table: &'a [u8],
    data: &'a [u8],
    table_origin: usize,
    data_origin: usize,
    width: usize,
    count: usize,
}

/// Check the complete table's bounds, order and minimal width before any cell is addressed.
pub(super) fn read_index<'a>(cursor: &mut Span<'a>, count: usize, budget: &mut Budget) -> Positioned<CellIndex<'a>> {
    let offset = cursor.absolute();
    let width = usize::from(cursor.byte(budget)?);
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Err(at("invalid_offset_width", offset));
    }
    if count > i64::MAX as usize / width - 1 {
        return Err(at("limit_exceeded", offset));
    }
    let size = (count + 1) * width;
    budget.elements(count as u64).place(offset)?;
    let table_origin = cursor.absolute();
    let table = cursor.take(size, budget)?;
    let data_origin = cursor.absolute();
    let data = cursor.rest(budget)?;
    let mut previous = 0;
    for index in 0..=count {
        let current = offset_at(table, index, width, table_origin, budget)?;
        if (index == 0 && current != 0) || current < previous || current > data.len() {
            return Err(at("invalid_offsets", table_origin + index * width));
        }
        previous = current;
    }
    if previous != data.len() {
        return Err(at("invalid_offsets", table_origin + count * width));
    }
    if width != minimal_width(previous) {
        return Err(at("nonminimal_offset_width", offset));
    }
    Ok(CellIndex {
        table,
        data,
        table_origin,
        data_origin,
        width,
        count,
    })
}

/// The smallest permitted offset width for a final offset.
fn minimal_width(length: usize) -> usize {
    match length {
        0..=255 => 1,
        256..=65_535 => 2,
        65_536..=4_294_967_295 => 4,
        _ => 8,
    }
}

/// Read one little-endian offset, charging it at its own table position.
fn offset_at(table: &[u8], index: usize, width: usize, origin: usize, budget: &mut Budget) -> Positioned<usize> {
    if index >= table.len() / width {
        return Err(at("invalid_offsets", origin));
    }
    let position = index * width;
    budget.bytes(width as u64).place(origin + position)?;
    budget.elements(1).place(origin + position)?;
    let value = table[position..position + width]
        .iter()
        .rev()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    usize::try_from(value)
        .ok()
        .filter(|value| *value <= i64::MAX as usize)
        .ok_or_else(|| at("invalid_offsets", origin + position))
}

impl<'a> CellIndex<'a> {
    /// The number of cells the table indexes.
    pub(super) fn count(&self) -> usize {
        self.count
    }

    /// One cell's bytes and its envelope offset, read through exactly two offsets.
    pub(super) fn selected(&self, position: usize, budget: &mut Budget) -> Positioned<(&'a [u8], usize)> {
        if position >= self.count {
            return Err(at("invalid_offsets", self.table_origin));
        }
        let left = offset_at(self.table, position, self.width, self.table_origin, budget)?;
        let right = offset_at(self.table, position + 1, self.width, self.table_origin, budget)?;
        if right < left || right > self.data.len() {
            return Err(at("invalid_offsets", self.table_origin));
        }
        Ok((&self.data[left..right], self.data_origin + left))
    }
}
