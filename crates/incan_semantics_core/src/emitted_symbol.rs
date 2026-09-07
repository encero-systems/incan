//! Recoverable RFC 120 projections for linker-visible Incan symbols.
//!
//! This module is deliberately an artifact-inspection boundary. Compiler semantic stages carry
//! [`CanonicalSymbolId`] directly and must never call the decoder to decide what a source reference means. The
//! encoder is available to backend emission; the decoder exists so a completed artifact can report which
//! compiler-owned identity one of its symbols carries without consulting generated Rust or a sidecar.

use std::fmt;

use crate::{
    CanonicalSymbolId, HirSourceSpan, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

/// Logical projection format named by RFC 120.
pub const INCAN_SYMBOL_PROJECTION_VERSION: &str = "incan-v1";

/// Rust-identifier-safe carrier prefix for an [`INCAN_SYMBOL_PROJECTION_VERSION`] payload.
///
/// Rust identifiers cannot contain `-`, so the logical `incan-v1` spelling is represented as `incan_v1` in the
/// emitted item name. The remainder is lower-case hexadecimal and therefore survives as one Rust identifier.
pub const INCAN_SYMBOL_RUST_PREFIX: &str = "__incan_v1_";

/// Failure to decode a malformed or unsupported Incan symbol payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmittedSymbolDecodeError {
    /// The marker names a projection version this compiler does not understand.
    UnsupportedVersion(String),
    /// The encoded payload is not well-formed lower-case hexadecimal.
    InvalidHex,
    /// A required field or variant tag is missing or invalid.
    InvalidPayload(&'static str),
    /// A length or integer in the payload does not fit on this host.
    IntegerOverflow,
    /// A string field is not valid UTF-8.
    InvalidUtf8,
    /// Bytes remained after the complete canonical identity was decoded.
    TrailingPayload,
}

impl fmt::Display for EmittedSymbolDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(f, "unsupported Incan emitted-symbol version `{version}`"),
            Self::InvalidHex => f.write_str("invalid hexadecimal Incan emitted-symbol payload"),
            Self::InvalidPayload(field) => write!(f, "invalid Incan emitted-symbol payload field `{field}`"),
            Self::IntegerOverflow => f.write_str("Incan emitted-symbol integer does not fit on this host"),
            Self::InvalidUtf8 => f.write_str("Incan emitted-symbol string is not valid UTF-8"),
            Self::TrailingPayload => f.write_str("Incan emitted-symbol payload has trailing bytes"),
        }
    }
}

impl std::error::Error for EmittedSymbolDecodeError {}

