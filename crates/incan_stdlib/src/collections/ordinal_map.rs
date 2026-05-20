//! Rust support for generated `std.collections.OrdinalMap` modules.
//!
//! The user-facing type and public API are authored in `stdlib/collections.incn`. This module only supplies the
//! crate-local Rust fragment that generated `std.collections` modules splice in when they need borrowed string-key
//! lookup without exposing borrow syntax in Incan source.

/// Add private-field-aware string lookup helpers to a generated `std.collections.OrdinalMap[str]`.
///
/// The macro must be expanded inside the generated `std.collections` module because the map's compact storage fields
/// are intentionally private to that module. Keeping this body in `incan_stdlib` avoids embedding the implementation
/// details in backend emission while still letting codegen route concrete `OrdinalMap[str]` calls to borrowed probes.
#[doc(hidden)]
#[macro_export]
macro_rules! __incan_ordinal_map_string_fast_impls {
    () => {
        impl OrdinalMap<String> {
            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_contains_str(&self, key: &str) -> bool {
                self.__incan_ordinal_find_str(key, true) >= 0
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_getitem_str(&self, key: &str) -> i64 {
                match self.__incan_ordinal_require_str(key) {
                    Ok(value) => value,
                    Err(_) => _missing_ordinal(),
                }
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_get_str(&self, key: &str) -> Option<i64> {
                let index = self.__incan_ordinal_find_str(key, true);
                if index < 0 {
                    None
                } else {
                    Some(self.__incan_ordinal_at_fast(index))
                }
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_require_str(&self, key: &str) -> Result<i64, OrdinalMapError> {
                match self.__incan_ordinal_get_str(key) {
                    Some(value) => Ok(value),
                    None => Err(_ordinal_map_error(
                        OrdinalMapErrorKind::MissingKey.clone(),
                        "OrdinalMap key is not present".to_string(),
                        -1i64,
                    )),
                }
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_get_unchecked_str(&self, key: &str) -> i64 {
                let index = self.__incan_ordinal_find_str(key, false);
                if index < 0 {
                    _missing_ordinal()
                } else {
                    self.__incan_ordinal_at_fast(index)
                }
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_get_many_str(&self, keys: &[String]) -> Vec<Option<i64>> {
                let mut out = Vec::with_capacity(keys.len());
                for key in keys {
                    out.push(self.__incan_ordinal_get_str(key.as_str()));
                }
                out
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_require_many_str(&self, keys: &[String]) -> Result<Vec<i64>, OrdinalMapError> {
                let mut out = Vec::with_capacity(keys.len());
                for (index, key) in keys.iter().enumerate() {
                    match self.__incan_ordinal_get_str(key.as_str()) {
                        Some(value) => out.push(value),
                        None => {
                            return Err(_ordinal_map_error(
                                OrdinalMapErrorKind::MissingKey.clone(),
                                "OrdinalMap key is not present".to_string(),
                                index as i64,
                            ));
                        }
                    }
                }
                Ok(out)
            }

            #[doc(hidden)]
            #[inline]
            pub fn __incan_ordinal_get_many_unchecked_str(&self, keys: &[String]) -> Vec<i64> {
                let mut out = Vec::with_capacity(keys.len());
                for key in keys {
                    out.push(self.__incan_ordinal_get_unchecked_str(key.as_str()));
                }
                out
            }

            #[inline]
            fn __incan_ordinal_find_str(&self, key: &str, verify_exact: bool) -> i64 {
                if self.slot_count_value == 0 {
                    return -1i64;
                }

                let key_bytes = key.as_bytes();
                let hash = $crate::collections::__private::ordinal_key_hash_bytes(key_bytes);
                let mut slot = hash % self.slot_count_value;
                let mut probes = 0i64;

                while probes < self.slot_count_value {
                    let encoded = self.__incan_ordinal_slot_at_fast(slot);
                    if encoded == 0 {
                        return -1i64;
                    }

                    let record_index = encoded - 1i64;
                    if self.__incan_ordinal_hash_at_fast(record_index) == hash
                        && (!verify_exact || self.__incan_ordinal_key_bytes_equal_str(record_index, key_bytes))
                    {
                        return record_index;
                    }

                    slot = (slot + 1i64) % self.slot_count_value;
                    probes += 1i64;
                }

                -1i64
            }

            #[inline]
            fn __incan_ordinal_at_fast(&self, record_index: i64) -> i64 {
                if record_index < 0 {
                    return _missing_ordinal();
                }
                let record_index = record_index as usize;
                if let Some(value) = self.ordinal_values.get(record_index) {
                    *value
                } else {
                    self.__incan_ordinal_read_compact_int_fast(
                        self.ordinals.as_slice(),
                        record_index as i64 * self.ordinal_width,
                        self.ordinal_width,
                    )
                }
            }

            #[inline]
            fn __incan_ordinal_hash_at_fast(&self, record_index: i64) -> i64 {
                if record_index < 0 {
                    return -1i64;
                }
                self.hash_values
                    .get(record_index as usize)
                    .copied()
                    .unwrap_or(-1i64)
            }

            #[inline]
            fn __incan_ordinal_slot_at_fast(&self, slot_index: i64) -> i64 {
                if slot_index < 0 {
                    return 0i64;
                }
                let slot_index_usize = slot_index as usize;
                if let Some(value) = self.slot_values.get(slot_index_usize) {
                    *value
                } else {
                    self.__incan_ordinal_read_compact_int_fast(
                        self.slots.as_slice(),
                        slot_index * self.slot_width,
                        self.slot_width,
                    )
                }
            }

            #[inline]
            fn __incan_ordinal_key_bytes_equal_str(&self, record_index: i64, key_bytes: &[u8]) -> bool {
                if record_index < 0 {
                    return false;
                }
                let record_index_usize = record_index as usize;
                if let Some(stored) = self.key_byte_values.get(record_index_usize) {
                    return stored.as_slice() == key_bytes;
                }

                let start = self.__incan_ordinal_read_compact_int_fast(
                    self.key_offsets.as_slice(),
                    record_index * self.key_offset_width,
                    self.key_offset_width,
                );
                let end = self.__incan_ordinal_read_compact_int_fast(
                    self.key_offsets.as_slice(),
                    (record_index + 1i64) * self.key_offset_width,
                    self.key_offset_width,
                );
                if start < 0 || end < start {
                    return false;
                }
                match self.key_records.get(start as usize..end as usize) {
                    Some(stored) => stored == key_bytes,
                    None => false,
                }
            }

            #[inline]
            fn __incan_ordinal_read_compact_int_fast(&self, data: &[u8], offset: i64, width: i64) -> i64 {
                if offset < 0 {
                    return 0i64;
                }
                let offset = offset as usize;
                let byte = |index: usize| data.get(offset + index).copied().unwrap_or(0) as i64;
                let mut value = byte(0);
                if width >= 2 {
                    value += byte(1) * 256i64;
                }
                if width >= 4 {
                    value += byte(2) * 65_536i64;
                    value += byte(3) * 16_777_216i64;
                }
                if width >= 8 {
                    value += byte(4) * 4_294_967_296i64;
                    value += byte(5) * 1_099_511_627_776i64;
                    value += byte(6) * 281_474_976_710_656i64;
                    value += byte(7) * 72_057_594_037_927_936i64;
                }
                value
            }
        }
    };
}
