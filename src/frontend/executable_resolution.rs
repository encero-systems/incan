//! Resolve a program's package execution requirements from the selected artifact graph before any effects.
//!
//! Consumer dependency aliases identify graph edges. Canonical origins identify declaring packages. Public facades
//! therefore resolve to their original provider, including a transitive provider, without synthesizing source bodies.
//! Artifact authenticity belongs to the package boundary. Local admission verifies the manifest-selected content
//! digest, then checks versions, public membership and coverage. Only required fragments are decoded.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::executable_representation::{
    DeclarationCoverage, ExecutableDeclaration, ExecutableRepresentationError, SurfaceIndex, require_supported_version,
};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, SymbolOrigin, canonical_module_identity};

use crate::frontend::library_manifest_index::LibraryArtifactMetadata;
use crate::library_manifest::LibraryManifest;
use crate::library_manifest::published_layout::{executable_surface_path, public_executable_identities};
use crate::provider::ProviderPlan;

/// A package requirement that cannot be satisfied before execution. No variant authorizes a fallback route.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableResolutionError {
    /// Local declarations execute from the current compilation, never from a synthesized package representation.
    #[error("`{declaration}` is not a package declaration")]
    NotAPackageDeclaration { declaration: String },
    /// The canonical provider is absent from the selected direct and transitive artifact graph.
    #[error("package `{library}` is not a resolved dependency of this compilation")]
    UnknownPackage { library: String },
    /// Canonical identity cannot distinguish conflicting resolved instances, so choosing one would be unsound.
    #[error(
        "package `{library}` has conflicting resolved artifacts ({versions}); executable resolution requires one unambiguous package instance"
    )]
    AmbiguousPackage { library: String, versions: String },
    /// A selected package has no semantic publication; linking-only routes remain valid.
    #[error(
        "package `{library}` version {version} publishes no executable representation, which this route requires to execute `{declaration}`"
    )]
    NoPublishedRepresentation {
        library: String,
        version: String,
        declaration: String,
    },
    /// A version, coverage or structural contract is unusable for this execution route.
    #[error("package `{library}` version {version} cannot satisfy the executable representation requirement: {source}")]
    UnusableRepresentation {
        library: String,
        version: String,
        source: ExecutableRepresentationError,
    },
    /// Artifact I/O failed; the package version is retained even when no representation bytes could be read.
    #[error("package `{library}` version {version} cannot read required executable artifact {path}: {reason}")]
    Unreadable {
        library: String,
        version: String,
        path: PathBuf,
        reason: String,
    },
    /// A transitive manifest edge failed the artifact graph's existing identity/integrity validation.
    #[error("package `{library}` version {version} has an unusable required dependency artifact: {reason}")]
    DependencyArtifact {
        library: String,
        version: String,
        reason: String,
    },
}

/// Fully loaded public execution closure and direct evidence of selective fragment loading.
#[derive(Debug, Default)]
pub struct ResolvedExecutableModules {
    /// Module-owned bodies and only the required public nominal context.
    pub modules: Vec<BodyIrModule>,
    /// Number of declaration payloads decoded, including separately addressed type context.
    pub decoded_declarations: usize,
    /// Number of encoded payload bytes read; index bytes and archive-integrity verification are separate costs.
    pub payload_bytes_read: u64,
    /// Bytes streamed through content verification, separate from selected payload decoding.
    pub content_bytes_verified: u64,
    /// Package versions bound to the returned module identities for preflight refusal diagnostics.
    pub package_versions: BTreeMap<String, String>,
}

/// A resolved provider instance, selected by the compilation's dependency edges and checked artifact context.
struct PackageArtifact {
    manifest: LibraryManifest,
    metadata: LibraryArtifactMetadata,
}

impl PackageArtifact {
    /// Attach package identity to a codec error without relabeling it as a language limitation.
    fn unusable(&self, source: ExecutableRepresentationError) -> ExecutableResolutionError {
        ExecutableResolutionError::UnusableRepresentation {
            library: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            source,
        }
    }

    /// Attach package identity and artifact location to an I/O failure.
    fn unreadable(&self, path: PathBuf, error: impl ToString) -> ExecutableResolutionError {
        ExecutableResolutionError::Unreadable {
            library: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            path,
            reason: error.to_string(),
        }
    }
}

/// Resolve one callable through the same complete admission path used by the CLI graph.
pub fn resolve_executable_declaration(
    plan: &ProviderPlan,
    identity: &CanonicalSymbolId,
) -> Result<Body, ExecutableResolutionError> {
    let resolved = resolve_executable_requirements(plan, &BTreeSet::from([identity.clone()]))?;
    resolved
        .modules
        .into_iter()
        .flat_map(|module| module.bodies)
        .find(|body| body.canonical.as_ref() == Some(identity))
        .ok_or_else(|| ExecutableResolutionError::NotAPackageDeclaration {
            declaration: identity.declaration_name.clone(),
        })
}

