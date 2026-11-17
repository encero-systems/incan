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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use incan_semantics_core::body_ir::{BodyIrModule, LoafCallable};
use incan_semantics_core::executable_representation::{
    DeclarationCoverage, ExecutableDeclaration, ExecutableRepresentationError, SurfaceIndex, require_supported_version,
};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin, canonical_module_identity};

use crate::library_manifest::executable_ranges::{RangeHead, RangeRecord};
use crate::library_manifest::executable_surface::ExecutableSurface;
use crate::loaf_index::{LoafIndex, LoafIndexError};

/// Bound temporary storage while verifying the selected executable artifact's content digest.
const CONTENT_DIGEST_BUFFER_BYTES: usize = 64 * 1024;

/// A package requirement that cannot be satisfied before execution. No variant authorizes a fallback route.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableResolutionError {
    /// A demanded manifest could not be read or changed since its inventory was admitted.
    #[error(transparent)]
    Artifact(#[from] LoafIndexError),
    /// Local declarations execute from the current compilation, never from a synthesized package representation.
    #[error("`{declaration}` is not a package declaration")]
    NotAPackageDeclaration { declaration: String },
    /// The canonical provider is absent from the selected direct and transitive artifact graph.
    #[error(
        "package `{library}` is not a resolved dependency of this compilation; required declaration: {}; dependency chain: {}",
        declaration.render_compact(), render_dependency_chain(chain)
    )]
    UnknownPackage {
        library: String,
        declaration: Box<CanonicalSymbolId>,
        chain: Vec<CanonicalSymbolId>,
    },
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
    /// Why each reached package is reached: for every declaration that entered it from a requested root or from
    /// another package, its requester chain from that root. A package selected only by an explicitly uncovered root
    /// has no entry.
    pub entries: BTreeMap<String, Vec<Vec<CanonicalSymbolId>>>,
}

/// One indexed Loaf's executable surface and governed-body file.
struct PackageArtifact {
    key: String,
    range_head: Option<RangeHead>,
    surface: Arc<ExecutableSurface>,
    /// The Loaf's `.incnbody`.
    bodies: PathBuf,
}

impl PackageArtifact {
    /// Attach package identity to a codec error without relabeling it as a language limitation.
    fn unusable(&self, source: ExecutableRepresentationError) -> ExecutableResolutionError {
        ExecutableResolutionError::UnusableRepresentation {
            library: self.surface.name.clone(),
            version: self.surface.version.clone(),
            source,
        }
    }

    /// Attach package identity and artifact location to an I/O failure.
    fn unreadable(&self, path: PathBuf, error: impl ToString) -> ExecutableResolutionError {
        ExecutableResolutionError::Unreadable {
            library: self.surface.name.clone(),
            version: self.surface.version.clone(),
            path,
            reason: error.to_string(),
        }
    }
}

/// Load the declaration records and governed bodies the program reaches in its Loaves (DD-0007).
///
/// Every requested root and its published closure is read from the declaring Loaf's `.incnbody`, decoded once even
/// when recursion or facades reach it again. An explicitly uncovered root contributes no facts; malformed artifacts,
/// missing public membership and digest mismatches are errors. Exact public identities already supplied by the
/// checked producer satisfy returning dependency edges without demanding an older artifact of the same package.
/// Nothing executes and no additional source is consulted.
pub fn resolve_loaf_declarations(
    loaves: &LoafIndex,
    requested: &BTreeSet<CanonicalSymbolId>,
    checked_local: &BTreeSet<CanonicalSymbolId>,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    if requested.is_empty() {
        return Ok(ResolvedExecutableModules::default());
    }
    let resolved = resolve_from_catalog(loaves, &catalog(loaves)?, requested, checked_local, true)?;
    loaves.validate_demands()?;
    Ok(resolved)
}

