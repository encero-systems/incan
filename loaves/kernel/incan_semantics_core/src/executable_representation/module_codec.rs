//! Module encoding and decoding, representation versions, and trait field bindings.

use super::*;

/// Encode an entire checked module for internal round-trip diagnostics.
///
/// Package publication uses [`build_surface`], which projects public coverage and its required execution closure.
/// This whole-module codec deliberately retains every checked fact for compiler regression tests.
pub fn encode_module(module: &BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    /// The borrowed twin of [`EncodedModuleRepresentation`]: postcard encodes a struct field by field, so this
    /// serializes byte-for-byte like the owned frame without copying the module first.
    #[derive(Serialize)]
    struct BorrowedFrame<'module> {
        version: u32,
        module: &'module BodyIrModule,
    }
    let framed = BorrowedFrame {
        version: EXECUTABLE_REPRESENTATION_VERSION,
        module,
    };
    let mut bytes = postcard::to_allocvec(&framed).map_err(|error| ExecutableRepresentationError::Malformed {
        reason: format!("could not encode the checked module: {error}"),
    })?;
    append_trait_fields(&mut bytes, &module.nominal_declarations)?;
    Ok(bytes)
}

/// Decode an internal whole-module snapshot, refusing an unfamiliar version before decoding its contents.
///
/// The version is checked before the module is used, so an unfamiliar representation is refused rather than
/// partially interpreted. A decode that succeeds has produced the same checked module the declaring compilation
/// encoded; there is no reconstruction step in which a consumer could arrive at a different answer.
pub fn decode_module(bytes: &[u8]) -> Result<BodyIrModule, ExecutableRepresentationError> {
    require_supported_version(bytes)?;
    let (mut framed, remainder): (EncodedModuleRepresentation, _) =
        postcard::take_from_bytes(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not decode the framed representation: {error}"),
        })?;
    restore_trait_fields(&mut framed.module.nominal_declarations, remainder)?;
    Ok(framed.module)
}

/// Encode one checked executable owner with its module and logical-source provenance.
///
/// This is the durable value for per-owner Body IR records. It deliberately serializes the kernel's native [`Body`]
/// contract rather than projecting it through an independently maintained compiler-record model.
pub fn encode_owner(
    module: &CompilerNodeId,
    source: &str,
    identity: &str,
    body: &Body,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    /// Postcard encodes the borrowed fields identically to [`EncodedOwnerRepresentation`] without cloning the body.
    #[derive(Serialize)]
    struct BorrowedOwnerFrame<'owner> {
        version: u32,
        module: &'owner CompilerNodeId,
        source: &'owner str,
        identity: &'owner str,
        body: &'owner Body,
    }
    postcard::to_allocvec(&BorrowedOwnerFrame {
        version: EXECUTABLE_REPRESENTATION_VERSION,
        module,
        source,
        identity,
        body,
    })
    .map_err(|error| ExecutableRepresentationError::Malformed {
        reason: format!("could not encode the executable owner: {error}"),
    })
}

const PHYSICAL_INPUTS: &[u8] = b"incan.physical-body/1\0";

const OWNER_COORDINATES: &[u8] = b"incan.body-coordinates/1\0";
/// Bound optional relocation work independently of an executable owner's supported size.
pub const MAX_OWNER_COORDINATES: usize = 128;

/// Append a bounded coordinate map to the native physical input stamp. The graph's optional call receipt follows it.
pub fn append_owner_coordinates(encoded: &mut Vec<u8>, coordinates: &[(u32, u32)]) {
    if coordinates.is_empty() || coordinates.len() > MAX_OWNER_COORDINATES {
        return;
    }
    encoded.extend_from_slice(OWNER_COORDINATES);
    encoded.push(coordinates.len() as u8);
    for &(start, end) in coordinates {
        encoded.extend_from_slice(&start.to_le_bytes());
        encoded.extend_from_slice(&end.to_le_bytes());
    }
}

/// Encode a checked graph owner together with the producing frontend's exact physical-input witness.
pub fn encode_graph_owner(
    module: &CompilerNodeId,
    source: &str,
    identity: &str,
    body: &Body,
    physical_inputs: Option<[u8; 32]>,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let mut encoded = encode_owner(module, source, identity, body)?;
    // These invocation-private fields are deliberately absent from the persisted Body. Such a frame
    // remains usable by graph readers but cannot replace the live executable representation.
    if let Some(inputs) = physical_inputs
        && body.nominal_instantiation.is_empty()
        && body.trait_default_instantiation.is_none()
        && body.native_attributes.is_empty()
    {
        encoded.extend_from_slice(PHYSICAL_INPUTS);
        encoded.extend_from_slice(&inputs);
    }
    Ok(encoded)
}

