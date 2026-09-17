//! Validation of a runtime foundation against its selected Rust facet graph.

use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    OvenRustcArtifactManifest, OvenRustcError, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
    selected_graph_source_digest,
};
use super::{
    OvenRuntimeFoundationPackageSource, OvenRuntimeFoundationSourceInventory, OvenRuntimeFoundationUnit,
    OvenRuntimeFoundationUnitExecution, ValidatedOvenRuntimeFoundation,
};

/// Check that every selected unit has one exact source inventory and no executable build-unit fields.
pub(crate) fn validate_runtime_foundation_source_inventories(
    foundation: &ValidatedOvenRuntimeFoundation,
    records: Vec<OvenRuntimeFoundationSourceInventory>,
) -> Result<BTreeMap<String, OvenRuntimeFoundationSourceInventory>, OvenRustcError> {
    let units = foundation
        .selected_graph()
        .graph()
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut inventories = BTreeMap::new();
    for record in records {
        validate_sha256_identity(
            &record.selected_identity,
            "runtime foundation source-inventory unit identity",
        )?;
        let unit = units.get(record.selected_identity.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation source inventories",
                format!("names unknown selected unit {}", record.selected_identity),
            )
        })?;
        validate_runtime_foundation_package_source(unit, &record.package)?;
        if inventories.insert(record.selected_identity.clone(), record).is_some() {
            return Err(runtime_foundation_invalid(
                "runtime foundation source inventories",
                "declares one selected unit more than once",
            ));
        }
    }
    let expected = units.keys().copied().collect::<BTreeSet<_>>();
    let actual = inventories.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
        let mut message = Vec::new();
        if !missing.is_empty() {
            message.push(format!(
                "omits selected unit source inventory(s): {}",
                missing.join(", ")
            ));
        }
        if !extra.is_empty() {
            message.push(format!(
                "names unknown selected unit source inventory(s): {}",
                extra.join(", ")
            ));
        }
        return Err(runtime_foundation_invalid(
            "runtime foundation source inventories",
            message.join("; "),
        ));
    }
    Ok(inventories)
}

/// Validate retained package source evidence against its existing selected compiler unit.
pub(crate) fn validate_runtime_foundation_package_source(
    unit: &OvenSelectedRustFacetUnit,
    package: &OvenRuntimeFoundationPackageSource,
) -> Result<(), OvenRustcError> {
    if package.root.owner != unit.source.owner {
        return Err(runtime_foundation_invalid(
            "runtime foundation package root",
            format!("unit {} package root has a different selected owner", unit.crate_name),
        ));
    }
    let package_root = portable_package_root_components(&package.root.path)?;
    let compiler_root = portable_package_root_components(&unit.source.root)?;
    if !compiler_root.starts_with(&package_root) {
        return Err(runtime_foundation_invalid(
            "runtime foundation package root",
            format!(
                "unit {} package root is not an ancestor of its selected compiler root",
                unit.crate_name
            ),
        ));
    }
    if package.manifest.path != "Cargo.toml" {
        return Err(runtime_foundation_invalid(
            "runtime foundation package manifest",
            format!("unit {} package manifest must be Cargo.toml", unit.crate_name),
        ));
    }
    validate_sha256_identity(&package.manifest.digest, "runtime foundation package manifest digest")?;
    let _ = selected_graph_source_digest(&package.members)
        .map_err(|error| runtime_foundation_invalid("runtime foundation package source members", error.to_string()))?;
    if package.members.windows(2).any(|pair| pair[0].path >= pair[1].path) {
        return Err(runtime_foundation_invalid(
            "runtime foundation package source members",
            format!("unit {} package members must be sorted and unique", unit.crate_name),
        ));
    }
    if package
        .members
        .iter()
        .any(|member| member.path == package.manifest.path)
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation package source members",
            format!("unit {} repeats its manifest in package members", unit.crate_name),
        ));
    }
    let compiler_root_relative_to_package = &compiler_root[package_root.len()..];
    for unit_member in &unit.source_members {
        let package_member_path = compiler_root_relative_to_package
            .iter()
            .copied()
            .chain(std::iter::once(unit_member.path.as_str()))
            .collect::<Vec<_>>()
            .join("/");
        if package_member_path == package.manifest.path {
            if unit_member.digest != package.manifest.digest {
                return Err(runtime_foundation_invalid(
                    "runtime foundation package manifest",
                    format!(
                        "unit {} gives compiler-visible manifest bytes conflicting with package inventory",
                        unit.crate_name
                    ),
                ));
            }
            continue;
        }
        match package.members.iter().find(|member| member.path == package_member_path) {
            Some(package_member) if package_member.digest == unit_member.digest => {}
            Some(_) => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation package source members",
                    format!(
                        "unit {} gives compiler-visible source member {package_member_path} conflicting package bytes",
                        unit.crate_name
                    ),
                ));
            }
            None => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation package source members",
                    format!(
                        "unit {} omits compiler-visible source member {package_member_path} from package inventory",
                        unit.crate_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate an owner-relative portable package root, treating `.` as the owner root and rejecting traversal or
/// platform-specific paths.
fn portable_package_root_components(value: &str) -> Result<Vec<&str>, OvenRustcError> {
    if value == "." {
        return Ok(Vec::new());
    }
    if value.trim().is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation package root",
            "must be a normalized portable owner-relative path",
        ));
    }
    Ok(value.split('/').collect())
}

