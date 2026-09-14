//! Validation of a runtime foundation against the selected Rust facet graph it describes: provider records, package
//! sources, declarations, host dependencies, receipts, the rebuild order, and the per-unit policy.

use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    OvenRustcArtifactManifest, OvenRustcError, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetGraph, OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetUnit,
    OvenSelectedRustFacetUnitRole, selected_graph_source_digest,
};
use super::{
    OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION, OvenRuntimeFoundationNativeLinkState,
    OvenRuntimeFoundationProviderDeclaration, OvenRuntimeFoundationProviderHostDependency,
    OvenRuntimeFoundationProviderPackageSource, OvenRuntimeFoundationProviderReceipt,
    OvenRuntimeFoundationProviderRecord, OvenRuntimeFoundationProviderState, OvenRuntimeFoundationUnit,
    OvenRuntimeFoundationUnitExecution, ValidatedOvenRuntimeFoundation, runtime_foundation_provider_effect_digest,
    runtime_foundation_provider_receipt_identity,
};

/// Check that each selected unit has exactly one authoritative provider-effect classification.
pub(super) fn validate_runtime_foundation_provider_records(
    foundation: &ValidatedOvenRuntimeFoundation,
    records: Vec<OvenRuntimeFoundationProviderRecord>,
) -> Result<BTreeMap<String, OvenRuntimeFoundationProviderRecord>, OvenRustcError> {
    let units = foundation
        .selected_graph()
        .graph()
        .units
        .iter()
        .map(|unit| (unit.identity.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut providers = BTreeMap::new();
    for record in records {
        validate_sha256_identity(&record.selected_identity, "runtime foundation provider unit identity")?;
        let unit = units.get(record.selected_identity.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation providers",
                format!("names unknown selected unit {}", record.selected_identity),
            )
        })?;
        validate_runtime_foundation_provider_record(unit, &units, &record)?;
        if providers.insert(record.selected_identity.clone(), record).is_some() {
            return Err(runtime_foundation_invalid(
                "runtime foundation providers",
                "declares one selected unit more than once",
            ));
        }
    }
    let expected = units.keys().copied().collect::<BTreeSet<_>>();
    let actual = providers.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
        let mut message = Vec::new();
        if !missing.is_empty() {
            message.push(format!("omits selected unit provider state(s): {}", missing.join(", ")));
        }
        if !extra.is_empty() {
            message.push(format!(
                "names unknown selected unit provider state(s): {}",
                extra.join(", ")
            ));
        }
        return Err(runtime_foundation_invalid(
            "runtime foundation providers",
            message.join("; "),
        ));
    }
    Ok(providers)
}

/// Ensure one provider record agrees with the one shared selected graph rather than recreating unit facts.
fn validate_runtime_foundation_provider_record(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    record: &OvenRuntimeFoundationProviderRecord,
) -> Result<(), OvenRustcError> {
    validate_runtime_foundation_provider_declaration(unit, units, &record.declaration)?;
    match (&record.declaration, &record.state) {
        (
            OvenRuntimeFoundationProviderDeclaration::NoBuildScript { .. },
            OvenRuntimeFoundationProviderState::NoProvider,
        ) => {
            if unit.generated_inputs.is_empty() {
                Ok(())
            } else {
                Err(runtime_foundation_invalid(
                    "runtime foundation providers",
                    format!(
                        "unit {} names generated input(s) but declares no build script",
                        unit.crate_name
                    ),
                ))
            }
        }
        (OvenRuntimeFoundationProviderDeclaration::NoBuildScript { .. }, _) => Err(runtime_foundation_invalid(
            "runtime foundation providers",
            format!(
                "unit {} declares no build script but carries provider execution facts",
                unit.crate_name
            ),
        )),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::NoProvider,
        ) => Err(runtime_foundation_invalid(
            "runtime foundation providers",
            format!(
                "unit {} declares a build script but omits its provider execution result",
                unit.crate_name
            ),
        )),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::Captured { receipt },
        ) => validate_runtime_foundation_provider_receipt(unit, record, receipt),
        (
            OvenRuntimeFoundationProviderDeclaration::BuildScript { .. },
            OvenRuntimeFoundationProviderState::Unsupported { reason },
        ) => {
            if reason.trim().is_empty() {
                return Err(runtime_foundation_invalid(
                    "runtime foundation providers",
                    format!("unit {} has an empty unsupported-provider reason", unit.crate_name),
                ));
            }
            Ok(())
        }
    }
}

