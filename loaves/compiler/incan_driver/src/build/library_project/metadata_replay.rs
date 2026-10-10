//! Source-current ordinary metadata selection and planning replay; the native SDK publisher is a temporary adapter.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::ProjectGenerator;
use crate::build::library_metadata::{
    LibraryMetadataDependency, LibraryMetadataRecipe, SelectedLibraryMetadata, select_library_metadata,
    select_library_metadata_reference,
};
use crate::build::library_outputs::packaged_library_loaf_store_root;
use crate::build::native_sdk::NativeSdkPublicationContext;
use crate::build::output_paths::packaged_library_metadata_files;
use crate::build::{NativeSdkCommandContext, OvenPackagedLibraryLoafManifest, OvenProjectBakeAuthorityContext};
use crate::error::{CliError, CliResult};
use crate::oven_store::open_default_oven_store;
use crate::session::CompilationSession;
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_provider::PackageFeaturePlan;
use oven_model::manifest::ProjectManifest;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::OvenStore;
use oven_store::{
    OvenProjectSourceTreeEvidence, OvenReceipt, digest_bytes, digest_project_source_tree, project_source_tree_evidence,
};

pub(crate) mod lock_transition;
pub(crate) mod ordinary_native;

use ordinary_native::OrdinaryNativeMetadataAuthority;

/// Retain one original native authority family; explicit ordinary inputs never discover an SDK.
enum NativeMetadataAuthority {
    SdkCommand(Arc<NativeSdkCommandContext>),
    SdkPublication,
    Ordinary(Arc<OrdinaryNativeMetadataAuthority>),
}

/// One source-current ordinary checked owner request and the current original dependency/native leases it binds.
pub(crate) struct MetadataPreparation {
    pub recipe: LibraryMetadataRecipe,
    pub receipt: OvenReceipt,
    pub store: OvenStore,
    native_authority: NativeMetadataAuthority,
    pub rustc: PathBuf,
    dependency_owners: Vec<Arc<SelectedLibraryMetadata>>,
    delivery_coordinates: BTreeMap<String, String>,
}

impl MetadataPreparation {
    /// Observe raw authored source and current checked dependency owners before root source loading or checking.
    /// Missing old owner authority is an explicit miss; malformed advertised authority is refused.
    pub fn observe(
        project: &ProjectManifest,
        session: &CompilationSession,
        out_dir: &Path,
        native_sdk: Option<&NativeSdkPublicationContext<'_>>,
        authority: Option<&mut OvenProjectBakeAuthorityContext>,
    ) -> CliResult<Option<Self>> {
        Self::observe_with_native_context(project, session, out_dir, native_sdk, authority, None)
    }

    /// Observe the same ordinary authority using an original caller-supplied native admission when present.
    /// Missing explicit input preserves legacy discovery; a supplied capability never falls back to discovery.
    pub(crate) fn observe_with_native_context(
        project: &ProjectManifest,
        session: &CompilationSession,
        out_dir: &Path,
        native_sdk: Option<&NativeSdkPublicationContext<'_>>,
        authority: Option<&mut OvenProjectBakeAuthorityContext>,
        explicit_native_context: Option<Arc<NativeSdkCommandContext>>,
    ) -> CliResult<Option<Self>> {
        Self::observe_with_authority(
            project,
            session,
            out_dir,
            native_sdk,
            authority,
            explicit_native_context.map(NativeMetadataAuthority::SdkCommand),
        )
    }

    /// Observe checked metadata using only original ordinary producer requests and compiler support sources.
    /// The caller must establish plain-library checked-demand coverage before supplying this authority.
    pub(crate) fn observe_with_ordinary_native_authority(
        project: &ProjectManifest,
        session: &CompilationSession,
        out_dir: &Path,
        native: Arc<OrdinaryNativeMetadataAuthority>,
    ) -> CliResult<Option<Self>> {
        native.verify()?;
        Self::observe_with_authority(
            project,
            session,
            out_dir,
            None,
            None,
            Some(NativeMetadataAuthority::Ordinary(native)),
        )
    }

