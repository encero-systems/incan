//! A digest over checked meaning, computed from values.
//!
//! RFC 106 requires the semantic digest be computed from checked values rather than by normalizing a rendered
//! form, and the rule is not stylistic. A rendering embeds positional data *inside* composite strings — an identity
//! spelling carrying its own `@start..end`, for example — so scrubbing text after the fact reliably misses cases
//! that omitting a field cannot. An earlier probe did exactly that, stripped `span=` and `#decl.`, and still leaked
//! the offset through a nested identity rendering.
//!
//! The approach here is the one RFC 106 cites as prior art: normalize the structured value, then serialize.
//! [`DigestSerializer`] walks a value through `serde` and feeds a hasher, dropping fields whose names are
//! positional, retaining expansion context from spans. Because it works field-by-field over the real value, a field
//! added to Body IR later is included automatically — the drift that a hand-written visitor suffers, where a new field
//! is silently omitted and the digest quietly under-invalidates, cannot happen here.
//!
//! Excluding a field is the *over*-invalidating direction when done wrong: a positional field left in makes an
//! unrelated edit look like a change, which costs time. Omitting a meaningful field is the dangerous direction, and
//! it is the one this design removes.

use serde::{Serialize, ser};
use sha2::{Digest, Sha256};

use crate::{CanonicalSymbolId, stable_identity::StableIdentityProjection};
use std::collections::BTreeMap;
use std::fmt::Display;

/// Checked names used only in type positions; literal strings are never interpreted as identities.
#[derive(Clone, Copy)]
struct MeaningContext<'a> {
    projection: &'a StableIdentityProjection,
    bindings: &'a BTreeMap<String, CanonicalSymbolId>,
}

/// Length-delimit a semantic value so adjoining values cannot forge the same hash input by shifting a boundary.
fn update_delimited(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Node-identifier fields excluded from every semantic digest.
///
/// Most are [`crate::CompilerNodeId`]s, which embed the declaration span in their own spelling and so carry
/// position transitively even though their names do not say so.
///
/// `scope_discriminant` is the subtler one. It indexes a module-wide table filled in traversal order
/// (`loaves/compiler/incan_frontend/src/symbols.rs`: `self.current_scope = self.scopes.len() - 1`), so inserting or
/// moving any declaration renumbers every declaration traversed after it. Its *presence* is meaningful — it separates a
/// nested binding from a module-level namesake — and [`crate::stable_identity::StableDeclarationId`] keeps that
/// in the identity. Its value is position and belongs nowhere.
const POSITIONAL_ID_FIELDS: &[&str] = &[
    "decl_id",
    "direct_call_id",
    "direct_declaration_id",
    "enum_declaration_id",
    "variant_declaration_id",
    "id",
    "module_id",
    "scope_discriminant",
];

/// Whether a struct field is positional and must not reach the digest.
///
/// Span fields are handled separately: their coordinates are positional, but their expansion identity is meaningful.
///
/// The failure directions are asymmetric. A positional field wrongly left in makes an unrelated edit look like a
/// change, costing a rebuild. A meaningful field wrongly excluded makes a real change invisible, producing a wrong
/// build. This rule matches only the exact names in [`POSITIONAL_ID_FIELDS`], so it errs the safe way.
fn is_positional_field(name: &str) -> bool {
    POSITIONAL_ID_FIELDS.contains(&name)
}

/// The error type for digest serialization.
///
/// Serializing into a hasher cannot fail on I/O, so the only error is one `serde` itself raises.
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
    value.serialize(DigestSerializer {
        hasher: &mut hasher,
        mode: DigestMode::Meaning,
        context: None,
    })?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// A `serde` serializer that feeds a hasher instead of producing a document.
///
/// Every value is written with a type tag and, for anything variable-length, an explicit length. Both matter: a tag
/// stops two differently-typed values with the same bytes from colliding, and a length stops adjacent values from
/// running together — without one, the single identifier `aIb` and the pair `a` `b` hash identically.
pub struct DigestSerializer<'hasher> {
    hasher: &'hasher mut Sha256,
    mode: DigestMode,
    context: Option<MeaningContext<'hasher>>,
}