/// Validate one retained package source against its existing selected compiler unit.
pub(super) fn validate_runtime_foundation_provider_package_source(
    unit: &OvenSelectedRustFacetUnit,
    package: &OvenRuntimeFoundationProviderPackageSource,
) -> Result<(), OvenRustcError> {
    if package.root.owner != unit.source.owner {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider package root",
            format!("unit {} package root has a different selected owner", unit.crate_name),
        ));
    }
    let package_root = portable_provider_package_root_components(&package.root.path)?;
    let compiler_root = portable_provider_package_root_components(&unit.source.root)?;
    if !compiler_root.starts_with(&package_root) {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider package root",
            format!(
                "unit {} package root does not contain its compiler root",
                unit.crate_name
            ),
        ));
    }
    if package.manifest.path != "Cargo.toml" {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider manifest",
            format!(
                "unit {} manifest must be Cargo.toml relative to its package root",
                unit.crate_name
            ),
        ));
    }
    validate_sha256_identity(&package.manifest.digest, "runtime foundation provider manifest digest")?;
    let _ = selected_graph_source_digest(&package.members)
        .map_err(|error| runtime_foundation_invalid("runtime foundation provider source members", error.to_string()))?;
    if package.members.windows(2).any(|pair| pair[0].path >= pair[1].path) {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider source members",
            format!(
                "unit {} provider package members must be sorted and unique",
                unit.crate_name
            ),
        ));
    }
    if package
        .members
        .iter()
        .any(|member| member.path == package.manifest.path)
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider manifest",
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
                    "runtime foundation provider manifest",
                    format!(
                        "unit {} gives compiler-visible manifest bytes conflicting with its package manifest",
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
                    "runtime foundation provider source members",
                    format!(
                        "unit {} gives compiler-visible source member {} conflicting package bytes",
                        unit.crate_name, package_member_path
                    ),
                ));
            }
            None => {
                return Err(runtime_foundation_invalid(
                    "runtime foundation provider source members",
                    format!(
                        "unit {} omits compiler-visible source member {} from its package inventory",
                        unit.crate_name, package_member_path
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Return whether one provider rerun observation names a byte already retained by the package declaration.
///
/// The manifest has its own typed field so it cannot be confused with a generic source member, but it remains a
/// real package-root file that a build script may read or explicitly request for rerun observation.
pub(super) fn runtime_foundation_provider_package_contains_path(
    package: &OvenRuntimeFoundationProviderPackageSource,
    path: &str,
) -> bool {
    path == package.manifest.path || package.members.iter().any(|member| member.path == path)
}

/// Split one portable package root into normalized components without letting host-native path syntax change scope.
fn portable_provider_package_root_components(value: &str) -> Result<Vec<&str>, OvenRustcError> {
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
            "runtime foundation provider package root",
            "must be a normalized portable owner-relative path",
        ));
    }
    Ok(value.split('/').collect())
}

/// Validate a producer-declared build script entirely against already selected source and host units.
pub(super) fn validate_runtime_foundation_provider_declaration(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    declaration: &OvenRuntimeFoundationProviderDeclaration,
) -> Result<(), OvenRustcError> {
    let (package, entrypoint, digest, edition, host_dependencies) = match declaration {
        OvenRuntimeFoundationProviderDeclaration::NoBuildScript { package } => {
            return validate_runtime_foundation_provider_package_source(unit, package);
        }
        OvenRuntimeFoundationProviderDeclaration::BuildScript {
            package,
            entrypoint,
            digest,
            edition,
            host_dependencies,
        } => (package, entrypoint, digest, edition, host_dependencies),
    };
    validate_runtime_foundation_provider_package_source(unit, package)?;
    let package_members = &package.members;
    validate_sha256_identity(digest, "runtime foundation provider build-script digest")?;
    let member = package_members
        .iter()
        .find(|member| member.path == *entrypoint)
        .ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider build-script entrypoint",
                format!(
                    "unit {} names a source member absent from the declared package inventory",
                    unit.crate_name
                ),
            )
        })?;
    if member.digest != *digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider build-script digest",
            format!(
                "unit {} build-script digest differs from its declared provider source member",
                unit.crate_name
            ),
        ));
    }
    if edition != &unit.edition {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider build-script edition",
            format!(
                "unit {} build-script edition differs from its selected unit",
                unit.crate_name
            ),
        ));
    }
    validate_runtime_foundation_provider_host_dependencies(unit, units, host_dependencies)
}

/// Bind exact host-domain build dependencies to already selected direct edges.
pub(super) fn validate_runtime_foundation_provider_host_dependencies(
    unit: &OvenSelectedRustFacetUnit,
    units: &BTreeMap<&str, &OvenSelectedRustFacetUnit>,
    host_dependencies: &[OvenRuntimeFoundationProviderHostDependency],
) -> Result<(), OvenRustcError> {
    if host_dependencies
        .windows(2)
        .any(|pair| pair[0].alias > pair[1].alias || (pair[0].alias == pair[1].alias && pair[0].unit >= pair[1].unit))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider host dependencies",
            format!("unit {} host dependencies must be sorted and unique", unit.crate_name),
        ));
    }
    let mut aliases = BTreeSet::new();
    for dependency in host_dependencies {
        if dependency.alias.trim().is_empty() || !aliases.insert(dependency.alias.as_str()) {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} has an empty or repeated build-script dependency alias",
                    unit.crate_name
                ),
            ));
        }
        validate_sha256_identity(&dependency.unit, "runtime foundation provider host dependency unit")?;
        let dependency_unit = units.get(dependency.unit.as_str()).ok_or_else(|| {
            runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!("unit {} names an unknown build-script host dependency", unit.crate_name),
            )
        })?;
        if dependency_unit.domain != OvenSelectedRustFacetDomain::Host {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!("unit {} names a non-host build-script dependency", unit.crate_name),
            ));
        }
        if dependency.unit == unit.identity {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} cannot depend on itself while compiling its build script",
                    unit.crate_name
                ),
            ));
        }
        if !unit
            .dependencies
            .iter()
            .any(|selected| selected.alias == dependency.alias && selected.unit == dependency.unit)
        {
            return Err(runtime_foundation_invalid(
                "runtime foundation provider host dependencies",
                format!(
                    "unit {} names a host dependency absent from its selected direct dependency edges",
                    unit.crate_name
                ),
            ));
        }
    }
    Ok(())
}