/// Decode one checked executable owner, refusing an unfamiliar version before exposing its Body IR.
pub fn decode_owner(bytes: &[u8]) -> Result<EncodedOwnerRepresentation, ExecutableRepresentationError> {
    require_supported_version(bytes)?;
    let (mut framed, remainder): (EncodedOwnerRepresentation, _) =
        postcard::take_from_bytes(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not decode the executable owner: {error}"),
        })?;
    let mut trailing = remainder;
    if let Some(physical) = trailing.strip_prefix(PHYSICAL_INPUTS) {
        let (digest, rest) =
            physical
                .split_first_chunk::<32>()
                .ok_or_else(|| ExecutableRepresentationError::Malformed {
                    reason: "executable owner has a truncated physical input frame".into(),
                })?;
        framed.physical_inputs = Some(*digest);
        trailing = rest;
    }
    if let Some(encoded) = trailing.strip_prefix(OWNER_COORDINATES) {
        let invalid = || ExecutableRepresentationError::Malformed {
            reason: "invalid native body coordinates".into(),
        };
        let (&count, bytes) = encoded.split_first().ok_or_else(invalid)?;
        let count = usize::from(count);
        if framed.physical_inputs.is_none() || count == 0 || count > MAX_OWNER_COORDINATES {
            return Err(invalid());
        }
        let (coordinates, rest) = bytes.split_at_checked(count * 8).ok_or_else(invalid)?;
        for pair in coordinates.chunks_exact(8) {
            let (start, end) = pair.split_at(4);
            let start = u32::from_le_bytes(start.try_into().map_err(|_| invalid())?);
            let end = u32::from_le_bytes(end.try_into().map_err(|_| invalid())?);
            if start > end {
                return Err(invalid());
            }
            framed.coordinates.push((start, end));
        }
        trailing = rest;
    }
    if !trailing.is_empty() {
        if !trailing.starts_with(b"CRMB\x01\x01\x00") {
            return Err(ExecutableRepresentationError::Malformed {
                reason: "executable owner has an unrecognized graph attachment".into(),
            });
        }
        framed.call_witness = Some(trailing.to_vec());
    }
    Ok(framed)
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

/// Reject an unfamiliar encoding from its stable leading version, before decoding any version-specific fields.
pub fn require_supported_version(bytes: &[u8]) -> Result<(), ExecutableRepresentationError> {
    let found = representation_version(bytes)?;
    if !supports_version(found) {
        return Err(ExecutableRepresentationError::UnsupportedVersion {
            found,
            supported: EXECUTABLE_REPRESENTATION_VERSION,
        });
    }
    Ok(())
}

/// Accept the current positional schema and the oldest version whose layout it still decodes unchanged.
pub fn supports_version(version: u32) -> bool {
    (OLDEST_SUPPORTED_VERSION..=EXECUTABLE_REPRESENTATION_VERSION).contains(&version)
}

/// Append canonical required-field witnesses without changing any positional nominal or adoption field.
pub(super) fn append_trait_fields(
    bytes: &mut Vec<u8>,
    nominals: &[NominalDeclaration],
) -> Result<(), ExecutableRepresentationError> {
    let mut bindings = Vec::new();
    for nominal in nominals {
        for adoption in &nominal.trait_adoptions {
            if !adoption.fields.is_empty() {
                bindings.push(TraitFieldBindings {
                    owner: nominal.canonical.clone(),
                    trait_owner: adoption.trait_identity.clone(),
                    fields: adoption.fields.clone(),
                });
            }
        }
    }
    if !bindings.is_empty() {
        bindings.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.trait_owner.cmp(&right.trait_owner))
        });
        bytes.extend(postcard::to_allocvec(&bindings).map_err(|error| malformed(error.to_string()))?);
    }
    Ok(())
}

/// Restore only fully bounded, owner-matched field witnesses; legacy fragments retain no such authority.
pub(super) fn restore_trait_fields(
    nominals: &mut [NominalDeclaration],
    bytes: &[u8],
) -> Result<(), ExecutableRepresentationError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let (bindings, remainder): (Vec<TraitFieldBindings>, _) =
        postcard::take_from_bytes(bytes).map_err(|error| malformed(error.to_string()))?;
    if bindings.is_empty() || !remainder.is_empty() {
        return Err(malformed("invalid required-field witness trailer"));
    }
    for binding in bindings {
        let nominal = nominals
            .iter_mut()
            .find(|nominal| nominal.canonical == binding.owner)
            .ok_or_else(|| malformed("required-field witness has no nominal owner"))?;
        let mut requirements = BTreeSet::new();
        for field in &binding.fields {
            if field.requirement.namespace != crate::SymbolNamespace::Member
                || field.requirement.kind != crate::SemanticSourceTargetKind::Field
                || field.requirement.declaration_name != field.implementation.declaration_name
                || !nominal.field_identities.contains(&field.implementation)
                || !requirements.insert(&field.requirement)
            {
                return Err(malformed("required-field witness differs from the checked layout"));
            }
        }
        let adoption = nominal
            .trait_adoptions
            .iter_mut()
            .find(|adoption| adoption.trait_identity == binding.trait_owner)
            .ok_or_else(|| malformed("required-field witness has no trait adoption"))?;
        if binding.fields.is_empty() || !adoption.fields.is_empty() {
            return Err(malformed("empty or duplicate required-field witness"));
        }
        adoption.fields = binding.fields;
    }
    Ok(())
}
