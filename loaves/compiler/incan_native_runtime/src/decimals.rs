//! Exact Decimal128 construction and value comparisons.

use incan_std_primitives::num::Decimal128;

/// Preserve the checked coefficient and written fractional scale without a float conversion.
pub fn decimal_from_parts(coefficient: i128, scale: u8) -> Decimal128 {
    Decimal128::new(coefficient, scale)
}

/// Render the carrier's written scale through its canonical Display implementation.
pub fn format_decimal(value: Decimal128) -> String {
    value.to_string()
}

/// Compare numeric values independently of their written scales.
pub fn equal_decimal(left: Decimal128, right: Decimal128) -> bool {
    left == right
}

/// The carrier's numeric inequality.
pub fn unequal_decimal(left: Decimal128, right: Decimal128) -> bool {
    left != right
}

/// The carrier's numeric ordering.
pub fn less_decimal(left: Decimal128, right: Decimal128) -> bool {
    left < right
}

/// The carrier's inclusive numeric ordering.
pub fn less_equal_decimal(left: Decimal128, right: Decimal128) -> bool {
    left <= right
}

/// The carrier's reverse numeric ordering.
pub fn greater_decimal(left: Decimal128, right: Decimal128) -> bool {
    left > right
}

/// The carrier's inclusive reverse numeric ordering.
pub fn greater_equal_decimal(left: Decimal128, right: Decimal128) -> bool {
    left >= right
}
