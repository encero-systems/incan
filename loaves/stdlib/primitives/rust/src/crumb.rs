//! Native byte construction for Crumb.
//!
//! Incan's generated code is slower than this codec needs; the Incan twin measures the gap.

/// Encoded bytes and the exact logical work charged by the Incan codec contract.
pub struct Encoded {
    pub data: Vec<u8>,
    pub byte_charge: u64,
    pub element_charge: u64,
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
        Buffer, Reader, binary_literal, decode_scalar, encode_integer_unsigned, encode_native_signed,
        encode_native_unsigned, frame, quote_text, scalar_text, spaces,
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
}
