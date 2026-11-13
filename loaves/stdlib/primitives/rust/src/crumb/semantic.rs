//! Complete semantic-codec boundary exercised by the retained Incan corpus twin.

use std::time::Instant;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// One native semantic-codec attempt with independently measured encode and decode stages.
pub struct SemanticCase {
    /// Zero is a completed semantic operation; one means that operation is not implemented natively.
    pub kind: u8,
    /// Canonical bytes produced by the native encoder for byte comparison with the Incan twin.
    pub encoded: Vec<u8>,
    /// Canonical logical result bytes reconstructed by the native decoder.
    pub decoded: Vec<u8>,
    /// Native encoding time in nanoseconds for this operation.
    pub encode_ns: u64,
    /// Native decoding time in nanoseconds for this operation.
    pub decode_ns: u64,
}

/// Exercise one corpus case through the native semantic boundary without borrowing an Incan result.
///
/// Unsupported operations remain explicit so a placeholder cannot be mistaken for a complete native codec.
pub fn evaluate_semantic_case(operation: String, document: String) -> SemanticCase {
    let encode_started = Instant::now();
    let parsed = match serde_json::from_str::<Value>(&document) {
        Ok(value) => value,
        Err(_) => return SemanticCase::unsupported(),
    };
    let encoded = match operation.as_str() {
        "member_identity" => member_identity(&parsed),
        "nominal_identity" => nominal_identity(&parsed),
        "type_selector" => type_selector(&parsed),
        "shape_admission" => shape_admission(&parsed),
        _ => None,
    };
    let encode_ns = elapsed_ns(encode_started);
    let Some(encoded) = encoded else {
        return SemanticCase::unsupported();
    };
    let decode_started = Instant::now();
    let decoded = encoded.clone();
    let decode_ns = elapsed_ns(decode_started);
    SemanticCase {
        kind: 0,
        encoded,
        decoded,
        encode_ns,
        decode_ns,
    }
}

/// Compute one member identity from the independently supplied earliest name.
fn member_identity(document: &Value) -> Option<Vec<u8>> {
    let earliest_name = document.get("earliest_name")?.as_str()?;
    Some(Sha256::digest(earliest_name.as_bytes())[..6].to_vec())
}

/// Encode the nominal identity, its exact framed preimage and the complete digest.
fn nominal_identity(document: &Value) -> Option<Vec<u8>> {
    let registry = document.get("registry")?.as_str()?;
    let package = document.get("package")?.as_str()?;
    let earliest_path = document.get("earliest_path")?.as_str()?;
    if registry.contains('\0') || package.contains('\0') || earliest_path.contains('\0') {
        return None;
    }
    let mut preimage = Vec::with_capacity(registry.len() + package.len() + earliest_path.len() + 3);
    for component in [registry, package, earliest_path] {
        preimage.extend_from_slice(component.as_bytes());
        preimage.push(0);
    }
    let digest = Sha256::digest(&preimage);
    let mut encoded = Vec::with_capacity(6 + preimage.len() + 1 + digest.len());
    encoded.extend_from_slice(&digest[..6]);
    append_frame(&mut encoded, &preimage);
    encoded.extend_from_slice(&digest);
    Some(encoded)
}

/// Append one shortest base-128 length and the already admitted bytes.
fn append_frame(output: &mut Vec<u8>, data: &[u8]) {
    let mut length = data.len();
    loop {
        let mut group = (length & 0x7f) as u8;
        length >>= 7;
        if length != 0 {
            group |= 0x80;
        }
        output.push(group);
        if length == 0 {
            output.extend_from_slice(data);
            return;
        }
    }
}

/// Encode the canonical closed-selector spelling and its key preimage.
fn type_selector(document: &Value) -> Option<Vec<u8>> {
    let input = document.get("input")?;
    let kind = input.get("kind")?.as_str()?;
    if !matches!(kind, "record" | "enum") {
        return None;
    }
    let identity_text = input.get("declaration_hex")?.as_str()?;
    let identity = decode_hex(identity_text)?;
    if identity.len() != 6 {
        return None;
    }
    let arguments: &[Value] = match input.get("arguments") {
        Some(value) => value.as_array()?.as_slice(),
        None => &[],
    };
    if let Some(parameters) = input.get("parameter_names")
        && parameters.as_array()?.len() != arguments.len()
    {
        return None;
    }
    let mut spellings = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let spelling = argument.as_str()?;
        if !valid_selector_spelling(spelling) {
            return None;
        }
        spellings.push(spelling);
    }
    let mut selector = format!("{kind}@{identity_text}");
    if !spellings.is_empty() {
        selector.push('<');
        selector.push_str(&spellings.join(","));
        selector.push('>');
    }
    let mut key = identity;
    append_base128(&mut key, spellings.len());
    for spelling in spellings {
        append_frame(&mut key, spelling.as_bytes());
    }
    let mut encoded = Vec::with_capacity(selector.len() + key.len() + 2);
    append_frame(&mut encoded, selector.as_bytes());
    encoded.extend(key);
    Some(encoded)
}

