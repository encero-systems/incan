//! Private decoded-manifest transport. Its self-describing values preserve serde's optional and tagged fields;
//! the public JSON schema is unchanged. Only generation-bound local caches may bypass semantic admission.

use std::collections::BTreeMap;

use serde::de::{DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess, Visitor, value};
use serde::{Deserialize, Serialize};

use super::LibraryManifest;
use super::wire::RawLibraryManifest;

/// An internal binary value preserves map keys and variant tags that the JSON model's serde attributes require.
#[derive(Serialize)]
enum RetainedValue {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Text(String),
    Sequence(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

/// A borrowed cursor over postcard's tagged values. Destination facts allocate once, without an intermediate tree.
struct FactsReader<'de> {
    remaining: &'de [u8],
}

/// One externally tagged variant with its payload still in the retained byte stream.
struct RetainedEnum<'a, 'de> {
    name: &'de str,
    reader: &'a mut FactsReader<'de>,
}

impl<'a, 'de> EnumAccess<'de> for RetainedEnum<'a, 'de> {
    type Error = value::Error;
    type Variant = &'a mut FactsReader<'de>;

    /// Decode the variant identity independently from its payload's destination shape.
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error> {
        Ok((seed.deserialize(self.name.into_deserializer())?, self.reader))
    }
}

impl<'de> VariantAccess<'de> for &mut FactsReader<'de> {
    type Error = value::Error;

    /// A unit variant carries only a null payload in the retained map representation.
    fn unit_variant(self) -> Result<(), Self::Error> {
        <()>::deserialize(self)
    }

    /// A newtype variant moves its complete payload into the destination.
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Self::Error> {
        seed.deserialize(self)
    }

    /// Tuple variants retain their ordinary sequence representation.
    fn tuple_variant<V: Visitor<'de>>(self, _length: usize, visitor: V) -> Result<V::Value, Self::Error> {
        serde::Deserializer::deserialize_any(self, visitor)
    }

    /// Struct variants retain named fields, including omitted optional fields.
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        serde::Deserializer::deserialize_any(self, visitor)
    }
}

/// Encode admitted facts without applying a positional encoding to JSON structs with omitted fields.
pub(super) fn encode(manifest: &LibraryManifest) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(RawLibraryManifest::from_semantic(manifest)).map_err(|error| error.to_string())?;
    let value = RetainedValue::from_json(value)?;
    postcard::to_allocvec(&value).map_err(|error| error.to_string())
}

/// Reconstitute already admitted facts. The caller must authenticate the source generation and compiler identity.
pub(super) fn decode(bytes: &[u8]) -> Result<LibraryManifest, String> {
    let mut reader = FactsReader { remaining: bytes };
    let raw = RawLibraryManifest::deserialize(&mut reader).map_err(|error| error.to_string())?;
    if !reader.remaining.is_empty() {
        return Err("trailing decoded manifest facts".into());
    }
    raw.into_semantic().map_err(|error| error.to_string())
}

impl<'de> FactsReader<'de> {
    /// Delegate primitive and length decoding to the same postcard implementation that writes this private format.
    fn take<T: Deserialize<'de>>(&mut self) -> Result<T, value::Error> {
        let (value, remaining) =
            postcard::take_from_bytes(self.remaining).map_err(<value::Error as serde::de::Error>::custom)?;
        self.remaining = remaining;
        Ok(value)
    }

    /// Bound container allocation by available bytes; every retained element consumes at least one tag byte.
    fn length(&mut self) -> Result<usize, value::Error> {
        let length = self.take()?;
        if length > self.remaining.len() {
            return Err(<value::Error as serde::de::Error>::custom(
                "oversized retained container",
            ));
        }
        Ok(length)
    }
}

impl RetainedValue {
    /// Preserve each JSON primitive's exact number domain and recursively retain named fields.
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(value),
            serde_json::Value::String(value) => Self::Text(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_u64() {
                    Self::Unsigned(value)
                } else if let Some(value) = value.as_i64() {
                    Self::Signed(value)
                } else {
                    Self::Float(value.as_f64().ok_or("unsupported retained number")?)
                }
            }
            serde_json::Value::Array(values) => {
                Self::Sequence(values.into_iter().map(Self::from_json).collect::<Result<_, _>>()?)
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .into_iter()
                    .map(|(key, value)| Ok((key, Self::from_json(value)?)))
                    .collect::<Result<_, String>>()?,
            ),
        })
    }
}

/// A sequence's declared remaining elements and the shared byte cursor.
struct FactsSequence<'a, 'de> {
    reader: &'a mut FactsReader<'de>,
    remaining: usize,
}

impl<'de> SeqAccess<'de> for FactsSequence<'_, 'de> {
    type Error = value::Error;

    /// Deserialize one destination element directly from the retained stream.
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.reader).map(Some)
    }

    /// Offer a bounded allocation hint to destination vectors.
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

impl<'de> MapAccess<'de> for FactsSequence<'_, 'de> {
    type Error = value::Error;

    /// Map keys are postcard strings rather than tagged values; borrow them for serde's field dispatch.
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let name: &str = self.reader.take()?;
        seed.deserialize(name.into_deserializer()).map(Some)
    }

    /// The value following a named field uses the ordinary retained-value decoder.
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        seed.deserialize(&mut *self.reader)
    }

    /// Offer a bounded allocation hint to destination maps.
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

impl<'de> serde::Deserializer<'de> for &mut FactsReader<'de> {
    type Error = value::Error;

    /// Dispatch the retained primitive or named container without guessing a destination type.
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.take::<u32>()? {
            0 => visitor.visit_unit(),
            1 => visitor.visit_bool(self.take()?),
            2 => visitor.visit_i64(self.take()?),
            3 => visitor.visit_u64(self.take()?),
            4 => visitor.visit_f64(self.take()?),
            5 => visitor.visit_borrowed_str(self.take()?),
            6 => {
                let remaining = self.length()?;
                visitor.visit_seq(FactsSequence {
                    reader: self,
                    remaining,
                })
            }
            7 => {
                let remaining = self.length()?;
                visitor.visit_map(FactsSequence {
                    reader: self,
                    remaining,
                })
            }
            _ => Err(<value::Error as serde::de::Error>::custom("unknown retained value tag")),
        }
    }

    /// JSON's null denotes an absent optional field; other values retain the destination's optional boundary.
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        if self.remaining.first() == Some(&0) {
            self.take::<u32>()?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    /// Newtype wrappers retain the same representation as their inner value.
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    /// Preserve externally tagged enum maps and unit-variant strings; internally tagged enums use ordinary maps.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.take::<u32>()? {
            5 => {
                let name: &str = self.take()?;
                visitor.visit_enum(name.into_deserializer())
            }
            7 => {
                if self.length()? != 1 {
                    return Err(<value::Error as serde::de::Error>::custom(
                        "retained enum must have one variant",
                    ));
                }
                let name = self.take()?;
                visitor.visit_enum(RetainedEnum { name, reader: self })
            }
            _ => Err(<value::Error as serde::de::Error>::custom("invalid retained enum")),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}