    /// Share source/dependency observation without changing the caller's explicit native authority family.
    fn observe_with_authority(
        project: &ProjectManifest,
        session: &CompilationSession,
        out_dir: &Path,
        native_sdk: Option<&NativeSdkPublicationContext<'_>>,
        authority: Option<&mut OvenProjectBakeAuthorityContext>,
        explicit_native: Option<NativeMetadataAuthority>,
    ) -> CliResult<Option<Self>> {
        if !metadata_output_is_observable(project.project_root(), out_dir) {
            tracing::debug!("ordinary checked metadata reuse unavailable for output within authored source");
            return Ok(None);
        }
        // Relative provider descriptors require an existing owned generated root, exactly as fresh publication does.
        std::fs::create_dir_all(out_dir).map_err(|error| invalid(error.to_string()))?;
        let store = open_default_oven_store()?;
        let producer_digest = crate::build::source_authority::current_compiler_identity_digest()?;
        let rustc = match &explicit_native {
            Some(NativeMetadataAuthority::Ordinary(native)) => native.rustc().to_path_buf(),
            _ => resolve_active_rustc().map_err(|error| invalid(error.to_string()))?,
        };
        let target = match &explicit_native {
            Some(NativeMetadataAuthority::Ordinary(native)) => native.target().to_string(),
            _ => authority
                .as_ref()
                .and_then(|authority| authority.requested_target.clone())
                .map(Ok)
                .unwrap_or_else(|| rustc_host_target(&rustc).map_err(|error| invalid(error.to_string())))?,
        };
        let toolchain = rustc_identity(&rustc).map_err(|error| invalid(error.to_string()))?;
        let feature_plan = session
            .package_feature_plan
            .as_ref()
            .ok_or_else(|| invalid("checked metadata lacks a current feature graph"))?;
        let source_digest = current_source_digest(project.project_root(), feature_plan)?;
        let Some(name) = project.project.as_ref().and_then(|project| project.name.clone()) else {
            return Ok(None);
        };
        let version = project
            .project
            .as_ref()
            .and_then(|project| project.version.clone())
            .unwrap_or_else(|| "0.1.0".into());
        let mut dependencies = BTreeMap::new();
        let mut dependency_owners = Vec::new();
        for (alias, manifest, artifact) in session.library_manifest_index.loaded_entries() {
            if artifact.kind != incan_frontend::library_manifest_index::LibraryArtifactKind::Materialized {
                return Ok(None);
            }
            let bytes = match std::fs::read(packaged_library_loaf_manifest_path(&artifact.crate_root)) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(invalid(error.to_string())),
            };
            let package: OvenPackagedLibraryLoafManifest =
                serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
            if package.schema_version == 6 && package.checked_metadata.is_none() {
                return Ok(None);
            }
            if package.schema_version != crate::build::OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION {
                return Err(invalid("unsupported advertised ordinary checked metadata authority"));
            }
            let Some(reference) = package.checked_metadata else {
                return Ok(None);
            };
            let dependency_store = OvenStore::with_release(
                packaged_library_loaf_store_root(&artifact.crate_root),
                *store.limits(),
                &incan_oven_facet::compiler_identity(),
            );
            let selected = select_library_metadata_reference(&dependency_store, &reference)?;
            let current_files =
                packaged_library_metadata_files(&artifact.manifest_path, manifest, &artifact.crate_root)?;
            if selected
                .manifest()
                .to_json_string()
                .map_err(|error| invalid(error.to_string()))?
                != manifest.to_json_string().map_err(|error| invalid(error.to_string()))?
                || selected.checked_files() != current_files
            {
                return Err(invalid(
                    "current dependency metadata disagrees with its immutable checked owner",
                ));
            }
            let Some(dependency_root) =
                incan_frontend::library_manifest_index::dependency_project_root(&artifact.crate_root)
            else {
                return Ok(None);
            };
            let Some(features) = feature_plan.package(&dependency_root) else {
                return Ok(None);
            };
            if selected.recipe().producer_digest != producer_digest
                || selected.recipe().target != target
                || selected.recipe().toolchain != toolchain
                || selected.recipe().source_digest != current_source_digest(&dependency_root, feature_plan)?
                || selected.recipe().features != features.features.active_features.iter().cloned().collect::<Vec<_>>()
            {
                return Ok(None);
            }
            dependencies.insert(
                alias.to_string(),
                LibraryMetadataDependency {
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    receipt_identity: reference.receipt.identity,
                    owner_identity: reference.owner_identity,
                    checked_digest: digest_bytes(
                        &serde_json::to_vec(&current_files).map_err(|error| invalid(error.to_string()))?,
                    ),
                },
            );
            dependency_owners.push(selected);
        }
        let delivery_coordinates = current_delivery_coordinates(out_dir, session)?;
        let native_authority = if let Some(native) = explicit_native {
            if native_sdk.is_some() {
                return Err(invalid(
                    "explicit native admission cannot replace SDK publication authority",
                ));
            }
            native
        } else if native_sdk.is_some() {
            NativeMetadataAuthority::SdkPublication
        } else {
            let context = match authority {
                Some(authority) => authority.native_sdk_context()?,
                None => NativeSdkCommandContext::discover()?,
            };
            let Some(context) = context else {
                return Ok(None);
            };
            NativeMetadataAuthority::SdkCommand(context)
        };
        let semantic_authority_digest = semantic_authority(session, native_sdk, &native_authority)?;
        let policy_digest = metadata_policy_digest(native_sdk, &delivery_coordinates)?;
        let recipe = LibraryMetadataRecipe {
            name,
            version,
            source_digest,
            producer_digest,
            semantic_authority_digest,
            dependencies,
            policy_digest,
            target,
            toolchain,
            features: session.active_features.iter().cloned().collect(),
        };
        let receipt = recipe.receipt(project.project_root())?;
        Ok(Some(Self {
            recipe,
            receipt,
            store,
            native_authority,
            rustc,
            dependency_owners,
            delivery_coordinates,
        }))
    }

    /// Borrow the original legacy command admission without discovering or substituting native authority.
    pub(crate) fn native_context(&self) -> Option<&Arc<NativeSdkCommandContext>> {
        match &self.native_authority {
            NativeMetadataAuthority::SdkCommand(context) => Some(context),
            _ => None,
        }
    }

    /// Borrow the retained ordinary producer observations for explicit current-profile caller wiring.
    pub(crate) fn ordinary_authority(&self) -> Option<&Arc<OrdinaryNativeMetadataAuthority>> {
        match &self.native_authority {
            NativeMetadataAuthority::Ordinary(native) => Some(native),
            _ => None,
        }
    }

    /// Revalidate original native leases without observing a different source generation during lock finalization.
    pub(crate) fn verify_native_authority(&self) -> CliResult<()> {
        match &self.native_authority {
            NativeMetadataAuthority::SdkCommand(context) => context.verify(),
            NativeMetadataAuthority::Ordinary(native) => native.verify(),
            NativeMetadataAuthority::SdkPublication => Ok(()),
        }
    }

    /// Borrow the original admitted checked dependency owners for the finalized metadata capability.
    pub fn dependency_owners(&self) -> &[Arc<SelectedLibraryMetadata>] {
        &self.dependency_owners
    }

    /// Admit one unchanged owner through the ordinary Store, retaining this command's dependency authority.
    pub fn select(&self) -> CliResult<Option<Arc<SelectedLibraryMetadata>>> {
        self.verify_native_authority()?;
        let Some(selected) = select_library_metadata(&self.store, &self.recipe, &self.receipt)? else {
            return Ok(None);
        };
        if selected.checked_requirements().is_none() {
            return Ok(None);
        }
        selected.retaining_dependencies(&self.dependency_owners).map(Some)
    }

    /// Check original dependency leases and recapture mutable source immediately before finalized publication.
    pub fn revalidate(
        &self,
        project: &ProjectManifest,
        session: &CompilationSession,
        out_dir: &Path,
        native_sdk: Option<&NativeSdkPublicationContext<'_>>,
    ) -> CliResult<()> {
        let features = session
            .package_feature_plan
            .as_ref()
            .ok_or_else(|| invalid("missing current feature authority"))?;
        if current_delivery_coordinates(out_dir, session)? != self.delivery_coordinates
            || current_source_digest(project.project_root(), features)? != self.recipe.source_digest
            || crate::build::source_authority::current_compiler_identity_digest()? != self.recipe.producer_digest
            || semantic_authority(session, native_sdk, &self.native_authority)? != self.recipe.semantic_authority_digest
        {
            return Err(invalid(
                "checked library preparation authority changed before publication",
            ));
        }
        for owner in &self.dependency_owners {
            owner.verify()?;
        }
        // The immutable owner remains authoritative, but mutable package handoff coordinates must still name it.
        for (alias, manifest, artifact) in session.library_manifest_index.loaded_entries() {
            let expected = self
                .recipe
                .dependencies
                .get(alias)
                .ok_or_else(|| invalid("current checked dependency is absent from the metadata recipe"))?;
            let files = packaged_library_metadata_files(&artifact.manifest_path, manifest, &artifact.crate_root)?;
            let package: OvenPackagedLibraryLoafManifest = serde_json::from_slice(
                &std::fs::read(packaged_library_loaf_manifest_path(&artifact.crate_root))
                    .map_err(|error| invalid(error.to_string()))?,
            )
            .map_err(|error| invalid(error.to_string()))?;
            let reference = package
                .checked_metadata
                .ok_or_else(|| invalid("checked dependency handoff lost its ordinary owner"))?;
            if package.schema_version != crate::build::OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION
                || reference.schema_version != crate::build::library_metadata::LIBRARY_METADATA_SCHEMA_VERSION
                || reference.owner_identity != expected.owner_identity
                || reference.receipt.identity != expected.receipt_identity
                || digest_bytes(&serde_json::to_vec(&files).map_err(|error| invalid(error.to_string()))?)
                    != expected.checked_digest
            {
                return Err(invalid("checked dependency handoff changed during library preparation"));
            }
            reference
                .receipt
                .verify_identity()
                .map_err(|error| invalid(error.to_string()))?;
            if !self
                .dependency_owners
                .iter()
                .any(|owner| owner.reference().receipt == reference.receipt)
            {
                return Err(invalid("checked dependency handoff substituted a receipt"));
            }
        }
        Ok(())
    }
}

