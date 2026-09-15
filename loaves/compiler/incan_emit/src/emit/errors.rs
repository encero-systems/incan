//! Define error types for IR → Rust emission.
//!
//! These errors represent *backend emission* failures (as opposed to parsing or typechecking).
//!
//! ## Notes
//!
//! - Prefer actionable messages: users should know what construct is unsupported and what to do instead (e.g., compute
//!   at runtime).

/// Error during IR emission.
#[derive(Debug)]
pub enum EmitError {
    SynParse(String),
    InternalInvariant(String),
    Unsupported(String),
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmitError::SynParse(msg) => write!(f, "syn parse error: {}", msg),
            EmitError::InternalInvariant(msg) => write!(f, "internal code generation invariant failed: {}", msg),
            EmitError::Unsupported(msg) => write!(f, "unsupported: {}", msg),
        }
    }
}

impl std::error::Error for EmitError {}

#[cfg(test)]
mod tests {
    use super::EmitError;

    #[test]
    fn internal_invariant_errors_are_not_reported_as_unsupported_constructs() {
        let error = EmitError::InternalInvariant("filtered comprehension plan requires a filter".to_string());
        let message = error.to_string();
        assert_eq!(
            message,
            "internal code generation invariant failed: filtered comprehension plan requires a filter"
        );
        assert!(!message.starts_with("unsupported:"));
    }
}
