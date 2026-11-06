//! Resolve a program's package execution requirements from the selected artifact graph before any effects.
//!
//! Consumer dependency aliases identify graph edges. Canonical origins identify declaring packages. Public facades
//! therefore resolve to their original provider, including a transitive provider, without synthesizing source bodies.
//! Artifact authenticity belongs to the package boundary. Local admission checks the representation version, the
//! manifest descriptor's version, the index bounds, and public membership and coverage, then streams the
//! manifest-selected content digest last. That order is deliberate and the opposite of what a trust boundary
//! usually suggests: the cheap structural refusals run before the whole file is read, and nothing parsed ahead of
//! the digest is acted on, because the digest must verify before any declaration executes. Only required fragments
//! are decoded.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{
    DeclarationCoverage, ExecutableDeclaration, ExecutableRepresentationError, SurfaceIndex, require_supported_version,
};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, SymbolOrigin, canonical_module_identity};

use crate::library_manifest::LibraryManifest;
use crate::library_manifest::published_layout::{executable_surface_path, public_executable_identities};
use crate::library_manifest_index::LibraryArtifactMetadata;
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

/// Authenticated declaration records and selective-loading evidence. Executable routes retain full public closure;
/// linked SDK numeric constants may retain only their immutable value and exact layout.
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
    /// Admitted name, version, content digest and feature projection; canonical package names alone cannot select an
    /// instance.
    provider_key: String,
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

/// Resolve every statically required package call and its published public closure before execution begins.
///
/// All versions, coverage records and manifest memberships are validated before returning. No body is executed, no
/// source is consulted, and each selected fragment is decoded once even when recursion or facades reach it again.
/// Requirements reached through a public package may name an active SDK provider; they use that provider's admitted
/// artifact with the same coverage and content checks. Conflicting instances refuse rather than replacing one another.
pub fn resolve_executable_requirements(
    plan: &ProviderPlan,
    required: &BTreeSet<CanonicalSymbolId>,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    if required.is_empty() {
        return Ok(ResolvedExecutableModules::default());
    }
    resolve_from_catalog(&executable_catalog(plan)?, required, false)
}

/// Load available checked publication context without demanding executable coverage from every referenced type.
///
/// An authenticated explicitly uncovered root contributes no facts. This is useful for ordinary native-linked
/// library publication, whose signatures may name linking-only types. Covered roots still require their complete
/// executable closure, and malformed artifacts, missing public membership and digest mismatches remain errors.
/// Actual execution must use [`resolve_executable_requirements`] to require every selected root.
pub fn resolve_available_executable_context(
    plan: &ProviderPlan,
    requested: &BTreeSet<CanonicalSymbolId>,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    if requested.is_empty() {
        return Ok(ResolvedExecutableModules::default());
    }
    resolve_from_catalog(&executable_catalog(plan)?, requested, true)
}

/// Resolve published records of active SDK packages, which the native route links as crates but still needs as checked
/// declarations, such as a stdlib model's field defaults. The same version, coverage and digest checks apply; nothing
/// is executed and no source is consulted.
pub fn resolve_sdk_requirements(
    plan: &ProviderPlan,
    required: &BTreeSet<CanonicalSymbolId>,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    if required.is_empty() {
        return Ok(ResolvedExecutableModules::default());
    }
    let catalog = sdk_catalog(plan)?;
    if let Some(value) = resolve_linked_numeric_constant(&catalog, required)? {
        return Ok(value);
    }
    resolve_from_catalog(&catalog, required, false)
}

