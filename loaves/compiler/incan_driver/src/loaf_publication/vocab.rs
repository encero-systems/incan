//! Validate the producer's versioned Incan registration at the ordinary library publication boundary.

use serde::Deserialize;

use crate::error::{CliError, CliResult};
use crate::unit::UnitDescription;
use incan_frontend::library_manifest::VocabExports;

/// The existing registration DTO accompanied by its producer identity and entry module.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedRegistration {
    /// Project-relative vocabulary entry module, retained under the legacy descriptor field spelling.
    crate_path: String,
    /// The Loaf publishing these descriptors.
    package_name: String,
    /// RFC 124 identity of the producer's complete native registration unit.
    unit_identity: String,
    /// Descriptors computed by the producer's native Incan entry.
    metadata: incan_vocab::VocabMetadata,
}

/// Read a prepared registration without running any producer code in the compiler or its consumers.
/// The published manifest retains the supported schema and validates descriptor contents through its normal reader.
pub(crate) fn prepared_registration(unit: &UnitDescription) -> CliResult<Option<VocabExports>> {
    let Some(path) = &unit.vocab else {
        return Ok(None);
    };
    let bytes = std::fs::read(path)
        .map_err(|error| CliError::failure(format!("cannot read vocabulary {}: {error}", path.display())))?;
    let prepared: PreparedRegistration = serde_json::from_slice(&bytes)
        .map_err(|error| CliError::failure(format!("invalid vocabulary {}: {error}", path.display())))?;
    let version = prepared.metadata.metadata_version;
    if version != incan_vocab::VOCAB_METADATA_VERSION {
        return Err(CliError::failure(format!(
            "vocabulary schema version {version}; supported version {}",
            incan_vocab::VOCAB_METADATA_VERSION,
        )));
    }
    if prepared.package_name != unit.package {
        return Err(CliError::failure("vocabulary registration belongs to another Loaf"));
    }
    if prepared.unit_identity.len() != 64 || !prepared.unit_identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::failure("vocabulary unit identity must be a SHA-256 digest"));
    }
    if prepared.metadata.desugarer.is_some() {
        return Err(CliError::failure(
            "native custom desugarer publication is not available yet",
        ));
    }
    Ok(Some(VocabExports {
        metadata_version: version,
        unit_identity: prepared.unit_identity,
        crate_path: prepared.crate_path,
        package_name: prepared.package_name,
        keyword_registrations: prepared.metadata.keyword_registrations,
        dsl_surfaces: prepared.metadata.dsl_surfaces,
        provider_manifest: prepared.metadata.library_manifest,
        desugarer_artifact: None,
    }))
}
