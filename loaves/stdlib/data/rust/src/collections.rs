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

    /// Return the stable encoding identifier for `str` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_str() -> String {
        "str:utf8".to_string()
    }

    /// Return the stable encoding identifier for `bytes` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_bytes() -> String {
        "bytes:raw".to_string()
    }

    /// Return the stable encoding identifier for `bool` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_bool() -> String {
        "bool:u8".to_string()
    }

    /// Return the stable encoding identifier for `Decimal128` ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_decimal() -> String {
        "decimal128:coefficient-i128-le:scale-u8".to_string()
    }

    /// Return the stable encoding identifier for signed integer ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_int(bits: u16) -> String {
        format!("int:i{bits}:le")
    }

    /// Return the stable encoding identifier for unsigned integer ordinal keys.
    #[inline]
    #[must_use]
    pub fn ordinal_key_encoding_uint(bits: u16) -> String {
        format!("uint:u{bits}:le")
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

    /// Decode a boolean `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_bool_from_bytes(data: Vec<u8>) -> Result<bool, String> {
        if data.len() != 1 {
            return Err(format!(
                "{} OrdinalMap key bytes must be 1 byte",
                ordinal_key_encoding_bool()
            ));
        }
        match data[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!(
                "{} OrdinalMap key byte must be 0 or 1",
                ordinal_key_encoding_bool()
            )),
        }
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

    /// Decode a decimal `OrdinalKey` payload.
    #[inline]
    pub fn ordinal_key_decimal_from_bytes(data: Vec<u8>) -> Result<incan_std_core::num::Decimal128, String> {
        let encoding = ordinal_key_encoding_decimal();
        if data.len() != 17 {
            return Err(format!("{encoding} OrdinalMap key bytes must be 17 bytes"));
        }
        let coefficient_bytes = <[u8; 16]>::try_from(&data[0..16])
            .map_err(|_| format!("{encoding} OrdinalMap coefficient bytes could not be decoded"))?;
        let scale = data[16];
        if scale > 38 {
            return Err(format!("{encoding} OrdinalMap scale byte must be <= 38"));
        }
        Ok(incan_std_core::num::Decimal128::new(
            i128::from_le_bytes(coefficient_bytes),
            scale,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::__private;

    #[test]
    fn ordinal_key_encoding_helpers_are_stable() {
        assert_eq!(__private::ordinal_key_encoding_str(), "str:utf8");
        assert_eq!(__private::ordinal_key_encoding_bytes(), "bytes:raw");
        assert_eq!(__private::ordinal_key_encoding_bool(), "bool:u8");
        assert_eq!(
            __private::ordinal_key_encoding_decimal(),
            "decimal128:coefficient-i128-le:scale-u8"
        );
        assert_eq!(__private::ordinal_key_encoding_int(32), "int:i32:le");
        assert_eq!(__private::ordinal_key_encoding_uint(64), "uint:u64:le");
    }
}
