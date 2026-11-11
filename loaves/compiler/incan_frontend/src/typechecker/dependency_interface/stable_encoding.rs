//! Canonical structural encoding for dependency-interface receipts.
//!
//! The receipt is an equality oracle, not a persistence format. It therefore follows each Rust value's Serde shape,
//! preserves sequence and struct-field order, and sorts map entries by their recursively encoded keys and values.
//! Type tags and length prefixes keep unlike shapes distinct without formatting or reparsing text.

use std::fmt::{self, Display};

use serde::Serialize;
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple, SerializeTupleStruct,
    SerializeTupleVariant,
};

/// Encode one semantic value without relying on container iteration order or `Debug` text.
pub(super) fn stable_encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut encoder = StableEncoder::default();
    if value.serialize(&mut encoder).is_err() {
        return vec![0xff];
    }
    encoder.output
}

/// Serde error required by the serializer contract; derived semantic values never construct it.
#[derive(Debug)]
pub(super) struct StableEncodingError;

impl Display for StableEncodingError {
    /// Describe the only possible failure class without embedding a value or path.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("dependency-interface value refused structural encoding")
    }
}

impl std::error::Error for StableEncodingError {}

impl ser::Error for StableEncodingError {
    /// Convert a custom serialization refusal into the serializer's opaque error marker.
    fn custom<T: Display>(_message: T) -> Self {
        Self
    }
}

/// Append-only canonical byte encoder used through Serde's structural visitor contract.
#[derive(Default)]
struct StableEncoder {
    output: Vec<u8>,
}

impl StableEncoder {
    /// Append an unsigned integer in platform-independent variable-width form.
    fn unsigned(&mut self, mut value: u128) {
        loop {
            let Ok(byte) = u8::try_from(value & 0x7f) else {
                return;
            };
            value >>= 7;
            if value == 0 {
                self.output.push(byte);
                return;
            }
            self.output.push(byte | 0x80);
        }
    }

    /// Append a length-delimited byte string.
    fn bytes(&mut self, bytes: &[u8]) {
        self.length(bytes.len());
        self.output.extend_from_slice(bytes);
    }

    /// Append a collection length after widening it without platform-dependent casts.
    fn length(&mut self, value: usize) {
        let Ok(value) = u64::try_from(value) else {
            self.output.push(0xff);
            return;
        };
        self.unsigned(u128::from(value));
    }

    /// Append a string as length-delimited UTF-8.
    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// Append one child encoding with an unambiguous boundary.
    fn child(&mut self, value: &[u8]) {
        self.bytes(value);
    }
}

/// Accumulated children for an ordered product or an order-independent map.
enum CompoundValues {
    Ordered(Vec<Vec<u8>>),
    Map {
        pending_key: Option<Vec<u8>>,
        entries: Vec<(Vec<u8>, Vec<u8>)>,
    },
}

/// One in-progress sequence, tuple, struct, or map encoding.
struct StableCompound<'a> {
    parent: &'a mut StableEncoder,
    values: CompoundValues,
}

impl StableCompound<'_> {
    /// Finish an ordered product by retaining the order Serde visited.
    fn finish_ordered(self) -> Result<(), StableEncodingError> {
        let CompoundValues::Ordered(values) = self.values else {
            return Err(StableEncodingError);
        };
        self.parent.length(values.len());
        for value in values {
            self.parent.child(&value);
        }
        Ok(())
    }

    /// Finish a map by sorting complete structural keys, then values as a deterministic tie-breaker.
    fn finish_map(self) -> Result<(), StableEncodingError> {
        let CompoundValues::Map {
            pending_key,
            mut entries,
        } = self.values
        else {
            return Err(StableEncodingError);
        };
        if pending_key.is_some() {
            return Err(StableEncodingError);
        }
        entries.sort();
        self.parent.length(entries.len());
        for (key, value) in entries {
            self.parent.child(&key);
            self.parent.child(&value);
        }
        Ok(())
    }

    /// Append one structurally encoded child to an ordered product.
    fn ordered<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), StableEncodingError> {
        let CompoundValues::Ordered(values) = &mut self.values else {
            return Err(StableEncodingError);
        };
        values.push(stable_encode(value));
        Ok(())
    }

    /// Append one named struct field while retaining declaration order.
    fn field<T: Serialize + ?Sized>(&mut self, name: &'static str, value: &T) -> Result<(), StableEncodingError> {
        let CompoundValues::Ordered(values) = &mut self.values else {
            return Err(StableEncodingError);
        };
        let mut field = StableEncoder::default();
        field.text(name);
        field.child(&stable_encode(value));
        values.push(field.output);
        Ok(())
    }
}

