//! The compiler-suite third-party foundation's own identity, and reuse of a stored foundation by it (#1564).
//!
//! TODO(#1561): the Cargo build this module keys is transitional — it retires when the compiler-suite family bakes
//! through the Loaf-native route the release family already uses. The key, the family record and the mirror import
//! describe a stored foundation and outlive the build that first produced it.
//!
//! The suite publisher runs Cargo for exactly one compilation: the third-party foundation, the registry and vendored
//! closure every shard links. That closure is a function of the private foundation manifest, the lock it was resolved
//! from, the checked-in patches it names, the build intent and the toolchain that compiled it — and of nothing the
//! compiler's own sources say. The suite, by contrast, is keyed on the compiler's receipt, so a compiler edit misses
//! the suite and, before this module, asked Cargo to compile the unchanged foundation again.
//!
//! [`OvenCompilerSuiteFoundationKey`] hashes only the foundation's inputs. A foundation entry carries the key in its
//! payload together with a portable rendering of Cargo's artifact index, so a later suite publication can select the
//! family by key — local store first, then `INCAN_OVEN_MIRRORS` through the verifying import — and plan its shards
//! from the stored index instead of from a Cargo build. The selected family is then re-published in the new suite's
//! batch under the new receipt: the store adopts an admitted entry's files by hard link after reading and re-digesting
//! every byte, and the scheduler's execution-time authorization (`build_unit_identity` and intent equal to the running
//! receipt's) stays exactly as it is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use oven_rustc::rustc::OvenRustcSupportingArtifact;
use oven_store::store::{OvenArtifactKind, OvenStoreExecutionPayload};

use super::{
    CargoMetadata, CargoUnitArtifactKey, CompilerSuiteArtifactCatalog, CompilerSuiteFoundationDependency,
    OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION, OvenArtifactMaterializedFile,
    OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationArtifactRecord,
    OvenCompilerTestSuiteFoundationFamily, OvenCompilerTestSuiteFoundationPayload, OvenLegacyCargoError, OvenReceipt,
    OvenStore, canonical_directory, compiler_suite_foundation_manifest, digest_bytes, relative_path,
    verified_regular_file,
};

/// Domain tag folded first into every foundation key; bump it when the key's inputs or their encoding change.
const FOUNDATION_KEY_DOMAIN: &[u8] = b"oven-compiler-suite-foundation-key-v1\0";

/// Virtual root a checked-in patch path is spelled under inside the hashed manifest, so the key does not carry the
/// checkout's location.
const FOUNDATION_KEY_PATCH_ROOT: &str = "/incan/compiler";

/// Cargo target name of the private foundation root, whose own rlib is materialized but never a shard extern.
const FOUNDATION_ROOT_TARGET_NAME: &str = "oven_compiler_foundation";

/// Content identity of one third-party foundation family, derived from the foundation's inputs only.
///
/// Two publications that would hand Cargo the same private manifest, the same lock, the same patch sources and the
/// same intent under the same Cargo and rustc on the same host produce the same key. A compiler source edit changes
/// none of those, so it keeps the key; a changed lock, manifest, patch, target, host, profile, toolchain or Cargo
/// release changes it. The key deliberately never folds a `compiler-suite-source:*` digest or the compiler Loaf's
/// compatibility identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OvenCompilerSuiteFoundationKey(String);

impl OvenCompilerSuiteFoundationKey {
    /// Borrow the `sha256:` rendering stored in a foundation payload.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A key with a chosen rendering, for tests that exercise selection and partitioning without a Cargo build.
    #[cfg(test)]
    pub(crate) fn fixture(rendering: &str) -> Self {
        Self(rendering.to_string())
    }

    /// Derive the key from the foundation's own inputs.
    ///
    /// The manifest is rendered again from the same dependency selection Cargo will be handed, with every checked-in
    /// patch path spelled under a virtual root, so the hashed text is the publisher's exact input minus the checkout
    /// location. The lock is the staged foundation lock, pruned from the compiler's; patch sources are hashed as
    /// authored trees. Target, profile and the rustc identity are the receipt intent; the host is the publisher's,
    /// because the foundation also carries proc-macro and build-script units compiled for it, and the rustc identity
    /// line names a release, not a host. The Cargo version string is the publisher's, because Cargo's own release
    /// feeds the metadata hash it hands rustc.
    pub(crate) fn derive(inputs: &OvenCompilerSuiteFoundationKeyInputs<'_>) -> Result<Self, OvenLegacyCargoError> {
        let compiler_root = canonical_directory(inputs.compiler_root, "compiler root")?;
        let mut hasher = Sha256::new();
        hasher.update(FOUNDATION_KEY_DOMAIN);

        // ---- Build intent and publisher identity ----
        fold_section(&mut hasher, "target", inputs.target.as_bytes());
        fold_section(&mut hasher, "host", inputs.host.as_bytes());
        fold_section(&mut hasher, "profile", inputs.profile.as_bytes());
        fold_section(&mut hasher, "toolchain", inputs.toolchain.as_bytes());
        fold_section(&mut hasher, "cargo", inputs.cargo_version.as_bytes());

        // ---- The private manifest, spelled without the checkout path ----
        let mut patches = BTreeMap::new();
        let mut portable = Vec::with_capacity(inputs.dependencies.len());
        for dependency in inputs.dependencies {
            let mut dependency = dependency.clone();
            if let Some(path) = dependency.path.take() {
                let relative = relative_path(&compiler_root, &path)?;
                dependency.path = Some(Path::new(FOUNDATION_KEY_PATCH_ROOT).join(&relative));
                patches.insert(relative, path);
            }
            portable.push(dependency);
        }
        let manifest = compiler_suite_foundation_manifest(&portable)?;
        fold_section(&mut hasher, "manifest", manifest.as_bytes());

        // ---- The staged lock ----
        fold_section(&mut hasher, "lock", digest_bytes(inputs.lock).as_bytes());

        // ---- Checked-in patch sources, the foundation's own source members ----
        for (relative, path) in patches {
            let digest = oven_store::digest_project_source_tree(&path).map_err(|error| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite foundation patch `{relative}` cannot be digested: {error}"
                ))
            })?;
            fold_section(&mut hasher, &format!("patch:{relative}"), digest.as_bytes());
        }
        Ok(Self(format!("sha256:{}", hex::encode(hasher.finalize()))))
    }
}

/// Fold one labeled section into the key hasher with unambiguous boundaries.
fn fold_section(hasher: &mut Sha256, label: &str, value: &[u8]) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(value);
    hasher.update([0]);
}