/// One artifact per package name across the index; two instances of one package refuse rather than replacing each
/// other.
fn catalog(loaves: &LoafIndex) -> Result<BTreeMap<String, String>, ExecutableResolutionError> {
    let mut catalog = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for (key, identity) in loaves.identities() {
        let stable = identity.stable_key();
        if let Some(previous) = identities.insert(&identity.name, stable.clone())
            && previous != stable
        {
            return Err(ExecutableResolutionError::AmbiguousPackage {
                library: identity.name.clone(),
                versions: format!("{previous}, {stable}"),
            });
        }
        catalog.entry(identity.name.clone()).or_insert_with(|| key.to_owned());
    }
    Ok(catalog)
}

/// Demand one manifest from the already disambiguated inventory catalog, retaining the same artifact for all edges.
fn demand_package<'a>(
    loaves: &LoafIndex,
    catalog: &BTreeMap<String, String>,
    packages: &'a mut BTreeMap<String, PackageArtifact>,
    library: &str,
    declaration: &CanonicalSymbolId,
    parents: &BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
) -> Result<&'a PackageArtifact, ExecutableResolutionError> {
    let slot = match packages.entry(library.to_owned()) {
        Entry::Occupied(slot) => return Ok(slot.into_mut()),
        Entry::Vacant(slot) => slot,
    };
    let unknown = || ExecutableResolutionError::UnknownPackage {
        library: library.to_owned(),
        declaration: Box::new(declaration.clone()),
        chain: dependency_chain(declaration, parents),
    };
    let key = catalog.get(library).ok_or_else(unknown)?;
    let (surface, bodies, range_head) = loaves.executable_surface_artifact(key)?.ok_or_else(unknown)?;
    Ok(slot.insert(PackageArtifact {
        key: key.clone(),
        surface,
        bodies,
        range_head,
    }))
}

/// Load every required declaration and its published closure from one package catalog.
fn resolve_from_catalog(
    loaves: &LoafIndex,
    catalog: &BTreeMap<String, String>,
    required: &BTreeSet<CanonicalSymbolId>,
    checked_local: &BTreeSet<CanonicalSymbolId>,
    optional_roots: bool,
) -> Result<ResolvedExecutableModules, ExecutableResolutionError> {
    let mut opened = BTreeMap::new();
    let mut packages = BTreeMap::new();
    let mut pending: BTreeSet<_> = required
        .iter()
        .cloned()
        .map(|identity| (identity, false, optional_roots))
        .collect();
    let mut parents = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut modules: BTreeMap<String, BodyIrModule> = BTreeMap::new();
    let mut resolved = ResolvedExecutableModules::default();
    while let Some((identity, implementation_allowed, optional_root)) = pending.pop_first() {
        if checked_local.contains(&identity) {
            continue;
        }
        let SymbolOrigin::Package { library, .. } = &identity.origin else {
            return Err(ExecutableResolutionError::NotAPackageDeclaration {
                declaration: identity.declaration_name.clone(),
            });
        };
        let package = demand_package(loaves, catalog, &mut packages, library, &identity, &parents)?;
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
            .insert(library.clone(), package.surface.version.clone());
        let surface = opened
            .get_mut(library)
            .ok_or_else(|| package.unusable(malformed("opened surface missing")))?;
        // Every incoming edge is authorized before deduplication. An explicit private root must refuse even if
        // another public root already loaded that implementation through a same-package dependency.
        surface.admit_range(loaves, package, &identity)?;
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
            retain_dependency_parent(&mut parents, required, owner, &identity);
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
            retain_dependency_parent(&mut parents, required, dependency, &identity);
            let same_package =
                matches!(&dependency.origin, SymbolOrigin::Package { library: owner, .. } if owner == library);
            (dependency.clone(), same_package, false)
        }));
        let (offset, length) = (*offset, *length);
        let declaration = surface.read(package, &identity, offset, length)?;
        if let ExecutableDeclaration::NominalWithTraits(_, implementations)
        | ExecutableDeclaration::NominalWithCallables { implementations, .. } = &declaration
        {
            // Lazy defaults are declaration requirements, even when the consumer never spells their trait. An
            // uncovered unused default contributes no body, just as an uncovered source-adoption root does.
            pending.extend(
                implementations
                    .iter()
                    .filter(|implementation| implementation.implementation == implementation.method)
                    .map(|implementation| {
                        retain_dependency_parent(&mut parents, required, &implementation.method, &identity);
                        (implementation.method.clone(), false, true)
                    }),
            );
        }
        let owner = canonical_module_identity(&identity)
            .ok_or_else(|| package.unusable(malformed("declaration has no module owner")))?;
        let module = modules
            .entry(owner.clone())
            .or_insert_with(|| BodyIrModule::declarations_only(owner));
        retain_declaration(module, declaration);
        resolved.decoded_declarations += 1;
        resolved.payload_bytes_read += length;
    }
    resolved.entries = package_entries(&resolved.package_versions, &visited, &parents);
    for module in modules.values_mut() {
        crate::body_ir::retain_published_module(module);
        // Canonical lookup is name-first; module initialization follows checked physical declaration order.
        module
            .static_declarations
            .sort_by_key(|storage| storage.canonical.declaration_span.start);
    }
    resolved.modules = modules.into_values().collect();
    Ok(resolved)
}

