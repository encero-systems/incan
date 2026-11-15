//! Native byte construction for Crumb.
//!
//! Incan's generated code is slower than this codec needs; the Incan twin measures the gap.

#[path = "crumb/layout.rs"]
mod layout;
#[path = "crumb/scalar_records.rs"]
mod scalar_records;
#[path = "crumb/semantic.rs"]
mod semantic;

pub use layout::{EnvelopeLayout, column_envelope, row_envelope};
pub use scalar_records::{ScalarRecordBatch, encode_scalar_records};
pub use semantic::{SemanticCase, evaluate_semantic_case};

/// Encoded bytes and the exact logical work charged by the Incan codec contract.
pub struct Encoded {
    pub data: Vec<u8>,
    pub byte_charge: u64,
    pub element_charge: u64,
}

/// One completed indexed-cell layout and the exact logical work charged by its Incan contract.
pub struct IndexedLayout {
    pub data: Vec<u8>,
    pub byte_charge: i64,
    pub element_charge: i64,
}

/// One canonical integer parse with distinct spelling and range failures.
pub struct ParsedInteger {
    pub kind: u8,
    pub signed: i128,
    pub unsigned: u128,
}

/// One canonical floating-point parse retaining its network-order IEEE bits.
pub struct ParsedFloat {
    pub kind: u8,
    pub data: Vec<u8>,
}

/// One fixed-scale decimal parse with contract-specific failure classification.
pub struct ParsedDecimal {
    pub kind: u8,
    pub coefficient: i128,
}

/// One decoded source scalar retained for crossing-free COMIC token scanning.
#[derive(Clone, Debug)]
pub struct TextScalar {
    pub text: String,
    pub width: i64,
}

/// A complete UTF-8 and raw-bidirectional-control validation result.
pub struct DecodedText {
    pub kind: u8,
    pub line: i64,
    pub column: i64,
    pub scalars: Vec<TextScalar>,
}

/// One bounded scan result; numeric tags keep Incan's typed error surface authoritative.
#[derive(Clone, Copy)]
pub struct Scan {
    pub kind: u8,
    pub end: i64,
    pub value: u64,
    pub byte_charge: i64,
    pub element_charge: i64,
    pub detail: i64,
}

/// One UTF-8 scalar decode result for the COMIC tokenizer.
pub struct DecodedScalar {
    pub kind: u8,
    pub text: String,
    pub width: i64,
}

/// An owned native cursor for bounded U, L, and F byte scanning.
#[derive(Clone, Debug)]
pub struct Reader {
    data: Vec<u8>,
    offset: usize,
}

/// Operation-local byte assembly without generated per-octet list traffic.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    data: Vec<u8>,
}

/// Operation-local text assembly without repeated generated concatenation.
#[derive(Clone, Debug, Default)]
pub struct TextBuilder {
    text: String,
}

impl TextBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, value: String) {
        self.text.push_str(&value);
    }

    pub fn finish(&self) -> String {
        self.text.clone()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> i64 {
        self.data.len() as i64
    }

    pub fn append(&mut self, value: Vec<u8>) {
        self.data.extend(value);
    }

    pub fn push(&mut self, value: u8) {
        self.data.push(value);
    }

    pub fn finish(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// Canonically quote text without generated repeated-string concatenation.
pub fn quote_text(value: String) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => push_unicode_escape(&mut output, character as u32),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => push_unicode_escape(&mut output, character as u32),
            _ => output.push(character),
        }
    }
    output.push('"');
    output
}

/// Canonically quote arbitrary bytes with printable ASCII preserved.
pub fn binary_literal(data: Vec<u8>) -> String {
    let mut output = String::with_capacity(data.len() + 3);
    output.push_str("b\"");
    for byte in data {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            32..=126 => output.push(char::from(byte)),
            _ => {
                output.push_str("\\x");
                output.push(hex_digit(byte >> 4));
                output.push(hex_digit(byte & 0x0f));
            }
        }
    }
    output.push('"');
    output
}

/// Produce deterministic indentation in one allocation.
pub fn spaces(count: i64) -> String {
    if count <= 0 {
        String::new()
    } else {
        " ".repeat(count as usize)
    }
}

