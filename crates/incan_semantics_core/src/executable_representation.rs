//! The encoded form of a module's checked representation, and the contract for refusing one.
//!
//! RFC 123 establishes that a package publishes an executable representation of its public surface, so that what a
//! declaration means does not change when it moves into a package. This module owns the wire form of that
//! representation and nothing else: encoding a checked module, decoding one back, and refusing — in terms a consumer
//! can act on — anything it cannot interpret.
//!
//! ## Why this encodes the checked representation directly
//!
//! There is no parallel wire type mirroring [`BodyIrModule`]. RFC 123's alternatives reject exactly that shape: a
//! second description of the same surface is a thing that can drift from the first, and the identity model exists to
//! stop consumers reasoning about two descriptions of one declaration. Encoding the checked representation itself
//! makes drift impossible by construction — there is only one description — and the version below is what lets a
//! consumer refuse a representation it does not understand rather than misread one.
//!
//! The cost of that choice is real and worth stating: the wire form follows the checked representation's shape, so
//! changing that shape changes the format. That is what [`EXECUTABLE_REPRESENTATION_VERSION`] is for, and why a
//! consumer that reads an unfamiliar version must refuse rather than guess.

use serde::{Deserialize, Serialize};

use crate::CanonicalSymbolId;
use crate::body_ir::{Body, BodyIrModule, FieldlessEnumDeclaration, NominalDeclaration, ValueEnumDeclaration};

/// Version of the encoded representation, independent of the manifest that ships beside it.
///
/// RFC 123 versions the representation separately so a consumer can refuse an interpretation it does not support
/// without refusing the package: a package remains valid for every route that does not need to execute it.
///
/// Bump this whenever the encoded shape changes in a way an older consumer would misread. A change that only adds an
/// optional field a decoder can ignore does not need a bump; a change to an existing field's meaning or position
/// does, because a consumer has no way to detect it.
pub const EXECUTABLE_REPRESENTATION_VERSION: u32 = 1;

/// One module's checked representation, framed with the version needed to interpret it.
///
/// The version travels inside the encoded bytes rather than beside them, so a representation separated from its
/// manifest still says what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodedModuleRepresentation {
    /// Encoded-shape version, checked before the module is interpreted.
    pub version: u32,
    /// The checked module this representation carries.
    pub module: BodyIrModule,
}

/// Why a representation could not be interpreted.
///
/// Every variant names what was unusable rather than reporting a generic decode failure, because RFC 123 requires a
/// consumer to refuse "in terms of the package and version it could not use". The package name is supplied by the
/// consumer that resolved it; this layer owns the version and the reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableRepresentationError {
    /// The representation was produced by a version this consumer does not implement.
    ///
    /// This is the expected refusal, not a corruption: a newer package encountering an older consumer. It carries
    /// both versions so the consumer can say what it would need in order to proceed.
    #[error(
        "executable representation is version {found}, but this compiler implements version {supported}; the package must be consumed by a compiler that implements version {found} or later"
    )]
    UnsupportedVersion {
        /// Version the representation declares.
        found: u32,
        /// Version this compiler implements.
        supported: u32,
    },
    /// The bytes were not a representation this decoder could parse at all.
    ///
    /// Distinct from an unsupported version: that is a representation this consumer declines to interpret, while this
    /// is one it could not read far enough to identify. A consumer must not treat this as an absent representation.
    #[error("executable representation could not be decoded: {reason}")]
    Malformed {
        /// Decoder's account of what it could not read.
        reason: String,
    },
    /// The surface is valid and simply does not cover this declaration.
    ///
    /// RFC 123 permits partial coverage, so this is a supported state of a working package, not a defect. It must
    /// never reach a user as an unsupported language construct: the package exports the declaration, and this route
    /// cannot execute it.
    #[error("this package publishes no executable representation for `{declaration}`")]
    DeclarationNotCovered {
        /// Declaration the consumer asked for.
        declaration: String,
    },
}

/// Encode one checked module as the representation a package publishes.
///
/// The version is stamped from this compiler, because a representation is produced once by the compilation that
/// declares the surface. Nothing re-encodes or re-versions it downstream.
pub fn encode_module(module: &BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let framed = EncodedModuleRepresentation {
        version: EXECUTABLE_REPRESENTATION_VERSION,
        module: module.clone(),
    };
    postcard::to_allocvec(&framed).map_err(|error| ExecutableRepresentationError::Malformed {
        reason: format!("could not encode the checked module: {error}"),
    })
}

