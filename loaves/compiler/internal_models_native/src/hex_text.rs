//! Lowercase hexadecimal text, matching the standard library's strict codec and its refusal text.

/// Encode bytes as lowercase hexadecimal text.
pub(crate) fn hexlify(data: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(data.len() * 2);
    for &byte in data {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    text
}

/// Map one ASCII hexadecimal byte to its four-bit value.
fn hex_value(code: u8) -> Option<u8> {
    match code {
        b'0'..=b'9' => Some(code - b'0'),
        b'A'..=b'F' => Some(code - b'A' + 10),
        b'a'..=b'f' => Some(code - b'a' + 10),
        _ => None,
    }
}

/// Decode strict hexadecimal text; the refusal is the standard codec's error detail.
pub(crate) fn unhexlify(text: &str) -> Result<Vec<u8>, String> {
    // The standard codec measures text in Unicode scalar values and reads its UTF-8 bytes in pairs.
    let scalars = text.chars().count();
    if scalars % 2 != 0 {
        return Err("hex input must contain an even number of characters".into());
    }
    let encoded = text.as_bytes();
    let mut out = Vec::with_capacity(scalars / 2);
    for pair in 0..scalars / 2 {
        let index = pair * 2;
        let digit = |position: usize| encoded.get(position).copied().and_then(hex_value);
        let Some(high) = digit(index) else {
            return Err(invalid_character(text, index));
        };
        let Some(low) = digit(index + 1) else {
            return Err(invalid_character(text, index + 1));
        };
        out.push(high * 16 + low);
    }
    Ok(out)
}

/// Name the scalar at a byte position the way the standard codec reports it.
fn invalid_character(text: &str, position: usize) -> String {
    let character = text.chars().nth(position).map(String::from).unwrap_or_default();
    format!("invalid hex character: {character}")
}
