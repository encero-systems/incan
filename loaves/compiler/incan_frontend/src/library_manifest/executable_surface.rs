//! What executable resolution reads of one Loaf (RFC 179): its name and version, its executable descriptor and its
//! own public executable identities. The installed compiler records publish the descriptor and the identities as
//! records of their own, so resolving a reached Loaf never decodes its whole contract metadata.

use std::collections::BTreeSet;

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use super::public_identities::public_executable_identities;
use super::{ExecutableRepresentationExport, LibraryManifest};

/// One Loaf's executable surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableSurface {
    pub name: String,
    pub version: String,
    /// The executable descriptor the manifest selects, when the Loaf published an executable representation.
    pub descriptor: Option<ExecutableRepresentationExport>,
    /// The public executable identities this Loaf itself declares; a re-exported identity belongs to its owner.
    pub public_identities: BTreeSet<CanonicalSymbolId>,
}

impl ExecutableSurface {
    /// Derive the surface from a complete or executable-projection manifest.
    pub fn of(manifest: &LibraryManifest) -> Self {
        Self {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            descriptor: manifest.contract_metadata.executable_representation.clone(),
            public_identities: own_public_identities(manifest),
        }
    }
}

/// The public executable identities `manifest` itself declares.
pub fn own_public_identities(manifest: &LibraryManifest) -> BTreeSet<CanonicalSymbolId> {
    public_executable_identities(manifest)
        .into_iter()
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == &manifest.name),
        )
        .collect()
}
