//! Shared parsing and rendering policy for Incan's runtime numeric-string conversions.
//!
//! Rust's numeric `FromStr` implementations do not accept underscore separators, while Incan source numerics do.
//! These helpers validate separators before removing them so generated code and direct execution cannot drift or
//! accidentally accept leading, trailing, or repeated underscores. The same module owns the opposite direction for
//! `float`: [`float_to_string`] is the one spelling of a `float` as text, so `str(x)`, `f"{x}"`, `println(x)`, and
//! the replacement executor's observable text all render it identically.

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

/// Render an Incan `float` the way Python spells a float: always visibly a float.
///
/// Rust's `Display for f64` drops the fractional part of an integral value (`100.0` prints as `100`), so a program
/// writing SQL, JSON, or CSV emits an integer where it meant a float and a downstream reader infers the wrong
/// type (#1372). Python's `repr` keeps the value recognizably a float in every case, and this follows it exactly:
///
/// - the shortest digit string that round-trips, with at least one fractional digit (`100.0`, `1.5`, `0.0`, `-2.0`);
/// - positional notation while `1e-4 <= |value| < 1e16` and exponential outside it (`10000000000.0` for `1e10`, but
///   `1e+16` and `1e-05`), the same switch-over Python uses;
/// - an exponent spelled with an explicit sign and at least two digits (`1e+16`, `1.5e-07`);
/// - `inf`, `-inf`, and `nan` for the non-finite values, in Python's lower-case spelling.
///
/// The digits and the positional/exponential switch come from Rust's `Debug for f64`, which already selects the
/// shortest round-trip representation at those thresholds; only the exponent spelling and `nan` differ, and both
/// are normalized here. This is deliberately `float` only: the exact `f32`/`f64` carriers keep their native Rust
/// spelling, which the replacement profile pins as their checked-carrier behavior.
pub fn float_to_string(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() { "-inf" } else { "inf" }.to_string();
    }
    let shortest = format!("{value:?}");
    let Some((mantissa, exponent)) = shortest.split_once('e') else {
        return shortest;
    };
    // Python writes the exponent with its sign and at least two digits.
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(digits) => ('-', digits),
        None => ('+', exponent),
    };
    format!("{mantissa}e{sign}{digits:0>2}")
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
    fn renders_floats_as_python_spells_them() {
        for (value, expected) in [
            (100.0, "100.0"),
            (1.5, "1.5"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (-2.0, "-2.0"),
            (25.0, "25.0"),
            (1e10, "10000000000.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (1e100, "1e+100"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.0 / 3.0, "0.3333333333333333"),
            (f64::MAX, "1.7976931348623157e+308"),
            // The positional/exponential switch-over sits exactly where Python's `repr` puts it: the last
            // positional value below `1e16` and the first exponential value below `1e-4`.
            (9999999999999998.0, "9999999999999998.0"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (0.00009999, "9.999e-05"),
            (-1e-5, "-1e-05"),
            (-1e16, "-1e+16"),
            (1e22, "1e+22"),
            (1e300 * 10.0, "1e+301"),
            // Subnormals keep their shortest round-trip digits.
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (5e-324, "5e-324"),
            (f64::MAX * 10.0, "inf"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-f64::NAN, "nan"),
        ] {
            assert_eq!(float_to_string(value), expected, "value `{value:?}`");
        }
    }

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