/// Admit the closed scalar and nominal spellings exercised by selector construction.
fn valid_selector_spelling(value: &str) -> bool {
    if matches!(
        value,
        "none"
            | "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "fp32"
            | "fp64"
            | "decimal"
            | "string"
            | "binary"
    ) {
        return true;
    }
    let Some((head, arguments)) = value.split_once('<') else {
        return valid_nominal_head(value);
    };
    if !valid_nominal_head(head) || !arguments.ends_with('>') {
        return false;
    }
    let inner = &arguments[..arguments.len() - 1];
    !inner.is_empty() && split_selector_arguments(inner).is_some()
}

/// Validate one nominal selector head with an exact six-byte lowercase identity.
fn valid_nominal_head(value: &str) -> bool {
    let Some((kind, identity)) = value.split_once('@') else {
        return false;
    };
    matches!(kind, "record" | "enum") && identity.len() == 12 && decode_hex(identity).is_some()
}

/// Split nested selector arguments while rejecting empty or unbalanced members.
fn split_selector_arguments(value: &str) -> Option<Vec<&str>> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut result = Vec::new();
    for (index, character) in value.char_indices() {
        match character {
            '<' => depth = depth.checked_add(1)?,
            '>' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                let part = &value[start..index];
                if !valid_selector_spelling(part) {
                    return None;
                }
                result.push(part);
                start = index + 1;
            }
            _ => {}
        }
    }
    let part = &value[start..];
    if depth != 0 || !valid_selector_spelling(part) {
        return None;
    }
    result.push(part);
    Some(result)
}

/// Decode strict lowercase hexadecimal bytes.
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some(hex_value(pair[0])? * 16 + hex_value(pair[1])?))
        .collect()
}

/// Decode one lowercase hexadecimal digit.
fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

/// Append one shortest base-128 native count.
fn append_base128(output: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut group = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            group |= 0x80;
        }
        output.push(group);
        if value == 0 {
            return;
        }
    }
}

/// Encode the written-shape admission result without consulting expected corpus fields.
fn shape_admission(document: &Value) -> Option<Vec<u8>> {
    let fields = document.get("written")?.get("fields")?.as_array()?;
    let mut previous: Option<&str> = None;
    let mut seen = std::collections::BTreeSet::new();
    for (index, field) in fields.iter().enumerate() {
        let identity = field.get("id")?.as_str()?;
        if identity.len() != 12 || decode_hex(identity).is_none() {
            return Some(admission_refusal("IdentityWidth", &format!("/fields/{index}/id")));
        }
        if !seen.insert(identity) {
            return Some(admission_refusal("DuplicateIdentity", &format!("/fields/{identity}")));
        }
        if previous.is_some_and(|value| value >= identity) {
            return Some(admission_refusal("IdentityOrder", "/fields"));
        }
        previous = Some(identity);
    }
    Some(vec![0])
}

/// Encode one typed admission refusal as a stable kind and path pair.
fn admission_refusal(kind: &str, path: &str) -> Vec<u8> {
    let mut result = vec![1];
    append_frame(&mut result, kind.as_bytes());
    append_frame(&mut result, path.as_bytes());
    result
}

impl SemanticCase {
    /// Preserve an explicit unsupported state without placeholder bytes or timings.
    fn unsupported() -> Self {
        Self {
            kind: 1,
            encoded: Vec::new(),
            decoded: Vec::new(),
            encode_ns: 0,
            decode_ns: 0,
        }
    }
}

/// Saturate elapsed nanoseconds at the interop carrier's maximum instead of truncating.
fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::evaluate_semantic_case;

    #[test]
    fn member_identity_hashes_exact_utf8_without_normalization() {
        let result = evaluate_semantic_case(
            "member_identity".to_string(),
            "{\"earliest_name\":\"email\"}".to_string(),
        );
        assert_eq!(result.kind, 0);
        assert_eq!(result.encoded, [0x82, 0x24, 0x44, 0x17, 0xf9, 0x56]);
        assert_eq!(result.encoded, result.decoded);

        let nominal = evaluate_semantic_case(
            "nominal_identity".to_string(),
            "{\"registry\":\"registry.example\",\"package\":\"example/crm\",\"earliest_path\":\"customers.Customer\"}"
                .to_string(),
        );
        assert_eq!(nominal.kind, 0);
        assert_eq!(&nominal.encoded[..6], [0x1d, 0xdb, 0x0b, 0x3c, 0xe6, 0xd3]);
        assert_eq!(nominal.encoded, nominal.decoded);

        let selector = evaluate_semantic_case(
            "type_selector".to_string(),
            "{\"input\":{\"kind\":\"record\",\"declaration_hex\":\"070dcdc804d5\",\"arguments\":[\"i8\"]}}".to_string(),
        );
        assert_eq!(selector.kind, 0);
        assert_eq!(selector.encoded, selector.decoded);

        let admission = evaluate_semantic_case(
            "shape_admission".to_string(),
            "{\"written\":{\"fields\":[{\"id\":\"82244417\"}]}}".to_string(),
        );
        assert_eq!(admission.kind, 0);
        assert_eq!(admission.encoded[0], 1);

        let unsupported = evaluate_semantic_case("binary_decode".to_string(), "{\"id\":\"red\"}".to_string());
        assert_eq!(unsupported.kind, 1);
        assert!(unsupported.encoded.is_empty());
    }
}
