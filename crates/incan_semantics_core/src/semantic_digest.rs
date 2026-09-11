//! A digest over checked meaning, computed from values.
//!
//! RFC 106 requires the semantic digest be computed from checked values rather than by normalising a rendered
//! form, and the rule is not stylistic. A rendering embeds positional data *inside* composite strings — an identity
//! spelling carrying its own `@start..end`, for example — so scrubbing text after the fact reliably misses cases
//! that omitting a field cannot. An earlier probe did exactly that, stripped `span=` and `#decl.`, and still leaked
//! the offset through a nested identity rendering.
//!
//! The approach here is the one RFC 106 cites as prior art: normalise the structured value, then serialise.
//! [`DigestSerializer`] walks a value through `serde` and feeds a hasher, dropping fields whose names are
//! positional. Because it works field-by-field over the real value, a field added to Body IR later is included
//! automatically — the drift that a hand-written visitor suffers, where a new field is silently omitted and the
//! digest quietly under-invalidates, cannot happen here.
//!
//! Excluding a field is the *over*-invalidating direction when done wrong: a positional field left in makes an
//! unrelated edit look like a change, which costs time. Omitting a meaningful field is the dangerous direction, and
//! it is the one this design removes.

use serde::{Serialize, ser};
use sha2::{Digest, Sha256};
use std::fmt::Display;

/// Node-identifier fields excluded from every semantic digest.
///
/// Most are [`crate::CompilerNodeId`]s, which embed the declaration span in their own spelling and so carry
/// position transitively even though their names do not say so.
///
/// `scope_discriminant` is the subtler one. It indexes a module-wide table filled in traversal order
/// (`src/frontend/symbols.rs`: `self.current_scope = self.scopes.len() - 1`), so inserting or moving any
/// declaration renumbers every declaration traversed after it. Its *presence* is meaningful — it separates a
/// nested binding from a module-level namesake — and [`crate::stable_identity::StableDeclarationId`] keeps that
/// in the identity. Its value is position and belongs nowhere.
const POSITIONAL_ID_FIELDS: &[&str] = &["decl_id", "direct_call_id", "id", "module_id", "scope_discriminant"];

/// Whether a struct field is positional and must not reach the digest.
///
/// The span rule is written structurally — `span` or any `*_span` — rather than as a list of known names. A list
/// was the first attempt and it missed `CanonicalSymbolId::declaration_span`, which sits several levels down inside
/// `Body::canonical`: every declaration in a module appeared to change when a comment was added anywhere in it.
/// Matching the shape means a span field added later is excluded without anyone remembering to add it.
///
/// The failure directions are asymmetric. A positional field wrongly left in makes an unrelated edit look like a
/// change, costing a rebuild. A meaningful field wrongly excluded makes a real change invisible, producing a wrong
/// build. This rule can only ever over-match on names ending in `_span`, so it errs the safe way.
fn is_positional_field(name: &str) -> bool {
    name == "span" || name.ends_with("_span") || POSITIONAL_ID_FIELDS.contains(&name)
}

/// The error type for digest serialisation.
///
/// Serialising into a hasher cannot fail on I/O, so the only error is one `serde` itself raises.
#[derive(Debug)]
pub struct DigestError(String);

impl Display for DigestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "semantic digest serialization failed: {}", self.0)
    }
}

impl std::error::Error for DigestError {}

impl ser::Error for DigestError {
    fn custom<T: Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// Hash any checked value into a stable semantic digest, omitting positional fields.
///
/// The digest is `sha256:`-prefixed so a caller can tell one apart from a bare hex string in a receipt or a graph
/// record, matching how RFC 124 spells payload digests.
///
/// # Errors
///
/// Returns [`DigestError`] only if the value's own `Serialize` implementation fails.
pub fn semantic_digest<T: Serialize>(value: &T) -> Result<String, DigestError> {
    let mut hasher = Sha256::new();
    value.serialize(DigestSerializer { hasher: &mut hasher })?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// A `serde` serialiser that feeds a hasher instead of producing a document.
///
/// Every value is written with a type tag and, for anything variable-length, an explicit length. Both matter: a tag
/// stops two differently-typed values with the same bytes from colliding, and a length stops adjacent values from
/// running together — without one, the single identifier `aIb` and the pair `a` `b` hash identically.
pub struct DigestSerializer<'hasher> {
    hasher: &'hasher mut Sha256,
}

impl DigestSerializer<'_> {
    /// Write a type tag, so values of different shapes cannot collide on identical payload bytes.
    fn tag(&mut self, tag: u8) {
        self.hasher.update([tag]);
    }

