//! Runtime support for `std.environ`.
//!
//! The Incan source module owns the public `EnvironError` shape. This Rust edge only reads the host environment and
//! returns stable, non-secret error categories so diagnostics do not stringify secret-bearing OS values.

/// Read a Unicode environment variable from the current process.
///
/// Error categories:
/// - `invalid_key`: `key` is empty.
/// - `missing`: `key` is not present in the current process environment.
/// - `not_unicode`: the host value is present but cannot be represented as Unicode text.
pub fn _get_raw(key: String) -> Result<String, String> {
    if key.is_empty() {
        return Err("invalid_key".to_string());
    }

    match std::env::var(&key) {
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Err("missing".to_string()),
        Err(std::env::VarError::NotUnicode(_)) => Err("not_unicode".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::_get_raw;

    #[test]
    fn empty_key_is_invalid() {
        assert_eq!(_get_raw("".to_string()), Err("invalid_key".to_string()));
    }

    #[test]
    fn missing_key_uses_stable_category() {
        let key = format!("INCAN_STDLIB_ENVIRON_MISSING_{}", std::process::id());
        assert_eq!(_get_raw(key), Err("missing".to_string()));
    }
}