/// Resolve every statically required package call and its published public closure before execution begins.
///
/// All versions, coverage records and manifest memberships are validated before returning. No body is executed, no
/// source is consulted, and each selected fragment is decoded once even when recursion or facades reach it again.
pub fn resolve_executable_requirements(
    plan: &ProviderPlan,
    required: &BTreeSet<CanonicalSymbolId>,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    if required.is_empty() {
        return Ok(ResolvedExecutableModules::default());
    }
    let catalog = package_catalog(plan)?;
    let mut opened = BTreeMap::new();
    let mut pending = required.clone();
    let mut visited = BTreeSet::new();
    let mut modules: BTreeMap<String, BodyIrModule> = BTreeMap::new();
    let mut resolved = ResolvedExecutableModules::default();
    while let Some(identity) = pending.pop_first() {
        if !visited.insert(identity.clone()) {
            continue;
        }
        let SymbolOrigin::Package { library, .. } = &identity.origin else {
            return Err(ExecutableResolutionError::NotAPackageDeclaration {
                declaration: identity.declaration_name.clone(),
            });
        };
        let package = catalog
            .get(library)
            .ok_or_else(|| ExecutableResolutionError::UnknownPackage {
                library: library.clone(),
            })?;
        if !opened.contains_key(library) {
            let surface = OpenSurface::open(package, &identity)?;
            resolved.content_bytes_verified += surface.content_bytes_verified;
            opened.insert(library.clone(), surface);
        }
        resolved
            .package_versions
            .insert(library.clone(), package.manifest.version.clone());
        let surface = opened
            .get_mut(library)
            .ok_or_else(|| package.unusable(malformed("opened surface missing")))?;
        let coverage = surface
            .index
            .coverage(&identity)
            .map_err(|error| package.unusable(error))?;
        if let DeclarationCoverage::TypeContext { owner } = coverage {
            pending.insert(owner.clone());
            continue;
        }
        let DeclarationCoverage::Covered {
            offset,
            length,
            requirements,
        } = coverage
        else {
            return Err(package.unusable(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            }));
        };
        pending.extend(requirements.iter().cloned());
        let (offset, length) = (*offset, *length);
        let declaration = surface.read(package, &identity, offset, length)?;
        let owner = canonical_module_identity(&identity)
            .ok_or_else(|| package.unusable(malformed("declaration has no module owner")))?;
        let module = modules.entry(owner.clone()).or_insert_with(|| BodyIrModule {
            module_id: CompilerNodeId::module(owner),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: Vec::new(),
        });
        match declaration {
            ExecutableDeclaration::Body(body) => {
                module.bodies.push(body);
            }
            ExecutableDeclaration::Nominal(value) => module.nominal_declarations.push(value),
            ExecutableDeclaration::FieldlessEnum(value) => module.fieldless_enum_declarations.push(value),
            ExecutableDeclaration::ValueEnum(value) => module.value_enum_declarations.push(value),
        }
        resolved.decoded_declarations += 1;
        resolved.payload_bytes_read += length;
    }
    resolved.modules = modules.into_values().collect();
    Ok(resolved)
}

/// Select from the shared provider plan's admitted artifacts; dependency edges are not traversed a second time.
fn package_catalog(plan: &ProviderPlan) -> Result<BTreeMap<String, PackageArtifact>, ExecutableResolutionError> {
    let mut identities = BTreeMap::new();
    let mut catalog = BTreeMap::new();
    for package in plan.public_artifacts() {
        if let Some(previous) = identities.insert(package.identity.name.clone(), package.identity.stable_key())
            && previous != package.identity.stable_key()
        {
            return Err(ExecutableResolutionError::AmbiguousPackage {
                library: package.identity.name.clone(),
                versions: format!("{previous}, {}", package.identity.stable_key()),
            });
        }
        catalog.insert(
            package.identity.name.clone(),
            PackageArtifact {
                manifest: (*package.manifest).clone(),
                metadata: package.artifact.clone(),
            },
        );
    }
    Ok(catalog)
}

/// File handle and already-decoded metadata; executable payloads remain on disk until their identity is selected.
struct OpenSurface {
    file: File,
    path: PathBuf,
    payload_offset: u64,
    index: SurfaceIndex,
    content_bytes_verified: u64,
}