fn push_unicode_escape(output: &mut String, value: u32) {
    output.push_str("\\u{");
    let mut shift = if value >= 0x1000 {
        12
    } else if value >= 0x100 {
        8
    } else if value >= 0x10 {
        4
    } else {
        0
    };
    loop {
        output.push(hex_digit(((value >> shift) & 0x0f) as u8));
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
    output.push('}');
}

fn hex_digit(value: u8) -> char {
    char::from(if value < 10 { b'0' + value } else { b'a' + value - 10 })
}

/// Decode exactly one UTF-8 scalar without validating unrelated following input.
pub fn decode_scalar(data: &Vec<u8>, index: i64) -> DecodedScalar {
    let invalid = || DecodedScalar {
        kind: 1,
        text: String::new(),
        width: 0,
    };
    let Ok(index) = usize::try_from(index) else {
        return invalid();
    };
    let Some(lead) = data.get(index).copied() else {
        return invalid();
    };
    let width = match lead {
        0..=127 => 1,
        194..=223 => 2,
        224..=239 => 3,
        240..=244 => 4,
        _ => return invalid(),
    };
    let Some(end) = index.checked_add(width) else {
        return invalid();
    };
    let Some(encoded) = data.get(index..end) else {
        return invalid();
    };
    let Ok(text) = std::str::from_utf8(encoded) else {
        return invalid();
    };
    DecodedScalar {
        kind: 0,
        text: text.to_string(),
        width: width as i64,
    }
}

/// Convert one checked code point to an owned scalar spelling.
pub fn scalar_text(value: i64) -> Option<String> {
    let value = u32::try_from(value).ok()?;
    Some(char::from_u32(value)?.to_string())
}

/// Decode source once so Incan token scanning does not cross the native boundary per scalar.
pub fn decode_text(data: &Vec<u8>) -> DecodedText {
    let text = match std::str::from_utf8(data) {
        Ok(value) => value,
        Err(error) => {
            let (line, column) = source_position(&data[..error.valid_up_to()]);
            return DecodedText {
                kind: 1,
                line,
                column,
                scalars: Vec::new(),
            };
        }
    };
    let mut scalars = Vec::with_capacity(text.chars().count());
    let mut line = 1;
    let mut column = 1;
    for character in text.chars() {
        if matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            return DecodedText {
                kind: 2,
                line,
                column,
                scalars: Vec::new(),
            };
        }
        let width = match character.len_utf8() {
            1 => 1,
            2 => 2,
            3 => 3,
            _ => 4,
        };
        scalars.push(TextScalar {
            text: character.to_string(),
            width,
        });
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    DecodedText {
        kind: 0,
        line: 1,
        column: 1,
        scalars,
    }
}

/// Locate one UTF-8 byte boundary using Unicode-scalar line and column counts.
fn source_position(prefix: &[u8]) -> (i64, i64) {
    let Ok(text) = std::str::from_utf8(prefix) else {
        return (1, 1);
    };
    let mut line = 1;
    let mut column = 1;
    for character in text.chars() {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

impl Reader {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, offset: 0 }
    }

    pub fn position(&self) -> i64 {
        self.offset as i64
    }

    pub fn remaining(&self) -> i64 {
        (self.data.len() - self.offset) as i64
    }

    pub fn scan_unsigned(&self, byte_limit: i64, element_limit: i64) -> Scan {
        self.scan(byte_limit, element_limit, false)
    }

    pub fn scan_length(&self, byte_limit: i64, element_limit: i64) -> Scan {
        self.scan(byte_limit, element_limit, true)
    }

    pub fn take_to(&mut self, end: i64) -> Vec<u8> {
        let end = end as usize;
        let result = self.data[self.offset..end].to_vec();
        self.offset = end;
        result
    }

    pub fn commit(&mut self, end: i64) {
        self.offset = end as usize;
    }

    pub fn take_frame(&mut self, start: i64, size: i64) -> Vec<u8> {
        let start = start as usize;
        let end = start + size as usize;
        let result = self.data[start..end].to_vec();
        self.offset = end;
        result
    }

    fn scan(&self, byte_limit: i64, element_limit: i64, length: bool) -> Scan {
        let mut scan = Scan::at(self.offset);
        loop {
            if scan.end as usize == self.data.len() {
                let detail = scan.end;
                return scan.failed(1, detail);
            }
            if scan.byte_charge == byte_limit {
                return scan.failed(4, 0);
            }
            scan.byte_charge += 1;
            if scan.element_charge == element_limit {
                return scan.failed(5, 0);
            }
            scan.element_charge += 1;
            let byte = self.data[scan.end as usize];
            scan.end += 1;
            if byte < 128 {
                if scan.end - self.offset as i64 > 1 && byte == 0 {
                    return scan.failed(2, self.offset as i64);
                }
                if length {
                    return self.read_length(scan);
                }
                return scan;
            }
        }
    }

    fn read_length(&self, mut scan: Scan) -> Scan {
        let count = scan.end as usize - self.offset;
        if count > 10 || (count == 10 && self.data[scan.end as usize - 1] > 1) {
            return scan.failed(3, self.offset as i64);
        }
        let mut value = 0u64;
        for byte in self.data[self.offset..scan.end as usize].iter().rev() {
            value = value * 128 + u64::from(byte & 0x7f);
        }
        scan.value = value;
        scan
    }
}

impl Scan {
    fn at(offset: usize) -> Self {
        Self {
            kind: 0,
            end: offset as i64,
            value: 0,
            byte_charge: 0,
            element_charge: 0,
            detail: 0,
        }
    }

    fn failed(mut self, kind: u8, detail: i64) -> Self {
        self.kind = kind;
        self.detail = detail;
        self
    }
}

/// Encode one native unsigned value with shortest little-endian base-128 groups.
pub fn encode_native_unsigned(value: u128) -> Encoded {
    let data = encode_base128(value, false);
    let groups = data.len() as u64;
    Encoded {
        data,
        byte_charge: groups,
        element_charge: top_bits(value) + groups,
    }
}

/// Zigzag and encode one native signed value without overflowing at either endpoint.
pub fn encode_native_signed(value: i128) -> Encoded {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    encode_zigzag(magnitude, negative)
}

/// Zigzag and encode one logical unsigned value, whose physical form may require 129 bits.
pub fn encode_integer_unsigned(value: u128) -> Encoded {
    encode_zigzag(value, false)
}

/// Prefix owned bytes with their shortest native length.
pub fn frame(data: Vec<u8>) -> Encoded {
    let prefix = encode_native_unsigned(data.len() as u128);
    let data_len = data.len() as u64;
    let prefix_len = prefix.data.len() as u64;
    let mut output = Vec::with_capacity(prefix.data.len() + data.len());
    output.extend_from_slice(&prefix.data);
    output.extend(data);
    Encoded {
        data: output,
        byte_charge: prefix.byte_charge + prefix_len + data_len,
        element_charge: prefix.element_charge,
    }
}

/// Lay out all cell offsets and payloads in one native call across the Incan boundary.
pub fn indexed(cells: Vec<Vec<u8>>, width: u8) -> IndexedLayout {
    let total = cells.iter().map(Vec::len).sum::<usize>();
    let table_length = (cells.len() + 1) * usize::from(width);
    let output_length = 1 + table_length + total;
    let mut data = Vec::with_capacity(output_length);
    data.push(width);
    let mut position = 0usize;
    push_offset(&mut data, position, width);
    for cell in &cells {
        position += cell.len();
        push_offset(&mut data, position, width);
    }
    for cell in cells {
        data.extend(cell);
    }
    IndexedLayout {
        data,
        byte_charge: (output_length * 2) as i64,
        element_charge: (table_length / usize::from(width) - 1) as i64,
    }
}

/// Parse one canonical decimal integer and enforce its declared exact width.
pub fn parse_integer(value: String, signed: bool, bits: u8) -> ParsedInteger {
    if !canonical_integer(&value) {
        return ParsedInteger {
            kind: 1,
            signed: 0,
            unsigned: 0,
        };
    }
    if signed {
        if let Ok(number) = value.parse::<i128>()
            && signed_in_width(number, bits)
        {
            return ParsedInteger {
                kind: 0,
                signed: number,
                unsigned: 0,
            };
        }
    } else if let Ok(number) = value.parse::<u128>()
        && unsigned_in_width(number, bits)
    {
        return ParsedInteger {
            kind: 0,
            signed: 0,
            unsigned: number,
        };
    }
    ParsedInteger {
        kind: 2,
        signed: 0,
        unsigned: 0,
    }
}

/// Parse one canonical COMIC float and retain its exact-width network-order bits.
pub fn parse_float(value: String, width: u8) -> ParsedFloat {
    if !nonfinite(&value) && !valid_finite_float(&value) {
        return ParsedFloat {
            kind: 1,
            data: Vec::new(),
        };
    }
    if width == 32 {
        return match value.parse::<f32>() {
            Ok(number) => ParsedFloat {
                kind: 0,
                data: number.to_bits().to_be_bytes().to_vec(),
            },
            Err(_) => ParsedFloat {
                kind: 2,
                data: Vec::new(),
            },
        };
    }
    match value.parse::<f64>() {
        Ok(number) => ParsedFloat {
            kind: 0,
            data: number.to_bits().to_be_bytes().to_vec(),
        },
        Err(_) => ParsedFloat {
            kind: 2,
            data: Vec::new(),
        },
    }
}

/// Parse one `d`-suffixed decimal under its declared precision and exact scale.
pub fn parse_decimal(value: String, precision: u8, scale: u8) -> ParsedDecimal {
    let Some(number) = value.strip_suffix('d') else {
        return decimal_failure(1);
    };
    let mut parts = number.split('.');
    let Some(integral) = parts.next() else {
        return decimal_failure(2);
    };
    let Some(fractional) = parts.next() else {
        return decimal_failure(2);
    };
    if parts.next().is_some()
        || fractional.len() != usize::from(scale)
        || !decimal_digits(integral, true)
        || !decimal_digits(fractional, false)
    {
        return decimal_failure(2);
    }
    let mut coefficient = String::with_capacity(integral.len() + fractional.len());
    coefficient.push_str(integral);
    coefficient.push_str(fractional);
    if coefficient
        .strip_prefix('-')
        .is_some_and(|digits| digits.bytes().all(|digit| digit == b'0'))
    {
        coefficient = "0".to_string();
    }
    if coefficient.trim_start_matches('-').len() > usize::from(precision) {
        return decimal_failure(3);
    }
    match coefficient.parse::<i128>() {
        Ok(number) => ParsedDecimal {
            kind: 0,
            coefficient: number,
        },
        Err(_) => decimal_failure(3),
    }
}

/// Construct one classified decimal failure without an optional payload.
fn decimal_failure(kind: u8) -> ParsedDecimal {
    ParsedDecimal { kind, coefficient: 0 }
}

/// Accept canonical integer digits with one optional minus and no negative zero.
fn canonical_integer(value: &str) -> bool {
    if value.is_empty() || value.starts_with('+') || value.contains('_') {
        return false;
    }
    let digits = if let Some(digits) = value.strip_prefix('-') {
        if digits == "0" {
            return false;
        }
        digits
    } else {
        value
    };
    canonical_unsigned(digits)
}

/// Accept nonempty unsigned decimal digits without leading zeros.
fn canonical_unsigned(value: &str) -> bool {
    !value.is_empty()
        && (value.len() == 1 || !value.starts_with('0'))
        && value.bytes().all(|digit| digit.is_ascii_digit())
}

/// Check a parsed signed integer against one valid core width.
fn signed_in_width(value: i128, bits: u8) -> bool {
    if bits == 128 {
        return true;
    }
    let limit = 1i128 << u32::from(bits - 1);
    value >= -limit && value < limit
}

/// Check a parsed unsigned integer against one valid core width.
fn unsigned_in_width(value: u128, bits: u8) -> bool {
    bits == 128 || value < (1u128 << u32::from(bits))
}

/// Recognize the exact lowercase nonfinite COMIC spellings.
fn nonfinite(value: &str) -> bool {
    matches!(value, "inf" | "-inf" | "nan")
}

/// Validate finite COMIC float syntax before delegating numeric conversion to Rust.
fn valid_finite_float(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with('+')
        || value.contains('_')
        || (!value.contains('.') && !value.contains('e'))
    {
        return false;
    }
    let mut exponent_parts = value.split('e');
    let Some(base) = exponent_parts.next() else {
        return false;
    };
    if !decimal_float_base(base) {
        return false;
    }
    if let Some(exponent) = exponent_parts.next()
        && (exponent.starts_with('+') || !decimal_digits(exponent, true))
    {
        return false;
    }
    exponent_parts.next().is_none()
}

/// Validate digits around one optional decimal point.
fn decimal_float_base(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(integral) = parts.next() else {
        return false;
    };
    if !decimal_digits(integral, true) {
        return false;
    }
    if let Some(fractional) = parts.next()
        && !decimal_digits(fractional, false)
    {
        return false;
    }
    parts.next().is_none()
}

/// Validate one decimal digit run with an optional leading minus.
fn decimal_digits(value: &str, signed: bool) -> bool {
    let digits = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    !digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit())
}