/// Preserve the first deterministic incoming edge without turning an explicitly selected root into a child.
fn retain_dependency_parent(
    parents: &mut BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
    roots: &BTreeSet<CanonicalSymbolId>,
    dependency: &CanonicalSymbolId,
    parent: &CanonicalSymbolId,
) {
    if !roots.contains(dependency) {
        parents.entry(dependency.clone()).or_insert_with(|| parent.clone());
    }
}

/// Group each loaded declaration that a root or another package requested under its package, with its requester
/// chain. Same-package edges stay inside the entry that reached them.
fn package_entries(
    packages: &BTreeMap<String, String>,
    visited: &BTreeSet<CanonicalSymbolId>,
    parents: &BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
) -> BTreeMap<String, Vec<Vec<CanonicalSymbolId>>> {
    let mut entries: BTreeMap<_, Vec<_>> = packages.keys().map(|library| (library.clone(), Vec::new())).collect();
    for identity in visited {
        let SymbolOrigin::Package { library, .. } = &identity.origin else {
            continue;
        };
        let crosses = parents.get(identity).is_none_or(
            |parent| !matches!(&parent.origin, SymbolOrigin::Package { library: owner, .. } if owner == library),
        );
        if crosses && let Some(entry) = entries.get_mut(library) {
            entry.push(dependency_chain(identity, parents));
        }
    }
    entries
}

/// Recover canonical requester ancestry; a cycle in declaration requirements stops at its first repeated identity.
fn dependency_chain(
    declaration: &CanonicalSymbolId,
    parents: &BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
) -> Vec<CanonicalSymbolId> {
    let mut chain = vec![declaration.clone()];
    let mut seen = BTreeSet::from([declaration.clone()]);
    let mut current = declaration;
    while let Some(parent) = parents.get(current) {
        if !seen.insert(parent.clone()) {
            break;
        }
        chain.push(parent.clone());
        current = parent;
    }
    chain.reverse();
    chain
}