/// Derive a deterministic rebuild order from the shared selected graph without resolving any new dependency edge.
pub(crate) fn runtime_foundation_rebuild_order(
    graph: &OvenSelectedRustFacetGraph,
    policies: &BTreeMap<String, OvenRuntimeFoundationUnit>,
) -> Result<Vec<String>, OvenRustcError> {
    let units = graph
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut pending = policies
        .iter()
        .filter_map(|(identity, policy)| {
            matches!(policy.execution, OvenRuntimeFoundationUnitExecution::Rebuild).then_some(identity.clone())
        })
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .filter_map(|identity| {
                let unit = units.get(identity.as_str())?;
                unit.dependencies
                    .iter()
                    .all(|dependency| !pending.contains(&dependency.unit))
                    .then_some(identity.clone())
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(runtime_foundation_invalid(
                "runtime foundation rebuild graph",
                "has no compiler-owned dependency frontier",
            ));
        }
        for identity in ready {
            pending.remove(&identity);
            order.push(identity);
        }
    }
    Ok(order)
}

/// Validate one declared policy against its selected graph unit and the sealed artifact/source catalogue.
pub(crate) fn validate_runtime_unit_policy(
    unit: &OvenSelectedRustFacetUnit,
    policy: &OvenRuntimeFoundationUnit,
    artifact_owner: &str,
    artifacts: &OvenRustcArtifactManifest,
    declared_artifacts: &BTreeMap<String, String>,
) -> Result<(), OvenRustcError> {
    match &policy.execution {
        OvenRuntimeFoundationUnitExecution::Prebuilt { artifact } => {
            if unit.source.kind != OvenSelectedRustFacetSourceKind::Registry {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt unit",
                    format!("unit {} is not registry-backed", unit.crate_name),
                ));
            }
            if artifact.crate_name != unit.crate_name {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt artifact",
                    format!(
                        "unit {} maps to artifact crate {}",
                        unit.crate_name, artifact.crate_name
                    ),
                ));
            }
            if !artifacts.externs.iter().any(|declared| declared == artifact) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt artifact",
                    format!(
                        "unit {} artifact {} is not exposed as a sealed direct extern",
                        unit.crate_name, artifact.relative_path
                    ),
                ));
            }
            match declared_artifacts.get(&artifact.relative_path) {
                Some(digest) if digest == &artifact.digest => {}
                Some(_) => {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation prebuilt artifact",
                        format!("artifact {} has a different declared digest", artifact.relative_path),
                    ));
                }
                None => {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation prebuilt artifact",
                        format!("artifact {} is absent from the sealed manifest", artifact.relative_path),
                    ));
                }
            }
            let source_matches = artifacts
                .registry_sources
                .iter()
                .filter(|source| {
                    source.package == unit.package
                        && source.version == unit.package_version
                        && source.source.digest == unit.source.digest
                })
                .collect::<Vec<_>>();
            let [source] = source_matches.as_slice() else {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt source",
                    format!(
                        "unit {} must match exactly one sealed registry-source record, found {}",
                        source_matches.len(),
                        unit.crate_name
                    ),
                ));
            };
            if unit.source.owner != artifact_owner || unit.source.root != source.source.relative_root {
                return Err(runtime_foundation_invalid(
                    "runtime foundation prebuilt source",
                    format!(
                        "unit {} does not bind its registry source to the sealed foundation owner/root",
                        unit.crate_name
                    ),
                ));
            }
            for input in &unit.generated_inputs {
                if input.source.owner != artifact_owner {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation generated input",
                        format!(
                            "unit {} names generated input {} outside the sealed foundation owner",
                            unit.crate_name, input.name
                        ),
                    ));
                }
                if declared_artifacts.get(&input.source.path) != Some(&input.digest) {
                    return Err(runtime_foundation_invalid(
                        "runtime foundation generated input",
                        format!(
                            "unit {} names generated input {} absent from the sealed artifact manifest",
                            unit.crate_name, input.name
                        ),
                    ));
                }
            }
        }
        OvenRuntimeFoundationUnitExecution::Rebuild => {
            if unit.source.kind != OvenSelectedRustFacetSourceKind::Compiler {
                return Err(runtime_foundation_invalid(
                    "runtime foundation rebuild unit",
                    format!("unit {} is not compiler-owned source", unit.crate_name),
                ));
            }
            if !matches!(
                (unit.role, unit.crate_kind),
                (
                    OvenSelectedRustFacetUnitRole::Library,
                    OvenSelectedRustFacetCrateKind::Rlib
                ) | (
                    OvenSelectedRustFacetUnitRole::ProcMacro,
                    OvenSelectedRustFacetCrateKind::ProcMacro
                )
            ) {
                return Err(runtime_foundation_invalid(
                    "runtime foundation rebuild unit",
                    format!(
                        "unit {} has unsupported rebuild role {:?} / kind {:?}",
                        unit.crate_name, unit.role, unit.crate_kind
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate a canonical SHA-256 content identity without treating a human-readable coordinate as compiler input.
pub(crate) fn validate_sha256_identity(value: &str, field: &'static str) -> Result<(), OvenRustcError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(runtime_foundation_invalid(field, "must be a SHA-256 identity"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(runtime_foundation_invalid(field, "has malformed SHA-256 bytes"));
    }
    Ok(())
}

/// Construct a typed runtime-foundation validation failure.
pub(crate) fn runtime_foundation_invalid(field: &'static str, message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field,
        message: message.into(),
    }
}
