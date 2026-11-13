//! Native byte construction for Crumb.
//!
//! Incan's generated code is slower than this codec needs; the Incan twin measures the gap.

/// Encoded bytes and the exact logical work charged by the Incan codec contract.
pub struct Encoded {
    pub data: Vec<u8>,
    pub byte_charge: u64,
    pub element_charge: u64,
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
    use super::{encode_integer_unsigned, encode_native_signed, encode_native_unsigned, frame};

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
}
