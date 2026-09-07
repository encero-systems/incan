//! Pure value-domain rules shared by numeric typechecking, lowering, execution, and runtime display.
//!
//! The language registry owns numeric names and metadata. This module owns the deterministic policy that applies
//! to concrete numeric values so compiler stages and runtime carriers do not grow independent bounds, widening, or
//! fixed-scale decimal implementations.

use crate::lang::types::numerics::{self, NumericFamily, NumericTypeId};

/// The inclusive value domain of one sized integer identity.
///
/// Pointer-sized identities use the bounds of the current host platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerBounds {
    /// A signed integer whose values lie between `minimum` and `maximum`, inclusive.
    Signed { minimum: i128, maximum: i128 },
    /// An unsigned integer whose values lie between zero and `maximum`, inclusive.
    Unsigned { maximum: u128 },
}

/// Return the canonical host-platform bounds for one integer numeric identity.
#[must_use]
pub const fn integer_bounds(kind: NumericTypeId) -> Option<IntegerBounds> {
    match kind {
        NumericTypeId::I8 => Some(IntegerBounds::Signed {
            minimum: i8::MIN as i128,
            maximum: i8::MAX as i128,
        }),
        NumericTypeId::I16 => Some(IntegerBounds::Signed {
            minimum: i16::MIN as i128,
            maximum: i16::MAX as i128,
        }),
        NumericTypeId::I32 => Some(IntegerBounds::Signed {
            minimum: i32::MIN as i128,
            maximum: i32::MAX as i128,
        }),
        NumericTypeId::I64 => Some(IntegerBounds::Signed {
            minimum: i64::MIN as i128,
            maximum: i64::MAX as i128,
        }),
        NumericTypeId::I128 => Some(IntegerBounds::Signed {
            minimum: i128::MIN,
            maximum: i128::MAX,
        }),
        NumericTypeId::ISize => Some(IntegerBounds::Signed {
            minimum: isize::MIN as i128,
            maximum: isize::MAX as i128,
        }),
        NumericTypeId::U8 => Some(IntegerBounds::Unsigned {
            maximum: u8::MAX as u128,
        }),
        NumericTypeId::U16 => Some(IntegerBounds::Unsigned {
            maximum: u16::MAX as u128,
        }),
        NumericTypeId::U32 => Some(IntegerBounds::Unsigned {
            maximum: u32::MAX as u128,
        }),
        NumericTypeId::U64 => Some(IntegerBounds::Unsigned {
            maximum: u64::MAX as u128,
        }),
        NumericTypeId::U128 => Some(IntegerBounds::Unsigned { maximum: u128::MAX }),
        NumericTypeId::USize => Some(IntegerBounds::Unsigned {
            maximum: usize::MAX as u128,
        }),
        NumericTypeId::F32 | NumericTypeId::F64 | NumericTypeId::Bool => None,
    }
}

/// Return whether one canonical numeric type can widen to another without value loss.
#[must_use]
pub fn numeric_type_losslessly_widens_to(actual: NumericTypeId, expected: NumericTypeId) -> bool {
    if actual == expected {
        return true;
    }
    let actual_info = numerics::info_for(actual);
    let expected_info = numerics::info_for(expected);
    match (actual_info.family, expected_info.family) {
        (NumericFamily::SignedInteger, NumericFamily::SignedInteger)
        | (NumericFamily::UnsignedInteger, NumericFamily::UnsignedInteger)
        | (NumericFamily::BinaryFloat, NumericFamily::BinaryFloat) => {
            width_at_least(expected_info.bit_width, actual_info.bit_width)
        }
        (NumericFamily::UnsignedInteger, NumericFamily::SignedInteger) => {
            match (actual_info.bit_width, expected_info.bit_width) {
                (Some(actual_bits), Some(expected_bits)) => expected_bits > actual_bits,
                _ => false,
            }
        }
        _ => false,
    }
}

/// Compare fixed bit widths for widening decisions; platform-width types only widen to themselves.
const fn width_at_least(expected: Option<u16>, actual: Option<u16>) -> bool {
    match (expected, actual) {
        (Some(expected), Some(actual)) => expected >= actual,
        _ => false,
    }
}

/// A parsed plain fixed-scale decimal literal body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedDecimal {
    /// The literal digits with the decimal point removed, including the sign.
    pub coefficient: i128,
    /// The number of fractional digits written in the source literal.
    pub literal_scale: u8,
}