/// Format canonical identities for diagnostics without assigning imported declaration offsets to a source file.
fn render_dependency_chain(chain: &[CanonicalSymbolId]) -> String {
    chain
        .iter()
        .map(CanonicalSymbolId::render_compact)
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Retain the selected payload's original declaration kind and associated checked trait or enum context.
fn retain_declaration(module: &mut BodyIrModule, declaration: ExecutableDeclaration) {
    match declaration {
        ExecutableDeclaration::Body(body) => {
            module.bodies.push(*body);
        }
        ExecutableDeclaration::CompiledCallableFrame { mut frame, callables } => {
            crate::body_ir::retain_published_frame(&mut frame);
            if let Some(callable) = incan_semantics_core::body_ir::LoafCallable::from_body(&frame) {
                module.loaf_callables.push(callable);
            }
            retain_native_callables(module, callables);
        }
        ExecutableDeclaration::NativeBody { body, callables } => {
            module.bodies.push(*body);
            retain_native_callables(module, callables);
        }
        ExecutableDeclaration::GenericDecoratedBody {
            body,
            binding,
            callables,
        } => {
            module.bodies.push(*body);
            module.decorated_bindings.push(*binding);
            retain_native_callables(module, callables);
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
        ExecutableDeclaration::NominalWithCallables {
            nominal,
            implementations,
            callables,
        } => {
            module.nominal_declarations.push(nominal);
            module.trait_implementations.extend(implementations);
            retain_native_callables(module, callables);
        }
    }
}
/// Merge authenticated native call selections without duplicating an already-retained ABI declaration.
fn retain_native_callables(module: &mut BodyIrModule, callables: Vec<LoafCallable>) {
    for callable in callables {
        if !module.loaf_callables.contains(&callable) {
            module.loaf_callables.push(callable);
        }
    }
}

/// File handle and already-decoded metadata; executable payloads remain on disk until their identity is selected.
struct OpenSurface {
    file: File,
    path: PathBuf,
    payload_offset: u64,
    index: SurfaceIndex,
    fragment_digests: BTreeMap<CanonicalSymbolId, String>,
    content_bytes_verified: u64,
}

impl OpenSurface {
    /// Accept only the stable version prefix before reading a bounded index, then compare it with the manifest.
    fn open(package: &PackageArtifact, requested: &CanonicalSymbolId) -> Result<Self, ExecutableResolutionError> {
        let path = package.bodies.clone();
        if package.surface.descriptor.is_none() || !path.is_file() {
            return Err(ExecutableResolutionError::NoPublishedRepresentation {
                library: package.surface.name.clone(),
                version: package.surface.version.clone(),
                declaration: requested.declaration_name.clone(),
            });
        }
        let mut file = File::open(&path).map_err(|error| package.unreadable(path.clone(), error))?;
        if let Some(head) = &package.range_head {
            let file_length = file
                .metadata()
                .map_err(|error| package.unreadable(path.clone(), error))?
                .len();
            if file_length != head.file_bytes {
                return Err(package.unusable(malformed(
                    "installed executable file length differs from its range head",
                )));
            }
            tracing::trace!(
                target: "incan::invocation", phase = "body_surface_open", path = %path.display(),
                loaf = %package.surface.name, digest = %head.descriptor.content_digest,
                proof = "sealed-range", header_bytes = 0u64, verified_bytes = 0u64, payload_offset = head.payload_offset,
            );
            return Ok(Self {
                file,
                path,
                payload_offset: head.payload_offset,
                index: SurfaceIndex {
                    library: head.package.clone(),
                    package_version: head.version.clone(),
                    declarations: BTreeMap::new(),
                    implementation: BTreeMap::new(),
                },
                fragment_digests: BTreeMap::new(),
                content_bytes_verified: 0,
            });
        }
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
            .surface
            .descriptor
            .as_ref()
            .ok_or_else(|| package.unusable(malformed("manifest descriptor disappeared")))?;
        if !incan_semantics_core::executable_representation::supports_version(descriptor.representation_version) {
            return Err(package.unusable(ExecutableRepresentationError::UnsupportedVersion {
                found: descriptor.representation_version,
                supported: incan_semantics_core::executable_representation::EXECUTABLE_REPRESENTATION_VERSION,
            }));
        }
        let encoded_version = incan_semantics_core::executable_representation::representation_version(&version)
            .map_err(|error| package.unusable(error))?;
        if descriptor.representation_version != encoded_version {
            return Err(package.unusable(malformed("manifest and executable representation versions differ")));
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
        if index.library != package.surface.name || index.package_version != package.surface.version {
            return Err(package.unusable(malformed(
                "representation package identity/version differs from manifest",
            )));
        }
        // The manifest's public surface includes identities this package only re-exports. Admission compares
        // against what this package itself declares, so a facade cannot widen the set a consumer may execute.
        let public = &package.surface.public_identities;
        if index.declarations.keys().cloned().collect::<BTreeSet<_>>() != *public {
            return Err(package.unusable(malformed(
                "declared representation coverage differs from the manifest public surface",
            )));
        }
        let content_bytes_verified =
            Self::verify_content_digest(&mut file, &path, package, &descriptor.content_digest)?;
        tracing::trace!(
            target: "incan::invocation", phase = "body_surface_open", path = %path.display(),
            loaf = %package.surface.name, digest = %descriptor.content_digest,
            header_bytes = payload_offset, verified_bytes = content_bytes_verified,
        );
        Ok(Self {
            file,
            path,
            payload_offset,
            index,
            fragment_digests: BTreeMap::new(),
            content_bytes_verified,
        })
    }

    /// Verify immutable publication content on the handle later used for range reads, without decoding payloads.
    ///
    /// The digest binds the manifest-selected bytes; it does not authenticate the package. Successful verification
    /// leaves the handle at EOF and reports the number of bytes read for the resolution evidence.
    fn verify_content_digest(
        file: &mut File,
        path: &Path,
        package: &PackageArtifact,
        expected_digest: &str,
    ) -> Result<u64, ExecutableResolutionError> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| package.unreadable(path.to_path_buf(), error))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; CONTENT_DIGEST_BUFFER_BYTES];
        let mut content_bytes_verified = 0;
        loop {
            let length = file
                .read(&mut buffer)
                .map_err(|error| package.unreadable(path.to_path_buf(), error))?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
            content_bytes_verified +=
                u64::try_from(length).map_err(|_| package.unusable(malformed("read length exceeds wire range")))?;
        }
        if hex::encode(digest.finalize()) != expected_digest {
            return Err(package.unusable(malformed(
                "executable content differs from its manifest-selected digest",
            )));
        }
        Ok(content_bytes_verified)
    }

    /// Import only this declaration's independently authenticated coverage into the in-memory index. Public
    /// authorization is still evaluated for every incoming edge before visited-node deduplication.
    fn admit_range(
        &mut self,
        loaves: &LoafIndex,
        package: &PackageArtifact,
        identity: &CanonicalSymbolId,
    ) -> Result<(), ExecutableResolutionError> {
        let Some(head) = &package.range_head else { return Ok(()) };
        if self.index.declarations.contains_key(identity) || self.index.implementation.contains_key(identity) {
            return Ok(());
        }
        let record = loaves.executable_range(&package.key, identity)?.ok_or_else(|| {
            package.unusable(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            })
        })?;
        validate_range(head, identity, &record).map_err(|error| package.unusable(error))?;
        if let Some(digest) = record.fragment_digest {
            self.fragment_digests.insert(identity.clone(), digest);
        }
        let members = if record.public {
            &mut self.index.declarations
        } else {
            &mut self.index.implementation
        };
        members.insert(identity.clone(), record.coverage);
        Ok(())
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
        if package.range_head.is_some() {
            let digest = self
                .fragment_digests
                .get(identity)
                .ok_or_else(|| package.unusable(malformed("sealed fragment has no digest")))?;
            verify_fragment(&bytes, digest).map_err(|error| package.unusable(error))?;
        }
        tracing::trace!(
            target: "incan::invocation", phase = "body_fragment_read", path = %self.path.display(),
            loaf = %package.surface.name, identity = %incan_semantics_core::encode_incan_symbol_identity(identity),
            offset, absolute_offset = absolute, bytes = length, range_authenticated = package.range_head.is_some(),
        );
        self.index
            .decode_declaration(identity, &bytes)
            .map_err(|error| package.unusable(error))
    }
}

