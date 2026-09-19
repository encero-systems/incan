//! Publishing and admitting a runtime-foundation asset: the immutable directory that carries a validated foundation,
//! its selected-package source inventories and the members they name, sealed under one identity and audited member by
//! member.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::super::{
    OvenRustcError, OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetOwnerKind,
    OvenSelectedRustFacetOwnerRoot, canonical_directory, digest_bytes, normalized_relative_path, safe_path,
    verified_regular_file,
};
use super::{
    OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME, OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION,
    OvenAdmittedRuntimeFoundationAsset, OvenRuntimeFoundation, OvenRuntimeFoundationAsset,
    OvenRuntimeFoundationSourceInventory, ValidatedOvenRuntimeFoundationAsset, runtime_foundation_invalid,
};

/// Publish one complete source-inventory-bearing runtime foundation into an installed asset root.
///
/// The caller must supply the selected foundation and exhaustive source-inventory facts explicitly; this function never
/// reads Cargo metadata, searches a target directory, or reconstructs source evidence. It first materializes the
/// supplied roots to verify every source/artifact byte and source-inventory gate, copies only descriptor-derived
/// foundation members into a sibling staging directory, re-admits that staged payload, and only then atomically exposes
/// it at `destination`.
///
/// The separately held Toolchain owner is verified but never copied into this asset root. It stays part of the
/// installed compiler distribution rather than becoming an unrecorded foundation member.
pub fn publish_runtime_foundation_asset(
    asset: OvenRuntimeFoundationAsset,
    source_foundation_root: &Path,
    toolchain_root: &Path,
    destination: &Path,
) -> Result<OvenAdmittedRuntimeFoundationAsset, OvenRustcError> {
    let destination_parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let destination_parent = canonical_directory(destination_parent, "runtime foundation asset destination parent")?;
    let destination_name = destination.file_name().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "must name one output directory below its parent",
        )
    })?;
    let destination = destination_parent.join(destination_name);
    // Do not rehash/copy a source inventory payload if an immutable installed root already owns this name.
    require_absent_runtime_foundation_destination(&destination)?;

    let mut asset = asset;
    canonicalize_runtime_foundation_asset_facts(&mut asset.foundation, &mut asset.source_inventories)?;
    let validated = asset.clone().validated()?;
    let source_foundation_root = canonical_directory(source_foundation_root, "runtime foundation source root")?;
    let toolchain_root = canonical_directory(toolchain_root, "runtime foundation toolchain root")?;
    let owner_roots =
        runtime_foundation_asset_owner_roots(&validated, source_foundation_root.clone(), toolchain_root.clone())?;
    // Verify source, generated, native and artifact facts before creating any output.
    let _ = validated.materialize_for_publication(&owner_roots)?;

    let members = runtime_foundation_asset_member_paths(&validated)?;
    let staging = create_runtime_foundation_asset_staging_directory(&destination)?;
    let result = (|| {
        copy_runtime_foundation_asset_members(&source_foundation_root, &staging, &members)?;
        write_runtime_foundation_asset_descriptor(&staging, &asset)?;

        // Re-admission after copying closes the source-to-stage race and proves the staged root is complete before
        // the rename makes it visible to a normal command.
        let staged = admit_runtime_foundation_asset_for_publication(&staging, &toolchain_root)?;
        let _ = staged.materialize_asset_for_publication()?;
        require_absent_runtime_foundation_destination(&destination)?;
        fs::rename(&staging, &destination).map_err(|source| OvenRustcError::Io {
            path: destination.clone(),
            source,
        })?;
        admit_runtime_foundation_asset_for_publication(&destination, &toolchain_root)
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Require that a publisher never replaces an already installed foundation asset.
fn require_absent_runtime_foundation_destination(destination: &Path) -> Result<(), OvenRustcError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(runtime_foundation_invalid(
            "runtime foundation asset destination",
            format!("{} already exists", destination.display()),
        )),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(OvenRustcError::Io {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

/// Create a sibling private directory so `rename` is atomic within the installed-assets filesystem.
fn create_runtime_foundation_asset_staging_directory(destination: &Path) -> Result<PathBuf, OvenRustcError> {
    let parent = destination.parent().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "has no parent directory for atomic publication",
        )
    })?;
    let name = destination.file_name().ok_or_else(|| {
        runtime_foundation_invalid(
            "runtime foundation asset destination",
            "has no final directory name for staging",
        )
    })?;
    let name = name.to_string_lossy();
    for attempt in 0..128_u32 {
        let staging = parent.join(format!(".{name}.staging-{}-{attempt}", std::process::id()));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(OvenRustcError::Io { path: staging, source });
            }
        }
    }
    Err(runtime_foundation_invalid(
        "runtime foundation asset destination",
        "could not reserve a private staging directory",
    ))
}

