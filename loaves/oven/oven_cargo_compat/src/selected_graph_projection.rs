//! Projection of sealed Cargo capture facts into Oven's portable selected Rust graph.
//!
//! Cargo capture observes compiled units, but it intentionally has no authority to assign portable source owners or
//! authored feature requests. This module joins that observation to publisher-retained physical bindings and emits a
//! rootless graph. The caller must bind roots with an admitted project or compiler-support authority afterwards.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use oven_model::manifest::ProjectManifest;
use oven_rustc::loaf::OvenLoaf;
use oven_rustc::rustc::{
    OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION, OvenCompilerSupportRootIntentAuthority, OvenRuntimeFoundation,
    OvenRuntimeFoundationPackageSource, OvenRuntimeFoundationSourceInventory, OvenRuntimeFoundationUnit,
    OvenRuntimeFoundationUnitExecution, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
    OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetLinkedLibrary, OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind,
    OvenSelectedRustFacetPath, OvenSelectedRustFacetPurpose, OvenSelectedRustFacetSelection,
    OvenSelectedRustFacetSource, OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember,
    OvenSelectedRustFacetTargetSpec, OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
    ValidatedOvenSelectedRustFacetGraph, bind_compiler_support_root_intents, selected_graph_environment_retains_text,
    selected_graph_sha256, selected_graph_unit_identity,
};
use oven_store::OvenReceipt;
use oven_store::{receipt_with_build_unit_input, receipt_with_compiler_support_root_intent};
use serde::{Deserialize, Serialize};

use super::LoafRegistryAuthority;
use super::{
    OvenLegacyCargoBuildScriptToolProbe, OvenLegacyCargoError, OvenLegacyCargoInspectionSource,
    OvenLegacyCargoSelectedGeneratedOutput, OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture,
};

/// Receipt key binding the complete selected build-script closure to a final publisher transaction.
pub const OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT: &str =
    oven_store::OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_BUILD_UNIT_INPUT;

/// Publisher-retained physical binding for one Cargo-selected unit.
///
/// The capture owns the observed package, target, feature and edge facts. This record supplies only the portable
/// owner-relative source facts that the capture cannot safely derive from local paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenLegacyCargoSelectedGraphUnitBinding {
    /// Complete source identity already retained beneath an immutable publisher owner.
    pub source: OvenSelectedRustFacetSource,
    /// Complete source members retained under that owner; never reconstructed from a capture checkout path.
    pub source_members: Vec<OvenSelectedRustFacetSourceMember>,
    /// Exact sealed registry catalog record when this selected unit came from a registry package.
    pub registry_source: Option<OvenRustcRegistrySourcePackage>,
    /// Source directories visible to inspection, expressed under retained owners.
    pub include_dirs: Vec<OvenSelectedRustFacetPath>,
    /// Source directories intentionally excluded from inspection, expressed under retained owners.
    pub exclude_dirs: Vec<OvenSelectedRustFacetPath>,
}

/// One retained generated-output binding consumed through an exact build-script edge.
///
/// The binding names the already copied output owner. It does not authorize executing the build script again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenLegacyCargoSelectedGeneratedBinding {
    /// Stable generated input name consumed by the selected unit.
    pub name: String,
    /// Exact retained output tree reference.
    pub source: OvenSelectedRustFacetPath,
    /// Complete digest of the retained output tree.
    pub digest: String,
}

/// One selected environment value matched to an exact build-script observation.
///
/// Every `rustc-env` line a build script emitted gets one binding, so the closure digest states what was observed
/// and what became of it; the binding decides whether the graph carries the value. RFC 119 makes a script's
/// directives capture provenance rather than unit authority, and the selected graph admits an environment entry
/// as text only under a registered public compiler fact, so an observation under any other name is retained here
/// and in the closure digest, and withheld from the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoSelectedEnvironmentBinding {
    /// Exact raw Cargo observation retained only for producer-side comparison.
    pub observed_value: String,
    /// Portable selected value that enters graph identity after the comparison succeeds, or `None` when the graph
    /// admits no retained representation for the name: the entry then stays identity-bound through the build-script
    /// closure digest and does not reach the graph.
    pub value: Option<OvenSelectedRustFacetEnvironmentValue>,
}

/// Complete typed linked-library closure matched to one build-script observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoSelectedLinkedLibraryBinding {
    /// Exact raw Cargo library directives in compiler-observed order, including duplicates.
    pub observed_libraries: Vec<String>,
    /// Exact raw Cargo search-path directives in compiler-observed order, including duplicates.
    pub observed_paths: Vec<String>,
    /// Ordered typed archive or provider facts that enter graph identity without deduplication.
    pub libraries: Vec<OvenSelectedRustFacetLinkedLibrary>,
}

/// One complete selected build-script closure consumed by an exact physical unit edge.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct OvenLegacyCargoSelectedBuildScriptBinding {
    /// Each exact selected environment key and its portable representation.
    pub environment: BTreeMap<String, OvenLegacyCargoSelectedEnvironmentBinding>,
    /// The exact typed linked-library closure, when the build script emitted link facts.
    pub linked_libraries: Option<OvenLegacyCargoSelectedLinkedLibraryBinding>,
    /// The registry declaration that governs this edge in place of its observation, when one was adopted.
    ///
    /// Absent for an observation-governed edge, so such edges keep their closure identity unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration: Option<OvenLegacyCargoDeclaredBuildScriptFacts>,
}

/// The registry declaration one adopted build-script edge is sealed under.
///
/// The declaration names the exact registry content it came from, so the build-script closure digest, and with it
/// the capture receipt, binds which published facts replaced the observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoDeclaredBuildScriptFacts {
    /// Package name as the registry publishes it.
    pub package: String,
    /// Exact version.
    pub version: String,
    /// Canonical checksum of the source both the capture and the registry describe.
    pub checksum: String,
    /// Identity of the exact index line the declaration was selected from.
    pub index_line_digest: String,
    /// Declared `cfg` answers, which the observation was required to match.
    pub cfg: Vec<String>,
    /// Declared generated inputs by name and digest, which the observation was required to match.
    pub out: BTreeMap<String, String>,
}

/// All non-Cargo physical facts needed to turn one capture into a portable raw graph.
///
/// Every member is publisher-retained evidence. The projection compares it to capture where a common fact exists;
/// it never turns an absolute Cargo path, effective feature, or package name into an authority identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenLegacyCargoSelectedGraphProjection {
    /// Frozen target, toolchain, profile, purpose and compiler cfg snapshots.
    pub selection: OvenSelectedRustFacetSelection,
    /// Complete owner table for sources, generated output and the selected toolchain.
    pub owners: Vec<OvenSelectedRustFacetOwner>,
    /// One binding for every non-build-script capture unit, keyed by capture index.
    pub units: BTreeMap<usize, OvenLegacyCargoSelectedGraphUnitBinding>,
    /// Generated-output bindings keyed by `(consumer unit, run-custom-build unit)` capture indices.
    pub generated: BTreeMap<(usize, usize), OvenLegacyCargoSelectedGeneratedBinding>,
    /// Environment and linked-library closures keyed by the same consuming build-script edge.
    pub build_scripts: BTreeMap<(usize, usize), OvenLegacyCargoSelectedBuildScriptBinding>,
}

/// Final receipt and selected graph produced by one physical capture and one authored compiler-support declaration.
pub struct OvenFinalizedCompilerSupportSelectedGraph {
    /// Pruned physical capture the graph was projected from; run-custom-build units stay as edge endpoints.
    pub capture: OvenLegacyCargoSelectedUnitCapture,
    /// Receipt retaining the physical build-script closure before authored roots are attached.
    pub capture_receipt: OvenReceipt,
    /// Distinct final receipt binding the exact compiler-support root authority.
    pub final_receipt: OvenReceipt,
    /// Rooted graph validated against both receipts; its units are in canonical identity order.
    pub graph: ValidatedOvenSelectedRustFacetGraph,
    /// The selected unit identity projected from each compiled `capture` unit, by capture index.
    ///
    /// Run-custom-build units have no entry: they are not graph units. This is the only authenticated
    /// correspondence between a physical unit and its selected identity once the graph is canonicalized.
    pub unit_identities: BTreeMap<usize, String>,
    /// For each pruned `capture` index, the index of the same physical unit in the capture the finalizer was given.
    ///
    /// Pruning renumbers dependency edges, so a pruned unit's capture identity differs from the original's; a
    /// publisher that binds artifacts observed in a fresh capture must derive identities from the original units.
    pub source_indices: Vec<usize>,
}

/// Bind every selected immutable registry unit to the exact artifact in its compiled Loaf.
///
/// The foundation's artifact owner is the graph's one Constituent owner: the identity every registry source in the
/// graph is rooted under, which the asset later maps to the foundation root. It is read from the graph rather than
/// recomputed by the caller because the projection labels sources with the receipt it was given, and the finalized
/// capture receipt is derived from the pruned capture, so the two identities need not agree.
pub fn runtime_foundation_from_compiled_loaf(
    finalized: &OvenFinalizedCompilerSupportSelectedGraph,
    loaf: &OvenLoaf,
    compiled_plan_identity: &str,
) -> Result<OvenRuntimeFoundation, OvenLegacyCargoError> {
    let graph = finalized.graph.graph();
    let constituents = graph
        .owners
        .iter()
        .filter(|owner| owner.kind == OvenSelectedRustFacetOwnerKind::Constituent)
        .collect::<Vec<_>>();
    let [artifact_owner] = constituents.as_slice() else {
        return Err(projection_error(
            "runtime foundation artifact owner",
            "the selected graph does not name exactly one Constituent owner",
        ));
    };
    let artifact_owner = artifact_owner.identity.as_str();
    if loaf.plan.intent.target != graph.selection.intent.target
        || loaf.plan.intent.toolchain != graph.selection.intent.toolchain
        || loaf.plan.intent.profile != graph.selection.intent.profile
    {
        return Err(projection_error(
            "runtime foundation compiled Loaf",
            "target, toolchain or profile differs from the selected graph",
        ));
    }
    let mut units = Vec::with_capacity(graph.units.len());
    let mut selected_artifacts = BTreeMap::new();
    for unit in &graph.units {
        if unit.source.kind != OvenSelectedRustFacetSourceKind::Registry {
            return Err(projection_error(
                "runtime foundation unit",
                "is not an immutable registry-backed prebuilt unit",
            ));
        }
        let matches = loaf
            .registry_leaves
            .iter()
            .filter(|leaf| {
                leaf.package == unit.package
                    && leaf.selected_unit_identity.as_deref() == Some(unit.identity.as_str())
                    && leaf_domain_matches(leaf, unit)
                    && leaf.version == unit.package_version
                    && leaf.crate_name == unit.crate_name
                    && leaf.features == unit.features
                    && registry_source_identity(&leaf.package, &leaf.version) == unit.source.identity
                    && leaf.source.digest == unit.source.digest
            })
            .collect::<Vec<_>>();
        let [leaf] = matches.as_slice() else {
            return Err(projection_error(
                "runtime foundation unit",
                "does not have exactly one matching compiled registry artifact",
            ));
        };
        if selected_artifacts
            .insert(leaf.artifact.relative_path.clone(), (unit.domain, unit.crate_kind))
            .is_some()
        {
            return Err(projection_error(
                "runtime foundation compiled artifact",
                "does not carry an exact one-to-one selected-unit binding",
            ));
        }
        units.push(OvenRuntimeFoundationUnit {
            selected_identity: unit.identity.clone(),
            domain: unit.domain,
            execution: OvenRuntimeFoundationUnitExecution::Prebuilt {
                artifact: leaf.artifact.clone(),
            },
        });
    }
    Ok(OvenRuntimeFoundation {
        schema_version: OVEN_RUNTIME_FOUNDATION_SCHEMA_VERSION,
        compiler_closure_digest: compiled_plan_identity.to_string(),
        artifact_owner: artifact_owner.to_string(),
        artifacts: loaf.plan.clone(),
        selected_graph: graph.clone(),
        units,
    })
}

/// The one portable identity a registry-backed selected unit's source carries: the `registry:` coordinate.
///
/// Every consumer of a selected graph — its validator, the policy exchange, the runtime foundation and executor —
/// names a registry source this way; the registry URL and checksum stay in the sealed catalog beside it.
pub fn registry_source_identity(package: &str, version: &str) -> String {
    format!("registry:{package}@{version}")
}