/// Parse a plain decimal literal body after any `d` suffix has been removed.
///
/// Exponent notation is deliberately rejected because fixed-scale decimal literals retain their written scale.
#[must_use]
pub fn parse_decimal_literal_body(body: &str) -> Option<ParsedDecimal> {
    if body.is_empty() || body.contains('e') || body.contains('E') {
        return None;
    }
    let (integer, fractional) = body.split_once('.').unwrap_or((body, ""));
    if integer.is_empty() && fractional.is_empty() {
        return None;
    }
    let literal_scale = u8::try_from(fractional.len()).ok()?;
    let mut coefficient = String::with_capacity(integer.len() + fractional.len());
    coefficient.push_str(if integer.is_empty() { "0" } else { integer });
    coefficient.push_str(fractional);
    coefficient.parse::<i128>().ok().map(|coefficient| ParsedDecimal {
        coefficient,
        literal_scale,
    })
}

/// Validate a decimal coefficient and written scale against a checked precision and scale.
#[must_use]
pub fn decimal_value_fits(precision: u8, scale: u8, coefficient: i128, literal_scale: u8) -> bool {
    if precision == 0 || precision > 38 || scale > precision || literal_scale > scale {
        return false;
    }
    let digits = coefficient.unsigned_abs().to_string().len();
    let integer_digits = digits.saturating_sub(usize::from(literal_scale)).max(1);
    let total_digits = integer_digits + usize::from(literal_scale);
    integer_digits <= usize::from(precision - scale) && total_digits <= usize::from(precision)
}

/// Render a decimal coefficient with exactly its retained written scale.
#[must_use]
pub fn format_decimal_value(coefficient: i128, literal_scale: u8) -> String {
    if literal_scale == 0 {
        return coefficient.to_string();
    }
    let negative = coefficient < 0;
    let digits = coefficient.unsigned_abs().to_string();
    let literal_scale = usize::from(literal_scale);
    let mut rendered = String::new();
    if negative {
        rendered.push('-');
    }
    if digits.len() <= literal_scale {
        rendered.push_str("0.");
        rendered.extend(std::iter::repeat_n('0', literal_scale - digits.len()));
        rendered.push_str(&digits);
    } else {
        let split = digits.len() - literal_scale;
        rendered.push_str(&digits[..split]);
        rendered.push('.');
        rendered.push_str(&digits[split..]);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integer-bound lookup distinguishes signed, unsigned, and non-integer registry ids.
    #[test]
    fn integer_bounds_cover_every_integer_family() {
        assert_eq!(
            integer_bounds(NumericTypeId::I128),
            Some(IntegerBounds::Signed {
                minimum: i128::MIN,
                maximum: i128::MAX,
            })
        );
        assert_eq!(
            integer_bounds(NumericTypeId::U128),
            Some(IntegerBounds::Unsigned { maximum: u128::MAX })
        );
        assert_eq!(integer_bounds(NumericTypeId::F64), None);
    }

    /// Widening admits only value domains wholly representable by the target.
    #[test]
    fn widening_requires_a_provably_lossless_domain_inclusion() {
        assert!(numeric_type_losslessly_widens_to(NumericTypeId::U8, NumericTypeId::I16));
        assert!(numeric_type_losslessly_widens_to(
            NumericTypeId::F32,
            NumericTypeId::F64
        ));
        assert!(!numeric_type_losslessly_widens_to(
            NumericTypeId::U16,
            NumericTypeId::I16
        ));
        assert!(!numeric_type_losslessly_widens_to(
            NumericTypeId::I64,
            NumericTypeId::F64
        ));
    }

    /// Decimal parsing, validation, and rendering retain the same written scale.
    #[test]
    fn decimal_parse_validation_and_rendering_share_written_scale() -> Result<(), &'static str> {
        let Some(parsed) = parse_decimal_literal_body("19.90") else {
            return Err("fixture is a plain decimal literal");
        };
        assert_eq!(parsed.coefficient, 1990);
        assert_eq!(parsed.literal_scale, 2);
        assert!(decimal_value_fits(6, 2, parsed.coefficient, parsed.literal_scale));
        assert_eq!(format_decimal_value(parsed.coefficient, parsed.literal_scale), "19.90");
        assert!(!decimal_value_fits(5, 2, 12345, 0));
        Ok(())
    }
}
