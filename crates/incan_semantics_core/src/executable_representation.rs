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

use crate::body_ir::BodyIrModule;

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
        EXECUTABLE_REPRESENTATION_VERSION, EncodedModuleRepresentation, ExecutableRepresentationError, decode_module,
        encode_module, representation_version,
    };
    use crate::CompilerNodeId;
    use crate::body_ir::BodyIrModule;

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
    fn unreadable_bytes_are_malformed_rather_than_an_unsupported_version() {
        // A consumer must be able to tell "I decline to interpret this" from "this is not a representation", because
        // only the first is a package it could execute with a different compiler.
        match decode_module(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]) {
            Err(ExecutableRepresentationError::Malformed { .. }) => {}
            other => panic!("unreadable bytes must refuse as malformed, got {other:?}"),
        }
    }
}