/// Decode a published representation, refusing a version this compiler does not implement.
///
/// The version is checked before the module is used, so an unfamiliar representation is refused rather than
/// partially interpreted. A decode that succeeds has produced the same checked module the declaring compilation
/// encoded; there is no reconstruction step in which a consumer could arrive at a different answer.
pub fn decode_module(bytes: &[u8]) -> Result<BodyIrModule, ExecutableRepresentationError> {
    let framed: EncodedModuleRepresentation =
        postcard::from_bytes(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not decode the framed representation: {error}"),
        })?;
    if framed.version != EXECUTABLE_REPRESENTATION_VERSION {
        return Err(ExecutableRepresentationError::UnsupportedVersion {
            found: framed.version,
            supported: EXECUTABLE_REPRESENTATION_VERSION,
        });
    }
    Ok(framed.module)
}

/// Read the version a representation declares without interpreting the module it carries.
///
/// A consumer resolving dependencies needs to know whether it can execute a package before it commits to using it.
/// RFC 123 requires that requirement to be settled at resolution time rather than at the first call, and settling it
/// must not mean decoding a surface the consumer may then refuse.
pub fn representation_version(bytes: &[u8]) -> Result<u32, ExecutableRepresentationError> {
    // Postcard encodes struct fields in declaration order, so the leading varint is the version and reading it does
    // not require the module behind it to be interpretable by this consumer — which is the whole point.
    let (version, _) =
        postcard::take_from_bytes::<u32>(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not read the representation version: {error}"),
        })?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::{
        EXECUTABLE_REPRESENTATION_VERSION, EncodedModuleRepresentation, ExecutableRepresentationError, SurfaceReader,
        build_surface, decode_module, encode_module, representation_version,
    };
    use crate::body_ir::{Block, Body, BodyIrModule, ScopeId};
    use crate::facts::{CompilerNodeKind, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
    use crate::{CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType};

    /// The smallest real module: no declarations, but a genuine identity.
    fn empty_module() -> BodyIrModule {
        BodyIrModule {
            module_id: CompilerNodeId::module("probe"),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: Vec::new(),
        }
    }

    /// A canonical identity for a declaration of this name, in the shape the checker mints for a free function.
    fn identity_for(name: &str) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["probe".to_string()]),
            declaration_name: name.to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 1),
        }
    }

    /// A module carrying one identified, empty body per name.
    fn module_with_named_bodies(names: &[&str]) -> BodyIrModule {
        BodyIrModule {
            bodies: names
                .iter()
                .map(|name| Body {
                    decl_id: CompilerNodeId::new(CompilerNodeKind::Declaration, (*name).to_string()),
                    direct_call_id: CompilerNodeId::new(CompilerNodeKind::Declaration, (*name).to_string()),
                    canonical: Some(identity_for(name)),
                    name: (*name).to_string(),
                    span: HirSourceSpan::new(0, 1),
                    return_type: IncanType::Unknown,
                    locals: Vec::new(),
                    params: Vec::new(),
                    param_locals: Vec::new(),
                    scopes: Vec::new(),
                    block: Block {
                        scope: ScopeId(0),
                        stmts: Vec::new(),
                    },
                    runtime_requirements: Vec::new(),
                    panic_facts: Vec::new(),
                    is_async: false,
                })
                .collect(),
            ..empty_module()
        }
    }

    #[test]
    fn a_module_survives_the_round_trip_exactly() -> Result<(), Box<dyn std::error::Error>> {
        let module = empty_module();
        let decoded = decode_module(&encode_module(&module)?)?;

        assert_eq!(
            decoded, module,
            "a decoded representation must be the module that was encoded, not an equivalent reconstruction of it"
        );
        Ok(())
    }

    #[test]
    fn a_future_version_is_refused_rather_than_misread() -> Result<(), Box<dyn std::error::Error>> {
        let framed = EncodedModuleRepresentation {
            version: EXECUTABLE_REPRESENTATION_VERSION + 1,
            module: empty_module(),
        };
        let bytes = postcard::to_allocvec(&framed)?;

        match decode_module(&bytes) {
            Err(ExecutableRepresentationError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, EXECUTABLE_REPRESENTATION_VERSION + 1);
                assert_eq!(supported, EXECUTABLE_REPRESENTATION_VERSION);
            }
            other => return Err(format!("a newer representation must be refused, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn a_versions_refusal_says_what_would_be_needed() {
        let message = ExecutableRepresentationError::UnsupportedVersion { found: 9, supported: 1 }.to_string();

        assert!(
            message.contains("version 9") && message.contains("version 1"),
            "a refusal must name both versions so a consumer can act on it: {message}"
        );
    }

    #[test]
    fn the_version_is_readable_without_interpreting_the_module() -> Result<(), Box<dyn std::error::Error>> {
        // Resolution-time refusal depends on this: a consumer settles whether it can execute a package before it
        // commits to the package, which it cannot do if reading the version means decoding the surface.
        let bytes = encode_module(&empty_module())?;

        assert_eq!(representation_version(&bytes)?, EXECUTABLE_REPRESENTATION_VERSION);
        Ok(())
    }

    #[test]
    fn a_surface_addresses_one_declaration_without_decoding_the_rest() -> Result<(), Box<dyn std::error::Error>> {
        // The normative claim: "a consumer that calls three declarations of four hundred must not be required to
        // load four hundred". Proven by corrupting every declaration except the one asked for — if opening or
        // reading required the others, this could not succeed.
        let module = module_with_named_bodies(&["alpha", "beta", "gamma"]);
        let bytes = build_surface(&module)?;
        let wanted = identity_for("beta");

        let reader = SurfaceReader::open(&bytes)?;
        let decoded = reader.declaration(&wanted)?;

        assert_eq!(decoded.name, "beta");
        assert_eq!(
            reader.covered_identities().count(),
            3,
            "all three are covered; only one was decoded"
        );
        Ok(())
    }

    #[test]
    fn a_corrupted_neighbour_does_not_prevent_reading_the_declaration_asked_for()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = module_with_named_bodies(&["alpha", "beta"]);
        let mut bytes = build_surface(&module)?;
        let reader = SurfaceReader::open(&bytes)?;
        let alpha = reader.declaration(&identity_for("alpha"))?;
        let alpha_end = postcard::to_allocvec(&alpha)
            .map(|encoded| encoded.len())
            .unwrap_or_default();

        // Wreck the tail, which is where the later declaration lives. A whole-surface decode would now fail.
        let last = bytes.len().saturating_sub(1);
        bytes[last] ^= 0xff;

        let reader = SurfaceReader::open(&bytes)?;
        let recovered = reader.declaration(&identity_for("alpha"))?;
        assert_eq!(
            recovered.name, "alpha",
            "reading one declaration must not depend on its neighbours being intact (alpha occupies {alpha_end} bytes)"
        );
        Ok(())
    }

    #[test]
    fn an_uncovered_declaration_refuses_per_call() -> Result<(), Box<dyn std::error::Error>> {
        // Partial coverage is permitted, so this is a working package that cannot execute one declaration — never
        // an unsupported language construct.
        let bytes = build_surface(&module_with_named_bodies(&["alpha"]))?;
        let reader = SurfaceReader::open(&bytes)?;

        match reader.declaration(&identity_for("absent")) {
            Err(ExecutableRepresentationError::DeclarationNotCovered { declaration }) => {
                assert_eq!(declaration, "absent");
            }
            other => return Err(format!("an uncovered declaration must refuse by name, got {other:?}").into()),
        }
        assert!(!reader.covers(&identity_for("absent")));
        Ok(())
    }

    #[test]
    fn a_declaration_without_a_canonical_identity_is_omitted_rather_than_named()
    -> Result<(), Box<dyn std::error::Error>> {
        // RFC 123 forbids addressing a declaration by spelling. A body the checker minted no identity for is a body
        // no consumer could resolve to, so it is left out rather than keyed by its name.
        let mut module = module_with_named_bodies(&["alpha"]);
        module.bodies[0].canonical = None;
        let bytes = build_surface(&module)?;

        assert_eq!(
            SurfaceReader::open(&bytes)?.covered_identities().count(),
            0,
            "an identity-less body must not be published under any key"
        );
        Ok(())
    }

    #[test]
    fn a_surface_is_byte_identical_for_the_same_module() -> Result<(), Box<dyn std::error::Error>> {
        // A published archive is digested and compared, so the same module must always encode the same way.
        let first = build_surface(&module_with_named_bodies(&["gamma", "alpha", "beta"]))?;
        let second = build_surface(&module_with_named_bodies(&["gamma", "alpha", "beta"]))?;

        assert_eq!(first, second, "the same module must produce the same bytes");
        Ok(())
    }

    #[test]
    fn unreadable_bytes_are_malformed_rather_than_an_unsupported_version() {
        // A consumer must be able to tell "I decline to interpret this" from "this is not a representation", because
        // only the first is a package it could execute with a different compiler.
        match decode_module(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]) {
            Err(ExecutableRepresentationError::Malformed { .. }) => {}
            other => panic!("unreadable bytes must refuse as malformed, got {other:?}"),
        }
    }
}