/// Everything the foundation's bytes depend on, as the publisher has it in hand after staging the manifest.
pub(crate) struct OvenCompilerSuiteFoundationKeyInputs<'a> {
    /// The compiler checkout the patch paths are relative to.
    pub compiler_root: &'a Path,
    /// The dependency selection the private manifest is rendered from.
    pub dependencies: &'a [CompilerSuiteFoundationDependency],
    /// The staged foundation `Cargo.lock` bytes.
    pub lock: &'a [u8],
    /// Receipt target triple.
    pub target: &'a str,
    /// The publisher's host triple (`rustc -vV` `host:`), which the foundation's proc-macro and build-script units
    /// are compiled for.
    pub host: &'a str,
    /// Receipt profile.
    pub profile: &'a str,
    /// Receipt toolchain identity (`rustc -vV`).
    pub toolchain: &'a str,
    /// The publisher's `cargo --version` line.
    pub cargo_version: &'a str,
}

/// Render Cargo's artifact index for one just-built foundation without machine-specific paths.
///
/// Every indexed unit is looked up in the publisher's Cargo metadata by the package ID Cargo reported; the record
/// then names the package as its lock does and the crate root relative to the package root. The private foundation
/// root is not a package the compiler graph knows and is skipped; any other unit absent from metadata is a publisher
/// contract failure and refuses the publication, so a change in how Cargo spells a package ID is noticed at the first
/// bake rather than as a silent planning miss on a later reuse.
pub(crate) fn compiler_suite_foundation_artifact_records(
    index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    metadata: &CargoMetadata,
    compiler_root: &Path,
    staging: &Path,
) -> Result<Vec<OvenCompilerTestSuiteFoundationArtifactRecord>, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let mut records = Vec::with_capacity(index.len());
    for (key, files) in index {
        let Some(package) = packages.get(key.package_id.as_str()) else {
            if key.target_name.replace('-', "_") == FOUNDATION_ROOT_TARGET_NAME {
                continue;
            }
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation artifact `{}` of package `{}` is absent from publisher Cargo metadata",
                key.target_name, key.package_id
            )));
        };
        let package_root = package_root(&package.manifest_path)?;
        let source_relative_path = relative_path(&package_root, &key.source_path)?;
        let package_relative_path = match package.source {
            Some(_) => None,
            None => Some(relative_path(&compiler_root, &package_root)?),
        };
        let files = files
            .iter()
            .map(|file| relative_path(staging, file))
            .collect::<Result<Vec<_>, _>>()?;
        records.push(OvenCompilerTestSuiteFoundationArtifactRecord {
            package: package.name.clone(),
            version: package.version.clone(),
            source: package.source.clone(),
            package_relative_path,
            target_name: key.target_name.clone(),
            source_relative_path,
            features: key.features.clone(),
            test_profile: key.test_profile,
            platform: key.platform.clone(),
            files,
        });
    }
    records.sort();
    Ok(records)
}

/// The canonical directory owning one package manifest.
fn package_root(manifest_path: &Path) -> Result<PathBuf, OvenLegacyCargoError> {
    let manifest = verified_regular_file(manifest_path, "Cargo package manifest")?;
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo package manifest",
            message: format!("{} has no package directory", manifest.display()),
        })
}

/// How the suite completion obtained its third-party foundation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OvenLegacyCargoFoundationSelection {
    /// An existing suite was selected; no foundation was needed.
    ExistingSuite,
    /// A family with this key was already admitted in the local store.
    ReusedFromStore,
    /// A family with this key was admitted from a configured mirror through the verifying import.
    ReusedFromMirror,
    /// No family matched the key; Cargo compiled the foundation.
    Built,
}

/// One admitted partition of a selected family, held under its execution lease until the suite batch is published.
pub(crate) struct SelectedCompilerSuiteFoundationPartition {
    /// The leased store entry.
    pub stored: OvenStoreExecutionPayload,
    /// Its decoded payload.
    pub payload: OvenCompilerTestSuiteFoundationPayload,
}

/// A complete foundation family selected by key, every partition leased.
pub(crate) struct SelectedCompilerSuiteFoundationFamily {
    /// The family record every partition agreed on.
    pub family: OvenCompilerTestSuiteFoundationFamily,
    /// Partitions in `partition_index` order.
    pub partitions: Vec<SelectedCompilerSuiteFoundationPartition>,
    /// Where the family was found.
    pub selection: OvenLegacyCargoFoundationSelection,
}

/// Select a complete family by key from the local store, and on a miss admit one from `mirrors` first.
///
/// `mirrors` is the `INCAN_OVEN_MIRRORS` list as the caller read it. The mirror lookup reads verified payloads only
/// to compare the key; an entry enters the local store solely through `store_mirror::import_keyed_from_mirrors`,
/// which proves every file as it copies. After an import the local selection runs again, so what is returned is
/// always a leased local family. `None` means Cargo has to build it.
pub(crate) fn select_or_import_compiler_suite_foundation_family(
    store: &OvenStore,
    receipt: &OvenReceipt,
    key: &OvenCompilerSuiteFoundationKey,
    mirrors: &[PathBuf],
) -> Result<Option<SelectedCompilerSuiteFoundationFamily>, OvenLegacyCargoError> {
    if let Some(family) = select_compiler_suite_foundation_family(store, receipt, key)? {
        return Ok(Some(family));
    }
    if mirrors.is_empty() {
        return Ok(None);
    }
    let imported = oven_store::store_mirror::import_keyed_from_mirrors(
        store,
        mirrors,
        receipt,
        OvenArtifactKind::CompilerTestSuiteFoundation,
        |_, payload| payload_carries_foundation_key(payload, key),
    )?;
    if imported.is_empty() {
        return Ok(None);
    }
    Ok(
        select_compiler_suite_foundation_family(store, receipt, key)?.map(|mut family| {
            family.selection = OvenLegacyCargoFoundationSelection::ReusedFromMirror;
            family
        }),
    )
}

/// Whether verified payload bytes describe a current-schema foundation partition of the family with `key`.
fn payload_carries_foundation_key(payload: &[u8], key: &OvenCompilerSuiteFoundationKey) -> bool {
    serde_json::from_slice::<OvenCompilerTestSuiteFoundationPayload>(payload).is_ok_and(|payload| {
        payload.schema_version == OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION
            && payload.family.as_ref().is_some_and(|family| family.key == key.as_str())
    })
}

