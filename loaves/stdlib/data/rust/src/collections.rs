//! Runtime support for `std.collections.OrdinalMap`.
//!
//! The user-facing map is authored in the component's own `collections.incn`; generated `std.collections` modules
//! splice in [`__incan_ordinal_map_string_fast_impls!`](crate::__incan_ordinal_map_string_fast_impls) for borrowed
//! string-key lookup and call the key-encoding helpers below. Both hash with xxh3, which is why they live in this
//! facet rather than in `incan_std_core`: a program that never imports `std.collections` links no hasher.

pub(crate) mod ordinal_map;

/// Compiler-only ordinal-key helpers used by generated Rust.
#[doc(hidden)]
pub mod __private {
    /// Hash canonical `OrdinalKey` bytes into the signed positive hash domain used by `std.collections.OrdinalMap`.
    #[inline]
    #[must_use]
    pub fn ordinal_key_hash_bytes(data: &[u8]) -> i64 {
        (::xxhash_rust::xxh3::xxh3_64(data) % 9_223_372_036_854_775_807u64) as i64
    }

    /// Decode an exact-width little-endian `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_exact_bytes<const N: usize>(data: Vec<u8>, encoding: &str) -> Result<[u8; N], String> {
        if data.len() != N {
            return Err(format!("{encoding} OrdinalMap key bytes must be {N} bytes"));
        }
        <[u8; N]>::try_from(data.as_slice())
            .map_err(|_| format!("{encoding} OrdinalMap key bytes could not be decoded"))
    }

    /// Decode a UTF-8 string `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_string_from_bytes(data: Vec<u8>) -> Result<String, String> {
        String::from_utf8(data).map_err(|err| err.to_string())
    }

    /// Encode a decimal `OrdinalKey` into its canonical coefficient/scale payload.
    #[inline]
    #[must_use]
    pub fn ordinal_key_decimal_bytes(value: &incan_std_core::num::Decimal128) -> [u8; 17] {
        let mut out = [0u8; 17];
        out[0..16].copy_from_slice(&value.coefficient().to_le_bytes());
        out[16] = value.scale();
        out
    }
}
