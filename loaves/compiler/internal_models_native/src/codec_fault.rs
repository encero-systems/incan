//! Canonical checker record refusals.

/// Infrastructure/capability refusals; none is a source-language diagnostic or a legacy fallback request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecFault {
    Malformed,
    ProfileMismatch,
    OwnerMismatch,
    BodyChanged,
    Unsupported(String),
    Limit,
    LimitExceeded(String, i64, i64),
}