/// Restrict reuse until source authority can exclude arbitrary supported generated-output placements explicitly.
fn metadata_output_is_observable(root: &Path, output: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let normalized = std::fs::canonicalize(output).or_else(|_| {
        let parent = output
            .parent()
            .ok_or_else(|| std::io::Error::other("output has no parent"))?;
        let name = output
            .file_name()
            .ok_or_else(|| std::io::Error::other("output has no name"))?;
        std::fs::canonicalize(parent).map(|parent| parent.join(name))
    });
    let Ok(output) = normalized else {
        return false;
    };
    match output.strip_prefix(root) {
        Ok(relative) => relative.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("target" | ".incan" | ".ralph-cache")
            )
        }),
        Err(_) => true,
    }
}

/// Keep physical delivery paths in the conservative recipe until immutable checked contracts are coordinate-free.
fn metadata_policy_digest(
    native_sdk: Option<&NativeSdkPublicationContext<'_>>,
    delivery_coordinates: &BTreeMap<String, String>,
) -> CliResult<String> {
    let policy = serde_json::json!({
        "contract": "ordinary-checked-library-requirements-v1",
        "namespace_grants": native_sdk.map(|context| &context.namespace_roots),
        "source_provider_mode": std::env::var_os(incan_provider::SDK_PROVIDER_BUILD_ENV).is_some(),
        "generated_facade": "temporary-rust-bridge-v1",
        "delivery_coordinates": delivery_coordinates,
    });
    Ok(digest_bytes(
        &serde_json::to_vec(&policy).map_err(|error| invalid(error.to_string()))?,
    ))
}