    /// Write a length-delimited byte run, so adjacent values cannot run together.
    fn bytes(&mut self, value: &[u8]) {
        self.hasher.update((value.len() as u64).to_le_bytes());
        self.hasher.update(value);
    }
}

macro_rules! digest_primitive {
    ($method:ident, $ty:ty, $tag:expr) => {
        fn $method(mut self, value: $ty) -> Result<(), DigestError> {
            self.tag($tag);
            self.hasher.update(value.to_le_bytes());
            Ok(())
        }
    };
}

impl<'hasher> ser::Serializer for DigestSerializer<'hasher> {
    type Ok = ();
    type Error = DigestError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = StructDigest<'hasher>;
    type SerializeStructVariant = StructDigest<'hasher>;

    fn serialize_bool(mut self, value: bool) -> Result<(), DigestError> {
        self.tag(0x01);
        self.hasher.update([u8::from(value)]);
        Ok(())
    }

    digest_primitive!(serialize_i8, i8, 0x02);
    digest_primitive!(serialize_i16, i16, 0x03);
    digest_primitive!(serialize_i32, i32, 0x04);
    digest_primitive!(serialize_i64, i64, 0x05);
    digest_primitive!(serialize_u8, u8, 0x06);
    digest_primitive!(serialize_u16, u16, 0x07);
    digest_primitive!(serialize_u32, u32, 0x08);
    digest_primitive!(serialize_u64, u64, 0x09);
    digest_primitive!(serialize_f32, f32, 0x0a);
    digest_primitive!(serialize_f64, f64, 0x0b);

    // `serde`'s defaults for these reject the value. Body IR carries 128-bit numeric constants, so without them
    // the digest cannot cover a standard library at all -- a gap no fixture exercised and only a corpus found.
    digest_primitive!(serialize_i128, i128, 0x19);
    digest_primitive!(serialize_u128, u128, 0x1a);

    fn serialize_char(mut self, value: char) -> Result<(), DigestError> {
        self.tag(0x0c);
        self.bytes(value.to_string().as_bytes());
        Ok(())
    }

    fn serialize_str(mut self, value: &str) -> Result<(), DigestError> {
        self.tag(0x0d);
        self.bytes(value.as_bytes());
        Ok(())
    }

    fn serialize_bytes(mut self, value: &[u8]) -> Result<(), DigestError> {
        self.tag(0x0e);
        self.bytes(value);
        Ok(())
    }