/// Copy exactly the foundation-owned physical members named by one validated descriptor.
fn copy_runtime_foundation_asset_members(
    source_root: &Path,
    staging_root: &Path,
    members: &RuntimeFoundationAssetMemberCatalog,
) -> Result<(), OvenRustcError> {
    for directory in &members.directories {
        let destination = staging_root.join(directory);
        fs::create_dir_all(&destination).map_err(|source| OvenRustcError::Io {
            path: destination,
            source,
        })?;
    }
    for relative_path in &members.files {
        if relative_path == OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME {
            continue;
        }
        let source = safe_path(source_root, relative_path, "runtime foundation source member")?;
        let source = verified_regular_file(&source, "runtime foundation source member")?;
        let destination = staging_root.join(relative_path);
        let parent = destination.parent().ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation asset member",
                format!("{relative_path} has no parent directory"),
            )
        })?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        fs::copy(&source, &destination).map_err(|source| OvenRustcError::Io {
            path: destination.clone(),
            source,
        })?;
        let _ = verified_regular_file(&destination, "published runtime foundation member")?;
    }
    Ok(())
}

/// Write the canonical complete foundation descriptor only after all declared payload members reached staging.
fn write_runtime_foundation_asset_descriptor(
    staging_root: &Path,
    asset: &OvenRuntimeFoundationAsset,
) -> Result<(), OvenRustcError> {
    let bytes = serde_json::to_vec(asset).map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation descriptor",
            format!("cannot encode canonical asset: {error}"),
        )
    })?;
    let descriptor = staging_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME);
    fs::write(&descriptor, bytes).map_err(|source| OvenRustcError::Io {
        path: descriptor,
        source,
    })
}

/// Load one release-owned runtime-foundation descriptor and audit its complete foundation-owned file set.
///
/// This is explicit publisher work. It reads only the named descriptor, its exact declared files, and the separately
/// supplied toolchain root. It never probes a Cargo manifest, registry/cache directory, target directory or ambient
/// compiler state. A normal command receives a later store-owned closure rather than calling this loader.
pub fn admit_runtime_foundation_asset_for_publication(
    foundation_root: &Path,
    toolchain_root: &Path,
) -> Result<OvenAdmittedRuntimeFoundationAsset, OvenRustcError> {
    let foundation_root = canonical_directory(foundation_root, "runtime foundation asset root")?;
    let toolchain_root = canonical_directory(toolchain_root, "runtime foundation toolchain root")?;
    let descriptor = verified_regular_file(
        &foundation_root.join(OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME),
        "runtime foundation descriptor",
    )?;
    let bytes = fs::read(&descriptor).map_err(|source| OvenRustcError::Io {
        path: descriptor.clone(),
        source,
    })?;
    let descriptor_value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation descriptor",
            format!("cannot decode {}: {error}", descriptor.display()),
        )
    })?;
    let schema_version = descriptor_value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| runtime_foundation_invalid("runtime foundation asset schema", "is missing or not an integer"))?;
    if schema_version != u64::from(OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION) {
        return Err(runtime_foundation_invalid(
            "runtime foundation asset schema",
            format!(
                "expected schema {OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION}, found {schema_version}; rebuild and republish the asset"
            ),
        ));
    }
    let asset = serde_json::from_value::<OvenRuntimeFoundationAsset>(descriptor_value).map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation descriptor",
            format!("cannot decode {}: {error}", descriptor.display()),
        )
    })?;
    let asset = asset.validated()?;
    let owner_roots = runtime_foundation_asset_owner_roots(&asset, foundation_root.clone(), toolchain_root)?;
    audit_runtime_foundation_asset_members(&foundation_root, &asset)?;
    Ok(OvenAdmittedRuntimeFoundationAsset { asset, owner_roots })
}

