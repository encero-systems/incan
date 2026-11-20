//! Shared integer power semantics and the explicit u8 overflow policies at the native runtime boundary.

/// The exponent bits an integer power honors: only the low 32 bits of the signed exponent.
const MAXIMUM_POWER_EXPONENT: i64 = 4_294_967_295;

/// Multiplication with the reference runtime's release policy: no overflow check, two's-complement wrapping.
pub(crate) trait WrappingFactor: Copy {
    /// Multiply, wrapping at the carrier's width.
    fn wrapping_factor(self, other: Self) -> Self;
}

macro_rules! wrapping_factor {
    ($($carrier:ty),*) => {
        $(impl WrappingFactor for $carrier {
            fn wrapping_factor(self, other: Self) -> Self {
                self.wrapping_mul(other)
            }
        })*
    };
}

wrapping_factor!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

/// Raise `left` to `right`: keep the low 32 exponent bits, accumulate odd factors while squaring, and multiply the
/// last factor without an unused final square, so the multiplication sequence (and its wrapping) is the original one.
pub(crate) fn integer_power<T: WrappingFactor>(left: T, right: i64, identity: T) -> T {
    let mut exponent = right & MAXIMUM_POWER_EXPONENT;
    if exponent == 0 {
        return identity;
    }
    let mut base = left;
    let mut accumulator = identity;
    while exponent > 1 {
        if exponent & 1 == 1 {
            accumulator = accumulator.wrapping_factor(base);
        }
        exponent /= 2;
        base = base.wrapping_factor(base);
    }
    accumulator.wrapping_factor(base)
}

/// Native u8 checked addition.
pub fn checked_add_u8(left: u8, right: u8) -> Option<u8> {
    left.checked_add(right)
}

/// Native u8 checked subtraction.
pub fn checked_sub_u8(left: u8, right: u8) -> Option<u8> {
    left.checked_sub(right)
}

/// Native u8 checked multiplication.
pub fn checked_mul_u8(left: u8, right: u8) -> Option<u8> {
    left.checked_mul(right)
}

/// Native u8 checked power.
pub fn checked_pow_u8(left: u8, right: u32) -> Option<u8> {
    left.checked_pow(right)
}

/// Native u8 wrapping addition.
pub fn wrapping_add_u8(left: u8, right: u8) -> u8 {
    left.wrapping_add(right)
}

/// Native u8 wrapping subtraction.
pub fn wrapping_sub_u8(left: u8, right: u8) -> u8 {
    left.wrapping_sub(right)
}

/// Native u8 wrapping multiplication.
pub fn wrapping_mul_u8(left: u8, right: u8) -> u8 {
    left.wrapping_mul(right)
}

/// Native u8 wrapping power.
pub fn wrapping_pow_u8(left: u8, right: u32) -> u8 {
    left.wrapping_pow(right)
}

/// Native u8 saturating addition.
pub fn saturating_add_u8(left: u8, right: u8) -> u8 {
    left.saturating_add(right)
}

/// Native u8 saturating subtraction.
pub fn saturating_sub_u8(left: u8, right: u8) -> u8 {
    left.saturating_sub(right)
}

/// Native u8 saturating multiplication.
pub fn saturating_mul_u8(left: u8, right: u8) -> u8 {
    left.saturating_mul(right)
}

/// Native u8 saturating power.
pub fn saturating_pow_u8(left: u8, right: u32) -> u8 {
    left.saturating_pow(right)
}