    fn serialize_none(mut self) -> Result<(), DigestError> {
        self.tag(0x0f);
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(mut self, value: &T) -> Result<(), DigestError> {
        self.tag(0x10);
        value.serialize(DigestSerializer { hasher: self.hasher })
    }

    fn serialize_unit(mut self) -> Result<(), DigestError> {
        self.tag(0x11);
        Ok(())
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<(), DigestError> {
        self.serialize_str(name)
    }

    /// A unit variant contributes its *name*, never its index, so reordering an enum's variants is not a change.
    fn serialize_unit_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), DigestError> {
        self.tag(0x12);
        self.bytes(variant.as_bytes());
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        self.tag(0x13);
        self.bytes(variant.as_bytes());
        value.serialize(DigestSerializer { hasher: self.hasher })
    }

    fn serialize_seq(mut self, length: Option<usize>) -> Result<Self, DigestError> {
        self.tag(0x14);
        self.hasher.update((length.unwrap_or_default() as u64).to_le_bytes());
        Ok(self)
    }

    fn serialize_tuple(self, length: usize) -> Result<Self, DigestError> {
        self.serialize_seq(Some(length))
    }

    fn serialize_tuple_struct(self, _name: &'static str, length: usize) -> Result<Self, DigestError> {
        self.serialize_seq(Some(length))
    }

    fn serialize_tuple_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self, DigestError> {
        self.tag(0x15);
        self.bytes(variant.as_bytes());
        self.hasher.update((length as u64).to_le_bytes());
        Ok(self)
    }

    fn serialize_map(mut self, length: Option<usize>) -> Result<Self, DigestError> {
        self.tag(0x16);
        self.hasher.update((length.unwrap_or_default() as u64).to_le_bytes());
        Ok(self)
    }

    fn serialize_struct(mut self, _name: &'static str, _length: usize) -> Result<StructDigest<'hasher>, DigestError> {
        self.tag(0x17);
        Ok(StructDigest { hasher: self.hasher })
    }

    fn serialize_struct_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _length: usize,
    ) -> Result<StructDigest<'hasher>, DigestError> {
        self.tag(0x18);
        self.bytes(variant.as_bytes());
        Ok(StructDigest { hasher: self.hasher })
    }
}

macro_rules! digest_sequence {
    ($trait_name:ident, $method:ident) => {
        impl ser::$trait_name for DigestSerializer<'_> {
            type Ok = ();
            type Error = DigestError;

            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
                value.serialize(DigestSerializer { hasher: self.hasher })
            }

            fn end(self) -> Result<(), DigestError> {
                Ok(())
            }
        }
    };
}

digest_sequence!(SerializeSeq, serialize_element);
digest_sequence!(SerializeTuple, serialize_element);
digest_sequence!(SerializeTupleStruct, serialize_field);
digest_sequence!(SerializeTupleVariant, serialize_field);

impl ser::SerializeMap for DigestSerializer<'_> {
    type Ok = ();
    type Error = DigestError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), DigestError> {
        key.serialize(DigestSerializer { hasher: self.hasher })
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        value.serialize(DigestSerializer { hasher: self.hasher })
    }

    fn end(self) -> Result<(), DigestError> {
        Ok(())
    }
}

/// Serialises a struct's fields, dropping the positional ones.
///
/// The field *name* is hashed alongside its value, so moving a value between two same-typed fields is a change
/// rather than a coincidence.
pub struct StructDigest<'hasher> {
    hasher: &'hasher mut Sha256,
}

impl ser::SerializeStruct for StructDigest<'_> {
    type Ok = ();
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), DigestError> {
        if is_positional_field(key) {
            return Ok(());
        }
        self.hasher.update((key.len() as u64).to_le_bytes());
        self.hasher.update(key.as_bytes());
        value.serialize(DigestSerializer { hasher: self.hasher })
    }

    fn end(self) -> Result<(), DigestError> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for StructDigest<'_> {
    type Ok = ();
    type Error = DigestError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), DigestError> {
        ser::SerializeStruct::serialize_field(self, key, value)
    }

    fn end(self) -> Result<(), DigestError> {
        Ok(())
    }
}

/// Return a body with its docstring removed, ready to digest.
///
/// An Incan docstring is a bare string expression at the head of a body, exactly as in Python, and it survives
/// lowering as a leading `StatementKind::Expr` whose operand is a string constant. It reaches generated Rust as a
/// `///` line, which cannot change a compiled artifact, so it must not change the semantic digest.
///
/// Documentation is still *output* — for published reference docs and for a manifest surface it is the product —
/// which is why RFC 106 gives it a separate `doc_digest` rather than discarding it. This function only decides
/// what the *semantic* digest covers.
///
/// The rule is deliberately narrow: only a leading bare string expression, only at the head of the body. A string
/// expression anywhere else is a real statement, and dropping it would hide a change.
pub fn body_without_docstring(body: &crate::body_ir::Body) -> crate::body_ir::Body {
    use crate::body_ir::{Constant, Operand, StatementKind};

    let mut projected = body.clone();
    let is_docstring = projected
        .block
        .stmts
        .first()
        .map(|statement| {
            matches!(
                &statement.kind,
                StatementKind::Expr {
                    value: Operand::Constant(Constant::Str(_))
                }
            )
        })
        .unwrap_or(false);
    if is_docstring {
        projected.block.stmts.remove(0);
    }
    projected
}

