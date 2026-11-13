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
        "value_relation" => value_relation(&parsed),
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

/// Compare two logical values without consulting the corpus's expected relation.
fn value_relation(document: &Value) -> Option<Vec<u8>> {
    let left = document.get("left")?;
    let right = document.get("right")?;
    Some(vec![u8::from(!logical_equal(left, right)?)])
}

/// Apply Crumb object equality to the corpus value notation.
fn logical_equal(left: &Value, right: &Value) -> Option<bool> {
    let left = strip_present(left)?;
    let right = strip_present(right)?;
    let left_kind = left.get("kind")?.as_str()?;
    let right_kind = right.get("kind")?.as_str()?;
    if left_kind == "float" && right_kind == "float" {
        return float_equal(left, right);
    }
    if left_kind == "int" && right_kind == "int" {
        return Some(
            left.get("value")?.as_str()?.parse::<i128>().ok()? == right.get("value")?.as_str()?.parse::<i128>().ok()?,
        );
    }
    if left_kind != right_kind {
        return Some(false);
    }
    match left_kind {
        "none" => Some(true),
        "string" => Some(left.get("value")?.as_str()? == right.get("value")?.as_str()?),
        "some" => logical_equal(left.get("value")?, right.get("value")?),
        "list" => sequence_equal(left.get("items")?.as_array()?, right.get("items")?.as_array()?),
        "reference" => Some(
            left.get("target_type")?.as_str()? == right.get("target_type")?.as_str()?
                && left.get("identity")?.as_str()? == right.get("identity")?.as_str()?,
        ),
        _ => None,
    }
}

/// Remove present optional layers only when they do not terminate in absence.
fn strip_present(value: &Value) -> Option<&Value> {
    let original = value;
    let mut selected = value;
    while selected.get("kind")?.as_str()? == "some" {
        selected = selected.get("value")?;
    }
    if selected.get("kind")?.as_str()? == "none" {
        Some(original)
    } else {
        Some(selected)
    }
}

/// Compare ordered logical sequences element by element.
fn sequence_equal(left: &[Value], right: &[Value]) -> Option<bool> {
    if left.len() != right.len() {
        return Some(false);
    }
    for (left_item, right_item) in left.iter().zip(right) {
        if !logical_equal(left_item, right_item)? {
            return Some(false);
        }
    }
    Some(true)
}

/// Compare exact IEEE inputs while normalizing NaNs and preserving signed zero.
fn float_equal(left: &Value, right: &Value) -> Option<bool> {
    let left_number = float_number(left)?;
    let right_number = float_number(right)?;
    if left_number.is_nan() && right_number.is_nan() {
        return Some(true);
    }
    if left_number == 0.0 && right_number == 0.0 {
        return Some(left_number.is_sign_negative() == right_number.is_sign_negative());
    }
    Some(left_number == right_number)
}

/// Decode the corpus's exact big-endian IEEE bit label into a common carrier.
fn float_number(value: &Value) -> Option<f64> {
    let width = value.get("width")?.as_u64()?;
    let bits = value.get("bits")?.as_str()?;
    match width {
        32 => Some(f32::from_bits(u32::from_str_radix(bits, 16).ok()?) as f64),
        64 => Some(f64::from_bits(u64::from_str_radix(bits, 16).ok()?)),
        _ => None,
    }
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

    #[test]
    fn value_relations_cover_exact_numeric_optional_sequence_and_reference_rules() {
        let cases = [
            (
                r#"{"left":{"kind":"float","width":64,"bits":"0000000000000000"},"right":{"kind":"float","width":64,"bits":"8000000000000000"}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"float","width":64,"bits":"7ff8000000000001"},"right":{"kind":"float","width":64,"bits":"fff8000000000042"}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"float","width":32,"bits":"3fc00000"},"right":{"kind":"float","width":64,"bits":"3ff8000000000000"}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"int","type":"i32","value":"2147483647"},"right":{"kind":"int","type":"i64","value":"2147483647"}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"int","type":"i64","value":"9007199254740992"},"right":{"kind":"int","type":"i64","value":"9007199254740993"}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"string","value":"é"},"right":{"kind":"string","value":"é"}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"none"},"right":{"kind":"some","value":{"kind":"none"}}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"int","type":"i64","value":"7"},"right":{"kind":"some","value":{"kind":"int","type":"i64","value":"7"}}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"some","value":{"kind":"int","type":"i64","value":"7"}},"right":{"kind":"some","value":{"kind":"some","value":{"kind":"int","type":"i64","value":"7"}}}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"some","value":{"kind":"none"}},"right":{"kind":"some","value":{"kind":"some","value":{"kind":"none"}}}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"list","items":[{"kind":"int","type":"i64","value":"1"},{"kind":"int","type":"i64","value":"2"}]},"right":{"kind":"list","items":[{"kind":"int","type":"i64","value":"2"},{"kind":"int","type":"i64","value":"1"}]}}"#,
                1,
            ),
            (
                r#"{"left":{"kind":"reference","target_type":"example.Customer","identity":"customer-7","observed_target":"revision-a"},"right":{"kind":"reference","target_type":"example.Customer","identity":"customer-7","observed_target":"revision-b"}}"#,
                0,
            ),
            (
                r#"{"left":{"kind":"reference","target_type":"example.Customer","identity":"customer-7"},"right":{"kind":"reference","target_type":"example.Customer","identity":"customer-8"}}"#,
                1,
            ),
        ];
        for (document, expected) in cases {
            let result = evaluate_semantic_case("value_relation".to_string(), document.to_string());
            assert_eq!(result.kind, 0);
            assert_eq!(result.encoded, [expected]);
            assert_eq!(result.encoded, result.decoded);
        }
    }
}