impl OpenSurface {
    /// Accept only the stable version prefix before reading a bounded index, then compare it with the manifest.
    fn open(package: &PackageArtifact, requested: &CanonicalSymbolId) -> Result<Self, ExecutableResolutionError> {
        let Some(path) = executable_surface_path(&package.metadata.manifest_path, &package.manifest) else {
            return Err(ExecutableResolutionError::NoPublishedRepresentation {
                library: package.manifest.name.clone(),
                version: package.manifest.version.clone(),
                declaration: requested.declaration_name.clone(),
            });
        };
        let mut file = File::open(&path).map_err(|error| package.unreadable(path.clone(), error))?;
        let mut version = Vec::new();
        for _ in 0..5 {
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte)
                .map_err(|error| package.unreadable(path.clone(), error))?;
            version.push(byte[0]);
            if byte[0] & 128 == 0 {
                break;
            }
        }
        require_supported_version(&version).map_err(|error| package.unusable(error))?;
        let descriptor = package
            .manifest
            .contract_metadata
            .executable_representation
            .as_ref()
            .ok_or_else(|| package.unusable(malformed("manifest descriptor disappeared")))?;
        if descriptor.representation_version
            != incan_semantics_core::executable_representation::EXECUTABLE_REPRESENTATION_VERSION
        {
            return Err(package.unusable(ExecutableRepresentationError::UnsupportedVersion {
                found: descriptor.representation_version,
                supported: incan_semantics_core::executable_representation::EXECUTABLE_REPRESENTATION_VERSION,
            }));
        }
        let mut length = [0u8; 8];
        file.read_exact(&mut length)
            .map_err(|error| package.unreadable(path.clone(), error))?;
        let index_length = u64::from_le_bytes(length);
        let prefix_length = u64::try_from(version.len())
            .map_err(|_| package.unusable(malformed("version length exceeds host range")))?
            + 8;
        let payload_offset = prefix_length
            .checked_add(index_length)
            .ok_or_else(|| package.unusable(malformed("index length overflow")))?;
        let file_length = file
            .metadata()
            .map_err(|error| package.unreadable(path.clone(), error))?
            .len();
        if index_length > 16 * 1024 * 1024 || payload_offset > file_length {
            return Err(package.unusable(malformed("index length exceeds its bounded envelope")));
        }
        let mut bytes = vec![
            0;
            usize::try_from(index_length)
                .map_err(|_| package.unusable(malformed("index exceeds host range")))?
        ];
        file.read_exact(&mut bytes)
            .map_err(|error| package.unreadable(path.clone(), error))?;
        let index =
            SurfaceIndex::decode(&bytes, file_length - payload_offset).map_err(|error| package.unusable(error))?;
        if index.library != package.manifest.name || index.package_version != package.manifest.version {
            return Err(package.unusable(malformed(
                "representation package identity/version differs from manifest",
            )));
        }
        let public = public_executable_identities(&package.manifest).into_iter().filter(|identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == &package.manifest.name)).collect::<BTreeSet<_>>();
        if index.declarations.keys().cloned().collect::<BTreeSet<_>>() != public {
            return Err(package.unusable(malformed(
                "declared representation coverage differs from the manifest public surface",
            )));
        }
        // A descriptor selects immutable publication content; it does not authenticate a package. Use the same open
        // handle for verification and later range reads, without decoding unrelated declaration payloads.
        file.seek(SeekFrom::Start(0))
            .map_err(|error| package.unreadable(path.clone(), error))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut content_bytes_verified = 0;
        loop {
            let length = file
                .read(&mut buffer)
                .map_err(|error| package.unreadable(path.clone(), error))?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
            content_bytes_verified += length as u64;
        }
        if hex::encode(digest.finalize()) != descriptor.content_digest {
            return Err(package.unusable(malformed(
                "executable content differs from its manifest-selected digest",
            )));
        }
        Ok(Self {
            file,
            path,
            payload_offset,
            index,
            content_bytes_verified,
        })
    }

    /// Range-read one selected fragment and validate its payload identity against its checked index key.
    fn read(
        &mut self,
        package: &PackageArtifact,
        identity: &CanonicalSymbolId,
        offset: u64,
        length: u64,
    ) -> Result<ExecutableDeclaration, ExecutableResolutionError> {
        let absolute = self
            .payload_offset
            .checked_add(offset)
            .ok_or_else(|| package.unusable(malformed("fragment offset overflow")))?;
        self.file
            .seek(SeekFrom::Start(absolute))
            .map_err(|error| package.unreadable(self.path.clone(), error))?;
        let mut bytes = vec![
            0;
            usize::try_from(length)
                .map_err(|_| package.unusable(malformed("fragment exceeds host address range")))?
        ];
        self.file
            .read_exact(&mut bytes)
            .map_err(|error| package.unreadable(self.path.clone(), error))?;
        self.index
            .decode_declaration(identity, &bytes)
            .map_err(|error| package.unusable(error))
    }
}

/// Preserve structural artifact failure without exposing a backend's unsupported-source diagnostic category.
fn malformed(reason: impl Into<String>) -> ExecutableRepresentationError {
    ExecutableRepresentationError::Malformed { reason: reason.into() }
}

#[cfg(test)]
mod tests;
