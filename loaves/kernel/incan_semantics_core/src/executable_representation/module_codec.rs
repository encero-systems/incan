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