impl<'a> ser::Serializer for &'a mut StableEncoder {
    type Ok = ();
    type Error = StableEncodingError;
    type SerializeSeq = StableCompound<'a>;
    type SerializeTuple = StableCompound<'a>;
    type SerializeTupleStruct = StableCompound<'a>;
    type SerializeTupleVariant = StableCompound<'a>;
    type SerializeMap = StableCompound<'a>;
    type SerializeStruct = StableCompound<'a>;
    type SerializeStructVariant = StableCompound<'a>;

    /// Encode a boolean with its primitive type tag.
    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        self.output.extend_from_slice(&[0x01, u8::from(value)]);
        Ok(())
    }

    /// Encode an eight-bit signed integer.
    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x02);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a sixteen-bit signed integer.
    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x03);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a thirty-two-bit signed integer.
    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x04);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a sixty-four-bit signed integer.
    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x05);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a 128-bit signed integer.
    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x06);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode an eight-bit unsigned integer.
    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.output.extend_from_slice(&[0x07, value]);
        Ok(())
    }

    /// Encode a sixteen-bit unsigned integer.
    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x08);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a thirty-two-bit unsigned integer.
    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x09);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a sixty-four-bit unsigned integer.
    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0a);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a 128-bit unsigned integer.
    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0b);
        self.output.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Encode a thirty-two-bit float by its exact IEEE bits.
    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0c);
        self.output.extend_from_slice(&value.to_bits().to_le_bytes());
        Ok(())
    }

    /// Encode a sixty-four-bit float by its exact IEEE bits.
    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0d);
        self.output.extend_from_slice(&value.to_bits().to_le_bytes());
        Ok(())
    }

    /// Encode a Unicode scalar by its numeric value.
    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0e);
        self.output.extend_from_slice(&u32::from(value).to_le_bytes());
        Ok(())
    }

    /// Encode UTF-8 text with a length boundary.
    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x0f);
        self.text(value);
        Ok(())
    }

    /// Encode raw bytes with a length boundary.
    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x10);
        self.bytes(value);
        Ok(())
    }

    /// Encode an absent optional value.
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x11);
        Ok(())
    }

    /// Encode a present optional value and its structural child.
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x12);
        self.child(&stable_encode(value));
        Ok(())
    }

    /// Encode a unit value.
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x13);
        Ok(())
    }

    /// Encode a named unit struct.
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x14);
        self.text(name);
        Ok(())
    }

    /// Encode a named unit enum variant.
    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x15);
        self.text(name);
        self.unsigned(u128::from(variant_index));
        self.text(variant);
        Ok(())
    }

    /// Encode a newtype struct and its structural value.
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x16);
        self.text(name);
        self.child(&stable_encode(value));
        Ok(())
    }

    /// Encode a newtype enum variant and its structural value.
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.output.push(0x17);
        self.text(name);
        self.unsigned(u128::from(variant_index));
        self.text(variant);
        self.child(&stable_encode(value));
        Ok(())
    }

    /// Begin an ordered sequence.
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.output.push(0x18);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length.unwrap_or(0))),
        })
    }

    /// Begin an ordered tuple.
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.output.push(0x19);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length)),
        })
    }

    /// Begin a named ordered tuple struct.
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.output.push(0x1a);
        self.text(name);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length)),
        })
    }

    /// Begin a named ordered tuple variant.
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.output.push(0x1b);
        self.text(name);
        self.unsigned(u128::from(variant_index));
        self.text(variant);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length)),
        })
    }

    /// Begin an order-independent map.
    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.output.push(0x1c);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Map {
                pending_key: None,
                entries: Vec::with_capacity(length.unwrap_or(0)),
            },
        })
    }

    /// Begin a named ordered struct.
    fn serialize_struct(self, name: &'static str, length: usize) -> Result<Self::SerializeStruct, Self::Error> {
        self.output.push(0x1d);
        self.text(name);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length)),
        })
    }

    /// Begin a named ordered struct variant.
    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.output.push(0x1e);
        self.text(name);
        self.unsigned(u128::from(variant_index));
        self.text(variant);
        Ok(StableCompound {
            parent: self,
            values: CompoundValues::Ordered(Vec::with_capacity(length)),
        })
    }

    /// Preserve the wrapped value's exact structural encoding.
    fn collect_str<T: Display + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }

    /// Report that the structural encoder is readable rather than compact-only.
    fn is_human_readable(&self) -> bool {
        true
    }
}