/// Select a complete, consistent family by key from the local store, leasing every partition.
///
/// Candidates are every current-schema foundation for this intent whose payload names the key. A family is complete
/// when partitions `0..partition_count` are all present and carry one identical family record; an incomplete or
/// contradictory set — an interrupted batch, or two publications that disagree — is a miss, never an error, because
/// Cargo can still produce the foundation. Partitions are grouped by the family's closure digest before anything else,
/// so two builds that share a key (one reclaimed halfway, one imported halfway from a mirror) are two candidate
/// families, and a partition of each can never be paired. Within one family, when a partition index has several
/// admitted entries (the same build re-published under later receipts, sharing its files by hard link) the smallest
/// identity is taken, so the choice is deterministic across runs; between complete families the smallest closure
/// digest wins for the same reason.
pub(crate) fn select_compiler_suite_foundation_family(
    store: &OvenStore,
    receipt: &OvenReceipt,
    key: &OvenCompilerSuiteFoundationKey,
) -> Result<Option<SelectedCompilerSuiteFoundationFamily>, OvenLegacyCargoError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::CompilerTestSuiteFoundation && manifest.intent == receipt.intent
    })?;

    // ---- Group the current-schema partitions of this key by build, then by partition index ----
    let mut by_build = BTreeMap::<String, BTreeMap<u32, Vec<SelectedCompilerSuiteFoundationPartition>>>::new();
    for stored in candidates {
        let Ok(payload) = serde_json::from_slice::<OvenCompilerTestSuiteFoundationPayload>(&stored.payload) else {
            continue;
        };
        if payload.schema_version != OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION {
            continue;
        }
        let Some(family) = payload.family.as_ref() else {
            continue;
        };
        if family.key != key.as_str() || family.partition_count == 0 || family.partition_index >= family.partition_count
        {
            continue;
        }
        by_build
            .entry(family.closure_digest.clone())
            .or_default()
            .entry(family.partition_index)
            .or_default()
            .push(SelectedCompilerSuiteFoundationPartition { stored, payload });
    }
    if by_build.is_empty() {
        return Ok(None);
    }

    // ---- Take the first build whose every partition is present and agrees on one family record ----
    for by_index in by_build.into_values() {
        let mut partitions = Vec::with_capacity(by_index.len());
        for (_, mut entries) in by_index {
            entries.sort_by(|left, right| left.stored.manifest.identity.cmp(&right.stored.manifest.identity));
            if let Some(chosen) = entries.into_iter().next() {
                partitions.push(chosen);
            }
        }
        let Some(family) = partitions
            .first()
            .and_then(|partition| partition.payload.family.clone())
        else {
            continue;
        };
        let complete = partitions.len() == usize::try_from(family.partition_count).unwrap_or(usize::MAX)
            && partitions.iter().enumerate().all(|(position, partition)| {
                partition.payload.family.as_ref().is_some_and(|record| {
                    usize::try_from(record.partition_index).ok() == Some(position)
                        && record.partition_count == family.partition_count
                        && record.key == family.key
                        && record.closure_digest == family.closure_digest
                        && record.dependency_search_paths == family.dependency_search_paths
                        && record.native_search_paths == family.native_search_paths
                        && record.artifact_index == family.artifact_index
                })
            });
        if complete {
            return Ok(Some(SelectedCompilerSuiteFoundationFamily {
                family,
                partitions,
                selection: OvenLegacyCargoFoundationSelection::ReusedFromStore,
            }));
        }
    }
    tracing::debug!(
        "compiler-suite foundation family {} is incomplete or inconsistent in the store; Cargo will rebuild it",
        key.as_str()
    );
    Ok(None)
}

/// The catalog and artifact index a selected family stands in for, as the shard planner consumes them.
pub(crate) struct RehydratedCompilerSuiteFoundation {
    /// The complete closure with every file located below its partition's admitted artifact root.
    pub catalog: CompilerSuiteArtifactCatalog,
    /// Cargo's artifact index rebuilt against the current unit graph's package IDs and crate-root paths.
    pub artifact_index: BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
}