/// Encode a complete canonical identity as one Rust-safe `incan-v1` item identifier.
///
/// The carrier is intentionally self-contained. It is not a hash or a key into compiler state: an artifact observer
/// can reconstruct the complete identity from this identifier alone.
pub fn encode_incan_symbol_identity(identity: &CanonicalSymbolId) -> String {
    let mut payload = Vec::new();
    payload.push(namespace_tag(identity.namespace));
    encode_origin(&mut payload, &identity.origin);
    write_string(&mut payload, &identity.declaration_name);
    encode_kind(&mut payload, &identity.kind);
    match identity.scope_discriminant {
        Some(ScopeDiscriminant(scope)) => {
            payload.push(1);
            write_usize(&mut payload, scope);
        }
        None => payload.push(0),
    }
    write_usize(&mut payload, identity.declaration_span.start);
    write_usize(&mut payload, identity.declaration_span.end);

    let mut emitted = String::with_capacity(INCAN_SYMBOL_RUST_PREFIX.len() + payload.len() * 2);
    emitted.push_str(INCAN_SYMBOL_RUST_PREFIX);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in payload {
        emitted.push(char::from(HEX[usize::from(byte >> 4)]));
        emitted.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    emitted
}

/// Decode one exact Rust identifier when it carries an Incan projection.
///
/// An identifier without any Incan marker is a runtime, host, interop, or otherwise non-Incan symbol and returns
/// `Ok(None)`. A marker for another `incan-vN` version is reported rather than guessed.
pub fn decode_incan_symbol_identity(identifier: &str) -> Result<Option<CanonicalSymbolId>, EmittedSymbolDecodeError> {
    let Some(hex) = identifier.strip_prefix(INCAN_SYMBOL_RUST_PREFIX) else {
        if let Some(version) = identifier.strip_prefix("__incan_v") {
            let version = version.split('_').next().unwrap_or(version);
            return Err(EmittedSymbolDecodeError::UnsupportedVersion(format!(
                "incan-v{version}"
            )));
        }
        return Ok(None);
    };
    if hex.is_empty() || hex.len() % 2 != 0 {
        return Err(EmittedSymbolDecodeError::InvalidHex);
    }
    let bytes = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = decode_hex_digit(pair[0])?;
            let low = decode_hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect::<Result<Vec<_>, EmittedSymbolDecodeError>>()?;
    let mut reader = PayloadReader::new(&bytes);
    let namespace = decode_namespace(reader.byte("namespace")?)?;
    let origin = decode_origin(&mut reader)?;
    let declaration_name = reader.string("declaration_name")?;
    let kind = decode_kind(&mut reader)?;
    let scope_discriminant = match reader.byte("scope_presence")? {
        0 => None,
        1 => Some(ScopeDiscriminant(reader.usize("scope_discriminant")?)),
        _ => return Err(EmittedSymbolDecodeError::InvalidPayload("scope_presence")),
    };
    let declaration_span = HirSourceSpan::new(
        reader.usize("declaration_span.start")?,
        reader.usize("declaration_span.end")?,
    );
    if !reader.remaining().is_empty() {
        return Err(EmittedSymbolDecodeError::TrailingPayload);
    }
    Ok(Some(CanonicalSymbolId {
        namespace,
        origin,
        declaration_name,
        kind,
        scope_discriminant,
        declaration_span,
    }))
}

/// Recover the first exact Incan identifier embedded in a demangled native symbol.
///
/// Rust v0 demangling renders path components separated by punctuation. Scanning only the identifier alphabet keeps
/// the decoder independent of crate names and generic suffixes while refusing to reinterpret arbitrary frame text.
pub fn decode_incan_identity_from_demangled_symbol(
    demangled: &str,
) -> Result<Option<CanonicalSymbolId>, EmittedSymbolDecodeError> {
    // A marker embedded in a host-owned token is ordinary host text, not Incan provenance. Only an entire
    // demangled identifier component whose first bytes are the reserved marker can carry the contract.
    let candidate = demangled
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .find(|component| component.starts_with("__incan_v"));
    candidate.map_or(Ok(None), decode_incan_symbol_identity)
}

/// Encode one canonical symbol namespace as its stable payload tag.
fn namespace_tag(namespace: SymbolNamespace) -> u8 {
    match namespace {
        SymbolNamespace::OrdinaryLexical => 0,
        SymbolNamespace::Member => 1,
        SymbolNamespace::ModulePath => 2,
    }
}

/// Decode one stable namespace tag, rejecting values outside the v1 contract.
fn decode_namespace(tag: u8) -> Result<SymbolNamespace, EmittedSymbolDecodeError> {
    match tag {
        0 => Ok(SymbolNamespace::OrdinaryLexical),
        1 => Ok(SymbolNamespace::Member),
        2 => Ok(SymbolNamespace::ModulePath),
        _ => Err(EmittedSymbolDecodeError::InvalidPayload("namespace")),
    }
}

/// Append one canonical symbol origin to an emitted-symbol payload.
fn encode_origin(out: &mut Vec<u8>, origin: &SymbolOrigin) {
    match origin {
        SymbolOrigin::Module(path) => {
            out.push(0);
            write_strings(out, path);
        }
        SymbolOrigin::Package { library, module_path } => {
            out.push(1);
            write_string(out, library);
            write_strings(out, module_path);
        }
        SymbolOrigin::RustCrate(path) => {
            out.push(2);
            write_strings(out, path);
        }
        SymbolOrigin::Builtin => out.push(3),
    }
}

/// Decode one canonical symbol origin from an emitted-symbol payload.
fn decode_origin(reader: &mut PayloadReader<'_>) -> Result<SymbolOrigin, EmittedSymbolDecodeError> {
    match reader.byte("origin")? {
        0 => Ok(SymbolOrigin::Module(reader.strings("origin.module")?)),
        1 => Ok(SymbolOrigin::Package {
            library: reader.string("origin.package.library")?,
            module_path: reader.strings("origin.package.module_path")?,
        }),
        2 => Ok(SymbolOrigin::RustCrate(reader.strings("origin.rust_crate")?)),
        3 => Ok(SymbolOrigin::Builtin),
        _ => Err(EmittedSymbolDecodeError::InvalidPayload("origin")),
    }
}

/// Append one semantic declaration-kind tag and any associated spelling.
fn encode_kind(out: &mut Vec<u8>, kind: &SemanticSourceTargetKind) {
    let tag = match kind {
        SemanticSourceTargetKind::Function => 0,
        SemanticSourceTargetKind::Model => 1,
        SemanticSourceTargetKind::Class => 2,
        SemanticSourceTargetKind::Newtype => 3,
        SemanticSourceTargetKind::Rusttype => 4,
        SemanticSourceTargetKind::Enum => 5,
        SemanticSourceTargetKind::TypeAlias => 6,
        SemanticSourceTargetKind::Partial => 7,
        SemanticSourceTargetKind::Variant => 8,
        SemanticSourceTargetKind::Trait => 9,
        SemanticSourceTargetKind::Capability => 10,
        SemanticSourceTargetKind::Field => 11,
        SemanticSourceTargetKind::Method => 12,
        SemanticSourceTargetKind::Property => 13,
        SemanticSourceTargetKind::Const => 14,
        SemanticSourceTargetKind::Static => 15,
        SemanticSourceTargetKind::Local => 16,
        SemanticSourceTargetKind::Parameter => 17,
        SemanticSourceTargetKind::Receiver => 18,
        SemanticSourceTargetKind::GenericBinder => 19,
        SemanticSourceTargetKind::Module => 20,
        SemanticSourceTargetKind::RustItem => 21,
        SemanticSourceTargetKind::Builtin => 22,
        SemanticSourceTargetKind::Other(_) => 255,
    };
    out.push(tag);
    if let SemanticSourceTargetKind::Other(value) = kind {
        write_string(out, value);
    }
}

/// Decode one semantic declaration kind from its stable payload representation.
fn decode_kind(reader: &mut PayloadReader<'_>) -> Result<SemanticSourceTargetKind, EmittedSymbolDecodeError> {
    match reader.byte("kind")? {
        0 => Ok(SemanticSourceTargetKind::Function),
        1 => Ok(SemanticSourceTargetKind::Model),
        2 => Ok(SemanticSourceTargetKind::Class),
        3 => Ok(SemanticSourceTargetKind::Newtype),
        4 => Ok(SemanticSourceTargetKind::Rusttype),
        5 => Ok(SemanticSourceTargetKind::Enum),
        6 => Ok(SemanticSourceTargetKind::TypeAlias),
        7 => Ok(SemanticSourceTargetKind::Partial),
        8 => Ok(SemanticSourceTargetKind::Variant),
        9 => Ok(SemanticSourceTargetKind::Trait),
        10 => Ok(SemanticSourceTargetKind::Capability),
        11 => Ok(SemanticSourceTargetKind::Field),
        12 => Ok(SemanticSourceTargetKind::Method),
        13 => Ok(SemanticSourceTargetKind::Property),
        14 => Ok(SemanticSourceTargetKind::Const),
        15 => Ok(SemanticSourceTargetKind::Static),
        16 => Ok(SemanticSourceTargetKind::Local),
        17 => Ok(SemanticSourceTargetKind::Parameter),
        18 => Ok(SemanticSourceTargetKind::Receiver),
        19 => Ok(SemanticSourceTargetKind::GenericBinder),
        20 => Ok(SemanticSourceTargetKind::Module),
        21 => Ok(SemanticSourceTargetKind::RustItem),
        22 => Ok(SemanticSourceTargetKind::Builtin),
        255 => Ok(SemanticSourceTargetKind::Other(reader.string("kind.other")?)),
        _ => Err(EmittedSymbolDecodeError::InvalidPayload("kind")),
    }
}

/// Append a length-prefixed sequence of UTF-8 strings.
fn write_strings(out: &mut Vec<u8>, values: &[String]) {
    write_u64(out, values.len() as u64);
    for value in values {
        write_string(out, value);
    }
}

/// Append one length-prefixed UTF-8 string.
fn write_string(out: &mut Vec<u8>, value: &str) {
    write_u64(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

/// Append one unsigned integer in canonical big-endian form.
fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// Append one platform-sized value using the payload's fixed-width integer representation.
fn write_usize(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u64).to_be_bytes());
}

/// Decode one lowercase hexadecimal byte used by a Rust-safe symbol spelling.
fn decode_hex_digit(value: u8) -> Result<u8, EmittedSymbolDecodeError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(EmittedSymbolDecodeError::InvalidHex),
    }
}

