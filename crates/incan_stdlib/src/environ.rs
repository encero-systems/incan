//! Runtime support for `std.environ`.
//!
//! The Incan source module owns the public `EnvironError` shape. This Rust edge only reads the host environment and
//! returns stable, non-secret error categories so diagnostics do not stringify secret-bearing OS values.

const INVALID_KEY: &str = "invalid_key";
const MISSING: &str = "missing";
const NOT_UNICODE: &str = "not_unicode";

/// Read a Unicode environment variable from the current process.
///
/// Error categories:
/// - `invalid_key`: `key` is empty or contains a host-invalid `=` or NUL character.
/// - `missing`: `key` is not present in the current process environment.
/// - `not_unicode`: the host value is present but cannot be represented as Unicode text.
pub fn _get_raw(key: String) -> Result<String, String> {
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return Err(INVALID_KEY.to_string());
    }

    classify_var_result(std::env::var(&key))
}

/// Collapse host environment failures into stable categories without formatting host values.
fn classify_var_result(result: Result<String, std::env::VarError>) -> Result<String, String> {
    match result {
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Err(MISSING.to_string()),
        Err(std::env::VarError::NotUnicode(_)) => Err(NOT_UNICODE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{_get_raw, classify_var_result};

    #[test]
    fn empty_key_is_invalid() {
        assert_eq!(_get_raw("".to_string()), Err("invalid_key".to_string()));
    }

    #[test]
    fn host_invalid_key_characters_are_invalid() {
        assert_eq!(_get_raw("A=B".to_string()), Err("invalid_key".to_string()));
        assert_eq!(_get_raw("A\0B".to_string()), Err("invalid_key".to_string()));
    }

    #[test]
    fn missing_key_uses_stable_category() {
        let key = format!("INCAN_STDLIB_ENVIRON_MISSING_{}", std::process::id());
        assert_eq!(_get_raw(key), Err("missing".to_string()));
    }

    #[test]
    fn non_unicode_values_use_a_stable_redacted_category() {
        let secret_bearing_host_value = std::ffi::OsString::from("must-not-appear");
        assert_eq!(
            classify_var_result(Err(std::env::VarError::NotUnicode(secret_bearing_host_value))),
            Err("not_unicode".to_string())
        );
    }
}