impl SerializeSeq for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append the next ordered sequence element.
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.ordered(value)
    }

    /// Finish the ordered sequence.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

impl SerializeTuple for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append the next ordered tuple element.
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.ordered(value)
    }

    /// Finish the ordered tuple.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

impl SerializeTupleStruct for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append the next ordered tuple-struct field.
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.ordered(value)
    }

    /// Finish the ordered tuple struct.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

impl SerializeTupleVariant for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append the next ordered tuple-variant field.
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.ordered(value)
    }

    /// Finish the ordered tuple variant.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

impl SerializeMap for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Retain a structural map key until its value arrives.
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        let CompoundValues::Map { pending_key, .. } = &mut self.values else {
            return Err(StableEncodingError);
        };
        if pending_key.is_some() {
            return Err(StableEncodingError);
        }
        *pending_key = Some(stable_encode(key));
        Ok(())
    }

    /// Pair one structural value with the pending map key.
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let CompoundValues::Map { pending_key, entries } = &mut self.values else {
            return Err(StableEncodingError);
        };
        let Some(key) = pending_key.take() else {
            return Err(StableEncodingError);
        };
        entries.push((key, stable_encode(value)));
        Ok(())
    }

    /// Finish the sorted map.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_map()
    }
}

impl SerializeStruct for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append one named struct field in declaration order.
    fn serialize_field<T: Serialize + ?Sized>(&mut self, name: &'static str, value: &T) -> Result<(), Self::Error> {
        self.field(name, value)
    }

    /// Finish the ordered struct.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

impl SerializeStructVariant for StableCompound<'_> {
    type Ok = ();
    type Error = StableEncodingError;

    /// Append one named struct-variant field in declaration order.
    fn serialize_field<T: Serialize + ?Sized>(&mut self, name: &'static str, value: &T) -> Result<(), Self::Error> {
        self.field(name, value)
    }

    /// Finish the ordered struct variant.
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.finish_ordered()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::stable_encode;

    /// Nested maps encode identically regardless of insertion order and differently when an ordered value changes.
    #[test]
    fn structural_encoding_sorts_maps_but_preserves_sequences() {
        let mut first = HashMap::new();
        first.insert("beta", vec![2_u32, 1]);
        first.insert("alpha", vec![1_u32, 2]);
        let mut second = HashMap::new();
        second.insert("alpha", vec![1_u32, 2]);
        second.insert("beta", vec![2_u32, 1]);
        let mut changed = second.clone();
        changed.insert("beta", vec![1_u32, 2]);

        assert_eq!(stable_encode(&first), stable_encode(&second));
        assert_ne!(stable_encode(&first), stable_encode(&changed));
    }
}