/// Bind the asset's two allowed owner identities to explicit physical roots.
fn runtime_foundation_asset_owner_roots(
    asset: &ValidatedOvenRuntimeFoundationAsset,
    foundation_root: PathBuf,
    toolchain_root: PathBuf,
) -> Result<Vec<OvenSelectedRustFacetOwnerRoot>, OvenRustcError> {
    let foundation = asset.foundation();
    let graph = foundation.selected_graph().graph();
    let foundation_owner = foundation.artifact_owner();
    let toolchain_owner = graph
        .owners
        .iter()
        .find(|owner| owner.kind == OvenSelectedRustFacetOwnerKind::Toolchain)
        .map(|owner| owner.identity.clone())
        .ok_or_else(|| runtime_foundation_invalid("runtime foundation owners", "has no Toolchain owner"))?;
    let mut owner_roots = Vec::with_capacity(graph.owners.len());
    let mut identities = BTreeSet::new();
    for owner in &graph.owners {
        if !identities.insert(owner.identity.as_str()) {
            return Err(runtime_foundation_invalid(
                "runtime foundation owners",
                "declare one owner identity more than once",
            ));
        }
        let root = match owner.kind {
            OvenSelectedRustFacetOwnerKind::Toolchain if owner.identity == toolchain_owner => toolchain_root.clone(),
            OvenSelectedRustFacetOwnerKind::Constituent if owner.identity == foundation_owner => {
                foundation_root.clone()
            }
            // Generated outputs are copied into descriptor-named directories below the sealed asset. Their separate
            // identities preserve producer provenance while the asset root remains the sole physical publication.
            OvenSelectedRustFacetOwnerKind::GeneratedOutput => foundation_root.clone(),
            _ => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation owners",
                    "may name only its artifact constituent, generated outputs sealed inside that asset, and the separately held Toolchain owner",
                ));
            }
        };
        owner_roots.push(OvenSelectedRustFacetOwnerRoot {
            identity: owner.identity.clone(),
            root,
        });
    }
    if !identities.contains(foundation_owner) || !identities.contains(toolchain_owner.as_str()) {
        return Err(runtime_foundation_invalid(
            "runtime foundation owners",
            "omit the artifact constituent or Toolchain owner",
        ));
    }
    Ok(owner_roots)
}

/// One exact descriptor-derived regular-file and directory catalogue for a release foundation root.
#[derive(Debug, Default)]
struct RuntimeFoundationAssetMemberCatalog {
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
}

/// Audit every member below the immutable foundation root against the descriptor-derived catalogue.
///
/// Source trees and artifacts carry their own digest catalogues and are rehashed by later materialization. This walk
/// establishes the complementary invariant: no undeclared file or directory, symlink or special filesystem member
/// may hide alongside them in the release payload.
fn audit_runtime_foundation_asset_members(
    foundation_root: &Path,
    asset: &ValidatedOvenRuntimeFoundationAsset,
) -> Result<(), OvenRustcError> {
    let expected = runtime_foundation_asset_member_paths(asset)?;
    let mut actual = RuntimeFoundationAssetMemberCatalog::default();
    collect_runtime_foundation_asset_members(foundation_root, foundation_root, &mut actual)?;
    if actual.files != expected.files || actual.directories != expected.directories {
        let missing_files = expected.files.difference(&actual.files).cloned().collect::<Vec<_>>();
        let extra_files = actual.files.difference(&expected.files).cloned().collect::<Vec<_>>();
        let missing_directories = expected
            .directories
            .difference(&actual.directories)
            .cloned()
            .collect::<Vec<_>>();
        let extra_directories = actual
            .directories
            .difference(&expected.directories)
            .cloned()
            .collect::<Vec<_>>();
        let mut message = Vec::new();
        if !missing_files.is_empty() {
            message.push(format!("omits declared file(s): {}", missing_files.join(", ")));
        }
        if !extra_files.is_empty() {
            message.push(format!("contains undeclared file(s): {}", extra_files.join(", ")));
        }
        if !missing_directories.is_empty() {
            message.push(format!(
                "omits declared directory(s): {}",
                missing_directories.join(", ")
            ));
        }
        if !extra_directories.is_empty() {
            message.push(format!(
                "contains undeclared directory(s): {}",
                extra_directories.join(", ")
            ));
        }
        return Err(runtime_foundation_invalid(
            "runtime foundation asset members",
            message.join("; "),
        ));
    }
    Ok(())
}

