//! Complete semantic-codec boundary exercised by the retained Incan corpus twin.

use std::time::Instant;

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
/// The red implementation deliberately reports every operation as unsupported. Parsing and serializing the
/// independently supplied corpus document prove the interop and timing boundary while preventing a placeholder from
/// being mistaken for a complete native codec.
pub fn evaluate_semantic_case(_operation: String, document: String) -> SemanticCase {
    let encode_started = Instant::now();
    let encoded =
        match serde_json::from_str::<serde_json::Value>(&document).and_then(|value| serde_json::to_vec(&value)) {
            Ok(value) => value,
            Err(_) => Vec::new(),
        };
    let encode_ns = elapsed_ns(encode_started);
    let decode_started = Instant::now();
    let decoded =
        match serde_json::from_slice::<serde_json::Value>(&encoded).and_then(|value| serde_json::to_vec(&value)) {
            Ok(value) => value,
            Err(_) => Vec::new(),
        };
    let decode_ns = elapsed_ns(decode_started);
    SemanticCase {
        kind: 1,
        encoded,
        decoded,
        encode_ns,
        decode_ns,
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
    fn unsupported_boundary_still_round_trips_independent_input() {
        let result = evaluate_semantic_case("binary_decode".to_string(), "{\"id\":\"red\"}".to_string());
        assert_eq!(result.kind, 1);
        assert_eq!(result.encoded, result.decoded);
    }
}
