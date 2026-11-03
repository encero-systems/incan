//! The ABI cache's string table. A crate's facts repeat a few hundred thousand distinct strings (paths, type names,
//! generic parameters) across millions of positions, so each record stores a string as its index in one table the
//! cache holds once. The encoding is postcard's, with every `str` replaced by that index; nothing else changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use serde::ser::{self, Serialize, Serializer};

/// The distinct strings of one cache, in first-use order.
#[derive(Default)]
pub(super) struct StringTable {
    ids: HashMap<String, u32>,
    strings: Vec<String>,
}

impl StringTable {
    /// The index of `text`, adding it on first use.
    fn intern(&mut self, text: &str) -> Result<u32, String> {
        if let Some(&id) = self.ids.get(text) {
            return Ok(id);
        }
        let id = u32::try_from(self.strings.len()).map_err(|_| "the ABI cache holds too many strings".to_owned())?;
        self.ids.insert(text.to_owned(), id);
        self.strings.push(text.to_owned());
        Ok(id)
    }

    /// The table as the cache stores it.
    pub(super) fn into_strings(self) -> Vec<String> {
        self.strings
    }
}

/// Encode `value` as postcard with each string replaced by its index in `table`.
pub(super) fn encode<T: Serialize + ?Sized>(value: &T, table: &RefCell<StringTable>) -> Result<Vec<u8>, String> {
    let mut serializer = postcard::Serializer {
        output: postcard::ser_flavors::AllocVec::new(),
    };
    value
        .serialize(Interning {
            inner: &mut serializer,
            table,
        })
        .map_err(|error| error.to_string())?;
    postcard::ser_flavors::Flavor::finalize(serializer.output).map_err(|error| error.to_string())
}

/// Decode one record whose strings index `strings`.
pub(super) fn decode<'de, T: de::Deserialize<'de>>(bytes: &'de [u8], strings: &[String]) -> Result<T, String> {
    let mut deserializer = postcard::Deserializer::from_bytes(bytes);
    T::deserialize(Restoring {
        inner: &mut deserializer,
        strings,
    })
    .map_err(|error| error.to_string())
}

// ---- Encoding ----

/// A serializer that forwards to `inner`, writing each string as its table index.
struct Interning<'t, S> {
    inner: S,
    table: &'t RefCell<StringTable>,
}

/// A value serialized through [`Interning`], so nested values keep interning inside compound serializers.
struct Interned<'v, 't, T: ?Sized> {
    value: &'v T,
    table: &'t RefCell<StringTable>,
}

impl<T: Serialize + ?Sized> Serialize for Interned<'_, '_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(Interning {
            inner: serializer,
            table: self.table,
        })
    }
}

/// A compound serializer whose elements, fields, keys and values intern their strings.
struct Compound<'t, C> {
    inner: C,
    table: &'t RefCell<StringTable>,
}

impl<'t, C> Compound<'t, C> {
    /// Wrap one nested value in the same table.
    fn nested<'v, T: ?Sized>(&self, value: &'v T) -> Interned<'v, 't, T> {
        Interned {
            value,
            table: self.table,
        }
    }
}

impl<'t, S: Serializer> Interning<'t, S> {
    /// Wrap one nested value in the same table.
    fn nested<'v, T: ?Sized>(&self, value: &'v T) -> Interned<'v, 't, T> {
        Interned {
            value,
            table: self.table,
        }
    }
}

/// Forward a primitive serializer method unchanged.
macro_rules! forward_primitive {
    ($($method:ident($type:ty)),* $(,)?) => {
        $(fn $method(self, value: $type) -> Result<S::Ok, S::Error> {
            self.inner.$method(value)
        })*
    };
}

impl<'t, S: Serializer> Serializer for Interning<'t, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Compound<'t, S::SerializeSeq>;
    type SerializeTuple = Compound<'t, S::SerializeTuple>;
    type SerializeTupleStruct = Compound<'t, S::SerializeTupleStruct>;
    type SerializeTupleVariant = Compound<'t, S::SerializeTupleVariant>;
    type SerializeMap = Compound<'t, S::SerializeMap>;
    type SerializeStruct = Compound<'t, S::SerializeStruct>;
    type SerializeStructVariant = Compound<'t, S::SerializeStructVariant>;

    forward_primitive!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_bytes(&[u8]),
    );

    fn serialize_str(self, value: &str) -> Result<S::Ok, S::Error> {
        let id = self
            .table
            .borrow_mut()
            .intern(value)
            .map_err(<S::Error as ser::Error>::custom)?;
        self.inner.serialize_u32(id)
    }

    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.inner.serialize_none()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        let nested = self.nested(value);
        self.inner.serialize_some(&nested)
    }

    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.inner.serialize_unit()
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        self.inner.serialize_unit_struct(name)
    }

    fn serialize_unit_variant(self, name: &'static str, index: u32, variant: &'static str) -> Result<S::Ok, S::Error> {
        self.inner.serialize_unit_variant(name, index, variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<S::Ok, S::Error> {
        let nested = self.nested(value);
        self.inner.serialize_newtype_struct(name, &nested)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        let nested = self.nested(value);
        self.inner.serialize_newtype_variant(name, index, variant, &nested)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_seq(len)?,
            table,
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_tuple(len)?,
            table,
        })
    }

    fn serialize_tuple_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeTupleStruct, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_tuple_struct(name, len)?,
            table,
        })
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_tuple_variant(name, index, variant, len)?,
            table,
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_map(len)?,
            table,
        })
    }

    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_struct(name, len)?,
            table,
        })
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        let table = self.table;
        Ok(Compound {
            inner: self.inner.serialize_struct_variant(name, index, variant, len)?,
            table,
        })
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