/// A sealed membership must describe this package's exact immutable surface and the selected canonical identity.
fn validate_range(
    head: &RangeHead,
    identity: &CanonicalSymbolId,
    record: &RangeRecord,
) -> Result<(), ExecutableRepresentationError> {
    if record.identity != *identity || record.surface_digest != head.descriptor.content_digest {
        return Err(malformed(
            "selected range names another declaration or executable surface",
        ));
    }
    match (&record.coverage, &record.fragment_digest) {
        (DeclarationCoverage::Covered { offset, length, .. }, Some(digest))
            if digest.len() == 64
                && digest.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                && head
                    .payload_offset
                    .checked_add(*offset)
                    .and_then(|start| start.checked_add(*length))
                    .is_some_and(|end| end <= head.file_bytes) =>
        {
            Ok(())
        }
        (DeclarationCoverage::TypeContext { .. } | DeclarationCoverage::Uncovered(_), None) => Ok(()),
        _ => Err(malformed("sealed coverage has an invalid address or digest")),
    }
}

/// Hash the same selected buffer the decoder consumes, including when the sidecar was replaced after packing.
fn verify_fragment(bytes: &[u8], expected: &str) -> Result<(), ExecutableRepresentationError> {
    if hex::encode(Sha256::digest(bytes)) != expected {
        return Err(malformed("selected fragment differs from its sealed digest"));
    }
    Ok(())
}