/// Extract a body's docstring, if it has one.
///
/// The counterpart to [`body_without_docstring`]: what that removes, this returns, so a caller can compute RFC
/// 106's separate `doc_digest` over the same definition of what documentation is.
pub fn body_docstring(body: &crate::body_ir::Body) -> Option<&str> {
    use crate::body_ir::{Constant, Operand, StatementKind};

    match &body.block.stmts.first()?.kind {
        StatementKind::Expr {
            value: Operand::Constant(Constant::Str(text)),
        } => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adjacent variable-length values must not be able to run together.
    ///
    /// Without a length between them, the pair `("ab", "c")` encodes to the same bytes as `("a", "bc")`: two
    /// parameter names differing only in where the boundary falls would share a digest, and a rename that moved a
    /// character across that boundary would be invisible. This is under-invalidation, which yields a wrong build.
    #[test]
    fn adjacent_values_are_length_delimited() -> Result<(), Box<dyn std::error::Error>> {
        // The collision needs the string tag byte to appear *inside* the content, otherwise the tag itself
        // happens to act as a delimiter and the case proves nothing. `\r` is 0x0d, the string tag, and a
        // carriage return in a source literal or docstring is entirely ordinary.
        //
        // Without a length, both of these encode to the same six bytes:
        //   0x0d 'a' 0x0d 'b' 0x0d 'c'
        let split_late = vec!["a\rb".to_string(), "c".to_string()];
        let split_early = vec!["a".to_string(), "b\rc".to_string()];
        assert_ne!(
            semantic_digest(&split_late)?,
            semantic_digest(&split_early)?,
            "two values differing only in where the boundary falls must not share a digest"
        );
        Ok(())
    }

    /// A field's name must reach the digest, so moving a value between two same-typed fields is a change.
    ///
    /// Without the name, a struct is just its values in order, and swapping two same-typed fields — a real
    /// behavioural change — produces an identical digest.
    #[test]
    fn field_names_reach_the_digest() -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Serialize)]
        struct Pair {
            first: u32,
            second: u32,
        }
        #[derive(Serialize)]
        struct Renamed {
            third: u32,
            fourth: u32,
        }
        assert_ne!(
            semantic_digest(&Pair { first: 1, second: 2 })?,
            semantic_digest(&Renamed { third: 1, fourth: 2 })?,
            "differently named fields holding the same values must differ"
        );
        assert_ne!(
            semantic_digest(&Pair { first: 1, second: 2 })?,
            semantic_digest(&Pair { first: 2, second: 1 })?,
            "swapping two same-typed field values must be a change"
        );
        Ok(())
    }

    /// Differently shaped values holding identical payload bytes must not collide.
    #[test]
    fn type_tags_separate_same_bytes_in_different_shapes() -> Result<(), Box<dyn std::error::Error>> {
        assert_ne!(semantic_digest(&Some("x"))?, semantic_digest(&"x")?);
        assert_ne!(semantic_digest(&1u32)?, semantic_digest(&1i32)?);
        assert_ne!(semantic_digest(&vec!["x"])?, semantic_digest(&"x")?);
        Ok(())
    }

    /// An enum variant contributes its name, so reordering variants in the source is not a change.
    #[test]
    fn variant_identity_is_by_name_not_index() -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Serialize)]
        enum First {
            #[allow(dead_code)]
            Alpha,
            Beta,
        }
        #[derive(Serialize)]
        enum Reordered {
            Beta,
            #[allow(dead_code)]
            Alpha,
        }
        assert_eq!(semantic_digest(&First::Beta)?, semantic_digest(&Reordered::Beta)?);
        Ok(())
    }
}