/// Append one already-validated little-endian fixed-width offset.
fn push_offset(output: &mut Vec<u8>, mut value: usize, width: u8) {
    for _ in 0..width {
        output.push((value & 0xff) as u8);
        value >>= 8;
    }
}

fn encode_zigzag(magnitude: u128, negative: bool) -> Encoded {
    let magnitude_len = magnitude_bytes(magnitude);
    let mut groups = zigzag_groups(magnitude, negative);
    if groups.is_empty() {
        groups.push(0);
    } else {
        let last = groups.len() - 1;
        for byte in &mut groups[..last] {
            *byte |= 0x80;
        }
    }
    let doubled_len = zigzag_bytes(magnitude, negative);
    let group_count = groups.len() as u64;
    let bit_charge = zigzag_top_bits(magnitude, negative);
    let negative_bytes = if negative { doubled_len } else { 0 };
    Encoded {
        data: groups,
        byte_charge: magnitude_len + 1 + negative_bytes + group_count,
        element_charge: magnitude_len + negative_bytes + bit_charge + group_count,
    }
}

fn encode_base128(mut value: u128, continued: bool) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        let mut group = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 || continued {
            group |= 0x80;
        }
        result.push(group);
        if value == 0 {
            return result;
        }
    }
}

fn zigzag_groups(magnitude: u128, negative: bool) -> Vec<u8> {
    let doubled = magnitude.wrapping_mul(2);
    let low = if negative { doubled.wrapping_sub(1) } else { doubled };
    let high = zigzag_high(magnitude, doubled, negative);
    let mut groups = Vec::with_capacity(19);
    for index in 0..19 {
        let group = if index < 18 {
            ((low >> (index * 7)) & 0x7f) as u8
        } else {
            ((low >> 126) as u8 & 0x03) | ((high as u8) << 2)
        };
        groups.push(group);
    }
    while groups.len() > 1 && groups.last() == Some(&0) {
        groups.pop();
    }
    let last = groups.len() - 1;
    for byte in &mut groups[..last] {
        *byte |= 0x80;
    }
    groups
}