/// Bind all potentially emitted physical provider descriptors before source use selects its narrower plan.
fn current_delivery_coordinates(output: &Path, session: &CompilationSession) -> CliResult<BTreeMap<String, String>> {
    let mut roots = BTreeMap::new();
    for (alias, _, artifact) in session.library_manifest_index.loaded_entries() {
        roots.insert(format!("public:{alias}"), artifact.crate_root.clone());
    }
    // Selection is intentionally conservative until checked and delivery representations are independent. Unused
    // available SDK providers may cause misses, but an emitted private edge cannot escape this coordinate contract.
    for record in session.provider_plan.active_sdk_records() {
        if let Some(artifact) = &record.artifact {
            roots.insert(
                format!("private:{}", record.identity.stable_key()),
                artifact.crate_root.clone(),
            );
        }
    }
    project_delivery_coordinates(output, &roots)
}

/// Use the same physical relative-path authority as fresh compiled provider metadata publication.
fn project_delivery_coordinates(
    output: &Path,
    roots: &BTreeMap<String, PathBuf>,
) -> CliResult<BTreeMap<String, String>> {
    roots
        .iter()
        .map(|(alias, root)| {
            crate::build::provider_metadata::relative_provider_artifact_path(output, root)
                .map(|relative| (alias.clone(), relative))
        })
        .collect()
}

/// Reconstruct an exact semantic feature projection and observe its authored metadata generation.
/// Activation explanations do not change semantic inputs; checked active features must reproduce exactly.
pub(crate) fn observe_library_source_digest(root: &Path, exact_features: &[String]) -> CliResult<String> {
    let manifest = crate::project::effective_project_manifest_for_exact_root(root)?;
    let selection = incan_provider::FeatureSelection {
        requested: exact_features.iter().cloned().collect(),
        no_default_features: true,
        all_features: false,
    };
    let features = PackageFeaturePlan::resolve(&manifest, &selection).map_err(|error| invalid(error.to_string()))?;
    if features
        .root_package()
        .is_none_or(|root| root.features.active_features.iter().cloned().collect::<Vec<_>>() != exact_features)
    {
        return Err(invalid(
            "checked metadata features differ from current authored feature closure",
        ));
    }
    current_source_digest(root, &features)
}

/// Observe the exact active authored source closure without invoking the Incan lexer, parser or checker.
pub(crate) fn current_source_digest(root: &Path, features: &PackageFeaturePlan) -> CliResult<String> {
    current_source_snapshot(root, features)?.digest()
}

/// Complete opaque source evidence and the exact non-tree graph facts needed to isolate one lock publication.
struct MetadataSourceSnapshot {
    source: BTreeMap<PathBuf, serde_json::Value>,
    edges: Vec<incan_provider::ResolvedFeatureDependencyEdge>,
    trees: BTreeMap<PathBuf, OvenProjectSourceTreeEvidence>,
    external_locks: BTreeMap<PathBuf, Option<String>>,
}

impl MetadataSourceSnapshot {
    /// Bind complete package trees, selected graph facts and any exact canonical lock outside those trees.
    fn digest(&self) -> CliResult<String> {
        Ok(digest_bytes(
            &serde_json::to_vec(&(&self.source, &self.edges, &self.external_locks))
                .map_err(|error| invalid(error.to_string()))?,
        ))
    }

    /// Prove that every authored/Rust/feature/graph input agrees apart from this one exact published lock.
    fn unchanged_except_published_lock(&self, later: &Self, lock: &Path, digest: &str) -> CliResult<bool> {
        if self.edges != later.edges
            || self.source.keys().ne(later.source.keys())
            || self.external_locks.keys().ne(later.external_locks.keys())
        {
            return Ok(false);
        }
        let mut observed_lock = false;
        for (path, original) in &self.external_locks {
            let current = later.external_locks.get(path);
            if path == lock {
                observed_lock = true;
                if current.and_then(Option::as_deref) != Some(digest) {
                    return Ok(false);
                }
            } else if current != Some(original) {
                return Ok(false);
            }
        }
        for (root, original) in &self.source {
            let mut before = original.clone();
            let mut after = later
                .source
                .get(root)
                .ok_or_else(|| invalid("lock transition source node disappeared"))?
                .clone();
            before
                .as_object_mut()
                .ok_or_else(|| invalid("lock transition source node is not an object"))?
                .remove("source");
            after
                .as_object_mut()
                .ok_or_else(|| invalid("lock transition source node is not an object"))?
                .remove("source");
            if before != after {
                return Ok(false);
            }
            let original = self
                .trees
                .get(root)
                .ok_or_else(|| invalid("lock transition lacks original source evidence"))?;
            let current = later
                .trees
                .get(root)
                .ok_or_else(|| invalid("lock transition lacks current source evidence"))?;
            if let Ok(relative) = lock.strip_prefix(root) {
                observed_lock = true;
                if current
                    .file_digest(relative)
                    .map_err(|error| invalid(error.to_string()))?
                    != Some(digest)
                    || !original
                        .unchanged_except_exact_file(current, relative)
                        .map_err(|error| invalid(error.to_string()))?
                {
                    return Ok(false);
                }
            } else if original.digest().map_err(|error| invalid(error.to_string()))?
                != current.digest().map_err(|error| invalid(error.to_string()))?
            {
                return Ok(false);
            }
        }
        Ok(observed_lock)
    }
}