impl<C: ser::SerializeSeq> ser::SerializeSeq for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_element(&nested)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeTuple> ser::SerializeTuple for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_element(&nested)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeTupleStruct> ser::SerializeTupleStruct for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_field(&nested)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeTupleVariant> ser::SerializeTupleVariant for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_field(&nested)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeMap> ser::SerializeMap for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), C::Error> {
        let nested = self.nested(key);
        self.inner.serialize_key(&nested)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_value(&nested)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeStruct> ser::SerializeStruct for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_field(key, &nested)
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), C::Error> {
        self.inner.skip_field(key)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

impl<C: ser::SerializeStructVariant> ser::SerializeStructVariant for Compound<'_, C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), C::Error> {
        let nested = self.nested(value);
        self.inner.serialize_field(key, &nested)
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), C::Error> {
        self.inner.skip_field(key)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.inner.end()
    }
}

// ---- Decoding ----

/// A deserializer that forwards to `inner`, reading each string as an index into `strings`.
struct Restoring<'s, D> {
    inner: D,
    strings: &'s [String],
}

/// A seed, visitor or access that restores strings in everything nested below it.
struct Restored<'s, T> {
    inner: T,
    strings: &'s [String],
}

impl<'s, T> Restored<'s, T> {
    /// Wrap `inner` in the same table.
    fn wrap<U>(&self, inner: U) -> Restored<'s, U> {
        Restored {
            inner,
            strings: self.strings,
        }
    }
}

