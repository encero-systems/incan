//! Monomorphic exact-carrier boundaries preserving numeric widths and the language's display contracts.
//!
//! `int_*` and `float_*` are the language casts: two's-complement truncation between integers, saturation from
//! floats (NaN to zero), as the reference runtime's compiled casts are. Integer powers and negation wrap, as the
//! reference runtime's release build does.

use incan_lang::numeric_strings::{parse_float_string, parse_int_string};
use incan_std_primitives::errors::raise_value_error;
use incan_std_primitives::num::{
    py_floor_div_f32 as native_floor_div_f32, py_floor_div_f64 as native_floor_div_f64, py_floor_div_signed,
    py_floor_div_unsigned, py_mod_f32 as native_mod_f32, py_mod_f64 as native_mod_f64, py_mod_signed, py_mod_unsigned,
};
use incan_std_primitives::strings::float_to_string;

use crate::integer_helpers::integer_power;

macro_rules! integer_carrier {
    ($carrier:ty, $format:ident, $int:ident, $float:ident, $modulo:ident, $floor:ident, $power:ident, $py_mod:ident, $py_floor:ident) => {
        /// Format the exact carrier with its native integer spelling.
        pub fn $format(value: $carrier) -> String {
            value.to_string()
        }

        /// Apply the language integer cast to the exact carrier.
        pub fn $int(value: $carrier) -> i64 {
            value as i64
        }

        /// Apply the language float cast to the exact carrier.
        pub fn $float(value: $carrier) -> f64 {
            value as f64
        }

        /// Python's flooring modulo at the exact width, refusing a zero divisor.
        pub fn $modulo(left: $carrier, right: $carrier) -> $carrier {
            $py_mod(left, right)
        }

        /// Python's floor division at the exact width, refusing a zero divisor.
        pub fn $floor(left: $carrier, right: $carrier) -> $carrier {
            $py_floor(left, right)
        }

        /// Integer power at the exact width, keeping the low 32 exponent bits and wrapping multiplication.
        pub fn $power(left: $carrier, right: i64) -> $carrier {
            integer_power(left, right, 1)
        }
    };
}

macro_rules! negation {
    ($($carrier:ty => $negate:ident),*) => {
        $(
            /// Negate with the carrier's release overflow policy: the minimum wraps to itself.
            pub fn $negate(value: $carrier) -> $carrier {
                value.wrapping_neg()
            }
        )*
    };
}

integer_carrier!(
    i8,
    format_i8,
    int_i8,
    float_i8,
    py_mod_i8,
    py_floor_div_i8,
    power_i8,
    py_mod_signed,
    py_floor_div_signed
);
integer_carrier!(
    i16,
    format_i16,
    int_i16,
    float_i16,
    py_mod_i16,
    py_floor_div_i16,
    power_i16,
    py_mod_signed,
    py_floor_div_signed
);
integer_carrier!(
    i32,
    format_i32,
    int_i32,
    float_i32,
    py_mod_i32,
    py_floor_div_i32,
    power_i32,
    py_mod_signed,
    py_floor_div_signed
);
integer_carrier!(
    i128,
    format_i128,
    int_i128,
    float_i128,
    py_mod_i128,
    py_floor_div_i128,
    power_i128,
    py_mod_signed,
    py_floor_div_signed
);
integer_carrier!(
    isize,
    format_isize,
    int_isize,
    float_isize,
    py_mod_isize,
    py_floor_div_isize,
    power_isize,
    py_mod_signed,
    py_floor_div_signed
);
integer_carrier!(
    u8,
    format_u8,
    int_u8,
    float_u8,
    py_mod_u8,
    py_floor_div_u8,
    power_u8,
    py_mod_unsigned,
    py_floor_div_unsigned
);
integer_carrier!(
    u16,
    format_u16,
    int_u16,
    float_u16,
    py_mod_u16,
    py_floor_div_u16,
    power_u16,
    py_mod_unsigned,
    py_floor_div_unsigned
);
integer_carrier!(
    u32,
    format_u32,
    int_u32,
    float_u32,
    py_mod_u32,
    py_floor_div_u32,
    power_u32,
    py_mod_unsigned,
    py_floor_div_unsigned
);
integer_carrier!(
    u64,
    format_u64,
    int_u64,
    float_u64,
    py_mod_u64,
    py_floor_div_u64,
    power_u64,
    py_mod_unsigned,
    py_floor_div_unsigned
);
integer_carrier!(
    u128,
    format_u128,
    int_u128,
    float_u128,
    py_mod_u128,
    py_floor_div_u128,
    power_u128,
    py_mod_unsigned,
    py_floor_div_unsigned
);
integer_carrier!(
    usize,
    format_usize,
    int_usize,
    float_usize,
    py_mod_usize,
    py_floor_div_usize,
    power_usize,
    py_mod_unsigned,
    py_floor_div_unsigned
);

negation!(i8 => negate_i8, i16 => negate_i16, i32 => negate_i32, i128 => negate_i128, isize => negate_isize);

/// Format f32 with its native display, appending `.0` to a visibly integral value.
pub fn format_f32(value: f32) -> String {
    let text = value.to_string();
    if text.contains('.') || text.contains('e') || text.contains('E') {
        return text;
    }
    text + ".0"
}

/// Apply the language integer cast to f32: saturating, NaN to zero.
pub fn int_f32(value: f32) -> i64 {
    value as i64
}

/// Widen f32 exactly.
pub fn float_f32(value: f32) -> f64 {
    f64::from(value)
}

/// Format f64 with the language float spelling.
pub fn format_f64(value: f64) -> String {
    float_to_string(value)
}

/// Apply the language integer cast to f64: saturating, NaN to zero.
pub fn int_f64(value: f64) -> i64 {
    value as i64
}

/// The f64 float cast is the identity.
pub fn float_f64(value: f64) -> f64 {
    value
}

/// Python's flooring float modulo, refusing a zero divisor.
pub fn py_mod_f32(left: f32, right: f32) -> f32 {
    native_mod_f32(left, right)
}

/// Python's float floor division, refusing a zero divisor.
pub fn py_floor_div_f32(left: f32, right: f32) -> f32 {
    native_floor_div_f32(left, right)
}

/// Python's flooring float modulo, refusing a zero divisor.
pub fn py_mod_f64(left: f64, right: f64) -> f64 {
    native_mod_f64(left, right)
}

/// Python's float floor division, refusing a zero divisor.
pub fn py_floor_div_f64(left: f64, right: f64) -> f64 {
    native_floor_div_f64(left, right)
}

/// Whether an f64 is finite.
pub fn is_finite_f64(value: f64) -> bool {
    value.is_finite()
}

/// Whether an f32 is finite.
pub fn is_finite_f32(value: f32) -> bool {
    value.is_finite()
}

/// Whether an f64 is NaN.
pub fn is_nan_f64(value: f64) -> bool {
    value.is_nan()
}

/// Whether an f32 is NaN.
pub fn is_nan_f32(value: f32) -> bool {
    value.is_nan()
}

/// Parse an integer through the kernel's single numeric-string policy, or raise the language's ValueError.
pub fn parse_int(value: &String) -> i64 {
    match parse_int_string(value) {
        Some(parsed) => parsed,
        None => raise_value_error(&format!("cannot convert '{value}' to int")),
    }
}

/// Parse a float through the kernel's single numeric-string policy, or raise the language's ValueError.
pub fn parse_float(value: &String) -> f64 {
    match parse_float_string(value) {
        Some(parsed) => parsed,
        None => raise_value_error(&format!("cannot convert '{value}' to float")),
    }
}