/// Load an immutable numeric newtype value for a linked SDK without requesting unrelated method bodies.
/// The root must be public and covered, its only dependency must be the same-package constructor owner, and
/// the authenticated owner payload must exactly match the constant's complete physical layout and binding.
/// Other values retain ordinary executable-closure resolution; no execution route uses this declaration-only path.
fn resolve_linked_numeric_constant(
    catalog: &BTreeMap<String, PackageArtifact>,
    required: &BTreeSet<CanonicalSymbolId>,
) -> Result<Option<ResolvedExecutableModules>, ExecutableResolutionError> {
    use incan_semantics_core::SemanticSourceTargetKind;
    use incan_semantics_core::body_ir::Constant;
    if required.len() != 1 {
        return Ok(None);
    }
    let Some(identity) = required.first() else {
        return Ok(None);
    };
    if identity.kind != SemanticSourceTargetKind::Const {
        return Ok(None);
    }
    let SymbolOrigin::Package { library, .. } = &identity.origin else {
        return Ok(None);
    };
    let Some(package) = catalog.get(library) else {
        return Ok(None);
    };
    let mut surface = OpenSurface::open(package, identity)?;
    let DeclarationCoverage::Covered {
        offset,
        length,
        requirements,
    } = surface
        .index
        .coverage(identity)
        .map_err(|error| package.unusable(error))?
    else {
        return Ok(None);
    };
    let (offset, length, dependencies) = (*offset, *length, requirements.clone());
    let declaration = surface.read(package, identity, offset, length)?;
    let ExecutableDeclaration::Constant(constant) = &declaration else {
        return Ok(None);
    };
    let Constant::NumericNewtype(value) = &constant.value else {
        return Ok(None);
    };
    let Some(owner) = value.constructor.canonical.as_ref() else {
        return Ok(None);
    };
    if !matches!(&owner.origin, SymbolOrigin::Package { library: origin, .. } if origin == library)
        || dependencies.len() != 1
        || !dependencies.contains(owner)
    {
        return Ok(None);
    }
    let DeclarationCoverage::Covered {
        offset: owner_offset,
        length: owner_length,
        ..
    } = surface
        .index
        .execution_coverage(owner)
        .map_err(|error| package.unusable(error))?
    else {
        return Ok(None);
    };
    let (owner_offset, owner_length) = (*owner_offset, *owner_length);
    let owner_declaration = surface.read(package, owner, owner_offset, owner_length)?;
    let layout = match &owner_declaration {
        ExecutableDeclaration::Nominal(layout) | ExecutableDeclaration::NominalWithTraits(layout, _) => layout,
        _ => return Ok(None),
    };
    if !value.matches_layout(layout) {
        return Err(package.unusable(malformed(
            "numeric SDK constant disagrees with its authenticated layout",
        )));
    }
    let owner = owner.clone();
    let mut modules = BTreeMap::new();
    for (identity, declaration) in [(identity, declaration), (&owner, owner_declaration)] {
        let path = canonical_module_identity(identity)
            .ok_or_else(|| package.unusable(malformed("numeric SDK constant has no module owner")))?;
        retain_declaration(
            modules.entry(path.clone()).or_insert_with(|| empty_module(path)),
            declaration,
        );
    }
    Ok(Some(ResolvedExecutableModules {
        modules: modules.into_values().collect(),
        decoded_declarations: 2,
        payload_bytes_read: length + owner_length,
        content_bytes_verified: surface.content_bytes_verified,
        package_versions: BTreeMap::from([(library.clone(), package.manifest.version.clone())]),
    }))
}

/// Join only the ordinary and active SDK artifacts already admitted by the compilation's provider plan.
fn executable_catalog(plan: &ProviderPlan) -> Result<BTreeMap<String, PackageArtifact>, ExecutableResolutionError> {
    let mut catalog = package_catalog(plan)?;
    for artifact in sdk_catalog(plan)?.into_values() {
        insert_artifact(&mut catalog, artifact)?;
    }
    Ok(catalog)
}

/// Preserve one admitted instance per canonical package name, rejecting different versions, digests or feature
/// projections.
fn insert_artifact(
    catalog: &mut BTreeMap<String, PackageArtifact>,
    artifact: PackageArtifact,
) -> Result<(), ExecutableResolutionError> {
    match catalog.entry(artifact.manifest.name.clone()) {
        Entry::Vacant(entry) => {
            entry.insert(artifact);
        }
        Entry::Occupied(entry) if entry.get().provider_key != artifact.provider_key => {
            return Err(ExecutableResolutionError::AmbiguousPackage {
                library: artifact.manifest.name,
                versions: format!("{}, {}", entry.get().provider_key, artifact.provider_key),
            });
        }
        Entry::Occupied(_) => {}
    }
    Ok(())
}

