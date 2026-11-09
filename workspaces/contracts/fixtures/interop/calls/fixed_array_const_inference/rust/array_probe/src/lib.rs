//! Exact fixed-array boundaries used to exercise callable const-generic inference.

/// Return an array unchanged while retaining both its element and length binders.
pub fn identity_array<T, const N: usize>(values: [T; N]) -> [T; N] {
    values
}

/// Sum every byte in an exact nested fixed array.
pub fn sum_nested(values: [[u8; 8]; 4]) -> u64 {
    values.into_iter().flatten().map(u64::from).sum()
}

/// Sum the bytes of an exact 32-byte identity value.
pub fn sum_32(values: [u8; 32]) -> u64 {
    values.into_iter().map(u64::from).sum()
}

/// Produce one exact nested value for the fixed-array inference boundary.
pub fn nested_twos() -> [[u8; 8]; 4] {
    [[2; 8]; 4]
}
