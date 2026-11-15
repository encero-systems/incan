//! What executable resolution and native lowering read of one Loaf (RFC 179): its name and version, its executable
//! descriptor, its own public executable identities and its runtime callable bindings. The installed compiler records
//! publish the descriptor, the identities and the bindings as records of their own, so resolving a reached Loaf or a
//! lowered runtime call never decodes its whole contract metadata.

use std::collections::BTreeSet;

use incan_semantics_core::body_ir::RuntimeCallableBinding;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

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
        .filter(|identity| owned(manifest, identity))
        .collect()
}

/// The runtime callables `manifest` binds for native lowering: every export with a two-segment public path that names
/// a function the Loaf itself declares, as its public path and canonical identity.
pub fn runtime_bindings(manifest: &LibraryManifest) -> Vec<RuntimeCallableBinding> {
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|export| export.public_path.len() == 2)
        .filter_map(|export| {
            Some(RuntimeCallableBinding {
                path: export.public_path.join("::"),
                canonical: export.canonical.as_ref()?.hydrate()?,
            })
        })
        .filter(|binding| {
            binding.canonical.kind == SemanticSourceTargetKind::Function && owned(manifest, &binding.canonical)
        })
        .collect()
}

/// Whether `manifest`'s Loaf itself declares `identity`.
fn owned(manifest: &LibraryManifest, identity: &CanonicalSymbolId) -> bool {
    matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == &manifest.name)
}