/// Forward a deserializer method whose visitor sees nested values.
macro_rules! forward_nested {
    ($($method:ident($($argument:ident: $type:ty),*)),* $(,)?) => {
        $(fn $method<V: Visitor<'de>>(self, $($argument: $type,)* visitor: V) -> Result<V::Value, D::Error> {
            let strings = self.strings;
            self.inner.$method($($argument,)* Restored { inner: visitor, strings })
        })*
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Restoring<'_, D> {
    type Error = D::Error;

    forward_nested!(
        deserialize_any(),
        deserialize_bool(),
        deserialize_i8(),
        deserialize_i16(),
        deserialize_i32(),
        deserialize_i64(),
        deserialize_i128(),
        deserialize_u8(),
        deserialize_u16(),
        deserialize_u32(),
        deserialize_u64(),
        deserialize_u128(),
        deserialize_f32(),
        deserialize_f64(),
        deserialize_char(),
        deserialize_bytes(),
        deserialize_byte_buf(),
        deserialize_option(),
        deserialize_unit(),
        deserialize_unit_struct(name: &'static str),
        deserialize_newtype_struct(name: &'static str),
        deserialize_seq(),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map(),
        deserialize_struct(name: &'static str, fields: &'static [&'static str]),
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier(),
        deserialize_ignored_any(),
    );

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        let strings = self.strings;
        self.inner.deserialize_u32(StringIndex { visitor, strings })
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        self.deserialize_str(visitor)
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

/// Reads a string's table index and hands the string to `visitor`.
struct StringIndex<'s, V> {
    visitor: V,
    strings: &'s [String],
}

impl<'de, V: Visitor<'de>> Visitor<'de> for StringIndex<'_, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an ABI cache string index")
    }

    fn visit_u32<E: de::Error>(self, id: u32) -> Result<V::Value, E> {
        let text = usize::try_from(id)
            .ok()
            .and_then(|id| self.strings.get(id))
            .ok_or_else(|| E::custom(format!("string index {id} is outside the ABI cache's table")))?;
        self.visitor.visit_str(text)
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Restored<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.inner.deserialize(Restoring {
            inner: deserializer,
            strings: self.strings,
        })
    }
}

/// Forward a visitor method that carries no nested deserializer.
macro_rules! forward_visit {
    ($($method:ident($type:ty)),* $(,)?) => {
        $(fn $method<E: de::Error>(self, value: $type) -> Result<V::Value, E> {
            self.inner.$method(value)
        })*
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Restored<'_, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    forward_visit!(
        visit_bool(bool),
        visit_i8(i8),
        visit_i16(i16),
        visit_i32(i32),
        visit_i64(i64),
        visit_i128(i128),
        visit_u8(u8),
        visit_u16(u16),
        visit_u32(u32),
        visit_u64(u64),
        visit_u128(u128),
        visit_f32(f32),
        visit_f64(f64),
        visit_char(char),
        visit_str(&str),
        visit_string(String),
        visit_bytes(&[u8]),
        visit_byte_buf(Vec<u8>),
    );

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<V::Value, E> {
        self.inner.visit_borrowed_str(value)
    }

    fn visit_borrowed_bytes<E: de::Error>(self, value: &'de [u8]) -> Result<V::Value, E> {
        self.inner.visit_borrowed_bytes(value)
    }

    fn visit_none<E: de::Error>(self) -> Result<V::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<V::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        let strings = self.strings;
        self.inner.visit_some(Restoring {
            inner: deserializer,
            strings,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<V::Value, D::Error> {
        let strings = self.strings;
        self.inner.visit_newtype_struct(Restoring {
            inner: deserializer,
            strings,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<V::Value, A::Error> {
        let access = self.wrap(sequence);
        self.inner.visit_seq(access)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        let access = self.wrap(map);
        self.inner.visit_map(access)
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<V::Value, A::Error> {
        let access = self.wrap(data);
        self.inner.visit_enum(access)
    }
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Restored<'_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, A::Error> {
        let seed = self.wrap(seed);
        self.inner.next_element_seed(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Restored<'_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, A::Error> {
        let seed = self.wrap(seed);
        self.inner.next_key_seed(seed)
    }

    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, A::Error> {
        let seed = self.wrap(seed);
        self.inner.next_value_seed(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'de, 's, A: EnumAccess<'de>> EnumAccess<'de> for Restored<'s, A> {
    type Error = A::Error;
    type Variant = Restored<'s, A::Variant>;

    fn variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<(T::Value, Self::Variant), A::Error> {
        let strings = self.strings;
        let (value, variant) = self.inner.variant_seed(Restored { inner: seed, strings })?;
        Ok((
            value,
            Restored {
                inner: variant,
                strings,
            },
        ))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Restored<'_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.inner.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        let seed = self.wrap(seed);
        self.inner.newtype_variant_seed(seed)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        let visitor = self.wrap(visitor);
        self.inner.tuple_variant(len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        let visitor = self.wrap(visitor);
        self.inner.struct_variant(fields, visitor)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};

    use super::{StringTable, decode, encode};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Shape {
        Unit,
        Named(String),
        Pair(String, u8),
        Fields { label: String, sizes: Vec<u16> },
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Record {
        path: String,
        alias: Option<String>,
        missing: Option<String>,
        tags: Vec<String>,
        shapes: Vec<Shape>,
        fields: BTreeMap<String, String>,
        flag: bool,
        letter: char,
    }

    fn sample(path: &str) -> Record {
        Record {
            path: path.to_owned(),
            alias: Some("core::fmt::Debug".to_owned()),
            missing: None,
            tags: vec!["core::fmt::Debug".to_owned(), path.to_owned()],
            shapes: vec![
                Shape::Unit,
                Shape::Named("T".to_owned()),
                Shape::Pair("T".to_owned(), 7),
                Shape::Fields {
                    label: "core::fmt::Debug".to_owned(),
                    sizes: vec![1, 300],
                },
            ],
            fields: BTreeMap::from([("T".to_owned(), path.to_owned())]),
            flag: true,
            letter: 'é',
        }
    }

    #[test]
    fn records_round_trip_through_one_shared_table() {
        let table = RefCell::new(StringTable::default());
        let first = encode(&sample("std::vec::Vec"), &table).expect("first record encodes");
        let second = encode(&sample("std::string::String"), &table).expect("second record encodes");
        let strings = table.into_inner().into_strings();
        assert_eq!(strings.len(), 4, "each distinct string is stored once: {strings:?}");
        assert_eq!(
            decode::<Record>(&first, &strings).expect("first record decodes"),
            sample("std::vec::Vec")
        );
        assert_eq!(
            decode::<Record>(&second, &strings).expect("second record decodes"),
            sample("std::string::String")
        );
    }

    #[test]
    fn an_index_outside_the_table_is_an_error() {
        let table = RefCell::new(StringTable::default());
        let bytes = encode(&"text".to_owned(), &table).expect("a string encodes");
        assert!(decode::<String>(&bytes, &[]).is_err());
    }
}