fn magnitude_bytes(value: u128) -> u64 {
    if value == 0 {
        0
    } else {
        u64::from(value.ilog2() / 8 + 1)
    }
}

fn zigzag_bytes(magnitude: u128, negative: bool) -> u64 {
    let bits = zigzag_bit_length(magnitude, negative);
    if bits == 0 { 0 } else { (bits + 7) / 8 }
}

fn zigzag_bit_length(magnitude: u128, negative: bool) -> u64 {
    if magnitude == 0 {
        return 0;
    }
    let doubled = if negative {
        magnitude.wrapping_mul(2).wrapping_sub(1)
    } else {
        magnitude.wrapping_mul(2)
    };
    if zigzag_high(magnitude, magnitude.wrapping_mul(2), negative) != 0 {
        129
    } else {
        u64::from(128 - doubled.leading_zeros())
    }
}

fn zigzag_high(magnitude: u128, doubled: u128, negative: bool) -> u128 {
    let high = magnitude >> 127;
    if negative && doubled == 0 && high != 0 {
        high - 1
    } else {
        high
    }
}

fn zigzag_top_bits(magnitude: u128, negative: bool) -> u64 {
    let bits = zigzag_bit_length(magnitude, negative);
    if bits == 0 { 0 } else { (bits - 1) % 8 + 1 }
}

