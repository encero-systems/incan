//! Installation authenticates the manifest-selected sidecar once. Its sealed records bind each selectable
//! declaration to its authorization, requirements and exact bytes (#2481, #2946).
use std::collections::BTreeSet;

use super::executable_surface::own_public_identities;
use super::{ExecutableRepresentationExport, LibraryManifest};
use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, representation_version};
use incan_semantics_core::{CanonicalSymbolId, encode_incan_symbol_identity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const HEAD: &str = "executable-range-set";
pub(crate) const RANGE: &str = "executable-range";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RangeHead {
    pub package: String,
    pub version: String,
    pub manifest_digest: String,
    pub descriptor: ExecutableRepresentationExport,
    #[serde(default)]
    pub header_digest: String,
    pub file_bytes: u64,
    pub payload_offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RangeRecord {
    pub surface_digest: String,
    pub identity: CanonicalSymbolId,
    pub public: bool,
    pub coverage: DeclarationCoverage,
    pub fragment_digest: Option<String>,
}

/// These families always use ordinal zero: the stable, unique selector is the qualified identity itself.
pub(crate) fn record_key(family: &str, member: &str) -> String {
    format!("{family}/00000000/{:x}", Sha256::digest(member.as_bytes()))
}

/// Project a verified sidecar as independent installed values. An ordinary package still uses the sidecar directly.
pub fn export_ranges(
    manifest: &LibraryManifest,
    manifest_digest: &str,
    bytes: &[u8],
) -> Result<Vec<(String, Vec<u8>)>, Box<dyn std::error::Error>> {
    let descriptor = manifest
        .contract_metadata
        .executable_representation
        .as_ref()
        .ok_or("missing executable representation descriptor")?;
    if format!("{:x}", Sha256::digest(bytes)) != descriptor.content_digest
        || representation_version(bytes)? != descriptor.representation_version
    {
        return Err("executable bytes or version differ from the selected manifest descriptor".into());
    }
    let reader = SurfaceReader::open(bytes)?;
    let index = reader.index();
    if index.library != manifest.name
        || index.package_version != manifest.version
        || index.declarations.keys().cloned().collect::<BTreeSet<_>>() != own_public_identities(manifest)
    {
        return Err("executable package, version or published membership differs from manifest".into());
    }
    let payload = reader.payload();
    let head = RangeHead {
        package: manifest.name.clone(),
        version: manifest.version.clone(),
        manifest_digest: manifest_digest.to_owned(),
        descriptor: descriptor.clone(),
        header_digest: format!("{:x}", Sha256::digest(&bytes[..bytes.len() - payload.len()])),
        file_bytes: u64::try_from(bytes.len())?,
        payload_offset: u64::try_from(bytes.len() - payload.len())?,
    };
    let mut records = vec![(
        record_key(HEAD, &manifest.name),
        LibraryManifest::encode_compiler_record_value(&head)?,
    )];
    for (public, declarations) in [(true, &index.declarations), (false, &index.implementation)] {
        for (identity, coverage) in declarations {
            let fragment_digest = match coverage {
                DeclarationCoverage::Covered { offset, length, .. } => {
                    let start = usize::try_from(*offset)?;
                    let end = start
                        .checked_add(usize::try_from(*length)?)
                        .ok_or("fragment range overflow")?;
                    Some(format!(
                        "{:x}",
                        Sha256::digest(payload.get(start..end).ok_or("fragment outside sidecar")?)
                    ))
                }
                _ => None,
            };
            let record = RangeRecord {
                surface_digest: descriptor.content_digest.clone(),
                identity: identity.clone(),
                public,
                coverage: coverage.clone(),
                fragment_digest,
            };
            records.push((
                record_key(RANGE, &encode_incan_symbol_identity(identity)),
                LibraryManifest::encode_compiler_record_value(&record)?,
            ));
        }
    }
    Ok(records)
}