/// Validate a complete typed receipt against both its own canonical hashes and the selected graph facts it projects.
fn validate_runtime_foundation_provider_receipt(
    unit: &OvenSelectedRustFacetUnit,
    record: &OvenRuntimeFoundationProviderRecord,
    receipt: &OvenRuntimeFoundationProviderReceipt,
) -> Result<(), OvenRustcError> {
    let OvenRuntimeFoundationProviderDeclaration::BuildScript { package, .. } = &record.declaration else {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt",
            format!(
                "unit {} has a receipt without a build-script declaration",
                unit.crate_name
            ),
        ));
    };
    if receipt.schema_version != OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt schema",
            format!(
                "expected schema {OVEN_RUNTIME_FOUNDATION_PROVIDER_RECEIPT_SCHEMA_VERSION}, found {}",
                receipt.schema_version
            ),
        ));
    }
    validate_sha256_identity(
        &receipt.provider_receipt_identity,
        "runtime foundation provider receipt identity",
    )?;
    validate_sha256_identity(&receipt.effect_digest, "runtime foundation provider effect digest")?;
    let expected_effect_digest = runtime_foundation_provider_effect_digest(&receipt.effects)?;
    if receipt.effect_digest != expected_effect_digest {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider effect digest",
            format!(
                "unit {} receipt does not match its complete typed effects",
                unit.crate_name
            ),
        ));
    }
    let expected_receipt_identity =
        runtime_foundation_provider_receipt_identity(&record.selected_identity, &record.declaration, &receipt.effects)?;
    if receipt.provider_receipt_identity != expected_receipt_identity {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider receipt identity",
            format!(
                "unit {} receipt does not match its declaration and effects",
                unit.crate_name
            ),
        ));
    }
    let effects = &receipt.effects;
    if effects.generated_inputs != unit.generated_inputs {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider generated inputs",
            format!(
                "unit {} provider outputs do not exactly match its selected generated inputs",
                unit.crate_name
            ),
        ));
    }
    if effects.emitted_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.emitted_cfg.iter().any(|cfg| cfg.trim().is_empty())
        || effects.emitted_cfg.iter().any(|cfg| !unit.cfg.contains(cfg))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider cfg",
            format!(
                "unit {} provider cfg values must be unique and selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if effects
        .emitted_environment
        .iter()
        .any(|(name, value)| name.trim().is_empty() || unit.environment.get(name) != Some(value))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider environment",
            format!(
                "unit {} provider environment must be selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if effects.checked_cfg.windows(2).any(|pair| pair[0] >= pair[1])
        || effects.checked_cfg.iter().any(|cfg| cfg.trim().is_empty())
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider checked cfg",
            format!(
                "unit {} provider checked cfg values must be unique and nonempty",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_paths.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_paths
            .iter()
            .any(|path| !runtime_foundation_provider_package_contains_path(package, path))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun paths",
            format!(
                "unit {} provider rerun paths must name declared package source members",
                unit.crate_name
            ),
        ));
    }
    if effects.rerun_environment.windows(2).any(|pair| pair[0] >= pair[1])
        || effects
            .rerun_environment
            .iter()
            .any(|name| name.trim().is_empty() || !unit.environment.contains_key(name))
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider rerun environment",
            format!(
                "unit {} provider rerun environment must be selected by the shared graph",
                unit.crate_name
            ),
        ));
    }
    if let OvenRuntimeFoundationNativeLinkState::Unsupported { reason } = &effects.native_link
        && reason.trim().is_empty()
    {
        return Err(runtime_foundation_invalid(
            "runtime foundation provider native link",
            format!("unit {} has an empty unsupported native-link reason", unit.crate_name),
        ));
    }
    Ok(())
}

/// Derive a deterministic rebuild order from the shared selected graph without resolving any new dependency edge.
pub(super) fn runtime_foundation_rebuild_order(
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
pub(super) fn validate_runtime_unit_policy(
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
pub(super) fn validate_sha256_identity(value: &str, field: &'static str) -> Result<(), OvenRustcError> {
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
pub(super) fn runtime_foundation_invalid(field: &'static str, message: impl Into<String>) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field,
        message: message.into(),
    }
}