/// Observe one canonical lock outside package trees, preserving explicit absence and rejecting symlink replacement.
fn external_lock_digest(lock: &Path) -> CliResult<Option<String>> {
    match std::fs::symlink_metadata(lock) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let (_, digest) =
                oven_store::store::digest_regular_file(lock).map_err(|error| invalid(error.to_string()))?;
            let current = std::fs::symlink_metadata(lock).map_err(|error| invalid(error.to_string()))?;
            if !current.file_type().is_file()
                || metadata.len() != current.len()
                || metadata.modified().map_err(|error| invalid(error.to_string()))?
                    != current.modified().map_err(|error| invalid(error.to_string()))?
            {
                return Err(invalid("canonical metadata lock changed during observation"));
            }
            Ok(Some(digest))
        }
        Ok(_) => Err(invalid("canonical metadata lock is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(invalid(error.to_string())),
    }
}

/// Snapshot the same shared observer, retaining per-file evidence without weaker parallel traversal rules.
fn current_source_snapshot(root: &Path, features: &PackageFeaturePlan) -> CliResult<MetadataSourceSnapshot> {
    let root = std::fs::canonicalize(root).map_err(|error| invalid(error.to_string()))?;
    let mut reachable = BTreeSet::from([root.clone()]);
    loop {
        let mut changed = false;
        for edge in features.edges() {
            if reachable.contains(&edge.from) && reachable.insert(edge.to.clone()) {
                changed = true
            }
        }
        if !changed {
            break;
        }
    }
    let mut source = BTreeMap::new();
    let mut trees = BTreeMap::new();
    let mut rust_sources = BTreeMap::new();
    for package in features
        .packages()
        .filter(|package| reachable.contains(&package.project_root))
    {
        let manifest = crate::project::effective_project_manifest_for_exact_root(&package.project_root)?;
        let mut rust_edges = BTreeMap::new();
        for (kind, dependencies) in [
            ("normal", manifest.rust_dependencies()),
            ("development", manifest.rust_dev_dependencies()),
        ] {
            for (alias, dependency) in dependencies {
                if let oven_model::manifest::DependencySource::Path { path } = &dependency.source {
                    rust_edges.insert(format!("{kind}:{alias}"), path.clone());
                }
            }
        }
        // Include conditional and optional path selections conservatively until a narrower semantic producer exists.
        for (alias, alternatives) in &manifest.loaf_dependencies {
            for dependency in alternatives {
                if let oven_model::manifest::DependencySource::Path { path } = &dependency.spec.source {
                    let edge = serde_json::to_string(&(alias, &dependency.loaf, &dependency.target))
                        .map_err(|error| invalid(error.to_string()))?;
                    rust_edges.insert(edge, path.clone());
                }
            }
        }
        let rust_edges = rust_edges
            .into_iter()
            .map(|(alias, path)| {
                Ok((
                    alias,
                    crate::build::ProjectSourceAuthorityDigester::digest_rust_path_crate_authority(
                        &path,
                        &mut rust_sources,
                    )?,
                ))
            })
            .collect::<CliResult<BTreeMap<_, _>>>()?;
        let tree = project_source_tree_evidence(&package.project_root).map_err(|error| invalid(error.to_string()))?;
        let source_digest = tree.digest().map_err(|error| invalid(error.to_string()))?;
        trees.insert(package.project_root.clone(), tree);
        source.insert(
            package.project_root.clone(),
            serde_json::json!({
                "name": package.package_name,
                "features": {
                    "active": package.features.active_features,
                    "optional_dependencies": package.features.active_optional_dependencies,
                    "dependency_features": package.features.dependency_features,
                    "required_components": package.features.required_sdk_components,
                },
                "rust_edges": rust_edges,
                "source": source_digest,
            }),
        );
    }
    if !source.contains_key(&root) {
        return Err(invalid("metadata source root is absent from the current feature graph"));
    }
    let edges = features
        .edges()
        .filter(|edge| reachable.contains(&edge.from))
        .cloned()
        .collect::<Vec<_>>();
    // A canonical workspace lock can be outside every selected package tree. Bind only that exact file,
    // including absence, rather than pulling unrelated workspace siblings into semantic source authority.
    let mut external_locks = BTreeMap::new();
    for package_root in trees.keys() {
        let lock = lock_transition::canonical_lock_coordinate(package_root)?;
        if !trees.keys().any(|root| lock.starts_with(root)) {
            external_locks.insert(lock.clone(), external_lock_digest(&lock)?);
        }
    }
    Ok(MetadataSourceSnapshot {
        source,
        edges,
        trees,
        external_locks,
    })
}

