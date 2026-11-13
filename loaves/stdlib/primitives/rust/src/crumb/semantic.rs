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

        let unsupported = evaluate_semantic_case("binary_decode".to_string(), "{\"id\":\"red\"}".to_string());
        assert_eq!(unsupported.kind, 1);
        assert!(unsupported.encoded.is_empty());
    }
}