/// Derive every allowed regular file and directory below the foundation root from the selected graph and artifact
/// manifest.
fn runtime_foundation_asset_member_paths(
    asset: &ValidatedOvenRuntimeFoundationAsset,
) -> Result<RuntimeFoundationAssetMemberCatalog, OvenRustcError> {
    let foundation = asset.foundation();
    let graph = foundation.selected_graph().graph();
    let foundation_owner = foundation.artifact_owner();
    // Build-script outputs live below the asset root under their own GeneratedOutput owners, the same root the
    // owner table maps them to; the asset must carry them beside what the constituent owns directly.
    let sealed_below_root = |owner: &str| {
        owner == foundation_owner
            || graph.owners.iter().any(|candidate| {
                candidate.identity == owner && candidate.kind == OvenSelectedRustFacetOwnerKind::GeneratedOutput
            })
    };
    let mut members = RuntimeFoundationAssetMemberCatalog::default();
    record_runtime_foundation_asset_file(
        &mut members,
        OVEN_RUNTIME_FOUNDATION_ASSET_FILENAME,
        "runtime foundation descriptor",
    )?;
    for artifact in foundation.artifacts().declared_artifact_paths()? {
        record_runtime_foundation_asset_file(&mut members, &artifact, "runtime foundation artifact")?;
    }
    for unit in &graph.units {
        if unit.source.owner == foundation_owner {
            record_runtime_foundation_asset_directory(
                &mut members,
                &unit.source.root,
                "runtime foundation source root",
            )?;
            for source_member in &unit.source_members {
                record_runtime_foundation_asset_file(
                    &mut members,
                    &runtime_foundation_asset_member_path(
                        &unit.source.root,
                        &source_member.path,
                        "runtime foundation source member",
                    )?,
                    "runtime foundation source member",
                )?;
            }
        }
        for directory in unit
            .include_dirs
            .iter()
            .chain(unit.exclude_dirs.iter())
            .filter(|directory| directory.owner == foundation_owner)
        {
            record_runtime_foundation_asset_directory(
                &mut members,
                &directory.path,
                "runtime foundation source directory",
            )?;
        }
        for environment in unit.environment.values() {
            if let OvenSelectedRustFacetEnvironmentValue::Path { value } = environment
                && sealed_below_root(&value.owner)
            {
                // Foundation-owned path environment values are directories in v1 (such as OUT_DIR). A file-valued
                // environment input needs its own digest-bearing schema rather than becoming an untracked exception.
                record_runtime_foundation_asset_directory(
                    &mut members,
                    &value.path,
                    "runtime foundation environment directory",
                )?;
            }
        }
        for generated in &unit.generated_inputs {
            if sealed_below_root(&generated.source.owner) {
                record_runtime_foundation_asset_directory(
                    &mut members,
                    &generated.source.path,
                    "runtime foundation generated root",
                )?;
                for member in &generated.members {
                    record_runtime_foundation_asset_file(
                        &mut members,
                        &runtime_foundation_asset_member_path(
                            &generated.source.path,
                            &member.path,
                            "runtime foundation generated member",
                        )?,
                        "runtime foundation generated member",
                    )?;
                }
            }
        }
    }
    for record in asset.source_inventories.values() {
        let package = &record.package;
        if package.root.owner != foundation_owner {
            continue;
        }
        record_runtime_foundation_asset_directory(
            &mut members,
            &package.root.path,
            "runtime foundation package source root",
        )?;
        record_runtime_foundation_asset_file(
            &mut members,
            &runtime_foundation_asset_member_path(
                &package.root.path,
                &package.manifest.path,
                "runtime foundation package manifest",
            )?,
            "runtime foundation package manifest",
        )?;
        for source_member in &package.members {
            record_runtime_foundation_asset_file(
                &mut members,
                &runtime_foundation_asset_member_path(
                    &package.root.path,
                    &source_member.path,
                    "runtime foundation package source member",
                )?,
                "runtime foundation package source member",
            )?;
        }
    }
    Ok(members)
}

/// Join a source-tree root and one source member while preserving the portable foundation-root path vocabulary.
fn runtime_foundation_asset_member_path(
    source_root: &str,
    member: &str,
    kind: &'static str,
) -> Result<String, OvenRustcError> {
    let path = if source_root == "." {
        member.to_string()
    } else {
        format!("{source_root}/{member}")
    };
    normalized_relative_path(&path, kind)
}

/// Add one declared regular file and every required parent directory to the exact asset catalogue.
fn record_runtime_foundation_asset_file(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
    kind: &'static str,
) -> Result<(), OvenRustcError> {
    let path = normalized_relative_path(path, kind)?;
    record_runtime_foundation_asset_parent_directories(members, &path)?;
    members.files.insert(path);
    Ok(())
}