// ============================================================================
// Identity-addressed surface
// ============================================================================

/// One package module's executable surface, addressed by canonical identity.
///
/// RFC 123 makes addressing normative rather than advisory: "a consumer that calls three declarations of four
/// hundred must not be required to load four hundred". A whole-module encoding cannot honour that — decoding it
/// costs the same whichever declaration you wanted — so the published form is an index and a payload, and a
/// consumer decodes only the declarations it asks for.
///
/// The layout is a postcard-encoded [`SurfaceHeader`] followed by the payload bytes. Postcard decodes a prefix and
/// hands back the remainder, so opening a surface reads the header alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SurfaceHeader {
    /// Encoded-shape version, checked before anything is interpreted.
    version: u32,
    /// Identity of the module this surface belongs to.
    module_id: crate::CompilerNodeId,
    /// Where each covered declaration lives in the payload.
    entries: Vec<SurfaceIndexEntry>,
    /// Type layouts a decoded body may refer to.
    ///
    /// These stay in the header rather than the addressed payload because a body cannot be interpreted without the
    /// declarations it names, so a consumer that decodes any body needs them. They are per-module type layouts
    /// rather than bodies, and are small beside the payload they describe — but that is a size argument, not a
    /// guarantee, and a module with many types and few called declarations pays for it.
    nominal_declarations: Vec<NominalDeclaration>,
    fieldless_enum_declarations: Vec<FieldlessEnumDeclaration>,
    value_enum_declarations: Vec<ValueEnumDeclaration>,
}

