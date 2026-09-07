//! Shared parsing policy for Incan's runtime numeric-string conversions.
//!
//! Rust's numeric `FromStr` implementations do not accept underscore separators, while Incan source numerics do.
//! These helpers validate separators before removing them so generated code and direct execution cannot drift or
//! accidentally accept leading, trailing, or repeated underscores.

use std::borrow::Cow;

/// Parse an Incan `int` string into the ordinary signed 64-bit runtime carrier.
///
/// Existing Rust parsing behavior is preserved apart from accepting underscores placed between two ASCII digits.
/// Invalid syntax and values outside the ordinary `int` range both return `None`; callers retain the original input
/// when constructing the language's canonical `ValueError`.
pub fn parse_int_string(input: &str) -> Option<i64> {
    normalize_numeric_string(input)?.parse().ok()
}

/// Parse an Incan `float` string into the ordinary binary-float runtime carrier.
///
/// Existing Rust parsing behavior is preserved apart from accepting underscores placed between two ASCII digits.
/// Invalid syntax returns `None`; callers retain the original input when constructing the language's canonical
/// `ValueError`.
pub fn parse_float_string(input: &str) -> Option<f64> {
    normalize_numeric_string(input)?.parse().ok()
}

/// Validate underscore placement and remove separators only after that validation succeeds.
///
/// Separators are valid only between two ASCII digits. The returned value borrows the input when no normalization is
/// required and owns a separator-free copy otherwise. Numeric consumers still validate the surrounding integer,
/// float, or decimal grammar after this shared separator check.
pub fn normalize_numeric_string(input: &str) -> Option<Cow<'_, str>> {
    let bytes = input.as_bytes();
    if !bytes.contains(&b'_') {
        return Some(Cow::Borrowed(input));
    }

    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'_'
            && (index == 0
                || index + 1 == bytes.len()
                || !bytes[index - 1].is_ascii_digit()
                || !bytes[index + 1].is_ascii_digit())
        {
            return None;
        }
    }

    Some(Cow::Owned(
        input.chars().filter(|character| *character != '_').collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_integer_separator_placements() {
        for (input, expected) in [
            ("1_000", 1_000),
            ("+1_000", 1_000),
            ("-1_000", -1_000),
            ("00_7", 7),
            ("-9_223_372_036_854_775_808", i64::MIN),
        ] {
            assert_eq!(parse_int_string(input), Some(expected), "input `{input}`");
        }
    }

    #[test]
    fn parses_valid_float_separator_placements() {
        for (input, expected) in [
            ("1_000", 1_000.0),
            ("1_000.50", 1_000.5),
            (".5_0", 0.5),
            ("5_0.", 50.0),
            ("1.25e1_0", 1.25e10),
            ("1_0.2_5E-1_0", 10.25e-10),
            ("-1_0.5_0e-1_0", -10.5e-10),
        ] {
            assert_eq!(parse_float_string(input), Some(expected), "input `{input}`");
        }
    }

    #[test]
    fn preserves_existing_unseparated_float_forms() {
        assert!(parse_float_string("NaN").is_some_and(f64::is_nan));
        assert_eq!(parse_float_string("inf"), Some(f64::INFINITY));
        assert_eq!(parse_float_string("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_float_string("1e9999"), Some(f64::INFINITY));
    }

    #[test]
    fn rejects_invalid_separator_placements() {
        for input in [
            "_1",
            "1_",
            "1__0",
            "+_1",
            "1_.0",
            "1._0",
            "1.0_",
            "1_e2",
            "1e_2",
            "1e+_2",
            "1e2_",
            "i_nf",
            "in_finity",
            "n_an",
        ] {
            assert_eq!(parse_int_string(input), None, "int input `{input}`");
            assert_eq!(parse_float_string(input), None, "float input `{input}`");
        }
    }

    #[test]
    fn retains_existing_range_and_syntax_failures() {
        assert_eq!(parse_int_string("9_223_372_036_854_775_808"), None);
        assert_eq!(parse_int_string(" 1_000"), None);
        assert_eq!(parse_float_string("1_000 "), None);
        assert_eq!(parse_float_string("not-a-number"), None);
    }
}