/// A span contributes expansion identity but never its diagnostic byte coordinates.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DigestMode {
    Meaning,
    Span(&'static str),
    Expansion(&'static str),
    TypeName,
    TypeBindings,
}

impl DigestSerializer<'_> {
    /// Write a type tag, so values of different shapes cannot collide on identical payload bytes.
    fn tag(&mut self, tag: u8) {
        if matches!(self.mode, DigestMode::Meaning | DigestMode::TypeBindings) {
            self.hasher.update([tag]);
        }
    }

    /// Write a length-delimited byte run, so adjacent values cannot run together.
    fn bytes(&mut self, value: &[u8]) {
        if matches!(self.mode, DigestMode::Meaning | DigestMode::TypeBindings) {
            update_delimited(self.hasher, value);
        }
    }
}

// A deliberate exception to the repository's rule against `macro_rules!` outside `incan_derive`. The two macros
// below write the serde `Serializer` methods for every primitive and every sequence shape, which differ only in
// the type tag and the byte encoding they fold; written by hand, a tag typo in one of the twenty methods would be
// a silent digest collision rather than a compile error. `type_projection.rs`'s `type_fields!` is the same
// exception for the same reason.
macro_rules! digest_primitive {
    ($method:ident, $ty:ty, $tag:expr) => {
        fn $method(mut self, value: $ty) -> Result<(), DigestError> {
            self.tag($tag);
            if self.mode == DigestMode::Meaning {
                self.hasher.update(value.to_le_bytes());
            }
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

    /// Tag a boolean and write its single byte.
    fn serialize_bool(mut self, value: bool) -> Result<(), DigestError> {
        self.tag(0x01);
        if self.mode == DigestMode::Meaning {
            self.hasher.update([u8::from(value)]);
        }
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

    /// Tag a character and write its UTF-8 encoding, length-delimited.
    fn serialize_char(mut self, value: char) -> Result<(), DigestError> {
        self.tag(0x0c);
        self.bytes(value.to_string().as_bytes());
        Ok(())
    }

    /// Tag a string and write its bytes, length-delimited so adjacent values cannot run together.
    fn serialize_str(mut self, value: &str) -> Result<(), DigestError> {
        let projected = if self.mode == DigestMode::TypeName {
            self.mode = DigestMode::Meaning;
            self.context
                .map(|context| {
                    context
                        .projection
                        .type_name(value, context.bindings)
                        .ok_or_else(|| DigestError(format!("unproven nominal identity: {value}")))
                })
                .transpose()?
        } else {
            None
        };
        let value = projected.as_deref().unwrap_or(value);
        self.tag(0x0d);
        self.bytes(value.as_bytes());
        Ok(())
    }

    /// Tag a byte string and write it, length-delimited.
    fn serialize_bytes(mut self, value: &[u8]) -> Result<(), DigestError> {
        self.tag(0x0e);
        self.bytes(value);
        Ok(())
    }

    /// Tag an absent optional, so absence and a present empty value differ.
    fn serialize_none(mut self) -> Result<(), DigestError> {
        self.tag(0x0f);
        Ok(())
    }

    /// Tag a present optional, then hash the value it wraps.
    fn serialize_some<T: ?Sized + Serialize>(mut self, value: &T) -> Result<(), DigestError> {
        if let DigestMode::Expansion(field) = self.mode {
            self.mode = DigestMode::Meaning;
            self.bytes(b"span.expansion");
            self.bytes(field.as_bytes());
        }
        self.tag(0x10);
        value.serialize(DigestSerializer {
            hasher: self.hasher,
            mode: self.mode,
            context: self.context,
        })
    }

    /// Tag the unit value.
    fn serialize_unit(mut self) -> Result<(), DigestError> {
        self.tag(0x11);
        Ok(())
    }

    /// Hash a unit struct by name, since it carries nothing else to distinguish it.
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

    /// Hash a newtype's inner value directly; the wrapper adds no meaning of its own.
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        value.serialize(self)
    }

    /// Tag a newtype variant by name, then hash the value it wraps.
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        mut self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), DigestError> {
        self.tag(0x13);
        self.bytes(variant.as_bytes());
        value.serialize(DigestSerializer {
            hasher: self.hasher,
            mode: if name == "IncanType" && variant == "Named" {
                DigestMode::TypeName
            } else {
                self.mode
            },
            context: self.context,
        })
    }

    /// Tag a sequence and write its length, so two sequences cannot concatenate into one.
    fn serialize_seq(mut self, length: Option<usize>) -> Result<Self, DigestError> {
        self.tag(0x14);
        if matches!(self.mode, DigestMode::Meaning | DigestMode::TypeBindings) {
            self.hasher.update((length.unwrap_or_default() as u64).to_le_bytes());
        }
        Ok(self)
    }

    /// Hash a tuple as a fixed-length sequence.
    fn serialize_tuple(self, length: usize) -> Result<Self, DigestError> {
        self.serialize_seq(Some(length))
    }

    /// Hash a tuple struct as a fixed-length sequence; its name adds nothing the fields do not.
    fn serialize_tuple_struct(self, _name: &'static str, length: usize) -> Result<Self, DigestError> {
        self.serialize_seq(Some(length))
    }

    /// Tag a tuple variant by name, then write its length and fields.
    fn serialize_tuple_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self, DigestError> {
        self.tag(0x15);
        self.bytes(variant.as_bytes());
        if self.mode == DigestMode::Meaning {
            self.hasher.update((length as u64).to_le_bytes());
        }
        Ok(self)
    }

    /// Tag a map and write its length, then hash each key and value in turn.
    fn serialize_map(mut self, length: Option<usize>) -> Result<Self, DigestError> {
        self.tag(0x16);
        if matches!(self.mode, DigestMode::Meaning | DigestMode::TypeBindings) {
            self.hasher.update((length.unwrap_or_default() as u64).to_le_bytes());
        }
        Ok(self)
    }

    /// Tag a struct; its fields are hashed by [`StructDigest`], which drops the positional ones.
    fn serialize_struct(mut self, name: &'static str, _length: usize) -> Result<StructDigest<'hasher>, DigestError> {
        if name == "CompilerNodeId" && self.context.is_some() {
            return Err(DigestError("unprojected compilation-local node identity".into()));
        }
        self.tag(0x17);
        Ok(StructDigest {
            hasher: self.hasher,
            mode: self.mode,
            context: self.context,
            type_fields: false,
            unsafe_receipt: name == "UnsafeAcknowledgment",
            identity: (name == "CanonicalSymbolId" && self.context.is_some()).then(|| CanonicalSymbolId {
                namespace: crate::SymbolNamespace::OrdinaryLexical,
                origin: crate::SymbolOrigin::Builtin,
                declaration_name: String::new(),
                kind: crate::SemanticSourceTargetKind::Function,
                scope_discriminant: None,
                declaration_span: crate::HirSourceSpan::new(0, 0),
            }),
        })
    }

    /// Tag a struct variant by name; its fields are hashed by [`StructDigest`].
    fn serialize_struct_variant(
        mut self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        _length: usize,
    ) -> Result<StructDigest<'hasher>, DigestError> {
        self.tag(0x18);
        self.bytes(variant.as_bytes());
        Ok(StructDigest {
            hasher: self.hasher,
            mode: self.mode,
            context: self.context,
            type_fields: name == "IncanType" && variant == "Generic",
            unsafe_receipt: false,
            identity: None,
        })
    }
}

macro_rules! digest_sequence {
    ($trait_name:ident, $method:ident) => {
        impl ser::$trait_name for DigestSerializer<'_> {
            type Ok = ();
            type Error = DigestError;

            /// Keep each sequence child in its parent's checked meaning context.
            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
                value.serialize(DigestSerializer {
                    hasher: self.hasher,
                    mode: self.mode,
                    context: self.context,
                })
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

    /// Resolve retained type-binding keys through their checked identities; ordinary map keys remain literal.
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), DigestError> {
        key.serialize(DigestSerializer {
            hasher: self.hasher,
            mode: if self.mode == DigestMode::TypeBindings {
                DigestMode::TypeName
            } else {
                self.mode
            },
            context: self.context,
        })
    }

    /// Return to ordinary meaning after a binding key so canonical values keep their structured identity.
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DigestError> {
        value.serialize(DigestSerializer {
            hasher: self.hasher,
            mode: if self.mode == DigestMode::TypeBindings {
                DigestMode::Meaning
            } else {
                self.mode
            },
            context: self.context,
        })
    }

    fn end(self) -> Result<(), DigestError> {
        Ok(())
    }
}

/// Serializes a struct's fields, dropping the positional ones.
///
/// The field *name* is hashed alongside its value, so moving a value between two same-typed fields is a change
/// rather than a coincidence.
pub struct StructDigest<'hasher> {
    hasher: &'hasher mut Sha256,
    mode: DigestMode,
    context: Option<MeaningContext<'hasher>>,
    type_fields: bool,
    unsafe_receipt: bool,
    identity: Option<CanonicalSymbolId>,
}