/// Add one declared directory and every parent directory to the exact asset catalogue.
fn record_runtime_foundation_asset_directory(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
    kind: &'static str,
) -> Result<(), OvenRustcError> {
    if path == "." {
        return Ok(());
    }
    let path = normalized_relative_path(path, kind)?;
    record_runtime_foundation_asset_parent_directories(members, &path)?;
    members.directories.insert(path);
    Ok(())
}

/// Add every non-root parent of a portable path to the exact asset directory catalogue.
fn record_runtime_foundation_asset_parent_directories(
    members: &mut RuntimeFoundationAssetMemberCatalog,
    path: &str,
) -> Result<(), OvenRustcError> {
    let mut current = PathBuf::from(path);
    while let Some(directory) = current.parent() {
        if directory.as_os_str().is_empty() {
            break;
        }
        let directory = normalized_relative_path(&directory.to_string_lossy(), "runtime foundation asset directory")?;
        members.directories.insert(directory.clone());
        current = PathBuf::from(directory);
    }
    Ok(())
}

/// Enumerate an asset root without following links or treating unrecorded directory contents as benign.
fn collect_runtime_foundation_asset_members(
    root: &Path,
    directory: &Path,
    members: &mut RuntimeFoundationAssetMemberCatalog,
) -> Result<(), OvenRustcError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("contains symlink {}", path.display()),
            ));
        }
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).map_err(|_| {
                runtime_foundation_invalid(
                    "runtime foundation asset members",
                    format!("directory {} escaped its foundation root", path.display()),
                )
            })?;
            let relative = normalized_relative_path(&relative.to_string_lossy(), "runtime foundation asset directory")?;
            if !members.directories.insert(relative) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation asset members",
                    "contains one portable directory path more than once",
                ));
            }
            collect_runtime_foundation_asset_members(root, &path, members)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("contains non-regular member {}", path.display()),
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            runtime_foundation_invalid(
                "runtime foundation asset members",
                format!("member {} escaped its foundation root", path.display()),
            )
        })?;
        let relative = normalized_relative_path(&relative.to_string_lossy(), "runtime foundation asset member")?;
        if !members.files.insert(relative) {
            return Err(runtime_foundation_invalid(
                "runtime foundation asset members",
                "contains one portable member path more than once",
            ));
        }
    }
    Ok(())
}

/// Derive the content identity of a release asset without confusing it with a compiled-unit output identity.
pub(crate) fn runtime_foundation_asset_identity(
    foundation: &OvenRuntimeFoundation,
    source_inventories: &[OvenRuntimeFoundationSourceInventory],
) -> Result<String, OvenRustcError> {
    let mut foundation = foundation.clone();
    let mut source_inventories = source_inventories.to_vec();
    canonicalize_runtime_foundation_asset_facts(&mut foundation, &mut source_inventories)?;
    let bytes = serde_json::to_vec(&(
        "incan.oven.runtime-foundation-asset/4",
        OVEN_RUNTIME_FOUNDATION_ASSET_SCHEMA_VERSION,
        &foundation,
        &source_inventories,
    ))
    .map_err(|error| {
        runtime_foundation_invalid(
            "runtime foundation asset identity",
            format!("cannot encode canonical asset facts: {error}"),
        )
    })?;
    Ok(digest_bytes(&bytes))
}

/// Normalize presentation-order fields before they become a release-asset identity input.
///
/// The selected graph already has a canonical form, while foundation execution policy and source inventories are
/// semantically maps keyed by selected identity. Their arrival order must not manufacture a second foundation or
/// defeat reuse. Artifact-manifest ordering is deliberately left intact because its search-path order can affect the
/// compiler invocation and is therefore part of the manifest's observable contract.
pub(crate) fn canonicalize_runtime_foundation_asset_facts(
    foundation: &mut OvenRuntimeFoundation,
    source_inventories: &mut [OvenRuntimeFoundationSourceInventory],
) -> Result<(), OvenRustcError> {
    foundation.selected_graph = foundation
        .selected_graph
        .clone()
        .validated()
        .map_err(|error| runtime_foundation_invalid("runtime foundation selected graph", error.to_string()))?
        .graph()
        .clone();
    foundation
        .units
        .sort_by(|left, right| left.selected_identity.cmp(&right.selected_identity));
    source_inventories.sort_by(|left, right| left.selected_identity.cmp(&right.selected_identity));
    for inventory in source_inventories {
        inventory.package.members.sort();
    }
    Ok(())
}