/// Whether a sealed registry leaf serves the same domain and unit kind as a selected graph unit.
fn leaf_domain_matches(leaf: &oven_rustc::rustc::OvenRustcRegistryLeaf, unit: &OvenSelectedRustFacetUnit) -> bool {
    use oven_rustc::rustc::{OvenRustcRegistryLeafDomain, OvenRustcRegistryLeafKind};
    let domain = match unit.domain {
        OvenSelectedRustFacetDomain::Host => OvenRustcRegistryLeafDomain::Host,
        OvenSelectedRustFacetDomain::Target => OvenRustcRegistryLeafDomain::Target,
    };
    let kind = match unit.crate_kind {
        OvenSelectedRustFacetCrateKind::ProcMacro => OvenRustcRegistryLeafKind::ProcMacro,
        _ => OvenRustcRegistryLeafKind::Rlib,
    };
    leaf.domain == domain && leaf.crate_kind == kind
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyManifestInventory {
    path: String,
    bytes_hex: String,
    sha256_hex: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyPackageIdentity {
    name: String,
    version: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicySourceInventory {
    package: PolicyPackageIdentity,
    domain: OvenSelectedRustFacetDomain,
    owner: String,
    package_root: String,
    manifest: PolicyManifestInventory,
    members: Vec<String>,
    build_unit_present: bool,
    effective_features: Vec<String>,
}

/// Join selected policy inventories to every physical unit sharing the exact retained source.
pub fn runtime_foundation_inventories_from_policy_response(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    response: &serde_json::Value,
) -> Result<Vec<OvenRuntimeFoundationSourceInventory>, OvenLegacyCargoError> {
    if response.get("schema").and_then(serde_json::Value::as_str) != Some("incan.oven.rust-policy-exchange/5")
        || response.get("operation").and_then(serde_json::Value::as_str) != Some("validate_selected_rust_graph")
        || response.get("status").and_then(serde_json::Value::as_str) != Some("selected")
        || response.get("graph_digest").and_then(serde_json::Value::as_str) != Some(selected.digest())
    {
        // A refusal names a structural kind and the exchange path it applies to; neither is prose, and both are
        // what a producer needs to act on. The engine may add the rule that refused, in words, for the operator.
        let status = response
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("absent");
        let error = response.get("error");
        let kind = error
            .and_then(|error| error.get("kind"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("-");
        let fields = error
            .and_then(|error| error.get("fields"))
            .and_then(serde_json::Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .unwrap_or_default();
        let detail = error
            .and_then(|error| error.get("detail"))
            .and_then(serde_json::Value::as_str)
            .map(|detail| format!(": {detail}"))
            .unwrap_or_default();
        return Err(projection_error(
            "Rust policy response",
            &format!(
                "does not select the exact requested graph (status `{status}`, error `{kind}` at `{fields}`{detail})"
            ),
        ));
    }
    let encoded = response
        .get("inventories")
        .cloned()
        .ok_or_else(|| projection_error("Rust policy inventories", "are absent"))?;
    let records: Vec<PolicySourceInventory> = serde_json::from_value(encoded)
        .map_err(|error| projection_error("Rust policy inventories", &error.to_string()))?;
    let mut catalogs = BTreeMap::new();
    for record in records {
        let bytes = hex::decode(&record.manifest.bytes_hex)
            .map_err(|_| projection_error("Rust policy manifest", "contains invalid byte hex"))?;
        let digest = selected_graph_sha256(&bytes);
        if digest.strip_prefix("sha256:") != Some(record.manifest.sha256_hex.as_str()) {
            return Err(projection_error(
                "Rust policy manifest",
                "bytes do not match its declared digest",
            ));
        }
        let key = (
            record.owner,
            record.package_root,
            record.package.name,
            record.package.version,
            record.package.source,
            selected_domain_key(record.domain),
        );
        if catalogs
            .insert(
                key,
                (
                    record.manifest.path,
                    digest,
                    record.members,
                    record.build_unit_present,
                    record.effective_features,
                ),
            )
            .is_some()
        {
            return Err(projection_error(
                "Rust policy inventory",
                "duplicates one retained source",
            ));
        }
    }
    let mut used = BTreeSet::new();
    let mut inventories = Vec::with_capacity(selected.graph().units.len());
    for unit in &selected.graph().units {
        let key = (
            unit.source.owner.clone(),
            unit.source.root.clone(),
            unit.package.clone(),
            unit.package_version.clone(),
            unit.source.identity.clone(),
            selected_domain_key(unit.domain),
        );
        let (manifest_path, manifest_digest, member_paths, _, effective_features) = catalogs
            .get(&key)
            .ok_or_else(|| projection_error("Rust policy inventories", "omit one selected source"))?;
        if effective_features != &unit.features {
            return Err(projection_error(
                "Rust policy inventory",
                "features differ from selected physical evidence",
            ));
        }
        let manifest = unit
            .source_members
            .iter()
            .find(|member| member.path == *manifest_path && member.digest == *manifest_digest)
            .ok_or_else(|| projection_error("Rust policy manifest", "does not match selected source evidence"))?;
        let members = unit
            .source_members
            .iter()
            .filter(|member| member.path != *manifest_path)
            .cloned()
            .collect::<Vec<_>>();
        if members.iter().map(|member| &member.path).collect::<Vec<_>>() != member_paths.iter().collect::<Vec<_>>() {
            return Err(projection_error(
                "Rust policy inventory",
                "does not exhaust selected source members",
            ));
        }
        inventories.push(OvenRuntimeFoundationSourceInventory {
            selected_identity: unit.identity.clone(),
            package: OvenRuntimeFoundationPackageSource {
                root: OvenSelectedRustFacetPath {
                    owner: unit.source.owner.clone(),
                    path: unit.source.root.clone(),
                },
                manifest: manifest.clone(),
                members,
            },
            build_unit_present: catalogs[&key].3,
        });
        used.insert(key);
    }
    if used.len() != catalogs.len() {
        return Err(projection_error(
            "Rust policy inventories",
            "contain an unselected source",
        ));
    }
    Ok(inventories)
}

/// Return a stable internal ordering key without changing the public wire spelling.
fn selected_domain_key(domain: OvenSelectedRustFacetDomain) -> u8 {
    match domain {
        OvenSelectedRustFacetDomain::Host => 0,
        OvenSelectedRustFacetDomain::Target => 1,
    }
}

/// Encode the strict Incan policy request from validated physical facts and retained package sources.
pub fn encode_selected_graph_policy_request(
    selected: &ValidatedOvenSelectedRustFacetGraph,
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sources: &[OvenLegacyCargoInspectionSource],
    intent_owner: &str,
) -> Result<serde_json::Value, OvenLegacyCargoError> {
    let graph = selected.graph();
    let units = graph
        .units
        .iter()
        .map(|unit| {
            Ok(serde_json::json!({
                "identity": unit.identity,
                "package": {"name": unit.package, "version": unit.package_version, "source": unit.source.identity},
                "domain": serde_json::to_value(unit.domain).map_err(|error| projection_error("policy unit domain", &error.to_string()))?,
                "role": serde_json::to_value(unit.role).map_err(|error| projection_error("policy unit role", &error.to_string()))?,
                "crate_kind": serde_json::to_value(unit.crate_kind).map_err(|error| projection_error("policy crate kind", &error.to_string()))?,
                "closure": {
                    "environment": unit.environment,
                    "generated_inputs": unit.generated_inputs,
                    "linked_libraries": unit.linked_libraries,
                    "sysroot_externs": unit.sysroot_externs,
                },
                "features": unit.features,
            }))
        })
        .collect::<Result<Vec<_>, OvenLegacyCargoError>>()?;
    let roots = graph
        .exposed_roots
        .values()
        .map(|root| serde_json::to_value(root).map_err(|error| projection_error("policy root", &error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let bindings = graph
        .units
        .iter()
        .flat_map(|unit| {
            unit.dependencies.iter().map(move |dependency| {
                serde_json::json!({
                    "parent_unit": unit.identity,
                    "alias": dependency.alias,
                    "child_unit": dependency.unit,
                })
            })
        })
        .collect::<Vec<_>>();
    // The engine keys catalogs by package identity and refuses a repeated key, so a package compiled in both
    // domains (a host rlib feeding a proc-macro and a target rlib) contributes its manifest once.
    let mut catalogs = Vec::new();
    let mut seen = BTreeSet::new();
    for unit in &graph.units {
        let key = (
            unit.package.clone(),
            unit.package_version.clone(),
            unit.source.identity.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        let matched = sources
            .iter()
            .filter(|source| {
                source.package == unit.package
                    && source.version == unit.package_version
                    && registry_source_identity(&source.package, &source.version) == unit.source.identity
            })
            .collect::<Vec<_>>();
        let [source] = matched.as_slice() else {
            return Err(projection_error(
                "policy catalog",
                "does not bind exactly one retained source",
            ));
        };
        let manifest_path = source.source_root.join("Cargo.toml");
        let manifest_bytes = fs::read(&manifest_path)
            .map_err(|error| projection_error("policy catalog manifest", &error.to_string()))?;
        let manifest_digest = selected_graph_sha256(&manifest_bytes);
        let declared_manifest = source
            .members
            .iter()
            .find(|member| member.path == "Cargo.toml")
            .ok_or_else(|| projection_error("policy catalog manifest", "is absent from retained members"))?;
        if declared_manifest.digest != manifest_digest {
            return Err(projection_error(
                "policy catalog manifest",
                "bytes differ from retained digest",
            ));
        }
        let build_unit_present = capture.units.iter().enumerate().any(|(consumer_index, captured)| {
            captured.package == unit.package
                && captured.package_version == unit.package_version
                && captured.dependencies.iter().any(|dependency| {
                    capture
                        .units
                        .get(dependency.unit_index)
                        .is_some_and(is_build_script_unit)
                        && dependency.build_script.is_some()
                        && consumer_index != dependency.unit_index
                })
        });
        catalogs.push(serde_json::json!({
            "package": {"name": unit.package, "version": unit.package_version, "source": unit.source.identity},
            "manifest": {
                "path": "Cargo.toml",
                "bytes_hex": hex::encode(&manifest_bytes),
                "sha256_hex": manifest_digest.trim_start_matches("sha256:"),
            },
            "owner": unit.source.owner,
            "intent_owner": intent_owner,
            "package_root": unit.source.root,
            "members": source.members.iter().filter(|member| member.path != "Cargo.toml").map(|member| member.path.clone()).collect::<Vec<_>>(),
            "build_unit_present": build_unit_present,
        }));
    }
    Ok(serde_json::json!({
        "schema": "incan.oven.rust-policy-exchange/5",
        "operation": "validate_selected_rust_graph",
        "graph_digest": selected.digest(),
        "context": {
            "host": {"triple": graph.selection.host, "cfg": graph.selection.host_cfg},
            "target": {"triple": graph.selection.intent.target, "cfg": graph.selection.target_cfg},
            "selection_purpose": serde_json::to_value(graph.selection.purpose).map_err(|error| projection_error("policy purpose", &error.to_string()))?,
        },
        "units": units,
        "roots": roots,
        "bindings": bindings,
        "catalogs": catalogs,
        "max_rounds": graph.units.len().saturating_mul(graph.units.len()).saturating_add(1),
    }))
}

/// Complete the capture-to-final-receipt transition for one compiler-support selected graph.
///
/// The base receipt authorizes the initial stable Cargo observation. This function binds the observed build-script
/// closure, projects exact physical identities, joins authored root intent, derives the distinct final receipt, and
/// admits the rooted graph. Final artifacts must be published only under `final_receipt`.
pub fn finalize_compiler_support_selected_graph(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
    intent_owner: &str,
    base_receipt: &OvenReceipt,
) -> Result<OvenFinalizedCompilerSupportSelectedGraph, OvenLegacyCargoError> {
    let (capture, sealed, root_units, source_indices) =
        compiler_support_capture(capture, sealed, manifest, active_optional_dependencies)?;
    if base_receipt
        .sources
        .build_unit_inputs
        .contains_key(OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT)
    {
        return Err(projection_error(
            "compiler-support base receipt",
            "already carries a selected build-script closure",
        ));
    }
    let closure_digest = legacy_cargo_build_script_closure_digest(&capture, &sealed.build_scripts)?;
    let capture_receipt = receipt_with_build_unit_input(
        base_receipt,
        OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT,
        closure_digest,
    )
    .map_err(|error| projection_error("compiler-support capture receipt", &error.to_string()))?;
    let projected = project_legacy_cargo_selected_graph(&capture, &sealed, Some(&capture_receipt))?;
    let referenced_owners = selected_graph_referenced_owners(&projected);
    let mut sealed = sealed;
    sealed
        .owners
        .retain(|owner| referenced_owners.contains(owner.identity.as_str()));
    let authority = compiler_support_root_intent_authority_with_roots(
        &capture,
        &sealed,
        Some(&capture_receipt),
        manifest,
        active_optional_dependencies,
        intent_owner,
        &capture_receipt,
        &root_units,
    )?;
    let authority_digest = oven_rustc::rustc::compiler_support_root_intent_digest(&authority)
        .map_err(|error| projection_error("compiler-support root intent", &error.to_string()))?;
    let final_receipt = receipt_with_compiler_support_root_intent(&capture_receipt, authority_digest)
        .map_err(|error| projection_error("compiler-support final receipt", &error.to_string()))?;
    let (graph, unit_identities) = project_and_bind_compiler_support_selected_graph(
        &capture,
        &sealed,
        &authority,
        &capture_receipt,
        &final_receipt,
    )?;
    Ok(OvenFinalizedCompilerSupportSelectedGraph {
        capture,
        capture_receipt,
        final_receipt,
        graph,
        unit_identities,
        source_indices,
    })
}

/// Collect every physical owner referenced by compiler-visible selected graph facts.
fn selected_graph_referenced_owners(graph: &OvenSelectedRustFacetGraph) -> BTreeSet<&str> {
    let mut owners = BTreeSet::new();
    match &graph.selection.target_spec {
        OvenSelectedRustFacetTargetSpec::BuiltIn { toolchain_owner, .. } => {
            owners.insert(toolchain_owner.as_str());
        }
        OvenSelectedRustFacetTargetSpec::Custom { source, .. } => {
            owners.insert(source.owner.as_str());
        }
    }
    for unit in &graph.units {
        owners.insert(unit.source.owner.as_str());
        owners.extend(unit.include_dirs.iter().map(|path| path.owner.as_str()));
        owners.extend(unit.exclude_dirs.iter().map(|path| path.owner.as_str()));
        owners.extend(unit.generated_inputs.iter().map(|input| input.source.owner.as_str()));
        for value in unit.environment.values() {
            if let OvenSelectedRustFacetEnvironmentValue::Path { value } = value {
                owners.insert(value.owner.as_str());
            }
        }
        for library in &unit.linked_libraries {
            match library {
                OvenSelectedRustFacetLinkedLibrary::Archive { artifact, .. } => {
                    owners.insert(artifact.owner.as_str());
                }
                OvenSelectedRustFacetLinkedLibrary::Provider { details } => {
                    owners.insert(details.provenance.owner.as_str());
                    owners.insert(details.search_root.owner.as_str());
                    owners.insert(details.artifact.owner.as_str());
                }
            }
        }
    }
    owners
}

/// Preserve the authored alias of each exact direct edge before removing Cargo transport roots.
fn compiler_support_root_indices(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
) -> Result<BTreeMap<String, usize>, OvenLegacyCargoError> {
    let mut roots = BTreeMap::new();
    for (alias, declaration) in manifest.rust_dependencies() {
        let rust_alias = alias.replace('-', "_");
        let mut matches = BTreeSet::new();
        for root_index in &capture.roots {
            let root = capture
                .units
                .get(*root_index)
                .ok_or_else(|| projection_error("compiler-support capture root", "names an absent physical unit"))?;
            for dependency in &root.dependencies {
                if dependency.extern_crate_name.as_deref() == Some(rust_alias.as_str()) {
                    matches.insert(dependency.unit_index);
                }
            }
        }
        if declaration.optional {
            let active = active_optional_dependencies.contains(alias);
            if !active && matches.is_empty() {
                continue;
            }
            if !active {
                return Err(projection_error(
                    "compiler-support declaration",
                    &format!("optional alias `{alias}` was selected without authored activation"),
                ));
            }
        }
        if matches.len() != 1 {
            return Err(projection_error(
                "compiler-support declaration",
                &format!(
                    "alias `{alias}` binds {} direct physical units; captured roots: {}",
                    matches.len(),
                    capture
                        .roots
                        .iter()
                        .filter_map(|index| capture.units.get(*index))
                        .map(|root| {
                            let aliases = root
                                .dependencies
                                .iter()
                                .filter_map(|dependency| dependency.extern_crate_name.as_deref())
                                .collect::<Vec<_>>()
                                .join(",");
                            format!("{}@{} [{}]", root.package, root.package_version, aliases)
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            ));
        }
        let index = matches
            .into_iter()
            .next()
            .ok_or_else(|| projection_error("compiler-support declaration", "lost its exact captured edge"))?;
        roots.insert(alias.clone(), index);
    }
    Ok(roots)
}

/// The pruned capture and projection, the authored root indices, and each pruned index's original index.
type CompilerSupportCapture = (
    OvenLegacyCargoSelectedUnitCapture,
    OvenLegacyCargoSelectedGraphProjection,
    BTreeMap<String, usize>,
    Vec<usize>,
);

/// Remove the generated Cargo transport root while retaining exactly the authored dependency closure it selected.
fn compiler_support_capture(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
) -> Result<CompilerSupportCapture, OvenLegacyCargoError> {
    let root_units = compiler_support_root_indices(capture, manifest, active_optional_dependencies)?;
    let mut selected = root_units.values().copied().collect::<BTreeSet<_>>();
    let roots = selected.clone();
    let mut pending = selected.iter().copied().collect::<Vec<_>>();
    while let Some(index) = pending.pop() {
        let unit = capture
            .units
            .get(index)
            .ok_or_else(|| projection_error("compiler-support closure", "names an absent physical unit"))?;
        // A run-custom-build unit is retained as the endpoint of its consumer's edge, for the facts sealed there.
        // Its own dependencies are inputs to an inert script Oven never selects or executes, not units the
        // consumer's compilation links, so the closure stops at the script.
        if is_build_script_unit(unit) {
            continue;
        }
        for dependency in &unit.dependencies {
            if selected.insert(dependency.unit_index) {
                pending.push(dependency.unit_index);
            }
        }
    }
    let old_indices = selected.into_iter().collect::<Vec<_>>();
    let remap = old_indices
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new))
        .collect::<BTreeMap<_, _>>();
    let mut units = Vec::with_capacity(old_indices.len());
    for old in &old_indices {
        let mut unit = capture.units[*old].clone();
        if is_build_script_unit(&unit) {
            // The script's own edges name its inputs, which the closure deliberately stopped at.
            unit.dependencies.clear();
        }
        for dependency in &mut unit.dependencies {
            dependency.unit_index = *remap.get(&dependency.unit_index).ok_or_else(|| {
                projection_error(
                    "compiler-support closure",
                    "omits one dependency of a retained physical unit",
                )
            })?;
        }
        units.push(unit);
    }
    let map_edge = |(consumer, build_unit): &(usize, usize)| Some((*remap.get(consumer)?, *remap.get(build_unit)?));
    let projection = OvenLegacyCargoSelectedGraphProjection {
        selection: sealed.selection.clone(),
        owners: sealed.owners.clone(),
        units: sealed
            .units
            .iter()
            .filter_map(|(old, binding)| remap.get(old).map(|new| (*new, binding.clone())))
            .collect(),
        generated: sealed
            .generated
            .iter()
            .filter_map(|(edge, binding)| map_edge(edge).map(|mapped| (mapped, binding.clone())))
            .collect(),
        build_scripts: sealed
            .build_scripts
            .iter()
            .filter_map(|(edge, binding)| map_edge(edge).map(|mapped| (mapped, binding.clone())))
            .collect(),
    };
    let capture = OvenLegacyCargoSelectedUnitCapture {
        roots: roots
            .iter()
            .map(|old| {
                remap
                    .get(old)
                    .copied()
                    .ok_or_else(|| projection_error("compiler-support root", "was not retained in its closure"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        units,
        rustc_invocations_observed: capture.rustc_invocations_observed,
        build_script_tool_probes: capture.build_script_tool_probes.clone(),
        compiler: capture.compiler.clone(),
    };
    let root_units = root_units
        .into_iter()
        .map(|(alias, old)| {
            remap.get(&old).copied().map(|index| (alias, index)).ok_or_else(|| {
                projection_error(
                    "compiler-support root",
                    "lost its authored alias during capture pruning",
                )
            })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok((capture, projection, root_units, old_indices))
}

/// Bind a verified stable-compiler capture to a built-in target description without inventing target-spec JSON.
///
/// The caller supplies the Store-owned Toolchain closure identity. The returned descriptor repeats only facts that
/// the stable capture already proved from the selected compiler, and binds its canonical target cfg snapshot.
pub fn legacy_cargo_builtin_target_spec(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    toolchain_owner: impl Into<String>,
) -> Result<OvenSelectedRustFacetTargetSpec, OvenLegacyCargoError> {
    let compiler = capture
        .compiler
        .as_ref()
        .ok_or_else(|| projection_error("selected compiler capture", "is absent"))?;
    if compiler.toolchain != compiler.rustc_identity {
        return Err(projection_error(
            "selected compiler capture",
            "toolchain intent does not match the verified rustc identity",
        ));
    }
    let target_cfg = serde_json::to_vec(&compiler.target_cfg)
        .map_err(|error| projection_error("selected target cfg", &error.to_string()))?;
    Ok(OvenSelectedRustFacetTargetSpec::BuiltIn {
        toolchain_owner: toolchain_owner.into(),
        target: compiler.target.clone(),
        rustc_identity: compiler.rustc_identity.clone(),
        target_cfg_digest: oven_rustc::rustc::selected_graph_sha256(&target_cfg),
    })
}

/// Construct exact portable registry-unit bindings from the publisher's retained inspection source catalogue.
///
/// The returned paths name the layout written by the Loaf source publisher. Package names alone never select a
/// source: version, registry, checksum, complete tree digest, root module and member inventory must all agree with
/// the stable capture before a binding is returned.
pub fn legacy_cargo_registry_unit_bindings(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sources: &[OvenLegacyCargoInspectionSource],
    foundation_owner: &str,
) -> Result<BTreeMap<usize, OvenLegacyCargoSelectedGraphUnitBinding>, OvenLegacyCargoError> {
    if foundation_owner.trim().is_empty() {
        return Err(projection_error("registry foundation owner", "is empty"));
    }
    let mut bindings = BTreeMap::new();
    for (index, unit) in capture.units.iter().enumerate() {
        // A run-custom-build unit is an execution node, not an inspection crate: its registry source authenticates
        // the build-script closure sealed on each consumer edge, and it must not receive a graph unit binding.
        if is_build_script_unit(unit) {
            continue;
        }
        let Some(captured) = unit.registry_source.as_ref() else {
            continue;
        };
        let candidates = sources
            .iter()
            .filter(|source| {
                source.package == unit.package
                    && source.version == unit.package_version
                    && source.registry == captured.registry
                    && source.checksum == captured.checksum
            })
            .collect::<Vec<_>>();
        let [source] = candidates.as_slice() else {
            return Err(projection_error(
                "selected registry source",
                "does not bind exactly one retained inspection source",
            ));
        };
        if source.source_digest != captured.digest || source.members != captured.members {
            return Err(projection_error(
                "selected registry source",
                "differs from the captured source digest or member inventory",
            ));
        }
        let directory = source
            .source_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| projection_error("selected registry source", "has no portable directory name"))?;
        let relative_root = format!("registry-sources/{directory}");
        let source_members = source
            .members
            .iter()
            .map(|member| OvenSelectedRustFacetSourceMember {
                path: member.path.clone(),
                digest: member.digest.clone(),
            })
            .collect::<Vec<_>>();
        bindings.insert(
            index,
            OvenLegacyCargoSelectedGraphUnitBinding {
                source: OvenSelectedRustFacetSource {
                    kind: OvenSelectedRustFacetSourceKind::Registry,
                    // The portable `registry:` coordinate, never Cargo's machine-facing package-id URL.
                    identity: registry_source_identity(&unit.package, &unit.package_version),
                    owner: foundation_owner.to_string(),
                    root: relative_root.clone(),
                    digest: source.source_digest.clone(),
                },
                source_members,
                registry_source: Some(OvenRustcRegistrySourcePackage {
                    package: unit.package.clone(),
                    version: unit.package_version.clone(),
                    features: source.features.clone(),
                    source: OvenRustcRegistrySource {
                        registry: source.registry.clone(),
                        checksum: source.checksum.clone(),
                        relative_root: relative_root.clone(),
                        digest: source.source_digest.clone(),
                    },
                }),
                include_dirs: vec![OvenSelectedRustFacetPath {
                    owner: foundation_owner.to_string(),
                    path: relative_root,
                }],
                exclude_dirs: Vec::new(),
            },
        );
    }
    Ok(bindings)
}

/// Captured generated-output owners paired with their consumer/build-unit edge bindings.
pub type OvenLegacyCargoGeneratedOutputBindings = (
    Vec<OvenSelectedRustFacetOwner>,
    BTreeMap<(usize, usize), OvenLegacyCargoSelectedGeneratedBinding>,
);

/// Construct generated-output owners and bindings from exact edge-scoped stable capture facts.
///
/// Empty member inventories are preserved: their canonical digest and declared directory remain distinct from an
/// absent output, allowing the foundation asset catalogue to reproduce the empty `OUT_DIR` after mirroring.
pub fn legacy_cargo_generated_output_bindings(
    capture: &OvenLegacyCargoSelectedUnitCapture,
) -> Result<OvenLegacyCargoGeneratedOutputBindings, OvenLegacyCargoError> {
    let mut owners = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    for (consumer, unit) in capture.units.iter().enumerate() {
        for dependency in &unit.dependencies {
            let Some(build_unit) = capture.units.get(dependency.unit_index) else {
                return Err(projection_error(
                    "selected generated output",
                    "names an absent build-script unit",
                ));
            };
            if !is_build_script_unit(build_unit) {
                continue;
            }
            let facts = dependency
                .build_script
                .as_ref()
                .or(build_unit.build_script.as_ref())
                .ok_or_else(|| projection_error("selected generated output", "has no retained build-script facts"))?;
            let Some(output) = facts.output.as_ref() else {
                continue;
            };
            let owner_identity = oven_rustc::rustc::selected_graph_sha256(
                format!("generated-output\0{}\0{}", output.relative_root, output.digest).as_bytes(),
            );
            owners.insert(
                owner_identity.clone(),
                OvenSelectedRustFacetOwner {
                    identity: owner_identity.clone(),
                    kind: OvenSelectedRustFacetOwnerKind::GeneratedOutput,
                },
            );
            if bindings
                .insert(
                    (consumer, dependency.unit_index),
                    OvenLegacyCargoSelectedGeneratedBinding {
                        name: "out_dir".to_string(),
                        source: OvenSelectedRustFacetPath {
                            owner: owner_identity,
                            path: output.relative_root.clone(),
                        },
                        digest: output.digest.clone(),
                    },
                )
                .is_some()
            {
                return Err(projection_error(
                    "selected generated output",
                    "is bound more than once for one physical edge",
                ));
            }
        }
    }
    Ok((owners.into_values().collect(), bindings))
}

/// Bind Cargo static/dynamic link directives to exact retained build-script output members.
///
/// System and framework directives require a separately admitted provider and are refused here. Archive selection is
/// exact by platform filename and must resolve to one retained output member; raw search paths never authorize a
/// filesystem lookup.
pub fn legacy_cargo_generated_archive_bindings(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    generated: &BTreeMap<(usize, usize), OvenLegacyCargoSelectedGeneratedBinding>,
) -> Result<BTreeMap<(usize, usize), OvenLegacyCargoSelectedLinkedLibraryBinding>, OvenLegacyCargoError> {
    let mut bindings = BTreeMap::new();
    for (consumer, unit) in capture.units.iter().enumerate() {
        for dependency in &unit.dependencies {
            let Some(build_unit) = capture.units.get(dependency.unit_index) else {
                return Err(projection_error(
                    "selected linked library",
                    "names an absent build-script unit",
                ));
            };
            if !is_build_script_unit(build_unit) {
                continue;
            }
            let facts = dependency
                .build_script
                .as_ref()
                .or(build_unit.build_script.as_ref())
                .ok_or_else(|| projection_error("selected linked library", "has no retained build-script facts"))?;
            if facts.linked_libraries.is_empty() && facts.linked_paths.is_empty() {
                continue;
            }
            let edge = (consumer, dependency.unit_index);
            let output = facts.output.as_ref().ok_or_else(|| {
                projection_error("selected linked library", "has no retained generated output inventory")
            })?;
            let generated = generated.get(&edge).ok_or_else(|| {
                projection_error("selected linked library", "has no exact generated-output owner binding")
            })?;
            let mut libraries = Vec::new();
            for directive in &facts.linked_libraries {
                let target = unit
                    .platform
                    .as_deref()
                    .ok_or_else(|| projection_error("selected linked library", "has no captured compilation target"))?;
                if facts.linked_paths.iter().any(|path| !path.starts_with("native=")) {
                    return Err(projection_error(
                        "selected linked library",
                        "contains an unsupported non-native search-path directive",
                    ));
                }
                let compiler = capture
                    .compiler
                    .as_ref()
                    .ok_or_else(|| projection_error("selected linked library", "has no captured compiler cfg"))?;
                let cfg = if target == compiler.host {
                    &compiler.host_cfg
                } else if target == compiler.target {
                    &compiler.target_cfg
                } else {
                    return Err(projection_error(
                        "selected linked library",
                        "uses a platform outside the captured host/target selection",
                    ));
                };
                let target_os = cfg.values.get("target_os").and_then(|values| values.first());
                let target_env = cfg.values.get("target_env").and_then(|values| values.first());
                let (kind, name, expected_file) = if let Some(name) = directive.strip_prefix("static=") {
                    (
                        oven_rustc::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
                        name,
                        if target_os.is_some_and(|value| value == "windows")
                            && target_env.is_some_and(|value| value == "msvc")
                        {
                            format!("{name}.lib")
                        } else if target_os.is_some_and(|value| value != "windows")
                            || (target_os.is_some_and(|value| value == "windows")
                                && target_env.is_some_and(|value| value == "gnu"))
                        {
                            format!("lib{name}.a")
                        } else {
                            return Err(projection_error(
                                "selected linked library",
                                "has no supported target archive convention",
                            ));
                        },
                    )
                } else if let Some(name) = directive.strip_prefix("dylib=") {
                    (
                        oven_rustc::rustc::OvenSelectedRustFacetLinkedLibraryKind::Dynamic,
                        name,
                        if target_os.is_some_and(|value| value == "macos") {
                            format!("lib{name}.dylib")
                        } else if target_os.is_some_and(|value| value == "windows")
                            && target_env.is_some_and(|value| value == "msvc")
                        {
                            format!("{name}.lib")
                        } else if target_os.is_some_and(|value| value != "windows") {
                            format!("lib{name}.so")
                        } else {
                            return Err(projection_error(
                                "selected linked library",
                                "has no supported target dynamic-library convention",
                            ));
                        },
                    )
                } else {
                    return Err(projection_error(
                        "selected linked library",
                        &format!(
                            "directive `{directive}` from package `{}` for target `{target}` requires a separately admitted system or framework provider",
                            unit.package
                        ),
                    ));
                };
                let search_roots = facts
                    .linked_paths
                    .iter()
                    .filter_map(|path| path.strip_prefix("native="))
                    .map(Path::new)
                    .map(|path| path.strip_prefix(&facts.out_dir))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        projection_error(
                            "selected linked library",
                            "search path is outside its retained generated-output directory",
                        )
                    })?;
                if search_roots.is_empty() {
                    return Err(projection_error(
                        "selected linked library",
                        "has no retained native search path",
                    ));
                }
                let candidates = output
                    .members
                    .iter()
                    .filter(|member| {
                        let member_path = Path::new(&member.path);
                        member_path
                            .file_name()
                            .and_then(|file| file.to_str())
                            .is_some_and(|file| file == expected_file)
                            && search_roots.iter().any(|root| member_path.parent() == Some(*root))
                    })
                    .collect::<Vec<_>>();
                let [member] = candidates.as_slice() else {
                    return Err(projection_error(
                        "selected linked library",
                        "does not name exactly one retained build-script output member",
                    ));
                };
                libraries.push(OvenSelectedRustFacetLinkedLibrary::Archive {
                    name: name.to_string(),
                    kind,
                    artifact: OvenSelectedRustFacetPath {
                        owner: generated.source.owner.clone(),
                        path: format!("{}/{}", output.relative_root, member.path),
                    },
                    digest: member.digest.clone(),
                });
            }
            bindings.insert(
                edge,
                OvenLegacyCargoSelectedLinkedLibraryBinding {
                    observed_libraries: facts.linked_libraries.clone(),
                    observed_paths: facts.linked_paths.clone(),
                    libraries,
                },
            );
        }
    }
    Ok(bindings)
}

/// Assemble the portable projection facts already owned by the explicit release publisher.
///
/// Registry and generated-output bindings come from retained capture evidence. Native linked inputs remain an
/// explicit argument because their typed archive/provider authority is established by the final Loaf publisher,
/// never reconstructed from raw Cargo `-l`/`-L` strings here.
pub fn legacy_cargo_foundation_projection(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    receipt: &OvenReceipt,
    sources: &[OvenLegacyCargoInspectionSource],
    foundation_owner: &str,
    toolchain_owner: &str,
    linked_libraries: &BTreeMap<(usize, usize), OvenLegacyCargoSelectedLinkedLibraryBinding>,
    authority: &LoafRegistryAuthority,
) -> Result<OvenLegacyCargoSelectedGraphProjection, OvenLegacyCargoError> {
    let compiler = capture
        .compiler
        .as_ref()
        .ok_or_else(|| projection_error("selected compiler capture", "is absent"))?;
    if receipt.intent.target != compiler.target || receipt.intent.toolchain != compiler.toolchain {
        return Err(projection_error(
            "foundation receipt intent",
            "does not match the captured compiler target and toolchain",
        ));
    }
    let toolchain_version = compiler
        .toolchain
        .split_whitespace()
        .find(|part| semver::Version::parse(part.trim_start_matches('v')).is_ok())
        .map(|part| part.trim_start_matches('v').to_string())
        .ok_or_else(|| projection_error("selected compiler toolchain", "contains no exact semantic version"))?;
    let mut owners = vec![
        OvenSelectedRustFacetOwner {
            identity: foundation_owner.to_string(),
            kind: OvenSelectedRustFacetOwnerKind::Constituent,
        },
        OvenSelectedRustFacetOwner {
            identity: toolchain_owner.to_string(),
            kind: OvenSelectedRustFacetOwnerKind::Toolchain,
        },
    ];
    let (generated_owners, generated) = legacy_cargo_generated_output_bindings(capture)?;
    owners.extend(generated_owners);
    owners.sort_by(|left, right| left.identity.cmp(&right.identity));
    if owners.windows(2).any(|pair| pair[0].identity == pair[1].identity) {
        return Err(projection_error("foundation owners", "contain a duplicate identity"));
    }

    // An adopted unit without a build-script edge still has a declaration to hold to: it must state no answers.
    for (index, adoption) in authority.adoptions() {
        let Some(unit) = capture.units.get(index) else {
            return Err(projection_error(
                "Loaf registry adoption",
                "names an absent physical unit",
            ));
        };
        let has_build_script = unit.dependencies.iter().any(|dependency| {
            capture
                .units
                .get(dependency.unit_index)
                .is_some_and(is_build_script_unit)
        });
        if !has_build_script {
            LoafRegistryAuthority::check_observation(adoption, None)?;
        }
    }
    let mut build_scripts = BTreeMap::new();
    for (consumer, unit) in capture.units.iter().enumerate() {
        for dependency in &unit.dependencies {
            let Some(build_unit) = capture.units.get(dependency.unit_index) else {
                return Err(projection_error(
                    "selected build-script edge",
                    "names an absent physical unit",
                ));
            };
            if !is_build_script_unit(build_unit) {
                continue;
            }
            let facts = dependency
                .build_script
                .as_ref()
                .or(build_unit.build_script.as_ref())
                .ok_or_else(|| projection_error("selected build-script edge", "has no retained facts"))?;
            // Where a registry declaration governs this consumer, the observation must agree with it, and the
            // projection carries the declaration: nothing the script emitted outside it, environment included.
            let adoption = authority.adoption(consumer);
            if let Some(adoption) = adoption {
                LoafRegistryAuthority::check_observation(adoption, Some(facts))?;
            }
            let edge = (consumer, dependency.unit_index);
            let typed_linked = linked_libraries.get(&edge).cloned();
            let has_linked = !facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty();
            if has_linked != typed_linked.is_some() {
                return Err(projection_error(
                    "selected linked-library closure",
                    "does not have one exact final publisher binding",
                ));
            }
            let environment = if adoption.is_some() {
                BTreeMap::new()
            } else {
                facts
                    .environment
                    .iter()
                    .map(|(name, value)| (name.clone(), observed_environment_binding(name, value)))
                    .collect()
            };
            build_scripts.insert(
                edge,
                OvenLegacyCargoSelectedBuildScriptBinding {
                    environment,
                    linked_libraries: typed_linked,
                    declaration: adoption.map(|adoption| OvenLegacyCargoDeclaredBuildScriptFacts {
                        package: adoption.package.clone(),
                        version: adoption.version.clone(),
                        checksum: adoption.checksum.clone(),
                        index_line_digest: adoption.index_line_digest.clone(),
                        cfg: adoption.record.cfg.clone(),
                        out: adoption
                            .record
                            .out
                            .iter()
                            .map(|out| (out.name.clone(), out.digest.clone()))
                            .collect(),
                    }),
                },
            );
        }
    }
    if linked_libraries.keys().any(|edge| !build_scripts.contains_key(edge)) {
        return Err(projection_error(
            "selected linked-library closure",
            "binds an absent build-script edge",
        ));
    }
    Ok(OvenLegacyCargoSelectedGraphProjection {
        selection: OvenSelectedRustFacetSelection {
            intent: oven_rustc::rustc::OvenSelectedRustFacetIntent {
                target: receipt.intent.target.clone(),
                toolchain: receipt.intent.toolchain.clone(),
                profile: receipt.intent.profile.clone(),
            },
            host: compiler.host.clone(),
            host_cfg: compiler.host_cfg.clone(),
            target_cfg: compiler.target_cfg.clone(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            toolchain_version,
            target_spec: legacy_cargo_builtin_target_spec(capture, toolchain_owner)?,
        },
        owners,
        units: legacy_cargo_registry_unit_bindings(capture, sources, foundation_owner)?,
        generated,
        build_scripts,
    })
}

/// Bind one observed `rustc-env` line of an observation-governed build-script edge.
///
/// The graph carries the value verbatim only when the validator would retain the name as text; a script's other
/// variables -- `libm` reemitting its feature list as `CFG_CARGO_FEATURES=["arch", "default"]`, say -- have no
/// admissible graph form under RFC 119 and are withheld, staying identity-bound through the closure digest. The
/// classification is the validator's own so the two sides cannot drift; a retained value may still be refused
/// there on its alphabet, which is the right outcome for a machine-local location under a public name.
fn observed_environment_binding(name: &str, observed: &str) -> OvenLegacyCargoSelectedEnvironmentBinding {
    let value = selected_graph_environment_retains_text(name).then(|| OvenSelectedRustFacetEnvironmentValue::Text {
        value: observed.to_string(),
    });
    OvenLegacyCargoSelectedEnvironmentBinding {
        observed_value: observed.to_string(),
        value,
    }
}

/// Identify the execution node that supplies retained build-script facts to a consumer edge.
fn is_build_script_unit(unit: &OvenLegacyCargoSelectedUnit) -> bool {
    unit.mode == "run-custom-build" && unit.target_kinds.iter().any(|kind| kind == "custom-build")
}

/// Project one exact physical Cargo capture into a rootless selected Rust graph.
///
/// The result deliberately has no exposed roots. Cargo's observed effective features become only per-unit compiler
/// evidence; the caller must add authored root request/default intent using a separately admitted authority before
/// validating or publishing the graph.
pub fn project_legacy_cargo_selected_graph(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    final_receipt: Option<&OvenReceipt>,
) -> Result<OvenSelectedRustFacetGraph, OvenLegacyCargoError> {
    project_legacy_cargo_selected_graph_with_identities(capture, sealed, final_receipt).map(|(graph, _)| graph)
}

/// Project the graph and return, beside it, the selected identity of each compiled capture unit by index.
pub fn project_legacy_cargo_selected_graph_with_identities(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    final_receipt: Option<&OvenReceipt>,
) -> Result<(OvenSelectedRustFacetGraph, BTreeMap<usize, String>), OvenLegacyCargoError> {
    validate_projection_compiler(capture, sealed)?;
    validate_projection_bindings(capture, sealed)?;
    validate_build_script_authority(capture, sealed, final_receipt)?;

    let mut pending = (0..capture.units.len())
        .filter(|index| !is_build_script_unit(&capture.units[*index]))
        .collect::<BTreeSet<_>>();
    let mut units = BTreeMap::new();

    while !pending.is_empty() {
        let mut progressed = false;
        for index in pending.clone() {
            let capture_unit = capture
                .units
                .get(index)
                .ok_or_else(|| projection_error("selected unit", "is absent"))?;
            let binding = sealed
                .units
                .get(&index)
                .ok_or_else(|| projection_error("selected graph unit binding", "is absent"))?;
            let dependencies = match projected_dependencies(capture, index, &units)? {
                Some(dependencies) => dependencies,
                None => continue,
            };
            let build_script_facts = projected_build_script_facts(capture, sealed, index)?;
            let crate_kind = captured_crate_kind(capture_unit)?;
            let domain = captured_domain(capture_unit, &sealed.selection)?;
            let role = captured_role(capture_unit, crate_kind, domain)?;
            validate_registry_binding(capture_unit, binding, &sealed.owners)?;
            validate_source_owner(binding, &sealed.owners)?;

            let mut unit = OvenSelectedRustFacetUnit {
                identity: String::new(),
                package: capture_unit.package.clone(),
                package_version: capture_unit.package_version.clone(),
                crate_name: capture_unit.target_name.replace('-', "_"),
                crate_kind,
                role,
                domain,
                edition: capture_unit.edition.clone(),
                source: binding.source.clone(),
                root_module: capture_unit.root_module.clone(),
                source_members: source_members(capture_unit, binding)?,
                features: capture_unit.effective_features.clone(),
                cfg: build_script_facts.cfg,
                sysroot_externs: capture_unit.sysroot_externs.clone(),
                environment: build_script_facts.environment,
                include_dirs: binding.include_dirs.clone(),
                exclude_dirs: binding.exclude_dirs.clone(),
                dependencies,
                generated_inputs: build_script_facts.generated_inputs,
                linked_libraries: build_script_facts.linked_libraries,
            };
            unit.identity = selected_graph_unit_identity(&sealed.selection, &unit)
                .map_err(|error| projection_error("selected graph unit identity", &error.to_string()))?;
            units.insert(index, unit);
            pending.remove(&index);
            progressed = true;
        }
        if !progressed {
            return Err(projection_error(
                "selected Cargo unit graph",
                "has a cycle or dependency whose physical unit cannot be projected",
            ));
        }
    }

    let unit_identities = units
        .iter()
        .map(|(index, unit)| (*index, unit.identity.clone()))
        .collect::<BTreeMap<_, _>>();
    let graph = OvenSelectedRustFacetGraph {
        schema_version: oven_rustc::rustc::OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
        selection: sealed.selection.clone(),
        owners: sealed.owners.clone(),
        units: units.into_values().collect(),
        exposed_roots: BTreeMap::new(),
    };
    Ok((graph, unit_identities))
}

/// Project physical capture and bind compiler-release roots under the verified final receipt.
///
/// This is the compiler-release publisher path: the capture receipt authorizes observation, then the distinct final
/// receipt seals the exact authored root-intent digest before this function admits the fully rooted graph.
pub fn project_and_bind_compiler_support_selected_graph(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    authority: &OvenCompilerSupportRootIntentAuthority,
    capture_receipt: &OvenReceipt,
    final_receipt: &OvenReceipt,
) -> Result<(ValidatedOvenSelectedRustFacetGraph, BTreeMap<usize, String>), OvenLegacyCargoError> {
    let (graph, unit_identities) =
        project_legacy_cargo_selected_graph_with_identities(capture, sealed, Some(final_receipt))?;
    let bound = bind_compiler_support_root_intents(graph, authority, capture_receipt, final_receipt)
        .map_err(|error| projection_error("compiler-support selected graph", &error.to_string()))?;
    // Root binding and validation canonicalize the graph; a unit's identity is content-derived and must survive.
    let identities = bound
        .graph()
        .units
        .iter()
        .map(|unit| unit.identity.as_str())
        .collect::<BTreeSet<_>>();
    if unit_identities.len() != identities.len()
        || unit_identities
            .values()
            .any(|identity| !identities.contains(identity.as_str()))
    {
        return Err(projection_error(
            "compiler-support selected graph",
            "does not preserve the projected identity of every compiled physical unit",
        ));
    }
    Ok((bound, unit_identities))
}

/// Bind authored compiler-support declarations to the exact physical units reached from captured Cargo roots.
///
/// The manifest owns aliases and requested/default feature intent. The stable capture owns physical edges, and the
/// rootless projected graph owns selected identities and source owners. This join deliberately does not recover
/// declaration policy from effective Cargo features or target names.
pub fn compiler_support_root_intent_authority(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    projection_receipt: Option<&OvenReceipt>,
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
    intent_owner: &str,
    capture_receipt: &OvenReceipt,
) -> Result<OvenCompilerSupportRootIntentAuthority, OvenLegacyCargoError> {
    let roots = compiler_support_root_indices(capture, manifest, active_optional_dependencies)?;
    compiler_support_root_intent_authority_with_roots(
        capture,
        sealed,
        projection_receipt,
        manifest,
        active_optional_dependencies,
        intent_owner,
        capture_receipt,
        &roots,
    )
}

/// Bind retained direct-edge aliases; never search another authored root's transitive dependencies.
#[allow(clippy::too_many_arguments)]
fn compiler_support_root_intent_authority_with_roots(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    projection_receipt: Option<&OvenReceipt>,
    manifest: &ProjectManifest,
    active_optional_dependencies: &BTreeSet<String>,
    intent_owner: &str,
    capture_receipt: &OvenReceipt,
    root_units: &BTreeMap<String, usize>,
) -> Result<OvenCompilerSupportRootIntentAuthority, OvenLegacyCargoError> {
    capture_receipt
        .verify_identity()
        .map_err(|error| projection_error("compiler-support capture receipt", &error.to_string()))?;
    if intent_owner.trim().is_empty() {
        return Err(projection_error("compiler-support declaration owner", "is empty"));
    }

    // Re-project from the exact sealed bindings here. Accepting a caller-supplied graph and positionally zipping its
    // units to capture indices would let omission, reordering, or substitution choose another physical unit.
    let projected = project_legacy_cargo_selected_graph(capture, sealed, projection_receipt)?;
    let selected_by_capture_index = capture
        .units
        .iter()
        .enumerate()
        .filter(|(_, unit)| !is_build_script_unit(unit))
        .zip(projected.units.iter())
        .map(|((index, _), unit)| (index, unit))
        .collect::<BTreeMap<_, _>>();
    if selected_by_capture_index.len() != projected.units.len() {
        return Err(projection_error(
            "compiler-support selected units",
            "do not correspond exhaustively to the physical capture",
        ));
    }

    let mut roots = Vec::new();
    for (alias, declaration) in manifest.rust_dependencies() {
        let matches = root_units.get(alias).copied().into_iter().collect::<Vec<_>>();
        // Authored activation chooses optional roots. Physical edges must agree with that choice; their presence
        // never supplies missing feature intent.
        if declaration.optional {
            let active = active_optional_dependencies.contains(alias);
            if !active && matches.is_empty() {
                continue;
            }
            if !active {
                return Err(projection_error(
                    "compiler-support declaration",
                    &format!("optional alias `{alias}` was selected without authored activation"),
                ));
            }
        }
        if matches.len() != 1 {
            return Err(projection_error(
                "compiler-support declaration",
                &format!("alias `{alias}` does not bind exactly one captured physical unit"),
            ));
        }
        let selected = selected_by_capture_index.get(&matches[0]).ok_or_else(|| {
            projection_error(
                "compiler-support declaration",
                &format!("alias `{alias}` binds a build-script or absent selected unit"),
            )
        })?;
        let selected_package = declaration
            .package
            .as_deref()
            .unwrap_or(declaration.crate_name.as_str());
        if selected.package != selected_package {
            return Err(projection_error(
                "compiler-support declaration",
                &format!(
                    "alias `{alias}` declares package `{selected_package}` but the captured edge selects `{}`",
                    selected.package
                ),
            ));
        }
        let mut requested_features = declaration.features.clone();
        requested_features.sort();
        requested_features.dedup();
        roots.push(oven_rustc::rustc::OvenCompilerSupportRootIntent {
            alias: alias.clone(),
            unit: selected.identity.clone(),
            requested_features,
            default_features: declaration.default_features,
            intent_owner: intent_owner.to_string(),
            source_owner: selected.source.owner.clone(),
        });
    }
    roots.sort_by(|left, right| left.alias.cmp(&right.alias));
    if roots.is_empty() {
        return Err(projection_error(
            "compiler-support declarations",
            "contain no required Rust dependency roots",
        ));
    }
    Ok(OvenCompilerSupportRootIntentAuthority {
        schema_version: oven_rustc::rustc::OVEN_COMPILER_SUPPORT_ROOT_INTENT_SCHEMA_VERSION,
        capture_receipt_identity: capture_receipt.identity.clone(),
        roots,
    })
}

/// Reject a capture whose compiler facts differ from the sealed selection snapshots.
fn validate_projection_compiler(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
) -> Result<(), OvenLegacyCargoError> {
    let compiler = capture
        .compiler
        .as_ref()
        .ok_or_else(|| projection_error("selected compiler capture", "is absent"))?;
    if !capture.rustc_invocations_observed {
        return Err(projection_error(
            "selected compiler capture",
            "has no bijectively matched stable rustc invocation trace",
        ));
    }
    if compiler.host != sealed.selection.host
        || compiler.target != sealed.selection.intent.target
        || compiler.toolchain != sealed.selection.intent.toolchain
        || compiler.rustc_identity != sealed.selection.intent.toolchain
        || compiler.host_cfg != sealed.selection.host_cfg
        || compiler.target_cfg != sealed.selection.target_cfg
    {
        return Err(projection_error(
            "selected compiler capture",
            "does not exactly match the sealed host, target, toolchain and cfg selection",
        ));
    }
    Ok(())
}

/// One canonical capture-to-binding record folded into the final receipt.
#[derive(Serialize)]
struct BuildScriptAuthorityRecord<'a> {
    consumer: usize,
    build_unit: usize,
    package: &'a str,
    cfg: &'a [String],
    environment: &'a BTreeMap<String, String>,
    linked_libraries: &'a [String],
    linked_paths: &'a [String],
    tool_probes: Vec<&'a OvenLegacyCargoBuildScriptToolProbe>,
    output: Option<&'a OvenLegacyCargoSelectedGeneratedOutput>,
    binding: Option<&'a OvenLegacyCargoSelectedBuildScriptBinding>,
}

/// Digest every selected build-script observation and its typed closure binding in canonical edge order.
pub fn legacy_cargo_build_script_closure_digest(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    bindings: &BTreeMap<(usize, usize), OvenLegacyCargoSelectedBuildScriptBinding>,
) -> Result<String, OvenLegacyCargoError> {
    let mut records = Vec::new();
    let mut probe_matches = vec![0_usize; capture.build_script_tool_probes.len()];
    for (consumer, unit) in capture.units.iter().enumerate() {
        for dependency in &unit.dependencies {
            let Some(build_unit) = capture.units.get(dependency.unit_index) else {
                return Err(projection_error(
                    "selected Cargo dependency",
                    "names an absent capture unit",
                ));
            };
            if !is_build_script_unit(build_unit) {
                continue;
            }
            let facts = dependency
                .build_script
                .as_ref()
                .or(build_unit.build_script.as_ref())
                .ok_or_else(|| projection_error("selected build-script unit", "has no structured retained facts"))?;
            let tool_probes = build_script_tool_probes(capture, unit, build_unit, facts, &mut probe_matches)?;
            records.push(BuildScriptAuthorityRecord {
                consumer,
                build_unit: dependency.unit_index,
                package: &build_unit.package_id,
                cfg: &facts.cfgs,
                environment: &facts.environment,
                linked_libraries: &facts.linked_libraries,
                linked_paths: &facts.linked_paths,
                tool_probes,
                output: facts.output.as_ref(),
                binding: bindings.get(&(consumer, dependency.unit_index)),
            });
        }
    }
    if probe_matches.iter().any(|matches| *matches != 1) {
        return Err(projection_error(
            "selected build-script tool probe",
            "is not consumed by one exact selected build-script edge",
        ));
    }
    let bytes = serde_json::to_vec(&("incan.oven.legacy-cargo-build-script-closure/1", records))
        .map_err(|error| projection_error("selected build-script closure", &error.to_string()))?;
    Ok(oven_rustc::rustc::selected_graph_sha256(&bytes))
}

/// Bind every transient compiler probe to one exact captured run-custom-build record.
fn build_script_tool_probes<'a>(
    capture: &'a OvenLegacyCargoSelectedUnitCapture,
    consumer: &'a OvenLegacyCargoSelectedUnit,
    build_unit: &'a OvenLegacyCargoSelectedUnit,
    facts: &'a super::OvenLegacyCargoBuildScriptFacts,
    probe_matches: &mut [usize],
) -> Result<Vec<&'a OvenLegacyCargoBuildScriptToolProbe>, OvenLegacyCargoError> {
    let domain = consumer
        .platform
        .as_deref()
        .ok_or_else(|| projection_error("selected build-script consumer", "has no captured target domain"))?;
    let mut selected = Vec::new();
    for (probe_index, probe) in capture.build_script_tool_probes.iter().enumerate() {
        let matches_unit = probe.package_id == build_unit.package_id
            && probe.out_dir == facts.out_dir
            && probe.target_context == domain;
        if matches_unit {
            selected.push(probe);
            probe_matches[probe_index] += 1;
        }
    }
    Ok(selected)
}

/// Require every selected build-script closure to be sealed by the verified final publisher receipt.
fn validate_build_script_authority(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    final_receipt: Option<&OvenReceipt>,
) -> Result<(), OvenLegacyCargoError> {
    let has_build_script = capture.units.iter().any(is_build_script_unit);
    if !has_build_script {
        return Ok(());
    }
    let receipt = final_receipt
        .ok_or_else(|| projection_error("selected build-script closure", "has no final receipt authority"))?;
    receipt
        .verify_identity()
        .map_err(|error| projection_error("selected build-script closure receipt", &error.to_string()))?;
    let digest = legacy_cargo_build_script_closure_digest(capture, &sealed.build_scripts)?;
    if receipt
        .sources
        .build_unit_inputs
        .get(OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT)
        != Some(&digest)
    {
        return Err(projection_error(
            "selected build-script closure receipt",
            "does not bind the exact capture-to-typed-closure digest",
        ));
    }
    Ok(())
}

/// Verify that every physical selected unit has one sealed binding and build scripts have none of their own.
fn validate_projection_bindings(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
) -> Result<(), OvenLegacyCargoError> {
    for (index, unit) in capture.units.iter().enumerate() {
        if is_build_script_unit(unit) {
            if sealed.units.contains_key(&index) {
                return Err(projection_error(
                    "selected graph unit binding",
                    "must not model a run-custom-build unit as an inspection crate",
                ));
            }
        } else if !sealed.units.contains_key(&index) {
            return Err(projection_error(
                "selected graph unit binding",
                "is missing a selected unit",
            ));
        }
    }
    if sealed.units.keys().any(|index| *index >= capture.units.len()) {
        return Err(projection_error(
            "selected graph unit binding",
            "names an absent capture unit",
        ));
    }
    for &(consumer, build_unit) in sealed.generated.keys() {
        let Some(consumer_unit) = capture.units.get(consumer) else {
            return Err(projection_error(
                "selected generated output binding",
                "names an absent consumer unit",
            ));
        };
        let Some(build_script_unit) = capture.units.get(build_unit) else {
            return Err(projection_error(
                "selected generated output binding",
                "names an absent build-script unit",
            ));
        };
        let facts = consumer_unit
            .dependencies
            .iter()
            .find(|edge| edge.unit_index == build_unit)
            .and_then(|edge| edge.build_script.as_ref().or(build_script_unit.build_script.as_ref()));
        if !is_build_script_unit(build_script_unit) || facts.and_then(|facts| facts.output.as_ref()).is_none() {
            return Err(projection_error(
                "selected generated output binding",
                "does not name one consumed retained build-script output",
            ));
        }
    }
    for &(consumer, build_unit) in sealed.build_scripts.keys() {
        let Some(consumer_unit) = capture.units.get(consumer) else {
            return Err(projection_error(
                "selected build-script binding",
                "names an absent consumer unit",
            ));
        };
        let Some(build_script_unit) = capture.units.get(build_unit) else {
            return Err(projection_error(
                "selected build-script binding",
                "names an absent build-script unit",
            ));
        };
        let facts = consumer_unit
            .dependencies
            .iter()
            .find(|edge| edge.unit_index == build_unit)
            .and_then(|edge| edge.build_script.as_ref().or(build_script_unit.build_script.as_ref()));
        if !is_build_script_unit(build_script_unit) || facts.is_none() {
            return Err(projection_error(
                "selected build-script binding",
                "does not name one consumed retained build-script fact set",
            ));
        }
    }
    Ok(())
}

/// Return one projected dependency list once all non-build-script children have identities.
fn projected_dependencies(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    parent: usize,
    projected: &BTreeMap<usize, OvenSelectedRustFacetUnit>,
) -> Result<Option<Vec<OvenSelectedRustFacetDependency>>, OvenLegacyCargoError> {
    let unit = capture
        .units
        .get(parent)
        .ok_or_else(|| projection_error("selected unit", "is absent"))?;
    let mut dependencies = Vec::new();
    for dependency in &unit.dependencies {
        let child = capture
            .units
            .get(dependency.unit_index)
            .ok_or_else(|| projection_error("selected Cargo dependency", "names an absent capture unit"))?;
        if is_build_script_unit(child) {
            continue;
        }
        let alias = dependency.extern_crate_name.clone().ok_or_else(|| {
            projection_error(
                "selected Cargo dependency",
                "has no exact rustc extern alias and cannot be represented as a portable graph edge",
            )
        })?;
        let Some(child) = projected.get(&dependency.unit_index) else {
            return Ok(None);
        };
        dependencies.push(OvenSelectedRustFacetDependency {
            alias,
            unit: child.identity.clone(),
        });
    }
    Ok(Some(dependencies))
}

/// Complete selected build-script facts after every raw observation matches a typed sealed closure.
struct ProjectedBuildScriptFacts {
    cfg: Vec<String>,
    environment: BTreeMap<String, OvenSelectedRustFacetEnvironmentValue>,
    generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    linked_libraries: Vec<OvenSelectedRustFacetLinkedLibrary>,
}

/// Attach checked build-script cfg, environment, generated output and linked-library facts to one consumer.
fn projected_build_script_facts(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    consumer: usize,
) -> Result<ProjectedBuildScriptFacts, OvenLegacyCargoError> {
    let unit = capture
        .units
        .get(consumer)
        .ok_or_else(|| projection_error("selected unit", "is absent"))?;
    let mut cfg = unit.cfg.clone();
    let mut environment = BTreeMap::new();
    let mut generated_inputs = Vec::new();
    let mut linked_libraries = Vec::new();
    for dependency in &unit.dependencies {
        let Some(build_unit) = capture.units.get(dependency.unit_index) else {
            return Err(projection_error(
                "selected Cargo dependency",
                "names an absent capture unit",
            ));
        };
        if !is_build_script_unit(build_unit) {
            continue;
        }
        let facts = dependency
            .build_script
            .as_ref()
            .or(build_unit.build_script.as_ref())
            .ok_or_else(|| projection_error("selected build-script unit", "has no structured retained facts"))?;
        let edge = (consumer, dependency.unit_index);
        let binding = sealed.build_scripts.get(&edge).cloned().unwrap_or_default();
        for (name, observed) in &facts.environment {
            let Some(bound) = binding.environment.get(name) else {
                // A registry declaration governs this edge: what the script emitted outside the declaration is
                // provenance in the capture, not a fact of the unit.
                if binding.declaration.is_some() {
                    continue;
                }
                return Err(projection_error(
                    "selected build-script environment",
                    "has no typed sealed binding",
                ));
            };
            if &bound.observed_value != observed {
                return Err(projection_error(
                    "selected build-script environment",
                    "does not match its exact captured value",
                ));
            }
            // A withheld entry is a checked observation the graph does not carry: the exact-value comparison
            // above still holds it to the capture, and the closure digest keeps its identity.
            let Some(value) = &bound.value else {
                continue;
            };
            match value {
                OvenSelectedRustFacetEnvironmentValue::Text { value } if value == observed => {}
                OvenSelectedRustFacetEnvironmentValue::Text { .. } => {
                    return Err(projection_error(
                        "selected build-script environment",
                        "text value does not exactly match its captured value",
                    ));
                }
                OvenSelectedRustFacetEnvironmentValue::Path { .. }
                | OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { .. } => {
                    return Err(projection_error(
                        "selected build-script environment",
                        "requires a receipt-bound owner rebinding or keyed capture digest",
                    ));
                }
            }
            if environment.insert(name.clone(), value.clone()).is_some() {
                return Err(projection_error(
                    "selected build-script environment",
                    "is supplied by more than one build-script edge",
                ));
            }
        }
        // Every sealed key was matched to an observation above; a declared edge legitimately binds fewer keys than
        // the script emitted, an observed edge binds exactly as many.
        let bound_keys = binding.environment.len();
        let observed_keys = facts.environment.len();
        if bound_keys > observed_keys || (binding.declaration.is_none() && bound_keys != observed_keys) {
            return Err(projection_error(
                "selected build-script environment",
                "binds an unobserved environment key",
            ));
        }
        let has_link_facts = !facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty();
        match (has_link_facts, binding.linked_libraries.as_ref()) {
            (false, None) => {}
            (true, Some(bound)) => {
                if bound.observed_libraries != facts.linked_libraries || bound.observed_paths != facts.linked_paths {
                    return Err(projection_error(
                        "selected linked-library closure",
                        "does not match exact captured Cargo link facts",
                    ));
                }
                linked_libraries.extend(bound.libraries.iter().cloned());
            }
            (true, None) => {
                return Err(projection_error(
                    "selected linked-library closure",
                    "has no typed sealed binding",
                ));
            }
            (false, Some(_)) => {
                return Err(projection_error(
                    "selected linked-library closure",
                    "binds no captured link facts",
                ));
            }
        }
        cfg.extend(facts.cfgs.iter().cloned());
        if let Some(output) = &facts.output {
            generated_inputs.push(projected_generated_input(
                sealed,
                consumer,
                dependency.unit_index,
                output,
            )?);
        }
    }
    cfg.sort();
    cfg.dedup();
    generated_inputs.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.digest.cmp(&right.digest)));
    // Cargo's link-directive order and multiplicity are compiler-visible; preserve both exactly.
    Ok(ProjectedBuildScriptFacts {
        cfg,
        environment,
        generated_inputs,
        linked_libraries,
    })
}

/// Verify one retained build-script output binding and convert it to the graph's generated-input form.
fn projected_generated_input(
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    consumer: usize,
    build_unit: usize,
    output: &OvenLegacyCargoSelectedGeneratedOutput,
) -> Result<OvenSelectedRustFacetGeneratedInput, OvenLegacyCargoError> {
    let binding = sealed
        .generated
        .get(&(consumer, build_unit))
        .ok_or_else(|| projection_error("selected generated output", "has no sealed owner binding"))?;
    let captured_members = output
        .members
        .iter()
        .map(|member| OvenSelectedRustFacetSourceMember {
            path: member.path.clone(),
            digest: member.digest.clone(),
        })
        .collect::<Vec<_>>();
    if binding.digest != output.digest
        || binding.source.path != output.relative_root
        // A generated inventory may be a checked empty directory; only authored source refuses an empty member set.
        || oven_rustc::rustc::selected_graph_generated_input_digest(&captured_members)
            .map_err(|error| projection_error("selected generated output", &error.to_string()))?
            != output.digest
    {
        return Err(projection_error(
            "selected generated output",
            "does not match the exact retained output digest and owner-relative root",
        ));
    }
    let owner = sealed
        .owners
        .iter()
        .find(|owner| owner.identity == binding.source.owner)
        .ok_or_else(|| projection_error("selected generated output", "names an absent owner"))?;
    if owner.kind != OvenSelectedRustFacetOwnerKind::GeneratedOutput {
        return Err(projection_error(
            "selected generated output",
            "must use a GeneratedOutput owner",
        ));
    }
    Ok(OvenSelectedRustFacetGeneratedInput {
        name: binding.name.clone(),
        source: binding.source.clone(),
        digest: binding.digest.clone(),
        members: captured_members,
    })
}

/// Return a source catalog only after checking it against the capture's registry evidence where available.
fn source_members(
    unit: &OvenLegacyCargoSelectedUnit,
    binding: &OvenLegacyCargoSelectedGraphUnitBinding,
) -> Result<Vec<OvenSelectedRustFacetSourceMember>, OvenLegacyCargoError> {
    let members = binding.source_members.clone();
    if members.is_empty() || !members.iter().any(|member| member.path == unit.root_module) {
        return Err(projection_error(
            "selected unit source",
            "does not retain the captured root module",
        ));
    }
    Ok(members)
}

/// Require each portable source owner to be declared with the provenance class its source kind permits.
fn validate_source_owner(
    binding: &OvenLegacyCargoSelectedGraphUnitBinding,
    owners: &[OvenSelectedRustFacetOwner],
) -> Result<(), OvenLegacyCargoError> {
    let owner = owners
        .iter()
        .find(|owner| owner.identity == binding.source.owner)
        .ok_or_else(|| projection_error("selected unit source", "names an absent sealed owner"))?;
    let valid = match binding.source.kind {
        OvenSelectedRustFacetSourceKind::Registry
        | OvenSelectedRustFacetSourceKind::Git
        | OvenSelectedRustFacetSourceKind::Path => matches!(
            owner.kind,
            OvenSelectedRustFacetOwnerKind::ProjectAuthority | OvenSelectedRustFacetOwnerKind::Constituent
        ),
        OvenSelectedRustFacetSourceKind::Generated => owner.kind == OvenSelectedRustFacetOwnerKind::GeneratedOutput,
        OvenSelectedRustFacetSourceKind::Compiler => owner.kind == OvenSelectedRustFacetOwnerKind::Toolchain,
    };
    if !valid {
        return Err(projection_error(
            "selected unit source",
            "owner kind does not prove the declared source provenance",
        ));
    }
    Ok(())
}

/// Require a registry capture to agree with its sealed source catalog without interpreting local source paths.
fn validate_registry_binding(
    unit: &OvenLegacyCargoSelectedUnit,
    binding: &OvenLegacyCargoSelectedGraphUnitBinding,
    owners: &[OvenSelectedRustFacetOwner],
) -> Result<(), OvenLegacyCargoError> {
    if let Some(registry) = &unit.registry_source {
        let captured_members = registry
            .members
            .iter()
            .map(|member| OvenSelectedRustFacetSourceMember {
                path: member.path.clone(),
                digest: member.digest.clone(),
            })
            .collect::<Vec<_>>();
        let catalog = binding
            .registry_source
            .as_ref()
            .ok_or_else(|| projection_error("selected registry source", "has no sealed registry catalog record"))?;
        let expected_identity = registry_source_identity(&unit.package, &unit.package_version);
        if unit.package_source.as_deref() != Some(registry.registry.as_str())
            || catalog.package != unit.package
            || catalog.version != unit.package_version
            || catalog.source.registry != registry.registry
            || catalog.source.checksum != registry.checksum
            || catalog.source.digest != registry.digest
            || catalog.source.relative_root != binding.source.root
            || binding.source.kind != OvenSelectedRustFacetSourceKind::Registry
            || binding.source.identity != expected_identity
            || binding.source.digest != registry.digest
            || registry.root_module != unit.root_module
            || binding.source_members != captured_members
            || !owners.iter().any(|owner| {
                owner.identity == binding.source.owner
                    && matches!(
                        owner.kind,
                        OvenSelectedRustFacetOwnerKind::ProjectAuthority | OvenSelectedRustFacetOwnerKind::Constituent
                    )
            })
        {
            return Err(projection_error(
                "selected registry source",
                "does not match its captured digest, member catalog, kind and root module",
            ));
        }
    } else if binding.registry_source.is_some() {
        return Err(projection_error(
            "selected unit source",
            "retains a registry catalog for a non-registry capture unit",
        ));
    }
    Ok(())
}

/// Derive only the output class that Cargo's observed `--crate-type` exactly establishes.
fn captured_crate_kind(
    unit: &OvenLegacyCargoSelectedUnit,
) -> Result<OvenSelectedRustFacetCrateKind, OvenLegacyCargoError> {
    let kinds = unit.crate_types.iter().map(String::as_str).collect::<BTreeSet<_>>();
    match kinds.into_iter().collect::<Vec<_>>().as_slice() {
        ["lib"] | ["rlib"] => Ok(OvenSelectedRustFacetCrateKind::Rlib),
        ["bin"] => Ok(OvenSelectedRustFacetCrateKind::Binary),
        ["proc-macro"] => Ok(OvenSelectedRustFacetCrateKind::ProcMacro),
        _ => Err(projection_error(
            "selected Cargo crate type",
            "is not one supported unambiguous schema-4 inspection output class",
        )),
    }
}

/// Derive compilation domain from the matched rustc invocation's explicit-target provenance.
///
/// Host and target triples can be equal in a normal native build. The exact `--target` presence, not a comparison
/// order between equal strings, distinguishes a host proc macro from a target binary in that case.
fn captured_domain(
    unit: &OvenLegacyCargoSelectedUnit,
    selection: &OvenSelectedRustFacetSelection,
) -> Result<OvenSelectedRustFacetDomain, OvenLegacyCargoError> {
    let platform = unit.platform.as_deref().ok_or_else(|| {
        projection_error(
            "selected Cargo unit platform",
            "is absent; projection cannot infer host or target domain",
        )
    })?;
    match unit.target_is_explicit {
        Some(false) if platform == selection.host => Ok(OvenSelectedRustFacetDomain::Host),
        Some(true) if platform == selection.intent.target => Ok(OvenSelectedRustFacetDomain::Target),
        Some(false) => Err(projection_error(
            "selected Cargo unit platform",
            "has no `--target` but does not match the sealed compiler host",
        )),
        Some(true) => Err(projection_error(
            "selected Cargo unit platform",
            "has `--target` but does not match the sealed target",
        )),
        None => Err(projection_error(
            "selected Cargo unit platform",
            "has no traced `--target` provenance",
        )),
    }
}

/// Derive an inspection role from Cargo's traced target kind and mode without accepting caller relabelling.
fn captured_role(
    unit: &OvenLegacyCargoSelectedUnit,
    crate_kind: OvenSelectedRustFacetCrateKind,
    domain: OvenSelectedRustFacetDomain,
) -> Result<OvenSelectedRustFacetUnitRole, OvenLegacyCargoError> {
    let kinds = unit.target_kinds.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let role = match (
        kinds.into_iter().collect::<Vec<_>>().as_slice(),
        unit.mode.as_str(),
        crate_kind,
        domain,
    ) {
        (["proc-macro"], "build", OvenSelectedRustFacetCrateKind::ProcMacro, OvenSelectedRustFacetDomain::Host) => {
            OvenSelectedRustFacetUnitRole::ProcMacro
        }
        (["lib"], "build", OvenSelectedRustFacetCrateKind::Rlib, _) => OvenSelectedRustFacetUnitRole::Library,
        (["bin"], "build", OvenSelectedRustFacetCrateKind::Binary, OvenSelectedRustFacetDomain::Target) => {
            OvenSelectedRustFacetUnitRole::Binary
        }
        (["lib"] | ["bin"], "test", OvenSelectedRustFacetCrateKind::Binary, OvenSelectedRustFacetDomain::Target) => {
            OvenSelectedRustFacetUnitRole::UnitTest
        }
        (["test"], "test", OvenSelectedRustFacetCrateKind::Binary, OvenSelectedRustFacetDomain::Target) => {
            OvenSelectedRustFacetUnitRole::IntegrationTest
        }
        (["example"], "build", OvenSelectedRustFacetCrateKind::Binary, OvenSelectedRustFacetDomain::Target) => {
            OvenSelectedRustFacetUnitRole::Example
        }
        (["bench"], "bench", OvenSelectedRustFacetCrateKind::Binary, OvenSelectedRustFacetDomain::Target) => {
            OvenSelectedRustFacetUnitRole::Benchmark
        }
        _ => {
            return Err(projection_error(
                "selected Cargo target",
                "has no supported unambiguous target-kind, mode, crate-kind and domain role",
            ));
        }
    };
    Ok(role)
}

/// Return a bounded producer refusal without exposing a local path from capture.
fn projection_error(field: &str, message: &str) -> OvenLegacyCargoError {
    OvenLegacyCargoError::Plan(format!("{field} {message}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;

    use oven_rustc::rustc::OvenRustcRegistrySource;
    use oven_rustc::rustc::{
        OvenCompilerSupportRootIntent, OvenCompilerSupportRootIntentAuthority, OvenSelectedRustFacetCfgSnapshot,
        OvenSelectedRustFacetIntent, OvenSelectedRustFacetPurpose, OvenSelectedRustFacetTargetSpec,
        compiler_support_root_intent_digest, selected_graph_sha256, selected_graph_source_digest,
    };
    use oven_store::{
        OvenGeneratedProjectRequest, receipt_generated_project, receipt_with_compiler_support_root_intent,
    };
    use tempfile::tempdir;

    use super::*;

    fn digest(bytes: &[u8]) -> String {
        selected_graph_sha256(bytes)
    }

    /// Return complete canonical cfg evidence for the retained fixture compiler.
    fn fixture_cfg_snapshot() -> OvenSelectedRustFacetCfgSnapshot {
        OvenSelectedRustFacetCfgSnapshot {
            flags: vec!["unix".to_string()],
            values: BTreeMap::from([
                ("target_arch".to_string(), vec!["x86_64".to_string()]),
                ("target_os".to_string(), vec!["linux".to_string()]),
            ]),
        }
    }

    /// Create the minimum generated source closure required for an honest fixture receipt.
    fn fixture_receipt_request(
        directory: &Path,
        name: &str,
    ) -> Result<OvenGeneratedProjectRequest, Box<dyn std::error::Error>> {
        let source = directory.join("generated.rs");
        fs::write(&source, b"pub fn fixture() {}\n")?;
        Ok(OvenGeneratedProjectRequest::new(
            directory,
            name,
            "1.0.0",
            "x86_64-unknown-linux-gnu",
            "rustc 1.98.0",
            "release",
            Vec::new(),
        )
        .with_generated_source("generated.rs", source))
    }

    fn members() -> Vec<OvenSelectedRustFacetSourceMember> {
        vec![
            OvenSelectedRustFacetSourceMember {
                path: "Cargo.toml".to_string(),
                digest: digest(b"[package]\nname='serde'\nversion='1.0.0'\n"),
            },
            OvenSelectedRustFacetSourceMember {
                path: "src/lib.rs".to_string(),
                digest: digest(b"pub fn fixture() {}\n"),
            },
        ]
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        let toolchain_owner = digest(b"toolchain owner");
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.98.0".to_string(),
                profile: "release".to_string(),
            },
            host: "x86_64-unknown-linux-gnu".to_string(),
            host_cfg: fixture_cfg_snapshot(),
            target_cfg: fixture_cfg_snapshot(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            toolchain_version: "1.98.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec::Custom {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner,
                    path: "target-specs/x86_64-unknown-linux-gnu.json".to_string(),
                },
                digest: digest(b"target spec"),
            },
        }
    }

    fn capture() -> Result<OvenLegacyCargoSelectedUnitCapture, Box<dyn std::error::Error>> {
        let source_members = members();
        Ok(OvenLegacyCargoSelectedUnitCapture {
            roots: vec![0],
            rustc_invocations_observed: true,
            build_script_tool_probes: Vec::new(),
            compiler: Some(super::super::OvenLegacyCargoSelectedCompilerContext {
                host: "x86_64-unknown-linux-gnu".to_string(),
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.98.0".to_string(),
                rustc_identity: "rustc 1.98.0".to_string(),
                host_cfg: fixture_cfg_snapshot(),
                target_cfg: fixture_cfg_snapshot(),
            }),
            units: vec![OvenLegacyCargoSelectedUnit {
                package_id: "registry+https://example.invalid/index#serde@1.0.0".to_string(),
                package: "serde".to_string(),
                package_version: "1.0.0".to_string(),
                package_source: Some("registry+https://example.invalid/index".to_string()),
                target_name: "serde".to_string(),
                target_kinds: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                source_path: PathBuf::from("/transient/serde/src/lib.rs"),
                artifact_paths: Vec::new(),
                root_module: "src/lib.rs".to_string(),
                edition: "2021".to_string(),
                mode: "build".to_string(),
                platform: Some("x86_64-unknown-linux-gnu".to_string()),
                target_is_explicit: Some(true),
                cfg: vec!["target_has_atomic=\"8\"".to_string()],
                effective_features: vec!["derive".to_string()],
                dependencies: Vec::new(),
                sysroot_externs: Vec::new(),
                build_script: None,
                registry_source: Some(super::super::OvenLegacyCargoSelectedRegistrySource {
                    registry: "registry+https://example.invalid/index".to_string(),
                    checksum: "sha256:fixture".to_string(),
                    digest: selected_graph_source_digest(&source_members)?,
                    root_module: "src/lib.rs".to_string(),
                    members: source_members
                        .iter()
                        .map(|member| super::super::OvenLegacyCargoInspectionSourceMember {
                            path: member.path.clone(),
                            digest: member.digest.clone(),
                        })
                        .collect(),
                }),
            }],
        })
    }

    /// A registry package with a build script captures two registry-backed units: the library and the
    /// run-custom-build execution node. Only the library is an inspection crate.
    #[test]
    fn registry_unit_bindings_never_model_a_build_script_as_an_inspection_crate()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        let library = capture.units[0].clone();
        let registry = library
            .registry_source
            .clone()
            .ok_or("fixture library must carry a registry source")?;
        let mut build_script = library.clone();
        build_script.target_name = "build-script-build".to_string();
        build_script.target_kinds = vec!["custom-build".to_string()];
        build_script.crate_types = vec!["bin".to_string()];
        build_script.source_path = PathBuf::from("/transient/serde/build.rs");
        build_script.root_module = "build.rs".to_string();
        build_script.mode = "run-custom-build".to_string();
        build_script.registry_source = Some(super::super::OvenLegacyCargoSelectedRegistrySource {
            root_module: "build.rs".to_string(),
            ..registry.clone()
        });
        capture.units.push(build_script);
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
                build_script: None,
            });
        let members = registry
            .members
            .iter()
            .map(|member| super::super::OvenLegacyCargoInspectionSourceMember {
                path: member.path.clone(),
                digest: member.digest.clone(),
            })
            .collect::<Vec<_>>();
        let source = super::super::OvenLegacyCargoInspectionSource {
            package: library.package.clone(),
            version: library.package_version.clone(),
            registry: registry.registry.clone(),
            checksum: registry.checksum.clone(),
            features: vec!["derive".to_string()],
            source_root: PathBuf::from("/staged/registry-sources/serde-1.0.0"),
            source_digest: registry.digest.clone(),
            members,
        };
        let bindings = legacy_cargo_registry_unit_bindings(&capture, std::slice::from_ref(&source), "sha256:owner")?;
        assert_eq!(
            bindings.keys().copied().collect::<Vec<_>>(),
            vec![0],
            "only the library unit is an inspection crate; the run-custom-build unit is sealed on its consumer edge"
        );
        let binding = bindings.get(&0).ok_or("library binding must exist")?;
        assert_eq!(binding.source.identity, "registry:serde@1.0.0");
        // The binder's own output must satisfy the registry validation the projection later applies to it.
        let owners = vec![OvenSelectedRustFacetOwner {
            identity: "sha256:owner".to_string(),
            kind: OvenSelectedRustFacetOwnerKind::Constituent,
        }];
        validate_registry_binding(&capture.units[0], binding, &owners)?;
        Ok(())
    }

    /// A transport root, one registry library with a build script, and the catalog that describes it: the shape
    /// the Release publisher captures. `checksum` is the registry source both the capture and a registry name.
    fn release_shaped_capture(
        checksum: &str,
        script_environment: BTreeMap<String, String>,
    ) -> Result<
        (
            OvenLegacyCargoSelectedUnitCapture,
            Vec<super::super::OvenLegacyCargoInspectionSource>,
        ),
        Box<dyn std::error::Error>,
    > {
        let mut capture = capture()?;
        let mut library = capture.units[0].clone();
        let mut registry = library
            .registry_source
            .clone()
            .ok_or("fixture library must carry a registry source")?;
        registry.checksum = checksum.to_string();
        library.registry_source = Some(registry.clone());
        capture.units[0] = library.clone();
        let mut build_script = library.clone();
        build_script.target_name = "build-script-build".to_string();
        build_script.target_kinds = vec!["custom-build".to_string()];
        build_script.crate_types = vec!["bin".to_string()];
        build_script.source_path = PathBuf::from("/transient/serde/build.rs");
        build_script.root_module = "build.rs".to_string();
        build_script.mode = "run-custom-build".to_string();
        build_script.registry_source = Some(super::super::OvenLegacyCargoSelectedRegistrySource {
            root_module: "build.rs".to_string(),
            ..registry.clone()
        });
        let mut root = library.clone();
        root.package_id = "path+file:///fixture#oven_release_stdlib@0.1.0".to_string();
        root.package = "oven_release_stdlib".to_string();
        root.package_version = "0.1.0".to_string();
        root.package_source = None;
        root.target_name = "oven_release_stdlib".to_string();
        root.registry_source = None;
        root.dependencies = vec![super::super::OvenLegacyCargoSelectedDependency {
            unit_index: 1,
            extern_crate_name: Some("serde".to_string()),
            build_script: None,
        }];
        // Indices are pre-shift: the root is inserted at 0 below, moving the library to 1 and its edge target to 2.
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
                build_script: Some(super::super::OvenLegacyCargoBuildScriptFacts {
                    cfgs: Vec::new(),
                    environment: script_environment,
                    linked_libraries: Vec::new(),
                    linked_paths: Vec::new(),
                    out_dir: PathBuf::from("/transient/serde/out"),
                    output: None,
                }),
            });
        capture.units.insert(0, root);
        for edge in &mut capture.units[1].dependencies {
            edge.unit_index += 1;
        }
        capture.units.push(build_script);
        capture.roots = vec![0];
        let members = registry
            .members
            .iter()
            .map(|member| super::super::OvenLegacyCargoInspectionSourceMember {
                path: member.path.clone(),
                digest: member.digest.clone(),
            })
            .collect::<Vec<_>>();
        let sources = vec![super::super::OvenLegacyCargoInspectionSource {
            package: library.package.clone(),
            version: library.package_version.clone(),
            registry: registry.registry.clone(),
            checksum: registry.checksum.clone(),
            features: vec!["derive".to_string()],
            source_root: PathBuf::from("/staged/registry-sources/serde-1.0.0"),
            source_digest: registry.digest.clone(),
            members,
        }];
        Ok((capture, sources))
    }

    /// The publisher's own sequence: provisional projection, closure digest, capture receipt, sealed projection,
    /// then compiler-support finalization under the given registry authority.
    fn finalize_release_shaped(
        capture: &OvenLegacyCargoSelectedUnitCapture,
        sources: &[super::super::OvenLegacyCargoInspectionSource],
        authority: &LoafRegistryAuthority,
    ) -> Result<OvenFinalizedCompilerSupportSelectedGraph, Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let receipt = receipt_generated_project(&fixture_receipt_request(directory.path(), "release-stdlib")?)?;
        let toolchain_owner = digest(b"release toolchain owner");
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"oven_release_stdlib\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\n",
            Path::new("loaf.toml"),
        )?;
        let (_, generated) = legacy_cargo_generated_output_bindings(capture)?;
        let linked = legacy_cargo_generated_archive_bindings(capture, &generated)?;
        let provisional = legacy_cargo_foundation_projection(
            capture,
            &receipt,
            sources,
            &receipt.identity,
            &toolchain_owner,
            &linked,
            authority,
        )?;
        let closure_digest = legacy_cargo_build_script_closure_digest(capture, &provisional.build_scripts)?;
        let capture_receipt =
            receipt_with_build_unit_input(&receipt, OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT, closure_digest)?;
        let projection = legacy_cargo_foundation_projection(
            capture,
            &receipt,
            sources,
            &capture_receipt.identity,
            &toolchain_owner,
            &linked,
            authority,
        )?;
        assert!(
            projection.units.contains_key(&1)
                && !projection.units.contains_key(&0)
                && !projection.units.contains_key(&2),
            "the binder seals registry units: the transport root is pruned later and the build script is an edge, got {:?}",
            projection.units.keys().collect::<Vec<_>>()
        );
        Ok(finalize_compiler_support_selected_graph(
            capture,
            &projection,
            &manifest,
            &BTreeSet::new(),
            &toolchain_owner,
            &receipt,
        )?)
    }

    /// The Release runtime-foundation publisher seals its capture with the production binder, never a hand-made
    /// projection. This walks that exact sequence for a transport root, one registry library and its build script,
    /// plus a package the script alone depends on, which Cargo compiled to run the script and Oven never links.
    #[test]
    fn foundation_projection_of_a_registry_closure_finalizes_through_the_production_binder()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut capture, mut sources) = release_shaped_capture("sha256:fixture", BTreeMap::new())?;
        let mut script_only = capture.units[1].clone();
        script_only.package_id = "registry+https://example.invalid/index#cc@1.0.0".to_string();
        script_only.package = "cc".to_string();
        script_only.target_name = "cc".to_string();
        script_only.source_path = PathBuf::from("/transient/cc/src/lib.rs");
        script_only.dependencies = Vec::new();
        capture.units.push(script_only);
        capture.units[2]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 3,
                extern_crate_name: Some("cc".to_string()),
                build_script: None,
            });
        let mut cc_source = sources[0].clone();
        cc_source.package = "cc".to_string();
        cc_source.source_root = PathBuf::from("/staged/registry-sources/cc-1.0.0");
        sources.push(cc_source);
        let finalized = finalize_release_shaped(&capture, &sources, &LoafRegistryAuthority::none())?;
        let graph = finalized.graph.graph();
        assert_eq!(
            graph.units.len(),
            1,
            "the transport root leaves, the build script is not a graph unit, and the script's own dependency never enters the closure"
        );
        assert_eq!(graph.units[0].source.identity, "registry:serde@1.0.0");
        assert_eq!(graph.exposed_roots["serde"].unit, graph.units[0].identity);
        Ok(())
    }

    /// The finalized capture receipt is derived from the pruned capture, so it is not the identity the projection
    /// rooted the registry sources under; the foundation must take its artifact owner from the graph's Constituent
    /// owner, or the asset later finds it absent from the owner table.
    #[test]
    fn runtime_foundation_artifact_owner_is_the_graph_constituent() -> Result<(), Box<dyn std::error::Error>> {
        use oven_rustc::loaf::{
            OVEN_LOAF_SCHEMA_VERSION, OvenLoafAccounting, OvenLoafCompatibility, OvenLoafProvenance,
        };
        use oven_rustc::rustc::{
            OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
            OvenRustcRegistryLeaf,
        };
        let (capture, sources) = release_shaped_capture("serde-checksum", BTreeMap::new())?;
        let finalized = finalize_release_shaped(&capture, &sources, &LoafRegistryAuthority::none())?;
        let graph = finalized.graph.graph();
        let constituent = graph
            .owners
            .iter()
            .find(|owner| owner.kind == OvenSelectedRustFacetOwnerKind::Constituent)
            .ok_or("the finalized graph names a Constituent owner")?;
        assert_ne!(
            constituent.identity, finalized.capture_receipt.identity,
            "pruning renumbers the build-script closure, so the finalized capture receipt is a different identity"
        );
        let [unit] = graph.units.as_slice() else {
            return Err("the release-shaped graph has one unit".into());
        };
        let intent = oven_store::OvenBuildIntent {
            target: graph.selection.intent.target.clone(),
            toolchain: graph.selection.intent.toolchain.clone(),
            profile: graph.selection.intent.profile.clone(),
            features: Vec::new(),
        };
        let plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![OvenRustcRegistryLeaf {
                domain: Default::default(),
                crate_kind: Default::default(),
                selected_unit_identity: Some(unit.identity.clone()),
                package: unit.package.clone(),
                version: unit.package_version.clone(),
                crate_name: unit.crate_name.clone(),
                features: unit.features.clone(),
                source: OvenRustcRegistrySource {
                    registry: "registry+https://example.invalid/index".to_string(),
                    checksum: "serde-checksum".to_string(),
                    relative_root: unit.source.root.clone(),
                    digest: unit.source.digest.clone(),
                },
                artifact: OvenRustcArtifactExtern {
                    crate_name: unit.crate_name.clone(),
                    relative_path: "deps/libserde.rlib".to_string(),
                    digest: "sha256:serde-artifact".to_string(),
                },
            }],
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: finalized.final_receipt.build_unit_identity.clone(),
            provenance: OvenLoafProvenance {
                compiler_version: "fixture".to_string(),
                rust_toolchain: intent.toolchain.clone(),
                sdk_provider_codegen_revision: "fixture".to_string(),
                baker: "legacy_cargo".to_string(),
            },
            accounting: OvenLoafAccounting {
                payload_logical_bytes: 0,
                payload_physical_bytes: 0,
            },
            compatibility: OvenLoafCompatibility {
                runtime_inputs: BTreeMap::new(),
                providers: Vec::new(),
            },
            registry_leaves: plan.registry_leaves.clone(),
            plan,
        };
        let foundation = runtime_foundation_from_compiled_loaf(&finalized, &loaf, "sha256:compiled-plan")?;
        assert_eq!(foundation.artifact_owner, constituent.identity);
        assert_eq!(foundation.units.len(), 1);
        Ok(())
    }

    fn adoption_registry(
        checksum: &str,
        cfg: &str,
    ) -> Result<(tempfile::TempDir, oven_model::loaf_registry::LoafRegistry), Box<dyn std::error::Error>> {
        let root = tempdir()?;
        let manifest_dir = root.path().join("crates-io/serde/1.0.0");
        fs::create_dir_all(&manifest_dir)?;
        fs::write(
            manifest_dir.join("loaf.toml"),
            format!(
                "[project]\nname = \"serde\"\nversion = \"1.0.0\"\n\n[source]\nregistry = \"https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{checksum}\"\n\n[[rust.facts]]\ntoolchain = \"rustc 1.98.0\"\ntarget = \"x86_64-unknown-linux-gnu\"\nprofile = \"release\"\nfeatures = [\"derive\"]\ncfg = [{cfg}]\n"
            ),
        )?;
        let index_dir = root.path().join("index/se/rd");
        fs::create_dir_all(&index_dir)?;
        fs::write(
            index_dir.join("serde"),
            format!(
                "{{\"cksum\":\"{checksum}\",\"manifest\":\"crates-io/serde/1.0.0/loaf.toml\",\"name\":\"serde\",\"source\":\"crates-io\",\"vers\":\"1.0.0\"}}\n"
            ),
        )?;
        let registry = oven_model::loaf_registry::LoafRegistry::open(root.path())?;
        Ok((root, registry))
    }

    /// A build script that reemits configuration through `rustc-env` gives the graph no admissible value: the
    /// validator retains an unregistered name neither as text nor otherwise, so the observation is withheld from
    /// the graph and the unit publishes with an empty environment. A registry declaration for the exact source and
    /// selection is the RFC 119 answer: the projection carries the declared facts and nothing else, and the graph
    /// looks the same either way.
    #[test]
    fn a_registry_declaration_governs_an_adopted_unit_and_must_agree_with_its_observation()
    -> Result<(), Box<dyn std::error::Error>> {
        let checksum = format!("sha256:{}", "a".repeat(64));
        let script_environment = BTreeMap::from([("CFG_OPT_LEVEL".to_string(), "3".to_string())]);
        let (capture, sources) = release_shaped_capture(&checksum, script_environment)?;

        let unadopted = finalize_release_shaped(&capture, &sources, &LoafRegistryAuthority::none())?;
        assert!(
            unadopted.graph.graph().units[0].environment.is_empty(),
            "without a declaration the script-emitted environment is withheld from the graph, not refused"
        );

        let (_root, registry) = adoption_registry(&checksum, "")?;
        let authority = LoafRegistryAuthority::resolve(&capture, &registry, "release")?;
        assert!(
            authority.adoption(1).is_some(),
            "the registry library is adopted for the captured selection"
        );
        assert!(authority.adoption(0).is_none() && authority.adoption(2).is_none());
        let records = authority.registry_records();
        assert_eq!(records.len(), 1, "one lock record per governed registry unit");
        assert_eq!(
            (
                records[0].package.as_str(),
                records[0].version.as_str(),
                records[0].checksum.as_str(),
                records[0].status.as_str(),
            ),
            ("serde", "1.0.0", checksum.as_str(), "harvested"),
            "a fixture index line lists no binding status, so the adoption is harvested"
        );
        assert!(oven_model::manifest::is_sha256_identity(&records[0].index_line_digest));
        let finalized = finalize_release_shaped(&capture, &sources, &authority)?;
        let unit = &finalized.graph.graph().units[0];
        assert!(
            unit.environment.is_empty(),
            "the declaration carries no environment, so neither does the graph"
        );
        assert!(authority.evidence_digest().is_some());

        let (_root, disagreeing) = adoption_registry(&checksum, "\"has_answer\"")?;
        let authority = LoafRegistryAuthority::resolve(&capture, &disagreeing, "release")?;
        let refused = finalize_release_shaped(&capture, &sources, &authority);
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().contains("disagrees with the observed build script")),
            "a declaration the observation contradicts is a refusal: {:?}",
            refused.as_ref().err().map(ToString::to_string)
        );

        let (_root, other_profile) = adoption_registry(&checksum, "")?;
        let authority = LoafRegistryAuthority::resolve(&capture, &other_profile, "debug")?;
        assert!(authority.is_empty(), "a record bound to another profile adopts nothing");
        Ok(())
    }

    /// `libm` reemits its configuration through `rustc-env` for its own test logging; the values are not portable
    /// text (`["arch", "default"]`) and the names are nobody's public compiler fact. Without a registry declaration
    /// the release publisher must still publish: those entries are withheld from the graph, a registered name the
    /// same script emitted is carried as text, and the closure digest binds every observation either way (#1704).
    #[test]
    fn an_unadopted_script_environment_is_withheld_from_the_graph_and_bound_by_the_closure_digest_issue1704()
    -> Result<(), Box<dyn std::error::Error>> {
        let checksum = format!("sha256:{}", "b".repeat(64));
        let withheld_value = "[\"arch\", \"default\"]";
        let script_environment = BTreeMap::from([
            ("CFG_CARGO_FEATURES".to_string(), withheld_value.to_string()),
            ("CFG_OPT_LEVEL".to_string(), "3".to_string()),
            ("CFG_TARGET_FEATURES".to_string(), "[\"neon\", \"sha2\"]".to_string()),
            ("PROFILE".to_string(), "release".to_string()),
        ]);
        let (capture, sources) = release_shaped_capture(&checksum, script_environment)?;

        let finalized = finalize_release_shaped(&capture, &sources, &LoafRegistryAuthority::none())?;
        let unit = &finalized.graph.graph().units[0];
        assert_eq!(
            unit.environment,
            BTreeMap::from([(
                "PROFILE".to_string(),
                OvenSelectedRustFacetEnvironmentValue::Text {
                    value: "release".to_string()
                }
            )]),
            "only the registered public fact reaches the graph"
        );
        let encoded = String::from_utf8(finalized.graph.to_json_bytes()?)?;
        assert!(
            !encoded.contains("CFG_CARGO_FEATURES") && !encoded.contains(withheld_value),
            "a withheld observation must not reach the serialized graph"
        );

        // The binder states the withholding per key, and the closure digest changes with the withheld value.
        let directory = tempdir()?;
        let receipt = receipt_generated_project(&fixture_receipt_request(directory.path(), "release-stdlib")?)?;
        let toolchain_owner = digest(b"release toolchain owner");
        let (_, generated) = legacy_cargo_generated_output_bindings(&capture)?;
        let linked = legacy_cargo_generated_archive_bindings(&capture, &generated)?;
        let bindings = legacy_cargo_foundation_projection(
            &capture,
            &receipt,
            &sources,
            &receipt.identity,
            &toolchain_owner,
            &linked,
            &LoafRegistryAuthority::none(),
        )?
        .build_scripts;
        let edge = bindings
            .get(&(1, 2))
            .ok_or("the library's build-script edge must be bound")?;
        assert_eq!(
            edge.environment.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["CFG_CARGO_FEATURES", "CFG_OPT_LEVEL", "CFG_TARGET_FEATURES", "PROFILE"],
            "every observed key gets one binding"
        );
        assert_eq!(edge.environment["CFG_CARGO_FEATURES"].value, None);
        assert_eq!(edge.environment["CFG_CARGO_FEATURES"].observed_value, withheld_value);
        assert_eq!(
            edge.environment["PROFILE"].value,
            Some(OvenSelectedRustFacetEnvironmentValue::Text {
                value: "release".to_string()
            })
        );
        let closure_digest = legacy_cargo_build_script_closure_digest(&capture, &bindings)?;
        let (changed_capture, _) = release_shaped_capture(
            &checksum,
            BTreeMap::from([
                ("CFG_CARGO_FEATURES".to_string(), "[\"default\"]".to_string()),
                ("CFG_OPT_LEVEL".to_string(), "3".to_string()),
                ("CFG_TARGET_FEATURES".to_string(), "[\"neon\", \"sha2\"]".to_string()),
                ("PROFILE".to_string(), "release".to_string()),
            ]),
        )?;
        let changed_bindings = legacy_cargo_foundation_projection(
            &changed_capture,
            &receipt,
            &sources,
            &receipt.identity,
            &toolchain_owner,
            &linked,
            &LoafRegistryAuthority::none(),
        )?
        .build_scripts;
        assert_ne!(
            closure_digest,
            legacy_cargo_build_script_closure_digest(&changed_capture, &changed_bindings)?,
            "a withheld value still binds the capture receipt through the closure digest"
        );
        Ok(())
    }

    fn sealed(
        capture: &OvenLegacyCargoSelectedUnitCapture,
    ) -> Result<OvenLegacyCargoSelectedGraphProjection, OvenLegacyCargoError> {
        let source_owner = digest(b"registry source owner");
        let toolchain_owner = selection().target_spec.toolchain_owner().to_string();
        let source_members = members();
        let source_digest = selected_graph_source_digest(&source_members)
            .map_err(|error| projection_error("fixture source digest", &error.to_string()))?;
        let registry = capture.units[0]
            .registry_source
            .as_ref()
            .ok_or_else(|| projection_error("fixture registry source", "is absent"))?;
        if source_digest != registry.digest {
            return Err(projection_error("fixture registry source", "has inconsistent digest"));
        }
        Ok(OvenLegacyCargoSelectedGraphProjection {
            selection: selection(),
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: source_owner.clone(),
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner,
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: BTreeMap::from([(
                0,
                OvenLegacyCargoSelectedGraphUnitBinding {
                    source: OvenSelectedRustFacetSource {
                        kind: OvenSelectedRustFacetSourceKind::Registry,
                        identity: "registry:serde@1.0.0".to_string(),
                        owner: source_owner.clone(),
                        root: ".".to_string(),
                        digest: source_digest.clone(),
                    },
                    source_members,
                    registry_source: Some(OvenRustcRegistrySourcePackage {
                        package: "serde".to_string(),
                        version: "1.0.0".to_string(),
                        features: vec!["derive".to_string()],
                        source: OvenRustcRegistrySource {
                            registry: "registry+https://example.invalid/index".to_string(),
                            checksum: "sha256:fixture".to_string(),
                            relative_root: ".".to_string(),
                            digest: source_digest.clone(),
                        },
                    }),
                    include_dirs: vec![OvenSelectedRustFacetPath {
                        owner: source_owner,
                        path: ".".to_string(),
                    }],
                    exclude_dirs: Vec::new(),
                },
            )]),
            generated: BTreeMap::new(),
            build_scripts: BTreeMap::new(),
        })
    }

    fn closure_final_receipt(
        capture: &OvenLegacyCargoSelectedUnitCapture,
        sealed: &OvenLegacyCargoSelectedGraphProjection,
    ) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let digest = legacy_cargo_build_script_closure_digest(capture, &sealed.build_scripts)?;
        Ok(receipt_generated_project(
            &fixture_receipt_request(directory.path(), "selected-graph-fixture")?
                .with_build_unit_input(OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT, digest),
        )?)
    }

    #[test]
    fn projection_retains_observed_compiler_support_features_without_creating_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?, None)?;
        assert!(graph.exposed_roots.is_empty());
        assert_eq!(graph.units[0].role, OvenSelectedRustFacetUnitRole::Library);
        assert_eq!(graph.units[0].features, ["derive"]);
        Ok(())
    }

    #[test]
    fn generated_archive_binding_requires_exact_search_root_target_and_member() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut capture = capture()?;
        capture.units.push(OvenLegacyCargoSelectedUnit {
            package_id: capture.units[0].package_id.clone(),
            package: capture.units[0].package.clone(),
            package_version: capture.units[0].package_version.clone(),
            package_source: capture.units[0].package_source.clone(),
            target_name: "build-script-build".to_string(),
            target_kinds: vec!["custom-build".to_string()],
            crate_types: vec!["bin".to_string()],
            source_path: PathBuf::from("/transient/serde/build.rs"),
            artifact_paths: Vec::new(),
            root_module: "build.rs".to_string(),
            edition: "2021".to_string(),
            mode: "run-custom-build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            target_is_explicit: Some(false),
            cfg: Vec::new(),
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: None,
        });
        let facts = super::super::OvenLegacyCargoBuildScriptFacts {
            cfgs: Vec::new(),
            environment: BTreeMap::new(),
            linked_libraries: vec!["static=fixture".to_string()],
            linked_paths: vec!["native=/transient/out/native".to_string()],
            out_dir: PathBuf::from("/transient/out"),
            output: Some(super::super::OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/fixture".to_string(),
                digest: digest(b"generated tree"),
                members: vec![
                    super::super::OvenLegacyCargoInspectionSourceMember {
                        path: "native/libfixture.a".to_string(),
                        digest: digest(b"right archive"),
                    },
                    super::super::OvenLegacyCargoInspectionSourceMember {
                        path: "other/libfixture.a".to_string(),
                        digest: digest(b"wrong archive"),
                    },
                ],
            }),
        };
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
                build_script: Some(facts.clone()),
            });
        let generated = BTreeMap::from([(
            (0, 1),
            OvenLegacyCargoSelectedGeneratedBinding {
                name: "out_dir".to_string(),
                source: OvenSelectedRustFacetPath {
                    owner: digest(b"generated owner"),
                    path: "generated-outputs/fixture".to_string(),
                },
                digest: digest(b"generated tree"),
            },
        )]);

        let bindings = legacy_cargo_generated_archive_bindings(&capture, &generated)?;
        let binding = bindings.get(&(0, 1)).ok_or("archive binding missing")?;
        let [
            OvenSelectedRustFacetLinkedLibrary::Archive {
                artifact,
                digest: archive_digest,
                ..
            },
        ] = binding.libraries.as_slice()
        else {
            return Err(std::io::Error::other("archive binding shape changed").into());
        };
        assert_eq!(artifact.path, "generated-outputs/fixture/native/libfixture.a");
        assert_eq!(archive_digest, &digest(b"right archive"));

        capture.units[0].dependencies[0]
            .build_script
            .as_mut()
            .ok_or("build-script facts missing")?
            .linked_paths = vec!["native=/transient/out/missing".to_string()];
        assert!(legacy_cargo_generated_archive_bindings(&capture, &generated).is_err());
        capture.units[0].dependencies[0]
            .build_script
            .as_mut()
            .ok_or("build-script facts missing")?
            .linked_paths = vec!["native=/transient/out/native".to_string()];
        capture.units[0].platform = Some("x86_64-pc-windows-gnu".to_string());
        let compiler = capture.compiler.as_mut().ok_or("fixture compiler context missing")?;
        compiler.target = "x86_64-pc-windows-gnu".to_string();
        compiler
            .target_cfg
            .values
            .insert("target_os".to_string(), vec!["windows".to_string()]);
        compiler
            .target_cfg
            .values
            .insert("target_env".to_string(), vec!["gnu".to_string()]);
        assert!(legacy_cargo_generated_archive_bindings(&capture, &generated).is_ok());
        capture.units[0].platform = Some("x86_64-pc-windows-msvc".to_string());
        let compiler = capture.compiler.as_mut().ok_or("fixture compiler context missing")?;
        compiler.target = "x86_64-pc-windows-msvc".to_string();
        compiler
            .target_cfg
            .values
            .insert("target_env".to_string(), vec!["msvc".to_string()]);
        assert!(legacy_cargo_generated_archive_bindings(&capture, &generated).is_err());
        Ok(())
    }

    #[test]
    fn built_in_target_spec_binds_the_verified_compiler_and_cfg_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let owner = digest(b"toolchain owner");
        let descriptor = legacy_cargo_builtin_target_spec(&capture, owner.clone())?;
        let OvenSelectedRustFacetTargetSpec::BuiltIn {
            toolchain_owner,
            target,
            rustc_identity,
            target_cfg_digest,
        } = descriptor
        else {
            return Err(std::io::Error::other("built-in capture produced a custom target spec").into());
        };
        assert_eq!(toolchain_owner, owner);
        assert_eq!(target, "x86_64-unknown-linux-gnu");
        assert_eq!(rustc_identity, "rustc 1.98.0");
        assert_eq!(
            target_cfg_digest,
            oven_rustc::rustc::selected_graph_sha256(&serde_json::to_vec(
                &capture
                    .compiler
                    .as_ref()
                    .ok_or("fixture lost compiler facts")?
                    .target_cfg
            )?)
        );
        Ok(())
    }

    #[test]
    fn projection_derives_target_role_from_captured_kind_and_mode() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0].target_name = "fixture-bin".to_string();
        capture.units[0].target_kinds = vec!["bin".to_string()];
        capture.units[0].crate_types = vec!["bin".to_string()];
        capture
            .compiler
            .as_mut()
            .ok_or("fixture compiler context missing")?
            .host = "aarch64-apple-darwin".to_string();
        let mut sealed = sealed(&capture)?;
        sealed.selection.host = "aarch64-apple-darwin".to_string();
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed, None)?;
        assert_eq!(graph.units[0].role, OvenSelectedRustFacetUnitRole::Binary);
        Ok(())
    }

    #[test]
    fn projection_uses_explicit_target_for_native_test_units() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0].target_kinds = vec!["test".to_string()];
        capture.units[0].crate_types = vec!["bin".to_string()];
        capture.units[0].mode = "test".to_string();
        capture.units[0].target_is_explicit = Some(true);
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?, None)?;
        assert_eq!(graph.units[0].role, OvenSelectedRustFacetUnitRole::IntegrationTest);
        Ok(())
    }

    #[test]
    fn projection_uses_absent_target_for_host_proc_macro() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0].target_kinds = vec!["proc-macro".to_string()];
        capture.units[0].crate_types = vec!["proc-macro".to_string()];
        capture.units[0].target_is_explicit = Some(false);
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?, None)?;
        assert_eq!(graph.units[0].role, OvenSelectedRustFacetUnitRole::ProcMacro);
        assert_eq!(graph.units[0].domain, OvenSelectedRustFacetDomain::Host);
        Ok(())
    }

    #[test]
    fn projection_refuses_untraced_target_provenance() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0].target_is_explicit = None;
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?, None).is_err());
        Ok(())
    }

    #[test]
    fn projection_refuses_registry_catalog_checksum_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let mut sealed = sealed(&capture)?;
        sealed
            .units
            .get_mut(&0)
            .ok_or("fixture unit missing")?
            .registry_source
            .as_mut()
            .ok_or("fixture registry catalog missing")?
            .source
            .checksum = "sha256:substituted".to_string();
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed, None).is_err());
        Ok(())
    }

    #[test]
    fn projection_refuses_a_changed_compiler_cfg_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let mut sealed = sealed(&capture)?;
        sealed.selection.target_cfg.flags.push("unix".to_string());
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed, None).is_err());
        Ok(())
    }

    #[test]
    fn projection_refuses_generic_capture_without_a_stable_trace() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.rustc_invocations_observed = false;
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?, None).is_err());
        Ok(())
    }

    #[test]
    fn compiler_support_projection_binds_only_the_sealed_root_authority() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let sealed = sealed(&capture)?;
        let raw = project_legacy_cargo_selected_graph(&capture, &sealed, None)?;
        let directory = tempdir()?;
        let capture_receipt =
            receipt_generated_project(&fixture_receipt_request(directory.path(), "compiler-release-fixture")?)?;
        let root = OvenCompilerSupportRootIntent {
            alias: "serde".to_string(),
            unit: raw.units[0].identity.clone(),
            requested_features: Vec::new(),
            default_features: true,
            intent_owner: sealed.selection.target_spec.toolchain_owner().to_string(),
            source_owner: raw.units[0].source.owner.clone(),
        };
        let authority = OvenCompilerSupportRootIntentAuthority {
            schema_version: oven_rustc::rustc::OVEN_COMPILER_SUPPORT_ROOT_INTENT_SCHEMA_VERSION,
            capture_receipt_identity: capture_receipt.identity.clone(),
            roots: vec![root],
        };
        let final_receipt = receipt_with_compiler_support_root_intent(
            &capture_receipt,
            compiler_support_root_intent_digest(&authority)?,
        )?;

        let (bound, _) = project_and_bind_compiler_support_selected_graph(
            &capture,
            &sealed,
            &authority,
            &capture_receipt,
            &final_receipt,
        )?;
        assert_eq!(
            bound.graph().exposed_roots["serde"].requested_features,
            Vec::<String>::new()
        );
        assert_eq!(bound.graph().units[0].features, ["derive"]);
        Ok(())
    }

    #[test]
    fn compiler_support_root_alias_does_not_select_a_transitive_variant() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        let mut sealed = sealed(&capture)?;
        let mut direct = capture.units[0].clone();
        direct.effective_features = vec!["alloc".to_string()];
        direct
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 0,
                extern_crate_name: Some("renamed_serde".to_string()),
                build_script: None,
            });
        capture.units.push(direct.clone());
        let mut binding = sealed.units.get(&0).cloned().ok_or("missing fixture source binding")?;
        binding
            .registry_source
            .as_mut()
            .ok_or("missing fixture registry source")?
            .features = vec!["alloc".to_string()];
        sealed.units.insert(1, binding);
        let mut transport = direct;
        transport.package = "transport".to_string();
        transport.dependencies = vec![super::super::OvenLegacyCargoSelectedDependency {
            unit_index: 1,
            extern_crate_name: Some("renamed_serde".to_string()),
            build_script: None,
        }];
        capture.units.push(transport);
        capture.roots = vec![2];
        let manifest = ProjectManifest::from_str(
            "[project]\nname='root-alias'\nversion='1.0.0'\n[rust-dependencies]\nrenamed_serde={package='serde',version='1',features=['alloc'],default-features=false}\n",
            Path::new("loaf.toml"),
        )?;
        let directory = tempdir()?;
        let receipt = receipt_generated_project(&fixture_receipt_request(directory.path(), "root-alias")?)?;
        let finalized = finalize_compiler_support_selected_graph(
            &capture,
            &sealed,
            &manifest,
            &BTreeSet::new(),
            sealed.selection.target_spec.toolchain_owner(),
            &receipt,
        )?;
        let graph = finalized.graph.graph();
        let root = &graph.exposed_roots["renamed_serde"];
        let selected = graph
            .units
            .iter()
            .find(|unit| unit.identity == root.unit)
            .ok_or("missing selected root")?;
        assert_eq!(selected.features, ["alloc"]);
        assert_eq!(
            graph.units.len(),
            2,
            "the transitive variant remains in the physical closure"
        );
        assert_ne!(selected.dependencies[0].unit, root.unit);
        Ok(())
    }

    #[test]
    fn compiler_support_authority_uses_authored_alias_and_feature_intent() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        let dependency = capture.units[0].clone();
        let mut root = dependency.clone();
        root.package_id = "path+file:///fixture#compiler-root@1.0.0".to_string();
        root.package = "compiler-root".to_string();
        root.package_version = "1.0.0".to_string();
        root.package_source = None;
        root.target_name = "compiler_root".to_string();
        root.root_module = "src/lib.rs".to_string();
        root.registry_source = None;
        root.dependencies = vec![super::super::OvenLegacyCargoSelectedDependency {
            unit_index: 0,
            extern_crate_name: Some("renamed_serde".to_string()),
            build_script: None,
        }];
        capture.units.push(root);
        capture.roots = vec![1];

        let sealed = {
            let mut sealed = sealed(&capture)?;
            let source_owner = digest(b"generated compiler root owner");
            sealed.owners.push(OvenSelectedRustFacetOwner {
                identity: source_owner.clone(),
                kind: OvenSelectedRustFacetOwnerKind::GeneratedOutput,
            });
            let members = members();
            sealed.units.insert(
                1,
                OvenLegacyCargoSelectedGraphUnitBinding {
                    source: OvenSelectedRustFacetSource {
                        kind: OvenSelectedRustFacetSourceKind::Generated,
                        identity: "generated:compiler-root@1.0.0".to_string(),
                        owner: source_owner,
                        root: "generated/compiler-root".to_string(),
                        digest: selected_graph_source_digest(&members)?,
                    },
                    source_members: members,
                    registry_source: None,
                    include_dirs: Vec::new(),
                    exclude_dirs: Vec::new(),
                },
            );
            sealed
        };
        let projected = project_legacy_cargo_selected_graph(&capture, &sealed, None)?;
        let receipt_directory = tempdir()?;
        let receipt = receipt_generated_project(&fixture_receipt_request(
            receipt_directory.path(),
            "compiler-support-authority",
        )?)?;
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"compiler-support-authority\"\nversion = \"1.0.0\"\n\n[rust-dependencies]\nrenamed_serde = { package = \"serde\", version = \"1\", features = [\"derive\", \"alloc\"], default-features = false }\n",
            Path::new("loaf.toml"),
        )?;
        let authority = compiler_support_root_intent_authority(
            &capture,
            &sealed,
            None,
            &manifest,
            &BTreeSet::new(),
            projected.selection.target_spec.toolchain_owner(),
            &receipt,
        )?;

        assert_eq!(authority.roots.len(), 1);
        assert_eq!(authority.roots[0].alias, "renamed_serde");
        assert_eq!(authority.roots[0].requested_features, ["alloc", "derive"]);
        assert!(!authority.roots[0].default_features);
        assert_eq!(authority.roots[0].unit, projected.units[0].identity);

        let finalized = finalize_compiler_support_selected_graph(
            &capture,
            &sealed,
            &manifest,
            &BTreeSet::new(),
            projected.selection.target_spec.toolchain_owner(),
            &receipt,
        )?;
        assert_ne!(finalized.capture_receipt.identity, receipt.identity);
        assert_ne!(finalized.final_receipt.identity, finalized.capture_receipt.identity);
        assert_eq!(finalized.graph.graph().units.len(), 1);
        assert_eq!(finalized.graph.graph().exposed_roots.len(), 1);
        assert_eq!(
            finalized.graph.graph().exposed_roots["renamed_serde"].unit,
            finalized.graph.graph().units[0].identity
        );
        let selected_unit = &finalized.graph.graph().units[0];
        let manifest_bytes = b"[package]\nname='serde'\nversion='1.0.0'\n";
        let response = serde_json::json!({
            "schema": "incan.oven.rust-policy-exchange/5",
            "operation": "validate_selected_rust_graph",
            "status": "selected",
            "graph_digest": finalized.graph.digest(),
            "inventories": [{
                "package": {
                    "name": selected_unit.package,
                    "version": selected_unit.package_version,
                    "source": selected_unit.source.identity,
                },
                "domain": "target",
                "owner": selected_unit.source.owner,
                "package_root": selected_unit.source.root,
                "manifest": {
                    "path": "Cargo.toml",
                    "bytes_hex": hex::encode(manifest_bytes),
                    "sha256_hex": digest(manifest_bytes).trim_start_matches("sha256:"),
                },
                "members": ["src/lib.rs"],
                "build_unit_present": false,
                "effective_features": selected_unit.features,
            }],
        });
        let inventories = runtime_foundation_inventories_from_policy_response(&finalized.graph, &response)?;
        assert_eq!(inventories.len(), 1);
        assert_eq!(inventories[0].selected_identity, selected_unit.identity);
        assert_eq!(inventories[0].package.manifest.path, "Cargo.toml");
        let mut tampered = response;
        tampered["inventories"][0]["manifest"]["bytes_hex"] = serde_json::json!("00");
        assert!(runtime_foundation_inventories_from_policy_response(&finalized.graph, &tampered).is_err());
        let stale_base = receipt_with_build_unit_input(
            &receipt,
            OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT,
            digest(b"stale closure"),
        )?;
        assert!(
            finalize_compiler_support_selected_graph(
                &capture,
                &sealed,
                &manifest,
                &BTreeSet::new(),
                projected.selection.target_spec.toolchain_owner(),
                &stale_base,
            )
            .is_err()
        );

        let wrong_package = ProjectManifest::from_str(
            "[project]\nname = \"compiler-support-authority\"\nversion = \"1.0.0\"\n\n[rust-dependencies]\nrenamed_serde = { package = \"serde_json\", version = \"1\" }\n",
            Path::new("loaf.toml"),
        )?;
        assert!(
            compiler_support_root_intent_authority(
                &capture,
                &sealed,
                None,
                &wrong_package,
                &BTreeSet::new(),
                projected.selection.target_spec.toolchain_owner(),
                &receipt,
            )
            .is_err()
        );
        Ok(())
    }

    /// One consumer whose build script retained the given generated inventory, sealed with a matching binding.
    fn generated_output_fixture(
        members: Vec<OvenSelectedRustFacetSourceMember>,
    ) -> Result<
        (
            OvenLegacyCargoSelectedUnitCapture,
            OvenLegacyCargoSelectedGraphProjection,
        ),
        Box<dyn std::error::Error>,
    > {
        let mut capture = capture()?;
        let generated_digest = oven_rustc::rustc::selected_graph_generated_input_digest(&members)?;
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
                build_script: Some(super::super::OvenLegacyCargoBuildScriptFacts {
                    cfgs: vec!["has_bindings".to_string()],
                    environment: BTreeMap::new(),
                    linked_libraries: Vec::new(),
                    linked_paths: Vec::new(),
                    out_dir: PathBuf::from("/transient/out"),
                    output: Some(super::super::OvenLegacyCargoSelectedGeneratedOutput {
                        relative_root: "generated-outputs/bindings".to_string(),
                        digest: generated_digest.clone(),
                        members: members
                            .iter()
                            .map(|member| super::super::OvenLegacyCargoInspectionSourceMember {
                                path: member.path.clone(),
                                digest: member.digest.clone(),
                            })
                            .collect(),
                    }),
                }),
            });
        capture.units.push(OvenLegacyCargoSelectedUnit {
            package_id: "registry+https://example.invalid/index#serde@1.0.0".to_string(),
            package: "serde".to_string(),
            package_version: "1.0.0".to_string(),
            package_source: Some("registry+https://example.invalid/index".to_string()),
            target_name: "build-script-build".to_string(),
            target_kinds: vec!["custom-build".to_string()],
            crate_types: vec!["bin".to_string()],
            source_path: PathBuf::from("/transient/serde/build.rs"),
            artifact_paths: Vec::new(),
            root_module: "build.rs".to_string(),
            edition: "2021".to_string(),
            mode: "run-custom-build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            target_is_explicit: Some(false),
            cfg: Vec::new(),
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: None,
            registry_source: None,
        });
        let mut sealed = sealed(&capture)?;
        let generated_owner = digest(b"generated owner");
        sealed.owners.push(OvenSelectedRustFacetOwner {
            identity: generated_owner.clone(),
            kind: OvenSelectedRustFacetOwnerKind::GeneratedOutput,
        });
        sealed.generated.insert(
            (0, 1),
            OvenLegacyCargoSelectedGeneratedBinding {
                name: "bindings".to_string(),
                source: OvenSelectedRustFacetPath {
                    owner: generated_owner,
                    path: "generated-outputs/bindings".to_string(),
                },
                digest: generated_digest,
            },
        );
        Ok((capture, sealed))
    }

    #[test]
    fn projection_attaches_only_the_exact_retained_generated_output() -> Result<(), Box<dyn std::error::Error>> {
        let (capture, sealed) = generated_output_fixture(vec![OvenSelectedRustFacetSourceMember {
            path: "bindings.rs".to_string(),
            digest: digest(b"pub const BINDING: u32 = 1;\n"),
        }])?;
        let final_receipt = closure_final_receipt(&capture, &sealed)?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed, Some(&final_receipt))?;
        assert_eq!(graph.units[0].cfg, ["has_bindings", "target_has_atomic=\"8\""]);
        assert_eq!(graph.units[0].generated_inputs.len(), 1);
        assert_eq!(graph.units[0].generated_inputs[0].name, "bindings");
        Ok(())
    }

    /// A build script that emits only cfgs leaves a checked empty `OUT_DIR`, which the capture retains as an empty
    /// inventory distinct from an absent output. The projection must admit it under the same digest rule.
    #[test]
    fn projection_attaches_a_checked_empty_generated_output() -> Result<(), Box<dyn std::error::Error>> {
        let (capture, sealed) = generated_output_fixture(Vec::new())?;
        let final_receipt = closure_final_receipt(&capture, &sealed)?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed, Some(&final_receipt))?;
        assert_eq!(graph.units[0].generated_inputs.len(), 1);
        assert!(graph.units[0].generated_inputs[0].members.is_empty());
        assert_eq!(
            graph.units[0].generated_inputs[0].digest,
            oven_rustc::rustc::selected_graph_generated_input_digest(&[])?
        );
        Ok(())
    }

    #[test]
    fn projection_refuses_unconsumed_generated_binding() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let mut sealed = sealed(&capture)?;
        sealed.generated.insert(
            (0, 0),
            OvenLegacyCargoSelectedGeneratedBinding {
                name: "unused".to_string(),
                source: OvenSelectedRustFacetPath {
                    owner: sealed.units[&0].source.owner.clone(),
                    path: "generated/unused".to_string(),
                },
                digest: digest(b"unused"),
            },
        );
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed, None).is_err());
        Ok(())
    }

    #[test]
    fn projection_projects_exact_sealed_environment_and_link_closure() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
                build_script: None,
            });
        capture.units.push(OvenLegacyCargoSelectedUnit {
            package_id: "registry+https://example.invalid/index#serde@1.0.0".to_string(),
            package: "serde".to_string(),
            package_version: "1.0.0".to_string(),
            package_source: Some("registry+https://example.invalid/index".to_string()),
            target_name: "build-script-build".to_string(),
            target_kinds: vec!["custom-build".to_string()],
            crate_types: vec!["bin".to_string()],
            source_path: PathBuf::from("/transient/serde/build.rs"),
            artifact_paths: Vec::new(),
            root_module: "build.rs".to_string(),
            edition: "2021".to_string(),
            mode: "run-custom-build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            target_is_explicit: Some(false),
            cfg: Vec::new(),
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: Some(super::super::OvenLegacyCargoBuildScriptFacts {
                cfgs: Vec::new(),
                environment: BTreeMap::from([("DEP_FIXTURE".to_string(), "/transient/out".to_string())]),
                linked_libraries: vec!["static=fixture".to_string(), "static=fixture".to_string()],
                linked_paths: vec!["native=/transient/out".to_string()],
                out_dir: PathBuf::from("/transient/out"),
                output: None,
            }),
            registry_source: None,
        });
        let facts = capture.units[1].build_script.take();
        capture.units[0]
            .dependencies
            .last_mut()
            .ok_or("selected consumer lost its build-script edge")?
            .build_script = facts;
        let mut sealed = sealed(&capture)?;
        let source_owner = sealed.units[&0].source.owner.clone();
        sealed.build_scripts.insert(
            (0, 1),
            OvenLegacyCargoSelectedBuildScriptBinding {
                environment: BTreeMap::from([(
                    "DEP_FIXTURE".to_string(),
                    OvenLegacyCargoSelectedEnvironmentBinding {
                        observed_value: "/transient/out".to_string(),
                        value: Some(OvenSelectedRustFacetEnvironmentValue::Text {
                            value: "/transient/out".to_string(),
                        }),
                    },
                )]),
                linked_libraries: Some(OvenLegacyCargoSelectedLinkedLibraryBinding {
                    observed_libraries: vec!["static=fixture".to_string(), "static=fixture".to_string()],
                    observed_paths: vec!["native=/transient/out".to_string()],
                    libraries: vec![
                        OvenSelectedRustFacetLinkedLibrary::Archive {
                            name: "fixture".to_string(),
                            kind: oven_rustc::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
                            artifact: OvenSelectedRustFacetPath {
                                owner: source_owner.clone(),
                                path: "linked/libfixture.a".to_string(),
                            },
                            digest: digest(b"fixture archive"),
                        },
                        OvenSelectedRustFacetLinkedLibrary::Archive {
                            name: "fixture".to_string(),
                            kind: oven_rustc::rustc::OvenSelectedRustFacetLinkedLibraryKind::Static,
                            artifact: OvenSelectedRustFacetPath {
                                owner: source_owner,
                                path: "linked/libfixture.a".to_string(),
                            },
                            digest: digest(b"fixture archive"),
                        },
                    ],
                }),
                declaration: None,
            },
        );
        capture
            .build_script_tool_probes
            .push(super::super::OvenLegacyCargoBuildScriptToolProbe {
                package_id: capture.units[1].package_id.clone(),
                out_dir: PathBuf::from("/transient/out"),
                target_context: "x86_64-unknown-linux-gnu".to_string(),
                rustc_target: "x86_64-unknown-linux-gnu".to_string(),
                digest: digest(b"first bounded probe"),
            });
        let first_probe_digest = legacy_cargo_build_script_closure_digest(&capture, &sealed.build_scripts)?;
        capture.build_script_tool_probes[0].digest = digest(b"changed bounded probe");
        assert_ne!(
            first_probe_digest,
            legacy_cargo_build_script_closure_digest(&capture, &sealed.build_scripts)?
        );
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed, None).is_err());
        let wrong_receipt_directory = tempdir()?;
        let wrong_final_receipt = receipt_generated_project(&fixture_receipt_request(
            wrong_receipt_directory.path(),
            "selected-graph-fixture",
        )?)?;
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed, Some(&wrong_final_receipt)).is_err());
        let final_receipt = closure_final_receipt(&capture, &sealed)?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed, Some(&final_receipt))?;
        assert_eq!(
            graph.units[0].environment["DEP_FIXTURE"],
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "/transient/out".to_string()
            }
        );
        assert_eq!(graph.units[0].linked_libraries.len(), 2);
        assert_eq!(graph.units[0].linked_libraries[0], graph.units[0].linked_libraries[1]);
        Ok(())
    }
}