/// Recover one field of a known canonical identity, never a rendered spelling or an arbitrary checked value.
fn identity_field<T: ?Sized + Serialize>(
    identity: &mut CanonicalSymbolId,
    key: &str,
    value: &T,
) -> Result<(), DigestError> {
    match key {
        "namespace" => identity.namespace = checked_identity_field(value)?,
        "origin" => identity.origin = checked_identity_field(value)?,
        "declaration_name" => identity.declaration_name = checked_identity_field(value)?,
        "kind" => identity.kind = checked_identity_field(value)?,
        "scope_discriminant" => identity.scope_discriminant = checked_identity_field(value)?,
        "declaration_span" => identity.declaration_span = checked_identity_field(value)?,
        _ => return Err(DigestError(format!("unrecognized canonical identity field: {key}"))),
    }
    Ok(())
}

/// Decode a field whose exact Rust type is fixed by CanonicalSymbolId's serialization contract.
fn checked_identity_field<T: ?Sized + Serialize, V: serde::de::DeserializeOwned>(value: &T) -> Result<V, DigestError> {
    let bytes = postcard::to_stdvec(value).map_err(|error| DigestError(error.to_string()))?;
    postcard::from_bytes(&bytes).map_err(|error| DigestError(error.to_string()))
}

impl ser::SerializeStruct for StructDigest<'_> {
    type Ok = ();
    type Error = DigestError;

    /// Project checked identity and type fields while excluding source coordinates and redundant physical IDs.
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), DigestError> {
        if let Some(identity) = &mut self.identity {
            return identity_field(identity, key, value);
        }
        if let DigestMode::Span(field) = self.mode {
            return if key == "expansion" {
                value.serialize(DigestSerializer {
                    hasher: self.hasher,
                    mode: DigestMode::Expansion(field),
                    context: self.context,
                })
            } else {
                Ok(())
            };
        }
        if key == "span" || key.ends_with("_span") {
            return value.serialize(DigestSerializer {
                hasher: self.hasher,
                mode: DigestMode::Span(key),
                context: self.context,
            });
        }
        if is_positional_field(key) {
            return Ok(());
        }
        // The enclosing unsafe region already retains its scope, expansion and operations. Its source receipt is
        // checked before this boundary and carries no additional executable meaning.
        if self.unsafe_receipt && key == "receipt" {
            return Ok(());
        }
        update_delimited(self.hasher, key.as_bytes());
        value.serialize(DigestSerializer {
            hasher: self.hasher,
            mode: if self.type_fields && key == "base" {
                DigestMode::TypeName
            } else if key == "named_type_identities" {
                DigestMode::TypeBindings
            } else {
                self.mode
            },
            context: self.context,
        })
    }

    /// Hash a complete canonical identity through the shared projection, retaining its expansion context.
    fn end(self) -> Result<(), DigestError> {
        if let (Some(identity), Some(context)) = (self.identity, self.context) {
            let stable = context
                .projection
                .declaration(&identity)
                .ok_or_else(|| DigestError(format!("unproven lexical identity: {}", identity.render_compact())))?;
            return (stable, identity.declaration_span.expansion).serialize(DigestSerializer {
                hasher: self.hasher,
                mode: DigestMode::Meaning,
                context: None,
            });
        }
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

/// Digest checked body meaning without cloning its locals, statements or identity tables.
///
/// An Incan docstring is a bare string expression at the head of a body and survives lowering as a leading
/// `StatementKind::Expr` whose operand is a string constant. Documentation does not change executable meaning and must
/// not enter the semantic digest.
///
/// Documentation remains output for published reference docs and manifest surfaces, so RFC 106 gives it a separate
/// `doc_digest`. This function detaches a leading docstring during serialization and restores it before returning,
/// including on a serialization error; every other field remains borrowed in place.
///
/// Only a leading bare string expression is documentation. A string expression anywhere else remains a real statement.
///
/// # Errors
///
/// Returns a serialization error, including missing retained identity proof, after restoring the complete input body.
pub fn body_semantic_digest(
    body: &mut crate::body_ir::Body,
    projection: &StableIdentityProjection,
) -> Result<String, DigestError> {
    if body_docstring(body).is_none() {
        return checked_body_digest(body, projection);
    }
    let docstring = body.block.stmts.remove(0);
    let digest = checked_body_digest(body, projection);
    body.block.stmts.insert(0, docstring);
    digest
}

/// Hash a borrowed body through its retained nominal proof; physical carrier strings never become meaning.
fn checked_body_digest(
    body: &crate::body_ir::Body,
    projection: &StableIdentityProjection,
) -> Result<String, DigestError> {
    let mut hasher = Sha256::new();
    body.serialize(DigestSerializer {
        hasher: &mut hasher,
        mode: DigestMode::Meaning,
        context: Some(MeaningContext {
            projection,
            bindings: &body.named_type_identities,
        }),
    })?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Extract a body's docstring, if it has one.
///
/// The counterpart to [`body_semantic_digest`]: what that excludes, this returns, so a caller can compute RFC
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
    /// behavioral change — produces an identical digest.
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
        #[derive(Serialize)]
        struct Located {
            span: crate::HirSourceSpan,
        }
        let authored = Located {
            span: crate::HirSourceSpan::new(1, 2),
        };
        let moved = Located {
            span: crate::HirSourceSpan::new(10, 20),
        };
        let generated = Located {
            span: authored.span.with_expansion(Some([1; 32])),
        };
        let other = Located {
            span: authored.span.with_expansion(Some([2; 32])),
        };
        assert_eq!(semantic_digest(&authored)?, semantic_digest(&moved)?);
        assert_ne!(semantic_digest(&authored)?, semantic_digest(&generated)?);
        assert_ne!(semantic_digest(&generated)?, semantic_digest(&other)?);
        check_retained_nominal_meaning()?;
        Ok(())
    }

    /// Physical names normalize only in checked type positions; literal text and distinct nominal owners stay distinct.
    fn check_retained_nominal_meaning() -> Result<(), Box<dyn std::error::Error>> {
        use crate::{HirSourceSpan, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
        #[derive(Serialize)]
        struct TypedValue {
            ty: IncanType,
            literal: String,
            named_type_identities: BTreeMap<String, CanonicalSymbolId>,
            target: crate::body_ir::ConstructorTarget,
        }
        let owner = CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["left".into()]),
            declaration_name: "Item".into(),
            kind: SemanticSourceTargetKind::Model,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(10, 20),
        };
        let mut moved = owner.clone();
        moved.declaration_span = HirSourceSpan::new(100, 200);
        let projection = StableIdentityProjection::default();
        let digest = |identity: CanonicalSymbolId, literal: &str| -> Result<String, Box<dyn std::error::Error>> {
            let key = crate::body_ir::nominal_carrier_key(&identity).ok_or("missing carrier")?;
            let bindings = BTreeMap::from([(key.clone(), identity)]);
            let identity = bindings.get(&key).ok_or("missing checked binding")?;
            let value = TypedValue {
                ty: IncanType::Generic {
                    base: "List".into(),
                    args: vec![IncanType::Named(key)],
                },
                literal: literal.into(),
                named_type_identities: bindings.clone(),
                target: crate::body_ir::ConstructorTarget {
                    name: "Item".into(),
                    canonical: Some(identity.clone()),
                    direct_declaration_id: Some(crate::CompilerNodeId::declaration_at(
                        "left",
                        identity.declaration_span,
                    )),
                    canonical_field_layout: Some(vec!["value".into()]),
                    binding: crate::body_ir::ArgumentBinding::resolved_positional(1),
                },
            };
            let mut hasher = Sha256::new();
            value.serialize(DigestSerializer {
                hasher: &mut hasher,
                mode: DigestMode::Meaning,
                context: Some(MeaningContext {
                    projection: &projection,
                    bindings: &bindings,
                }),
            })?;
            Ok(hex::encode(hasher.finalize()))
        };
        let text = crate::body_ir::nominal_carrier_key(&owner).ok_or("missing literal carrier")?;
        assert_eq!(digest(owner.clone(), &text)?, digest(moved.clone(), &text)?);
        assert_ne!(digest(owner.clone(), &text)?, digest(owner.clone(), "other literal")?);
        moved.origin = SymbolOrigin::Module(vec!["right".into()]);
        assert_ne!(digest(owner, &text)?, digest(moved, &text)?);
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