fn top_bits(value: u128) -> u64 {
    if value == 0 {
        0
    } else {
        u64::from(128 - value.leading_zeros() - (value.ilog2() / 8) * 8)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Buffer, Reader, TextBuilder, binary_literal, decode_scalar, decode_text, encode_integer_unsigned,
        encode_native_signed, encode_native_unsigned, frame, indexed, parse_decimal, parse_float, parse_integer,
        quote_text, scalar_text, spaces,
    };

    #[test]
    fn native_boundaries_match_crumb_framing() {
        assert_eq!(encode_native_unsigned(0).data, [0]);
        assert_eq!(encode_native_unsigned(128).data, [0x80, 0x01]);
        assert_eq!(encode_native_signed(0).data, [0x00]);
        assert_eq!(encode_native_signed(-1).data, [0x01]);
        assert_eq!(encode_native_signed(-128).data, [0xff, 0x01]);
        assert_eq!(encode_native_signed(i128::MIN).data[18], 0x03);
        assert_eq!(frame(b"abc".to_vec()).data, b"\x03abc");
    }

    #[test]
    fn zigzag_keeps_the_unsigned_129th_bit() {
        assert_eq!(encode_integer_unsigned(u128::MAX).data.len(), 19);
        assert_eq!(encode_integer_unsigned(u128::MAX).data[18], 0x07);
    }

    #[test]
    fn reader_scans_lengths_under_independent_limits() {
        let mut reader = Reader::new(b"\x80\x01\x03abc".to_vec());
        let length = reader.scan_length(2, 2);
        assert_eq!((length.kind, length.value, length.end), (0, 128, 2));
        reader.commit(length.end);
        let frame = reader.scan_length(8, 8);
        assert_eq!((frame.kind, frame.value, frame.end), (0, 3, 3));
        assert_eq!(reader.take_frame(frame.end, frame.value as i64), b"abc");

        let limited = Reader::new(b"\x80\x01".to_vec()).scan_unsigned(2, 1);
        assert_eq!((limited.kind, limited.byte_charge, limited.element_charge), (5, 2, 1));
    }

    #[test]
    fn buffer_assembles_owned_bytes() {
        let mut buffer = Buffer::new();
        buffer.push(1);
        buffer.append(vec![2, 3]);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.finish(), [1, 2, 3]);
    }

    #[test]
    fn indexed_layout_batches_offsets_payloads_and_charges() {
        let layout = indexed(vec![b"a".to_vec(), b"bc".to_vec()], 1);
        assert_eq!(layout.data, b"\x01\x00\x01\x03abc");
        assert_eq!((layout.byte_charge, layout.element_charge), (14, 2));

        let wide = indexed(vec![vec![0; 256]], 2);
        assert_eq!(&wide.data[..5], b"\x02\x00\x00\x00\x01");
        assert_eq!((wide.byte_charge, wide.element_charge), (522, 1));
    }

    #[test]
    fn number_parsers_preserve_comic_failure_classes_and_bits() {
        assert_eq!(parse_integer("-128".to_string(), true, 8).signed, -128);
        assert_eq!(parse_integer("128".to_string(), true, 8).kind, 2);
        assert_eq!(parse_integer("-0".to_string(), true, 128).kind, 1);
        assert_eq!(parse_integer(u128::MAX.to_string(), false, 128).unsigned, u128::MAX);

        assert_eq!(parse_float("1.5".to_string(), 32).data, 1.5f32.to_bits().to_be_bytes());
        assert_eq!(parse_float("1".to_string(), 64).kind, 1);
        assert_eq!(parse_float("nan".to_string(), 64).data.len(), 8);

        assert_eq!(parse_decimal("-0.00d".to_string(), 3, 2).coefficient, 0);
        assert_eq!(parse_decimal("1.2d".to_string(), 3, 2).kind, 2);
        assert_eq!(parse_decimal("123.45d".to_string(), 4, 2).kind, 3);
    }

    #[test]
    fn text_builder_assembles_owned_scalars() {
        let mut builder = TextBuilder::new();
        builder.push("a".to_string());
        builder.push("🙂".to_string());
        assert_eq!(builder.finish(), "a🙂");
    }

    #[test]
    fn text_literals_preserve_canonical_escaping() {
        assert_eq!(
            quote_text("a\n\0\u{85}\u{202e}\\\"".to_string()),
            "\"a\\n\\u{0}\\u{85}\\u{202e}\\\\\\\"\""
        );
        assert_eq!(
            binary_literal(vec![b'a', b'"', b'\\', 0, 0xff]),
            "b\"a\\\"\\\\\\x00\\xff\""
        );
        assert_eq!(spaces(4), "    ");
    }

    #[test]
    fn scalar_helpers_validate_one_code_point() {
        let decoded = decode_scalar(&"éz".as_bytes().to_vec(), 0);
        assert_eq!((decoded.kind, decoded.text.as_str(), decoded.width), (0, "é", 2));
        assert_eq!(decode_scalar(&vec![0xc0, 0x80], 0).kind, 1);
        assert_eq!(scalar_text(0x1f642).as_deref(), Some("🙂"));
        assert_eq!(scalar_text(0xd800), None);
    }

    #[test]
    fn text_decode_batches_scalars_and_reports_raw_bidi_positions() {
        let decoded = decode_text(&"aé\n🙂".as_bytes().to_vec());
        assert_eq!(decoded.kind, 0);
        assert_eq!(decoded.scalars.len(), 4);
        assert_eq!(decoded.scalars[1].width, 2);
        assert_eq!(decoded.scalars[3].text, "🙂");

        let bidi = decode_text(&"ok\nq\u{202e}".as_bytes().to_vec());
        assert_eq!((bidi.kind, bidi.line, bidi.column), (2, 2, 2));
        assert_eq!(decode_text(&vec![0xff]).kind, 1);
    }
}