/// Conservatively bind full native/macro graph, all current checked provider contracts and authored standard sources.
/// Narrow producer/semantic closure selection remains open; changing the actual compiler still invalidates this key.
fn semantic_authority(
    session: &CompilationSession,
    native_sdk: Option<&NativeSdkPublicationContext<'_>>,
    authority: &NativeMetadataAuthority,
) -> CliResult<String> {
    crate::build::library_metadata::validate_rust_fact_agreement(
        session
            .provider_plan
            .records()
            .filter_map(|record| record.manifest.as_deref()),
    )?;
    let providers = session.provider_plan.records().map(|record| {
        Ok(serde_json::json!({
            "identity": record.identity, "authority": record.authority,
            "namespace_claims": record.namespace_claims, "available": record.available,
            "enabled": record.enabled, "implementation_facets": record.implementation_facets,
            "checked_manifest": record.manifest.as_ref().map(|manifest| manifest.to_json_string()).transpose().map_err(|error| invalid(error.to_string()))?,
        }))
    }).collect::<CliResult<Vec<_>>>()?;
    let (native, stdlib) = match authority {
        NativeMetadataAuthority::Ordinary(native) => {
            if native_sdk.is_some() {
                return Err(invalid("ordinary metadata cannot substitute SDK publication authority"));
            }
            (native.semantic_inputs()?, Some(native.standard_source_digest()?))
        }
        NativeMetadataAuthority::SdkPublication => {
            let context = native_sdk.ok_or_else(|| invalid("original SDK publication authority is unavailable"))?;
            let native = serde_json::json!({
                "inspection": context.closure.inspection_project(),
                "native": context.closure.units().iter().map(|unit| unit.native_artifact()).collect::<Result<Vec<_>, _>>().map_err(|error| invalid(error.to_string()))?,
            });
            (native, legacy_standard_source_digest()?)
        }
        NativeMetadataAuthority::SdkCommand(context) => {
            if native_sdk.is_some() {
                return Err(invalid("SDK command metadata cannot substitute publication authority"));
            }
            let receipts: BTreeMap<String, String> =
                serde_json::from_slice(context.receipt_catalog()?).map_err(|error| invalid(error.to_string()))?;
            (
                serde_json::json!({ "receipts": receipts }),
                legacy_standard_source_digest()?,
            )
        }
    };
    Ok(digest_bytes(
        &serde_json::to_vec(&(providers, native, stdlib)).map_err(|error| invalid(error.to_string()))?,
    ))
}

/// Preserve legacy discovery only for existing SDK-backed callers.
fn legacy_standard_source_digest() -> CliResult<Option<String>> {
    oven_model::toolchain_layout::find_stdlib_root()
        .map(|root| digest_project_source_tree(&root).map_err(|error| invalid(error.to_string())))
        .transpose()
}

/// Keep source observation and replay refusals in the ordinary preparation error family.
fn invalid(value: impl Into<String>) -> CliError {
    CliError::failure(value.into())
}

/// Complete finalized ordinary publication, retaining source/current dependency authority until Store admission.
pub(crate) struct PendingMetadataPublication {
    pub preparation: MetadataPreparation,
    pub session: CompilationSession,
    pub project: ProjectManifest,
    pub requirements: crate::build::library_metadata::requirements::CheckedLibraryRequirements,
}

impl PendingMetadataPublication {
    /// Publish only after the ordinary output writer has finalized the full checked file closure.
    pub fn publish(self, prepared: &crate::build::PreparedLibraryProject) -> CliResult<Arc<SelectedLibraryMetadata>> {
        self.preparation
            .revalidate(&self.project, &self.session, &prepared.out_dir, None)?;
        let owner = crate::build::library_metadata::publish_library_metadata_with_requirements(
            &self.preparation.store,
            &self.preparation.recipe,
            &self.preparation.receipt,
            &prepared.out_dir,
            &prepared.manifest_path,
            self.requirements.rust_abi_queries.clone(),
            Some(self.requirements),
        )?;
        owner.retaining_dependencies(&self.preparation.dependency_owners)
    }
}

/// Current command settings applied to admitted ordinary metadata; no old profile selection enters this request.
pub(super) struct ReplayRequest<'a> {
    pub preparation: &'a MetadataPreparation,
    pub selected: Arc<SelectedLibraryMetadata>,
    pub project: &'a ProjectManifest,
    pub session: &'a CompilationSession,
    pub native_sdk: Option<&'a NativeSdkPublicationContext<'a>>,
    pub out_dir: PathBuf,
    pub entrypoint: PathBuf,
    pub cargo_policy: &'a crate::cargo_policy::CargoPolicy,
    pub package_features: &'a incan_provider::FeatureSelection,
    pub sdk_profile_override: Option<&'a str>,
    pub oven_plan_mode: crate::build::OvenProjectPlanMode,
    pub include_interop_execution: bool,
    pub authority: Option<&'a mut OvenProjectBakeAuthorityContext>,
    pub ordinary_native: Option<Arc<crate::build::ordinary_library_native::OrdinaryLibraryNativeProfiles>>,
}