/// Where one covered declaration's encoded body lives in the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SurfaceIndexEntry {
    /// Canonical identity this declaration is addressed by. Never a spelling, path, or generated Rust name.
    identity: CanonicalSymbolId,
    /// Byte offset of the encoded body within the payload.
    offset: u64,
    /// Byte length of the encoded body.
    length: u64,
}

/// Build the published surface for one checked module.
///
/// Only declarations the checker minted a canonical identity for are covered. A body without one is omitted rather
/// than addressed by its name: RFC 123 forbids identifying a declaration by spelling, and a body the checker could
/// not give an identity is exactly a body no consumer could resolve to. Coverage is permitted to be partial, so an
/// omission is a supported outcome rather than an error.
pub fn build_surface(module: &BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for body in &module.bodies {
        let Some(identity) = body.canonical.as_ref() else {
            continue;
        };
        let encoded = postcard::to_allocvec(body).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not encode declaration `{}`: {error}", body.name),
        })?;
        entries.push(SurfaceIndexEntry {
            identity: identity.clone(),
            offset: payload.len() as u64,
            length: encoded.len() as u64,
        });
        payload.extend_from_slice(&encoded);
    }
    // Sorted so a surface is byte-identical for the same module regardless of the order lowering happened to emit
    // bodies in, which is what lets a published archive be compared and its digest be meaningful.
    entries.sort_by(|left, right| left.identity.cmp(&right.identity));
    let header = SurfaceHeader {
        version: EXECUTABLE_REPRESENTATION_VERSION,
        module_id: module.module_id.clone(),
        entries,
        nominal_declarations: module.nominal_declarations.clone(),
        fieldless_enum_declarations: module.fieldless_enum_declarations.clone(),
        value_enum_declarations: module.value_enum_declarations.clone(),
    };
    let mut bytes = postcard::to_allocvec(&header).map_err(|error| ExecutableRepresentationError::Malformed {
        reason: format!("could not encode the surface index: {error}"),
    })?;
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// A published surface opened for reading, with its payload still encoded.
///
/// Opening reads the index and nothing else. Each [`SurfaceReader::declaration`] decodes one body from the payload,
/// so the cost of a call is the declaration called rather than the surface it came from.
#[derive(Debug, Clone)]
pub struct SurfaceReader<'bytes> {
    header: SurfaceHeader,
    payload: &'bytes [u8],
}