/// Load every required declaration and its published closure from one package catalog.
fn resolve_from_catalog(
    catalog: &BTreeMap<String, PackageArtifact>,
    required: &BTreeSet<CanonicalSymbolId>,
    optional_roots: bool,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    let mut opened = BTreeMap::new();
    let mut pending: BTreeSet<_> = required
        .iter()
        .cloned()
        .map(|identity| (identity, false, optional_roots))
        .collect();
    let mut visited = BTreeSet::new();
    let mut modules: BTreeMap<String, BodyIrModule> = BTreeMap::new();
    let mut resolved = ResolvedExecutableModules::default();
    while let Some((identity, implementation_allowed, optional_root)) = pending.pop_first() {
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
        // A checked module binding selects an admitted package namespace; it has no executable body. Calls and
        // type uses retain their own canonical requirements and must still resolve from the selected artifact.
        if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Module {
            continue;
        }
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
        // Every incoming edge is authorized before deduplication. An explicit private root must refuse even if
        // another public root already loaded that implementation through a same-package dependency.
        let coverage = if implementation_allowed {
            surface.index.execution_coverage(&identity)
        } else {
            surface.index.coverage(&identity)
        }
        .map_err(|error| package.unusable(error))?;
        // Context discovery omits only an explicitly uncovered requested root after ordinary admission. Do not
        // mark it visited: a later dependency edge to the same declaration must still require executable coverage.
        if optional_root && matches!(coverage, DeclarationCoverage::Uncovered(_)) {
            continue;
        }
        if !visited.insert(identity.clone()) {
            continue;
        }
        if let DeclarationCoverage::TypeContext { owner } = coverage {
            pending.insert((owner.clone(), true, false));
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
        pending.extend(requirements.iter().map(|dependency| {
            let same_package =
                matches!(&dependency.origin, SymbolOrigin::Package { library: owner, .. } if owner == library);
            (dependency.clone(), same_package, false)
        }));
        let (offset, length) = (*offset, *length);
        let declaration = surface.read(package, &identity, offset, length)?;
        let owner = canonical_module_identity(&identity)
            .ok_or_else(|| package.unusable(malformed("declaration has no module owner")))?;
        let module = modules.entry(owner.clone()).or_insert_with(|| empty_module(owner));
        retain_declaration(module, declaration);
        resolved.decoded_declarations += 1;
        resolved.payload_bytes_read += length;
    }
    for module in modules.values_mut() {
        // Canonical lookup is name-first; module initialization follows checked physical declaration order.
        module
            .static_declarations
            .sort_by_key(|storage| storage.canonical.declaration_span.start);
    }
    resolved.modules = modules.into_values().collect();
    Ok(resolved)
}

/// Assemble only selected fragments under their checked canonical module owner, without source reconstruction.
fn empty_module(owner: String) -> BodyIrModule {
    BodyIrModule {
        string_conversion_contract: incan_semantics_core::body_ir::StringConversionContract::Unselected,
        decorated_bindings: Vec::new(),
        entrypoint_effects: Vec::new(),
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        trait_declarations: Vec::new(),
        trait_implementations: Vec::new(),
        module_id: CompilerNodeId::module(owner),
        stdlib_delegations: Vec::new(),
        native_union_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        static_declarations: Vec::new(),
        nominal_declarations: Vec::new(),
        fieldless_enum_declarations: Vec::new(),
        value_enum_declarations: Vec::new(),
        bodies: Vec::new(),
    }
}

/// Retain the selected payload's original declaration kind and associated checked trait or enum context.
fn retain_declaration(module: &mut BodyIrModule, declaration: ExecutableDeclaration) {
    match declaration {
        ExecutableDeclaration::Body(body) => {
            module.bodies.push(*body);
        }
        ExecutableDeclaration::DecoratedBody { body, binding, storage } => {
            module.bodies.push(*body);
            module.decorated_bindings.push(*binding);
            module.static_declarations.push(*storage);
        }
        ExecutableDeclaration::InvocationDecoratedBody { body, binding } => {
            module.bodies.push(*body);
            module.decorated_bindings.push(*binding);
        }
        ExecutableDeclaration::Nominal(value) => module.nominal_declarations.push(value),
        ExecutableDeclaration::FieldlessEnum(value) => module.fieldless_enum_declarations.push(value),
        ExecutableDeclaration::ValueEnum(value) => module.value_enum_declarations.push(value),
        ExecutableDeclaration::ValueEnumWithLayout(value, layout) => {
            module.value_enum_declarations.push(value);
            module.enum_declarations.push(layout);
        }
        ExecutableDeclaration::Enum(value) => module.enum_declarations.push(value),
        ExecutableDeclaration::Trait(value) => module.trait_declarations.push(value),
        ExecutableDeclaration::Constant(value) => module.constant_declarations.push(value),
        ExecutableDeclaration::TypeAlias(value) => module.type_alias_declarations.push(value),
        ExecutableDeclaration::Static(value) => module.static_declarations.push(value),
        ExecutableDeclaration::NominalWithTraits(value, implementations) => {
            module.nominal_declarations.push(value);
            module.trait_implementations.extend(implementations);
        }
    }
}
/// Select materialized active SDK artifacts without collapsing conflicting instances of a package name.
fn sdk_catalog(plan: &ProviderPlan) -> Result<BTreeMap<String, PackageArtifact>, ExecutableResolutionError> {
    let mut catalog = BTreeMap::new();
    for provider in plan.active_sdk_records() {
        let (Some(manifest), Some(metadata)) = (provider.manifest.as_deref(), provider.artifact.as_ref()) else {
            continue;
        };
        insert_artifact(
            &mut catalog,
            PackageArtifact {
                provider_key: provider.identity.stable_key(),
                manifest: manifest.clone(),
                metadata: metadata.clone(),
            },
        )?;
    }
    Ok(catalog)
}

/// Select from the shared provider plan's admitted artifacts; dependency edges are not traversed a second time.
fn package_catalog(plan: &ProviderPlan) -> Result<BTreeMap<String, PackageArtifact>, ExecutableResolutionError> {
    let mut catalog = BTreeMap::new();
    for package in plan.public_artifacts() {
        insert_artifact(
            &mut catalog,
            PackageArtifact {
                provider_key: package.identity.stable_key(),
                manifest: (*package.manifest).clone(),
                metadata: package.artifact.clone(),
            },
        )?;
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
        // The frame is written by `encode_surface` as a postcard-encoded `u32` version, then an 8-byte
        // little-endian index length, then the index, then the payload. `SurfaceReader` parses that same frame
        // from a slice with `postcard::take_from_bytes`; this reader cannot, because it streams from a file and
        // must know where the version ends before it can seek.
        //
        // So this loop reimplements postcard's varint framing: continuation bit 0x80, at most five bytes for a
        // `u32`. That is a coupling to a third-party wire format, and it is the reason the two readers are pinned
        // against each other by test rather than left to drift -- see the paired test in the crate that owns the
        // frame. If postcard ever changes its integer encoding, this loop is what breaks, and it breaks only on
        // real artifacts, because every in-memory test goes through the other reader.
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
        if index_length > incan_semantics_core::executable_representation::MAX_SURFACE_INDEX_BYTES
            || payload_offset > file_length
        {
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
        // The manifest's public surface includes identities this package only re-exports. Admission compares
        // against what this package itself declares, so a facade cannot widen the set a consumer may execute.
        let public = public_executable_identities(&package.manifest)
            .into_iter()
            .filter(|identity| {
                matches!(
                    &identity.origin,
                    SymbolOrigin::Package { library, .. } if library == &package.manifest.name
                )
            })
            .collect::<BTreeSet<_>>();
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
            content_bytes_verified +=
                u64::try_from(length).map_err(|_| package.unusable(malformed("read length exceeds wire range")))?;
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