/// Preserve structural artifact failure without exposing a backend's unsupported-source diagnostic category.
fn malformed(reason: impl Into<String>) -> ExecutableRepresentationError {
    ExecutableRepresentationError::Malformed { reason: reason.into() }
}
#[cfg(test)]
mod tests {
    use super::{ExecutableResolutionError, dependency_chain, package_entries, resolve_loaf_declarations};
    use crate::loaf_index::LoafIndex;
    use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin};
    use std::collections::{BTreeMap, BTreeSet};

    /// Construct a governed identity without source files, so diagnostics must retain its actual package owner.
    fn declaration(package: &str, name: &str) -> CanonicalSymbolId {
        let mut identity = CanonicalSymbolId::module_declaration(
            vec!["api".to_owned()],
            name,
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(10, 20),
        );
        identity.origin = SymbolOrigin::Package {
            library: package.to_owned(),
            module_path: vec!["api".to_owned()],
        };
        identity
    }

    /// Missing providers must identify the selected declaration, not only the package that contains it.
    #[test]
    fn missing_provider_retains_canonical_requirement() -> Result<(), Box<dyn std::error::Error>> {
        let wanted = declaration("absent_provider", "read_bytes");
        let outcome = resolve_loaf_declarations(
            &LoafIndex::default(),
            &BTreeSet::from([wanted.clone()]),
            &BTreeSet::new(),
        );
        let error = match outcome {
            Err(error) => error,
            Ok(_) => return Err("absent provider unexpectedly resolved".into()),
        };
        let rendered = error.to_string();
        match error {
            ExecutableResolutionError::UnknownPackage {
                library,
                declaration,
                chain,
            } => {
                assert_eq!(library, "absent_provider");
                assert_eq!(*declaration, wanted);
                assert_eq!(chain, vec![wanted]);
            }
            _ => return Err("missing provider lost its structured requirement".into()),
        }
        assert!(rendered.contains("pub::absent_provider::api::read_bytes"));
        assert!(rendered.contains("dependency chain"));
        Ok(())
    }

    /// Cyclic declaration requirements remain finite diagnostics and retain each canonical owner once.
    #[test]
    fn cyclic_requester_chain_stops_without_losing_the_required_declaration() {
        let first = declaration("first", "read");
        let second = declaration("second", "read");
        let parents = BTreeMap::from([(first.clone(), second.clone()), (second.clone(), first.clone())]);
        assert_eq!(dependency_chain(&first, &parents), vec![second, first]);
    }

    /// A package's entries are its roots and the declarations another package requested; same-package edges stay
    /// inside the entry that reached them, and a package selected only by an uncovered root keeps an empty entry.
    #[test]
    fn package_entries_list_only_declarations_that_cross_into_a_package() {
        let root = declaration("codecs", "hexlify");
        let helper = declaration("codecs", "nibble");
        let runtime = declaration("runtime", "append_list");
        let parents = BTreeMap::from([(helper.clone(), root.clone()), (runtime.clone(), helper.clone())]);
        let visited = BTreeSet::from([root.clone(), helper.clone(), runtime.clone()]);
        let packages = BTreeMap::from([
            ("codecs".to_owned(), "1".to_owned()),
            ("runtime".to_owned(), "1".to_owned()),
            ("uncovered".to_owned(), "1".to_owned()),
        ]);
        let entries = package_entries(&packages, &visited, &parents);
        assert_eq!(entries["codecs"], vec![vec![root.clone()]]);
        assert_eq!(entries["runtime"], vec![vec![root, helper, runtime]]);
        assert!(entries["uncovered"].is_empty());
    }
}