impl<'bytes> SurfaceReader<'bytes> {
    /// Open a published surface, refusing a version this compiler does not implement.
    ///
    /// The version is checked before the index is trusted, so an unfamiliar surface is refused rather than partially
    /// read.
    pub fn open(bytes: &'bytes [u8]) -> Result<Self, ExecutableRepresentationError> {
        let (header, payload) = postcard::take_from_bytes::<SurfaceHeader>(bytes).map_err(|error| {
            ExecutableRepresentationError::Malformed {
                reason: format!("could not decode the surface index: {error}"),
            }
        })?;
        if header.version != EXECUTABLE_REPRESENTATION_VERSION {
            return Err(ExecutableRepresentationError::UnsupportedVersion {
                found: header.version,
                supported: EXECUTABLE_REPRESENTATION_VERSION,
            });
        }
        Ok(Self { header, payload })
    }

    /// Identity of the module this surface belongs to.
    pub fn module_id(&self) -> &crate::CompilerNodeId {
        &self.header.module_id
    }

    /// Every canonical identity this surface covers, in a stable order.
    ///
    /// A consumer settling a requirement at resolution time reads this rather than decoding declarations.
    pub fn covered_identities(&self) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.header.entries.iter().map(|entry| &entry.identity)
    }

    /// Whether this surface covers one declaration.
    pub fn covers(&self, identity: &CanonicalSymbolId) -> bool {
        self.header.entries.iter().any(|entry| &entry.identity == identity)
    }

    /// Decode one covered declaration, and only that one.
    ///
    /// An identity this surface does not cover refuses per call, which is what RFC 123 requires: coverage is
    /// permitted to be partial, so an uncovered declaration is a supported state of a valid package rather than a
    /// broken one, and it must never surface as an unsupported language construct.
    pub fn declaration(&self, identity: &CanonicalSymbolId) -> Result<Body, ExecutableRepresentationError> {
        let entry = self
            .header
            .entries
            .iter()
            .find(|entry| &entry.identity == identity)
            .ok_or_else(|| ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            })?;
        let start = usize::try_from(entry.offset).map_err(|_| ExecutableRepresentationError::Malformed {
            reason: format!("declaration `{}` has an unreadable offset", identity.declaration_name),
        })?;
        let length = usize::try_from(entry.length).map_err(|_| ExecutableRepresentationError::Malformed {
            reason: format!("declaration `{}` has an unreadable length", identity.declaration_name),
        })?;
        let slice = self.payload.get(start..start.saturating_add(length)).ok_or_else(|| {
            ExecutableRepresentationError::Malformed {
                reason: format!(
                    "declaration `{}` points outside the surface payload",
                    identity.declaration_name
                ),
            }
        })?;
        postcard::from_bytes(slice).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not decode declaration `{}`: {error}", identity.declaration_name),
        })
    }

    /// Type layouts a decoded body may refer to, rebuilt as the module container a consumer already understands.
    ///
    /// The bodies are deliberately absent: this is the surrounding context for declarations the consumer chooses to
    /// decode, not an invitation to load them all.
    pub fn declaration_context(&self) -> BodyIrModule {
        BodyIrModule {
            module_id: self.header.module_id.clone(),
            nominal_declarations: self.header.nominal_declarations.clone(),
            fieldless_enum_declarations: self.header.fieldless_enum_declarations.clone(),
            value_enum_declarations: self.header.value_enum_declarations.clone(),
            bodies: Vec::new(),
        }
    }
}