/// Rebuild the shard planner's inputs from a selected family instead of from a Cargo build.
///
/// Every file the family's closure names must be materialized by exactly one partition with the same digest; the
/// portable index is resolved against the current Cargo metadata, so a record whose package or crate root the graph no
/// longer has is a planning refusal naming the key, never a silent gap.
pub(crate) fn rehydrate_compiler_suite_foundation(
    family: &SelectedCompilerSuiteFoundationFamily,
    metadata: &CargoMetadata,
    compiler_root: &Path,
) -> Result<RehydratedCompilerSuiteFoundation, OvenLegacyCargoError> {
    let key = family.family.key.as_str();
    // ---- Locate every closure file below its partition's artifact root ----
    let mut located = BTreeMap::<String, (PathBuf, String)>::new();
    for partition in &family.partitions {
        let materialized = partition
            .stored
            .manifest
            .materialized_files
            .iter()
            .map(|file| (file.relative_path.as_str(), file.digest.as_str()))
            .collect::<BTreeMap<_, _>>();
        for artifact in &partition.payload.artifact_closure.supporting_artifacts {
            if materialized.get(artifact.relative_path.as_str()) != Some(&artifact.digest.as_str()) {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite foundation family {key} partition `{}` does not materialize `{}` with its declared digest",
                    partition.payload.label, artifact.relative_path
                )));
            }
            let source_path = partition.stored.artifact_root.join(&artifact.relative_path);
            if located
                .insert(artifact.relative_path.clone(), (source_path, artifact.digest.clone()))
                .is_some()
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite foundation family {key} materializes `{}` in more than one partition",
                    artifact.relative_path
                )));
            }
        }
    }
    if located.is_empty() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite foundation family {key} names no artifacts"
        )));
    }

    // ---- The complete closure and catalog ----
    let supporting_artifacts = located
        .iter()
        .map(|(relative_path, (_, digest))| OvenRustcSupportingArtifact {
            relative_path: relative_path.clone(),
            digest: digest.clone(),
        })
        .collect::<Vec<_>>();
    let materialized_files = located
        .iter()
        .map(|(relative_path, (source_path, _))| OvenArtifactMaterializedFile {
            source_path: source_path.clone(),
            relative_path: relative_path.clone(),
        })
        .collect::<Vec<_>>();
    let by_source_path = located
        .iter()
        .map(|(relative_path, (source_path, digest))| (source_path.clone(), (relative_path.clone(), digest.clone())))
        .collect::<BTreeMap<_, _>>();
    let catalog = CompilerSuiteArtifactCatalog {
        closure: OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: family.family.dependency_search_paths.clone(),
            native_search_paths: family.family.native_search_paths.clone(),
            supporting_artifacts,
        },
        materialized_files,
        by_source_path,
    };

    // ---- The artifact index against the current graph's package identities ----
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut packages = BTreeMap::new();
    for package in &metadata.packages {
        let root = package_root(&package.manifest_path)?;
        let package_relative_path = match package.source {
            Some(_) => None,
            None => root
                .strip_prefix(&compiler_root)
                .ok()
                .map(|relative| relative.to_string_lossy().replace('\\', "/")),
        };
        packages.insert(
            (
                package.name.clone(),
                package.version.clone(),
                package.source.clone(),
                package_relative_path,
            ),
            (package.id.clone(), root),
        );
    }
    let mut artifact_index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for record in &family.family.artifact_index {
        let identity = (
            record.package.clone(),
            record.version.clone(),
            record.source.clone(),
            record.package_relative_path.clone(),
        );
        let Some((package_id, root)) = packages.get(&identity) else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation family {key} records package `{} {}` that the current compiler graph does not resolve",
                record.package, record.version
            )));
        };
        let source_path = verified_regular_file(
            &root.join(&record.source_relative_path),
            "compiler-suite foundation crate root",
        )?;
        let files = record
            .files
            .iter()
            .map(|file| {
                located
                    .get(file)
                    .map(|(source_path, _)| source_path.clone())
                    .ok_or_else(|| {
                        OvenLegacyCargoError::Plan(format!(
                            "compiler-suite foundation family {key} indexes `{file}` that no partition materializes"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry = artifact_index
            .entry(CargoUnitArtifactKey {
                package_id: package_id.clone(),
                target_name: record.target_name.clone(),
                source_path,
                features: record.features.clone(),
                test_profile: record.test_profile,
                platform: record.platform.clone(),
            })
            .or_default();
        entry.extend(files);
        entry.sort();
        entry.dedup();
    }
    Ok(RehydratedCompilerSuiteFoundation {
        catalog,
        artifact_index,
    })
}

/// The relative paths every partition of a family must materialize, for a publisher assembling reuse requests.
///
/// A partition is re-published with every file its admitted manifest lists, not only the closure files, so an
/// entry's materialized set is copied whole exactly as the mirror import does.
pub(crate) fn selected_partition_materialized_files(
    partition: &SelectedCompilerSuiteFoundationPartition,
) -> Vec<OvenArtifactMaterializedFile> {
    partition
        .stored
        .manifest
        .materialized_files
        .iter()
        .map(|file| OvenArtifactMaterializedFile {
            source_path: partition.stored.artifact_root.join(&file.relative_path),
            relative_path: file.relative_path.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use oven_store::store::{OvenArtifactPublishRequest, OvenStoreLimits};
    use oven_store::{OvenCompilerSuiteRequest, receipt_native_compiler_suite};

    use super::super::CargoMetadataPackage;
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const TARGET: &str = "aarch64-apple-darwin";
    const REGISTRY: &str = "registry+https://example.invalid/index";

    /// A compiler checkout with one library source and one checked-in third-party patch.
    fn write_compiler_root(root: &Path) -> TestResult {
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(root.join("Cargo.lock"), "version = 4\n")?;
        fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        let patch = root.join("loaves/third_party/patched");
        fs::create_dir_all(patch.join("src"))?;
        fs::write(
            patch.join("Cargo.toml"),
            "[package]\nname = \"patched\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(patch.join("src/lib.rs"), "pub fn patched() {}\n")?;
        Ok(())
    }

    /// The dependency selection a publisher would render for the fixture checkout.
    fn dependencies(root: &Path) -> Result<Vec<CompilerSuiteFoundationDependency>, Box<dyn std::error::Error>> {
        Ok(vec![
            CompilerSuiteFoundationDependency {
                alias: "oven_foundation_0000".to_string(),
                package: "patched".to_string(),
                version: "0.1.0".to_string(),
                source: None,
                features: Vec::new(),
                path: Some(fs::canonicalize(root.join("loaves/third_party/patched"))?),
            },
            CompilerSuiteFoundationDependency {
                alias: "oven_foundation_0001".to_string(),
                package: "serde".to_string(),
                version: "1.0.0".to_string(),
                source: Some(REGISTRY.to_string()),
                features: vec!["derive".to_string()],
                path: None,
            },
        ])
    }

    /// Derive the fixture key with one input overridden by the caller.
    #[allow(clippy::too_many_arguments)]
    fn derive_key(
        root: &Path,
        dependencies: &[CompilerSuiteFoundationDependency],
        lock: &[u8],
        target: &str,
        host: &str,
        profile: &str,
        toolchain: &str,
        cargo_version: &str,
    ) -> Result<OvenCompilerSuiteFoundationKey, OvenLegacyCargoError> {
        OvenCompilerSuiteFoundationKey::derive(&OvenCompilerSuiteFoundationKeyInputs {
            compiler_root: root,
            dependencies,
            lock,
            target,
            host,
            profile,
            toolchain,
            cargo_version,
        })
    }

    #[test]
    fn foundation_key_depends_on_each_foundation_input_and_nothing_of_the_compiler() -> TestResult {
        let checkout = tempfile::tempdir()?;
        let root = checkout.path();
        write_compiler_root(root)?;
        let selection = dependencies(root)?;
        let baseline = derive_key(
            root,
            &selection,
            b"lock",
            TARGET,
            TARGET,
            "oven-test",
            "rustc 1.98.0",
            "cargo 1.98.0",
        )?;
        assert!(baseline.as_str().starts_with("sha256:"));
        assert_eq!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "the key is a pure function of its inputs"
        );

        // ---- A compiler source edit is not a foundation input ----
        fs::write(root.join("src/lib.rs"), "pub fn fixture_edited() {}\n")?;
        fs::write(root.join("src/new_module.rs"), "pub fn added() {}\n")?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.2.0\"\n",
        )?;
        assert_eq!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "editing, adding or re-versioning compiler source leaves the foundation key alone"
        );

        // ---- Each foundation input changes it ----
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock2",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "lock"
        );
        let mut refeatured = selection.clone();
        refeatured[1].features.push("rc".to_string());
        assert_ne!(
            baseline,
            derive_key(
                root,
                &refeatured,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "manifest (dependency features)"
        );
        let mut reversioned = selection.clone();
        reversioned[1].version = "1.0.1".to_string();
        assert_ne!(
            baseline,
            derive_key(
                root,
                &reversioned,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "manifest (dependency version)"
        );
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                "x86_64-unknown-linux-gnu",
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "target"
        );
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                "x86_64-unknown-linux-gnu",
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "host"
        );
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "release",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "profile"
        );
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.99.0",
                "cargo 1.98.0"
            )?,
            "toolchain"
        );
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.99.0"
            )?,
            "cargo"
        );
        fs::write(
            root.join("loaves/third_party/patched/src/lib.rs"),
            "pub fn patched_differently() {}\n",
        )?;
        assert_ne!(
            baseline,
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            "the foundation's own patch source"
        );

        // ---- The checkout's location is not an input either ----
        let elsewhere = tempfile::tempdir()?;
        write_compiler_root(elsewhere.path())?;
        assert_eq!(
            derive_key(
                root,
                &selection,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0"
            )?,
            {
                fs::write(
                    elsewhere.path().join("loaves/third_party/patched/src/lib.rs"),
                    "pub fn patched_differently() {}\n",
                )?;
                derive_key(
                    elsewhere.path(),
                    &dependencies(elsewhere.path())?,
                    b"lock",
                    TARGET,
                    TARGET,
                    "oven-test",
                    "rustc 1.98.0",
                    "cargo 1.98.0",
                )?
            },
            "the same inputs under another checkout path produce the same key"
        );
        Ok(())
    }

    /// The registry package root a metadata call would report for `serde`, with its crate root on disk.
    fn write_registry_package(cargo_home: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let package = cargo_home.join("registry/src/index-fixture/serde-1.0.0");
        fs::create_dir_all(package.join("src"))?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"serde\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(package.join("src/lib.rs"), "pub fn serde() {}\n")?;
        Ok(fs::canonicalize(package)?)
    }

    /// Publisher metadata for the fixture checkout: the workspace root, the registry package and the patch.
    fn metadata(root: &Path, serde_root: &Path) -> CargoMetadata {
        CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: format!("path+file://{}#fixture@0.1.0", root.display()),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: root.join("Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: format!("{REGISTRY}#serde@1.0.0"),
                    name: "serde".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: serde_root.join("Cargo.toml"),
                    source: Some(REGISTRY.to_string()),
                },
                CargoMetadataPackage {
                    id: format!(
                        "path+file://{}#patched@0.1.0",
                        root.join("loaves/third_party/patched").display()
                    ),
                    name: "patched".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: root.join("loaves/third_party/patched/Cargo.toml"),
                    source: None,
                },
            ],
            resolve: None,
        }
    }

    /// The two files a built fixture foundation would leave in staging: a target rlib and a host proc-macro dylib.
    fn write_staged_foundation(staging: &Path) -> Result<(String, String), Box<dyn std::error::Error>> {
        let target_deps = staging.join(format!("third-party-foundation-target/{TARGET}/oven-test/deps"));
        let host_deps = staging.join("third-party-foundation-target/oven-test/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        fs::write(target_deps.join("libserde-abc123.rlib"), vec![b's'; 4096])?;
        fs::write(host_deps.join("libpatched-def456.dylib"), vec![b'p'; 2048])?;
        Ok((
            format!("third-party-foundation-target/{TARGET}/oven-test/deps/libserde-abc123.rlib"),
            "third-party-foundation-target/oven-test/deps/libpatched-def456.dylib".to_string(),
        ))
    }

    /// The compiler-suite receipt for the fixture checkout as it stands now.
    fn receipt(root: &Path) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
        Ok(receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            root,
            "compiler-suite-fixture",
            TARGET,
            "rustc 1.98.0",
            "oven-test",
            Vec::new(),
        ))?)
    }

    /// Everything one test needs to publish and select a fixture family.
    struct Fixture {
        checkout: tempfile::TempDir,
        cargo_home: tempfile::TempDir,
        staging: tempfile::TempDir,
        serde_root: PathBuf,
        serde_file: String,
        patched_file: String,
        key: OvenCompilerSuiteFoundationKey,
    }

    impl Fixture {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let checkout = tempfile::tempdir()?;
            write_compiler_root(checkout.path())?;
            let cargo_home = tempfile::tempdir()?;
            let serde_root = write_registry_package(cargo_home.path())?;
            let staging = tempfile::tempdir()?;
            let (serde_file, patched_file) = write_staged_foundation(staging.path())?;
            let dependencies = dependencies(checkout.path())?;
            let key = derive_key(
                checkout.path(),
                &dependencies,
                b"lock",
                TARGET,
                TARGET,
                "oven-test",
                "rustc 1.98.0",
                "cargo 1.98.0",
            )?;
            Ok(Self {
                checkout,
                cargo_home,
                staging,
                serde_root,
                serde_file,
                patched_file,
                key,
            })
        }

        fn root(&self) -> &Path {
            self.checkout.path()
        }

        fn metadata(&self) -> CargoMetadata {
            metadata(self.root(), &self.serde_root)
        }

        /// Cargo's artifact index as the build path would produce it: absolute crate roots and staged files.
        fn built_index(&self) -> Result<BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>, Box<dyn std::error::Error>> {
            let mut index = BTreeMap::new();
            index.insert(
                CargoUnitArtifactKey {
                    package_id: format!("{REGISTRY}#serde@1.0.0"),
                    target_name: "serde".to_string(),
                    source_path: fs::canonicalize(self.serde_root.join("src/lib.rs"))?,
                    features: vec!["derive".to_string()],
                    test_profile: false,
                    platform: Some(TARGET.to_string()),
                },
                vec![fs::canonicalize(self.staging.path().join(&self.serde_file))?],
            );
            index.insert(
                CargoUnitArtifactKey {
                    package_id: format!(
                        "path+file://{}#patched@0.1.0",
                        self.root().join("loaves/third_party/patched").display()
                    ),
                    target_name: "patched".to_string(),
                    source_path: fs::canonicalize(self.root().join("loaves/third_party/patched/src/lib.rs"))?,
                    features: Vec::new(),
                    test_profile: false,
                    platform: None,
                },
                vec![fs::canonicalize(self.staging.path().join(&self.patched_file))?],
            );
            index.insert(
                CargoUnitArtifactKey {
                    package_id: format!(
                        "path+file://{}#oven-compiler-foundation@0.0.0",
                        self.staging.path().join("third-party-foundation").display()
                    ),
                    target_name: "oven-compiler-foundation".to_string(),
                    source_path: self.staging.path().join("third-party-foundation/src/lib.rs"),
                    features: Vec::new(),
                    test_profile: false,
                    platform: Some(TARGET.to_string()),
                },
                vec![fs::canonicalize(self.staging.path().join(&self.serde_file))?],
            );
            Ok(index)
        }

        /// The closure the catalog scan would produce for the staged files.
        fn closure(&self) -> Result<OvenCompilerTestSuiteArtifactClosure, Box<dyn std::error::Error>> {
            Ok(OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: vec![
                    format!("third-party-foundation-target/{TARGET}/oven-test/deps"),
                    "third-party-foundation-target/oven-test/deps".to_string(),
                ],
                native_search_paths: Vec::new(),
                supporting_artifacts: vec![
                    OvenRustcSupportingArtifact {
                        relative_path: self.serde_file.clone(),
                        digest: digest_bytes(&fs::read(self.staging.path().join(&self.serde_file))?),
                    },
                    OvenRustcSupportingArtifact {
                        relative_path: self.patched_file.clone(),
                        digest: digest_bytes(&fs::read(self.staging.path().join(&self.patched_file))?),
                    },
                ],
            })
        }

        /// Publish the fixture family as two partitions under `receipt`, the way the completion batch does.
        fn publish_family(
            &self,
            store: &OvenStore,
            receipt: &OvenReceipt,
            key: &OvenCompilerSuiteFoundationKey,
        ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
            let closure = self.closure()?;
            let records = compiler_suite_foundation_artifact_records(
                &self.built_index()?,
                &self.metadata(),
                self.root(),
                self.staging.path(),
            )?;
            let plans = super::super::compiler_suite_foundation_plans(
                &closure,
                &[
                    OvenArtifactMaterializedFile {
                        source_path: self.staging.path().join(&self.serde_file),
                        relative_path: self.serde_file.clone(),
                    },
                    OvenArtifactMaterializedFile {
                        source_path: self.staging.path().join(&self.patched_file),
                        relative_path: self.patched_file.clone(),
                    },
                ],
                // Small enough that the two files land in two partitions.
                4096 + 64 * 1024 + 512,
                key,
                records,
            )?;
            assert_eq!(plans.len(), 2, "the fixture family has two partitions");
            let requests = plans
                .into_iter()
                .map(|plan| {
                    Ok(OvenArtifactPublishRequest {
                        receipt: receipt.clone(),
                        domain: "compiler-suite".to_string(),
                        kind: OvenArtifactKind::CompilerTestSuiteFoundation,
                        payload: serde_json::to_vec(&plan.payload)?,
                        materialized_files: plan.materialized_files,
                        materialized_directories: Vec::new(),
                    })
                })
                .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
            Ok(store
                .publish_batch(&requests)?
                .into_iter()
                .map(|manifest| manifest.identity)
                .collect())
        }
    }

    fn limits() -> OvenStoreLimits {
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024)
    }

    #[test]
    fn foundation_artifact_records_are_portable_and_round_trip_through_a_stored_family() -> TestResult {
        let fixture = Fixture::new()?;
        let records = compiler_suite_foundation_artifact_records(
            &fixture.built_index()?,
            &fixture.metadata(),
            fixture.root(),
            fixture.staging.path(),
        )?;
        assert_eq!(records.len(), 2, "the private foundation root is not a record");
        let serde = records
            .iter()
            .find(|record| record.package == "serde")
            .ok_or("serde record missing")?;
        assert_eq!(serde.source.as_deref(), Some(REGISTRY));
        assert_eq!(serde.package_relative_path, None);
        assert_eq!(serde.source_relative_path, "src/lib.rs");
        assert_eq!(serde.features, ["derive"]);
        assert_eq!(serde.platform.as_deref(), Some(TARGET));
        assert_eq!(serde.files, std::slice::from_ref(&fixture.serde_file));
        let patched = records
            .iter()
            .find(|record| record.package == "patched")
            .ok_or("patched record missing")?;
        assert_eq!(patched.source, None);
        assert_eq!(
            patched.package_relative_path.as_deref(),
            Some("loaves/third_party/patched")
        );
        assert_eq!(patched.files, std::slice::from_ref(&fixture.patched_file));
        let rendered = serde_json::to_string(&records)?;
        assert!(
            !rendered.contains(&fixture.staging.path().display().to_string())
                && !rendered.contains(&fixture.cargo_home.path().display().to_string())
                && !rendered.contains(&fixture.root().display().to_string()),
            "no machine path survives in a record: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn foundation_family_is_selected_by_key_and_stands_in_for_a_cargo_build() -> TestResult {
        let fixture = Fixture::new()?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), limits());
        let first = receipt(fixture.root())?;
        let published = fixture.publish_family(&store, &first, &fixture.key)?;

        // ---- A compiler edit: a new receipt, the same foundation key ----
        fs::write(fixture.root().join("src/lib.rs"), "pub fn fixture_edited() {}\n")?;
        let second = receipt(fixture.root())?;
        assert_ne!(first.identity, second.identity);
        let key = derive_key(
            fixture.root(),
            &dependencies(fixture.root())?,
            b"lock",
            TARGET,
            TARGET,
            "oven-test",
            "rustc 1.98.0",
            "cargo 1.98.0",
        )?;
        assert_eq!(key, fixture.key);

        let family = select_or_import_compiler_suite_foundation_family(&store, &second, &key, &[])?
            .ok_or("the family was not selected by key")?;
        assert_eq!(family.selection, OvenLegacyCargoFoundationSelection::ReusedFromStore);
        assert_eq!(family.partitions.len(), 2);
        assert_eq!(family.family.key, key.as_str());
        assert!(
            family
                .partitions
                .iter()
                .all(|partition| published.contains(&partition.stored.manifest.identity))
        );

        // ---- The planner inputs come back relocated below the admitted entries ----
        let prepared = super::super::reused_compiler_suite_foundation(family, &fixture.metadata(), fixture.root())?;
        assert_eq!(prepared.build_elapsed_ms, 0);
        assert_eq!(prepared.selection, OvenLegacyCargoFoundationSelection::ReusedFromStore);
        assert_eq!(prepared.catalog.closure, fixture.closure()?);
        assert_eq!(prepared.catalog.materialized_files.len(), 2);
        for file in &prepared.catalog.materialized_files {
            assert!(
                file.source_path.starts_with(store_root.path()),
                "{}",
                file.source_path.display()
            );
            assert!(file.source_path.is_file());
            assert!(
                file.source_path.ends_with(&file.relative_path),
                "{} must end with {}",
                file.source_path.display(),
                file.relative_path
            );
        }
        let serde_key = CargoUnitArtifactKey {
            package_id: format!("{REGISTRY}#serde@1.0.0"),
            target_name: "serde".to_string(),
            source_path: fs::canonicalize(fixture.serde_root.join("src/lib.rs"))?,
            features: vec!["derive".to_string()],
            test_profile: false,
            platform: Some(TARGET.to_string()),
        };
        let serde_files = prepared
            .artifact_index
            .get(&serde_key)
            .ok_or("the serde unit is not indexed after rehydration")?;
        assert_eq!(serde_files.len(), 1);
        assert_eq!(
            prepared
                .catalog
                .by_source_path
                .get(&serde_files[0])
                .map(|(relative, _)| relative.as_str()),
            Some(fixture.serde_file.as_str())
        );
        assert_eq!(prepared.artifact_index.len(), 2);
        assert_eq!(prepared.plans.len(), 2);

        // ---- Re-publication under the new receipt adopts the admitted files by link ----
        let requests = prepared
            .plans
            .into_iter()
            .map(|plan| {
                Ok(OvenArtifactPublishRequest {
                    receipt: second.clone(),
                    domain: "compiler-suite".to_string(),
                    kind: OvenArtifactKind::CompilerTestSuiteFoundation,
                    payload: serde_json::to_vec(&plan.payload)?,
                    materialized_files: plan.materialized_files,
                    materialized_directories: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let republished = store.publish_batch(&requests)?;
        assert_eq!(republished.len(), 2);
        for manifest in &republished {
            assert!(!published.contains(&manifest.identity), "a new receipt is a new entry");
            assert_eq!(manifest.receipt_identity, second.identity);
            assert_eq!(manifest.build_unit_identity, second.build_unit_identity);
        }
        let held = store.select_payloads_for_execution(
            &republished
                .iter()
                .map(|manifest| manifest.identity.clone())
                .collect::<Vec<_>>(),
        )?;
        for entry in &held {
            for file in &entry.manifest.materialized_files {
                let metadata = fs::metadata(entry.artifact_root.join(&file.relative_path))?;
                assert!(
                    metadata.nlink() >= 2,
                    "{} was copied rather than adopted by link",
                    file.relative_path
                );
            }
            let payload = serde_json::from_slice::<OvenCompilerTestSuiteFoundationPayload>(&entry.payload)?;
            assert_eq!(
                payload.family.as_ref().map(|family| family.key.as_str()),
                Some(key.as_str())
            );
        }
        Ok(())
    }

    #[test]
    fn foundation_selection_misses_on_another_key_an_incomplete_family_or_a_prior_schema() -> TestResult {
        let fixture = Fixture::new()?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), limits());
        let first = receipt(fixture.root())?;
        let published = fixture.publish_family(&store, &first, &fixture.key)?;

        // ---- Another key: what a changed lock, manifest, target, profile or toolchain derives ----
        let other = OvenCompilerSuiteFoundationKey::fixture("sha256:another-foundation");
        assert!(select_or_import_compiler_suite_foundation_family(&store, &first, &other, &[])?.is_none());

        // ---- Another intent under the same key is not a candidate ----
        let mut other_intent = first.clone();
        other_intent.intent.profile = "release".to_string();
        assert!(select_or_import_compiler_suite_foundation_family(&store, &other_intent, &fixture.key, &[])?.is_none());

        // ---- A prior-schema partition carries no family and is never selected ----
        let prior = OvenCompilerTestSuiteFoundationPayload {
            schema_version: 1,
            label: "foundation-0000".to_string(),
            artifact_closure: fixture.closure()?,
            family: None,
        };
        store.publish(&OvenArtifactPublishRequest {
            receipt: first.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuiteFoundation,
            payload: serde_json::to_vec(&prior)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let selected = select_or_import_compiler_suite_foundation_family(&store, &first, &fixture.key, &[])?
            .ok_or("the complete family is still selectable beside a prior-schema entry")?;
        assert_eq!(selected.partitions.len(), 2);
        drop(selected);

        // ---- An incomplete family is a miss, not an error ----
        let incomplete = tempfile::tempdir()?;
        let incomplete_store = OvenStore::new(incomplete.path(), limits());
        republish_one_partition(&store, &published, 1, &incomplete_store, &first)?;
        assert!(
            select_or_import_compiler_suite_foundation_family(&incomplete_store, &first, &fixture.key, &[])?.is_none(),
            "one partition of two is not a family"
        );
        Ok(())
    }

    /// Copy the partition with `partition_index` of the family published as `identities` in `from` into `into`
    /// under `receipt`, the way a store that lost or never received the family's other partitions would hold it.
    fn republish_one_partition(
        from: &OvenStore,
        identities: &[String],
        partition_index: u32,
        into: &OvenStore,
        receipt: &OvenReceipt,
    ) -> TestResult {
        let held = from.select_payloads_for_execution(identities)?;
        let partition = held
            .iter()
            .find(|entry| {
                serde_json::from_slice::<OvenCompilerTestSuiteFoundationPayload>(&entry.payload)
                    .ok()
                    .and_then(|payload| payload.family)
                    .is_some_and(|family| family.partition_index == partition_index)
            })
            .ok_or("partition missing")?;
        into.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuiteFoundation,
            payload: partition.payload.clone(),
            materialized_files: partition
                .manifest
                .materialized_files
                .iter()
                .map(|file| OvenArtifactMaterializedFile {
                    source_path: partition.artifact_root.join(&file.relative_path),
                    relative_path: file.relative_path.clone(),
                })
                .collect(),
            materialized_directories: Vec::new(),
        })?;
        Ok(())
    }

    #[test]
    fn a_partial_family_on_a_mirror_is_never_selected_so_cargo_builds() -> TestResult {
        let fixture = Fixture::new()?;
        let producer = receipt(fixture.root())?;
        let complete_root = tempfile::tempdir()?;
        let complete = OvenStore::new(complete_root.path(), limits());
        let identities = fixture.publish_family(&complete, &producer, &fixture.key)?;
        // The mirror holds partition 1 of two: the other was reclaimed there, or its batch never finished.
        let mirror_root = tempfile::tempdir()?;
        let mirror = OvenStore::new(mirror_root.path(), limits());
        republish_one_partition(&complete, &identities, 1, &mirror, &producer)?;

        fs::write(fixture.root().join("src/lib.rs"), "pub fn fixture_edited() {}\n")?;
        let consumer = receipt(fixture.root())?;
        let local_root = tempfile::tempdir()?;
        let local = OvenStore::new(local_root.path(), limits());
        let selected = select_or_import_compiler_suite_foundation_family(
            &local,
            &consumer,
            &fixture.key,
            &[mirror_root.path().to_path_buf()],
        )?;
        assert!(
            selected.is_none(),
            "one partition of two on a mirror is not a family; Cargo builds the foundation"
        );
        // What the import admitted is verified and served locally, but it never stands in for the family alone.
        let admitted = local.select_payloads_matching_for_execution(|_| true)?;
        assert_eq!(
            admitted.len(),
            1,
            "the mirror's one partition was imported through verification"
        );
        drop(admitted);
        assert!(
            select_compiler_suite_foundation_family(&local, &consumer, &fixture.key)?.is_none(),
            "the stray partition never forms a family on a later lookup either"
        );
        Ok(())
    }

    #[test]
    fn partitions_of_two_builds_under_one_key_are_never_paired() -> TestResult {
        // Build A and build B share the key (same inputs) but not the bytes: B's patched artifact was compiled again.
        let build_a = Fixture::new()?;
        let build_b = Fixture::new()?;
        // The same size keeps the two-partition split; only the bytes differ.
        fs::write(build_b.staging.path().join(&build_b.patched_file), vec![b'q'; 2048])?;
        assert_ne!(
            super::super::compiler_suite_foundation_closure_digest(&build_a.closure()?),
            super::super::compiler_suite_foundation_closure_digest(&build_b.closure()?),
            "the fixture builds differ by content"
        );
        let producer_a = receipt(build_a.root())?;
        let producer_b = receipt(build_b.root())?;
        let store_a_root = tempfile::tempdir()?;
        let store_a = OvenStore::new(store_a_root.path(), limits());
        let identities_a = build_a.publish_family(&store_a, &producer_a, &build_a.key)?;
        let store_b_root = tempfile::tempdir()?;
        let store_b = OvenStore::new(store_b_root.path(), limits());
        let identities_b = build_b.publish_family(&store_b, &producer_b, &build_a.key)?;

        // ---- Partition 0 of A beside partition 1 of B: every index is present, and it is still not a family ----
        let mixed_root = tempfile::tempdir()?;
        let mixed = OvenStore::new(mixed_root.path(), limits());
        republish_one_partition(&store_a, &identities_a, 0, &mixed, &producer_a)?;
        republish_one_partition(&store_b, &identities_b, 1, &mixed, &producer_b)?;
        assert!(
            select_compiler_suite_foundation_family(&mixed, &producer_a, &build_a.key)?.is_none(),
            "one partition of each build under one key is two incomplete families, not one complete one"
        );

        // ---- A complete build beside a stray partition of the other: the complete build, whole ----
        republish_one_partition(&store_a, &identities_a, 1, &mixed, &producer_a)?;
        let selected = select_compiler_suite_foundation_family(&mixed, &producer_a, &build_a.key)?
            .ok_or("build A is complete in the store")?;
        let digest_a = super::super::compiler_suite_foundation_closure_digest(&build_a.closure()?);
        assert_eq!(selected.family.closure_digest, digest_a);
        assert!(
            selected.partitions.iter().all(|partition| {
                partition
                    .payload
                    .family
                    .as_ref()
                    .is_some_and(|record| record.closure_digest == digest_a)
            }),
            "every selected partition comes from build A"
        );
        Ok(())
    }

    #[test]
    fn foundation_family_is_admitted_from_a_mirror_through_verification() -> TestResult {
        let fixture = Fixture::new()?;
        let mirror_root = tempfile::tempdir()?;
        let mirror = OvenStore::new(mirror_root.path(), limits());
        // The mirror was baked by another checkout of the same sources: another receipt, the same key.
        let producer = receipt(fixture.root())?;
        let on_mirror = fixture.publish_family(&mirror, &producer, &fixture.key)?;
        fs::write(fixture.root().join("src/lib.rs"), "pub fn fixture_edited() {}\n")?;
        let consumer = receipt(fixture.root())?;
        assert_ne!(producer.identity, consumer.identity);

        let local_root = tempfile::tempdir()?;
        let local = OvenStore::new(local_root.path(), limits());
        let family = select_or_import_compiler_suite_foundation_family(
            &local,
            &consumer,
            &fixture.key,
            &[mirror_root.path().to_path_buf()],
        )?
        .ok_or("the mirror family was not admitted")?;
        assert_eq!(family.selection, OvenLegacyCargoFoundationSelection::ReusedFromMirror);
        assert_eq!(family.partitions.len(), 2);
        for partition in &family.partitions {
            assert!(
                partition.stored.artifact_root.starts_with(local_root.path()),
                "served locally after import"
            );
            assert_eq!(partition.stored.manifest.receipt_identity, consumer.identity);
            assert!(!on_mirror.contains(&partition.stored.manifest.identity));
            for file in &partition.stored.manifest.materialized_files {
                assert!(partition.stored.artifact_root.join(&file.relative_path).is_file());
            }
        }
        drop(family);
        // The next lookup is answered locally without consulting any mirror.
        let again = select_or_import_compiler_suite_foundation_family(&local, &consumer, &fixture.key, &[])?;
        assert_eq!(
            again.map(|family| family.selection),
            Some(OvenLegacyCargoFoundationSelection::ReusedFromStore)
        );

        // ---- A tampered mirror entry is refused and nothing is admitted ----
        let tampered_root = tempfile::tempdir()?;
        let tampered = OvenStore::new(tampered_root.path(), limits());
        let identities = fixture.publish_family(&tampered, &producer, &fixture.key)?;
        let held = tampered.select_payloads_for_execution(&identities)?;
        let victims = held
            .iter()
            .flat_map(|entry| {
                entry
                    .manifest
                    .materialized_files
                    .iter()
                    .map(|file| entry.artifact_root.join(&file.relative_path))
            })
            .collect::<Vec<_>>();
        drop(held);
        assert_eq!(victims.len(), 2, "one file per partition to tamper with");
        for victim in &victims {
            fs::remove_file(victim)?;
            fs::write(victim, b"altered on the mirror")?;
        }
        let empty_root = tempfile::tempdir()?;
        let empty = OvenStore::new(empty_root.path(), limits());
        let result = select_or_import_compiler_suite_foundation_family(
            &empty,
            &consumer,
            &fixture.key,
            &[tampered_root.path().to_path_buf()],
        );
        assert!(
            matches!(
                result,
                Err(OvenLegacyCargoError::Store(
                    oven_store::store::OvenStoreError::Integrity { .. }
                ))
            ),
            "got {:?}",
            result.err()
        );
        assert!(empty.select_payloads_matching_for_execution(|_| true)?.is_empty());
        Ok(())
    }
}