struct PayloadReader<'a> {
    remaining: &'a [u8],
}

impl<'a> PayloadReader<'a> {
    /// Start reading a canonical emitted-symbol payload.
    fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    /// Return the payload bytes that have not yet been consumed.
    fn remaining(&self) -> &'a [u8] {
        self.remaining
    }

    /// Consume exactly `length` bytes or attribute a truncated payload to `field`.
    fn take(&mut self, length: usize, field: &'static str) -> Result<&'a [u8], EmittedSymbolDecodeError> {
        if self.remaining.len() < length {
            return Err(EmittedSymbolDecodeError::InvalidPayload(field));
        }
        let (head, tail) = self.remaining.split_at(length);
        self.remaining = tail;
        Ok(head)
    }

    /// Consume one tagged byte for `field`.
    fn byte(&mut self, field: &'static str) -> Result<u8, EmittedSymbolDecodeError> {
        Ok(self.take(1, field)?[0])
    }

    /// Consume one fixed-width integer and convert it to the host index type.
    fn usize(&mut self, field: &'static str) -> Result<usize, EmittedSymbolDecodeError> {
        let bytes: [u8; 8] = self
            .take(8, field)?
            .try_into()
            .map_err(|_| EmittedSymbolDecodeError::InvalidPayload(field))?;
        usize::try_from(u64::from_be_bytes(bytes)).map_err(|_| EmittedSymbolDecodeError::IntegerOverflow)
    }

    /// Consume one length-prefixed UTF-8 string.
    fn string(&mut self, field: &'static str) -> Result<String, EmittedSymbolDecodeError> {
        let length = self.usize(field)?;
        let bytes = self.take(length, field)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| EmittedSymbolDecodeError::InvalidUtf8)
    }

    /// Consume one bounded sequence of length-prefixed UTF-8 strings.
    fn strings(&mut self, field: &'static str) -> Result<Vec<String>, EmittedSymbolDecodeError> {
        let length = self.usize(field)?;
        // Every encoded string needs at least its eight-byte length prefix. Bound the advertised count before
        // reserving or iterating so a malformed payload cannot request an attacker-controlled allocation.
        if length > self.remaining.len() / std::mem::size_of::<u64>() {
            return Err(EmittedSymbolDecodeError::InvalidPayload(field));
        }
        let mut values = Vec::with_capacity(length);
        for _ in 0..length {
            values.push(self.string(field)?);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    #![deny(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn identities() -> Vec<CanonicalSymbolId> {
        let kinds = [
            SemanticSourceTargetKind::Function,
            SemanticSourceTargetKind::Model,
            SemanticSourceTargetKind::Class,
            SemanticSourceTargetKind::Newtype,
            SemanticSourceTargetKind::Rusttype,
            SemanticSourceTargetKind::Enum,
            SemanticSourceTargetKind::TypeAlias,
            SemanticSourceTargetKind::Partial,
            SemanticSourceTargetKind::Variant,
            SemanticSourceTargetKind::Trait,
            SemanticSourceTargetKind::Capability,
            SemanticSourceTargetKind::Field,
            SemanticSourceTargetKind::Method,
            SemanticSourceTargetKind::Property,
            SemanticSourceTargetKind::Const,
            SemanticSourceTargetKind::Static,
            SemanticSourceTargetKind::Local,
            SemanticSourceTargetKind::Parameter,
            SemanticSourceTargetKind::Receiver,
            SemanticSourceTargetKind::GenericBinder,
            SemanticSourceTargetKind::Module,
            SemanticSourceTargetKind::RustItem,
            SemanticSourceTargetKind::Builtin,
            SemanticSourceTargetKind::Other("future-kind".to_string()),
        ];
        kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| CanonicalSymbolId {
                namespace: match index % 3 {
                    0 => SymbolNamespace::OrdinaryLexical,
                    1 => SymbolNamespace::Member,
                    _ => SymbolNamespace::ModulePath,
                },
                origin: match index % 4 {
                    0 => SymbolOrigin::Module(vec!["app".to_string(), "café".to_string()]),
                    1 => SymbolOrigin::Package {
                        library: "widgets".to_string(),
                        module_path: vec!["api".to_string()],
                    },
                    2 => SymbolOrigin::RustCrate(vec!["std".to_string(), "io".to_string()]),
                    _ => SymbolOrigin::Builtin,
                },
                declaration_name: format!("symbol_{index}_λ"),
                kind,
                scope_discriminant: (index % 2 == 0).then_some(ScopeDiscriminant(index)),
                declaration_span: HirSourceSpan::new(index * 7, index * 7 + 5),
            })
            .collect()
    }

    #[test]
    fn incan_v1_round_trips_every_canonical_identity_variant() -> Result<(), Box<dyn std::error::Error>> {
        for identity in identities() {
            let emitted = encode_incan_symbol_identity(&identity);
            assert!(emitted.starts_with(INCAN_SYMBOL_RUST_PREFIX));
            assert!(emitted.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric()));
            assert_eq!(decode_incan_symbol_identity(&emitted)?, Some(identity));
        }
        Ok(())
    }

    #[test]
    fn demangled_observer_classifies_non_incan_symbols_without_guessing() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            decode_incan_identity_from_demangled_symbol("std::panicking::begin_panic")?,
            None
        );
        assert_eq!(
            decode_incan_identity_from_demangled_symbol("host_bridge::invoke")?,
            None
        );
        assert_eq!(decode_incan_identity_from_demangled_symbol("rust::ffi::adapter")?, None);
        Ok(())
    }

    #[test]
    fn demangled_observer_extracts_one_exact_path_component() -> Result<(), Box<dyn std::error::Error>> {
        let identity = identities().remove(0);
        let emitted = encode_incan_symbol_identity(&identity);
        let demangled = format!("fixture::{emitted}::<u64>");
        assert_eq!(decode_incan_identity_from_demangled_symbol(&demangled)?, Some(identity));
        Ok(())
    }

    #[test]
    fn malformed_or_unknown_markers_fail_closed() {
        assert_eq!(
            decode_incan_symbol_identity("__incan_v2_00"),
            Err(EmittedSymbolDecodeError::UnsupportedVersion("incan-v2".to_string()))
        );
        assert_eq!(
            decode_incan_symbol_identity("__incan_v1_0g"),
            Err(EmittedSymbolDecodeError::InvalidHex)
        );
        assert_eq!(decode_incan_symbol_identity("ordinary_function"), Ok(None));
        let valid = encode_incan_symbol_identity(&identities().remove(0));
        assert_eq!(
            decode_incan_identity_from_demangled_symbol(&format!("host{valid}")),
            Ok(None)
        );
    }

    #[test]
    fn advertised_collection_count_is_bounded_by_remaining_payload() {
        // namespace, module origin, then an impossible module-path item count with no item bytes.
        let malformed = "__incan_v1_0000ffffffffffffffff";
        assert!(matches!(
            decode_incan_symbol_identity(malformed),
            Err(EmittedSymbolDecodeError::InvalidPayload("origin.module"))
                | Err(EmittedSymbolDecodeError::IntegerOverflow)
        ));
    }
}