/// Replay checked output before frontend source collection, then rebuild execution state through the shared planner.
pub(super) fn prepare_replayed_library(request: ReplayRequest<'_>) -> CliResult<super::LibraryPreparation> {
    use crate::build::plan_authority::oven_source_inline_dependency_specs;
    use crate::build::provider_compilation::{
        checked_packaged_provider_profiles, import_packaged_provider_loafs_for_explicit_bake,
    };
    use crate::build_report::{
        BuildOvenReport, BuildReportDraft, BuildReportMode, SourceFileReport, dependencies_report,
        incan_dependencies_report, interop_report, oven_generated_project_report, semantic_report,
    };
    use incan_provider::inventory::extend_requirements_with_provider_plan;
    use incan_provider::requirements::semantic_sdk_path_dependencies;

    let ReplayRequest {
        preparation,
        selected,
        project,
        session,
        native_sdk,
        out_dir,
        entrypoint,
        cargo_policy,
        package_features,
        sdk_profile_override,
        oven_plan_mode,
        include_interop_execution,
        authority,
        ordinary_native,
    } = request;
    let started = Instant::now();
    preparation.revalidate(project, session, &out_dir, native_sdk)?;
    match (preparation.ordinary_authority(), &ordinary_native) {
        (Some(original), Some(native)) if Arc::ptr_eq(original, native.metadata()) => native.verify()?,
        (None, None) => (),
        _ => {
            return Err(invalid(
                "ordinary metadata replay lost its original current-profile authority",
            ));
        }
    }
    let contract = selected
        .checked_requirements()
        .ok_or_else(|| invalid("metadata owner lacks checked planning inputs"))?;
    contract.validate()?;
    if ordinary_native.is_some() {
        contract.require_support_only_native()?;
        if include_interop_execution {
            return Err(invalid("ordinary support-only replay cannot request interop execution"));
        }
    }
    selected.replay(&out_dir)?;
    tracing::debug!(
        package = %preparation.recipe.name,
        metadata_owner = %selected.reference().owner_identity,
        "ordinary checked library metadata replay selected before frontend preparation"
    );
    let library_manifest = selected.manifest().clone();
    let manifest_path = out_dir.join(format!("{}.incnlib", library_manifest.name));
    let surface_path =
        incan_frontend::library_manifest::published_layout::executable_surface_path(&manifest_path, &library_manifest)
            .ok_or_else(|| invalid("checked replay has no executable surface"))?;
    let executable_surface = std::fs::read(surface_path).map_err(|error| invalid(error.to_string()))?;
    if native_sdk.is_some() {
        return Ok(super::LibraryPreparation::Native {
            manifest: Box::new(library_manifest),
            executable: executable_surface,
            metadata_owner: Some(selected),
        });
    }
    crate::lock::resolution::validate_oven_lock_policy_with_session(
        crate::lock::OvenLockValidationRequest {
            project_root: project.project_root(),
            manifest: Some(project),
            entry_file: &entrypoint,
            cargo_features: &oven_model::lock::CargoFeatureSelection::default(),
            cargo_policy,
            package_features,
            sdk_profile_override,
        },
        session,
    )?;
    let provider_plan = session.provider_plan_for_used_module_paths(contract.used_module_paths.clone())?;
    let mut requirements = contract.current_requirements(project, &session.library_manifest_index)?;
    extend_requirements_with_provider_plan(&mut requirements, &provider_plan)?;
    let (mut resolved, imports) = contract.current_dependencies(project, &requirements)?;
    crate::build::library_outputs::remove_generated_library_self_dependencies(&mut resolved, project.project_root());
    let semantic_paths = if let Some(native) = ordinary_native.as_ref() {
        native.provider_semantic_dependencies(&provider_plan, &requirements)?
    } else {
        semantic_sdk_path_dependencies(&requirements)
    };
    let provider_semantics = session.provider_semantic_identities(&provider_plan, &semantic_paths)?;
    let mut build_inputs = if ordinary_native.is_some() {
        BTreeMap::new()
    } else {
        crate::build_unit::oven_build_unit_inputs_with_provider_identities_and_native_sdk(
            &provider_plan,
            &requirements,
            &resolved,
            &provider_semantics,
            preparation.native_context().map(Arc::as_ref),
        )?
    };
    if project.vocab().is_some() && oven_cargo_compat::source_compiler_vocab_support_is_available() {
        build_inputs.insert(
            oven_rustc::loaf::OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT.into(),
            "v1".into(),
        );
    }
    if include_interop_execution {
        crate::build::caller_owned::append_oven_interop_execution_build_inputs(
            &mut build_inputs,
            Some(project),
            &preparation.recipe.target,
        )?;
    }
    let mut generator = ProjectGenerator::new(&out_dir, &library_manifest.name, false);
    generator.set_package_name(Some(library_manifest.name.clone()));
    generator.set_package_metadata(
        Some(library_manifest.version.clone()),
        project.project.as_ref().and_then(|project| project.license.clone()),
    );
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(requirements.sdk_path_dependencies.clone());
    generator.set_stdlib_facets(requirements.stdlib_facets.clone());
    generator.set_include_dev_dependencies(oven_plan_mode == crate::build::OvenProjectPlanMode::ExplicitBake);
    generator.set_rust_edition(project.rust_edition().map(str::to_string));
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    let checked_api = library_manifest
        .contract_metadata
        .api
        .as_ref()
        .ok_or_else(|| invalid("checked replay has no finalized public API"))?;
    generator.set_public_namespace_facades(checked_api);
    // Regenerate only current compatibility project coordinates. The checked Rust facade itself must stay exact.
    let facade = std::fs::read_to_string(generator.crate_root_path()).map_err(|error| invalid(error.to_string()))?;
    generator
        .generate(&facade)
        .map_err(|error| invalid(error.to_string()))?;
    if std::fs::read(generator.crate_root_path()).map_err(|error| invalid(error.to_string()))? != facade.as_bytes() {
        return Err(invalid("current generator changed the admitted checked facade"));
    }
    let inline = oven_source_inline_dependency_specs(&resolved, &contract.source_inline_crates)?;
    let profiles = checked_packaged_provider_profiles(
        &provider_plan,
        &crate::build::plan_authority::explicit_bake_profiles(),
        &preparation.recipe.target,
        &preparation.recipe.toolchain,
        authority,
    )?;
    import_packaged_provider_loafs_for_explicit_bake(oven_plan_mode, &preparation.store, &profiles)?;
    let mut timings_ms = BTreeMap::new();
    let oven = super::current_plans::prepare_current_library_profiles(
        super::current_plans::CurrentLibraryPlanInputs {
            project_root: project.project_root(),
            project_name: &library_manifest.name,
            project_version: &library_manifest.version,
            generator: &generator,
            provider_plan: &provider_plan,
            checked_provider_profiles: &profiles,
            build_inputs: &build_inputs,
            inline_dependencies: &inline,
            rustc: preparation.rustc.clone(),
            target: preparation.recipe.target.clone(),
            toolchain: preparation.recipe.toolchain.clone(),
            store: &preparation.store,
            native_sdk_context: preparation.native_context().cloned(),
            ordinary_runtime: ordinary_native
                .as_ref()
                .map(|native| {
                    crate::build_unit::OrdinaryLibraryRuntimeInputs::from_checked(
                        Arc::clone(native),
                        project.project_root(),
                        &provider_plan,
                        &requirements,
                        &resolved,
                        &provider_semantics,
                    )
                })
                .transpose()?,
            oven_plan_mode,
            rust_edition: project.rust_edition().map(str::to_string),
        },
        &mut timings_ms,
    )?;
    let chosen = oven
        .profiles
        .get("release")
        .or_else(|| oven.profiles.values().next())
        .ok_or_else(|| invalid("checked replay prepared no current profile"))?;
    let report = BuildReportDraft {
        mode: BuildReportMode::Library, profile: "release".into(),
        project: crate::build::manifest_project_report(Some(project), &library_manifest.name, project.project_root()),
        entrypoint: Some(entrypoint.to_string_lossy().into_owned()), library_root: Some(project.project_root().to_string_lossy().into_owned()),
        source_files: contract.source_modules.iter().map(|(path, module)| SourceFileReport { path: project.project_root().join(path).to_string_lossy().into_owned(), module_path: module.clone() }).collect(),
        generated: oven_generated_project_report(generator.output_dir(), &generator.crate_root_path(), &generator.output_dir().join("oven")),
        artifacts: Vec::new(), dependencies: dependencies_report(&resolved.dependencies, &resolved.dev_dependencies, incan_dependencies_report(project.library_dependencies().iter().collect()), requirements.stdlib_facets),
        semantic: semantic_report(session.sdk_inventory.as_deref(), session.sdk_components.as_ref(), session.package_feature_plan.as_ref(), &provider_plan),
        cargo: None, oven: Some(BuildOvenReport { receipt_identity: chosen.receipt.identity.clone(), build_unit_identity: chosen.receipt.build_unit_identity.clone(), plan_identity: chosen.plan_selection.report_identity() }),
        interop: interop_report(&imports, contract.rust_extern_paths.clone(), contract.rust_abi_queries.iter().cloned().collect()),
        notes: vec!["Selected unchanged checked metadata from its ordinary Loaf; rebuilt current profile plans from checked requirements.".into()],
        backend: contract.backend.clone(),
    };
    crate::build::record_timing(&mut timings_ms, "library_checked_metadata_replay", started);
    timings_ms.insert("library_load_sources".into(), 0);
    timings_ms.insert("library_build_manifest_metadata".into(), 0);
    crate::build::record_timing(&mut timings_ms, "library_prepare_total", started);
    let checked_exports = contract.caller_exports()?;
    Ok(super::LibraryPreparation::Project(Box::new(
        crate::build::PreparedLibraryProject {
            checked_exports,
            generator,
            project_root: project.project_root().to_path_buf(),
            entrypoint,
            out_dir,
            manifest_path,
            library_manifest,
            executable_surface,
            timings_ms,
            report,
            oven: Some(oven),
            metadata_owner: Some(selected),
            pending_metadata: None,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir: None,
        },
    )))
}

#[cfg(test)]
mod tests;
