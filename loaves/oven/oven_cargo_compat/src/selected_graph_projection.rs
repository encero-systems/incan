//! Projection of sealed Cargo capture facts into Oven's portable selected Rust graph.
//!
//! Cargo capture observes compiled units, but it intentionally has no authority to assign portable source owners or
//! authored feature requests. This module joins that observation to publisher-retained physical bindings and emits a
//! rootless graph. The caller must bind roots with an admitted project or compiler-support authority afterwards.

use std::collections::{BTreeMap, BTreeSet};

use oven_rustc::rustc::{
    OvenCompilerSupportRootIntentAuthority, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
    OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency, OvenSelectedRustFacetDomain,
    OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetLinkedLibrary, OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind,
    OvenSelectedRustFacetPath, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
    OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetUnit,
    OvenSelectedRustFacetUnitRole, ValidatedOvenSelectedRustFacetGraph, bind_compiler_support_root_intents,
    selected_graph_unit_identity,
};
use oven_store::OvenReceipt;
use serde::Serialize;

use super::{
    OvenLegacyCargoBuildScriptToolProbe, OvenLegacyCargoError, OvenLegacyCargoSelectedGeneratedOutput,
    OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture,
};

/// Receipt key binding the complete selected build-script closure to a final publisher transaction.
pub const OVEN_LEGACY_CARGO_BUILD_SCRIPT_CLOSURE_INPUT: &str = "legacy-cargo-build-script-closure";

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoSelectedEnvironmentBinding {
    /// Exact raw Cargo observation retained only for producer-side comparison.
    pub observed_value: String,
    /// Portable selected value that enters graph identity after the comparison succeeds.
    pub value: OvenSelectedRustFacetEnvironmentValue,
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

/// Identify only the execution node whose Cargo build script produced reusable compiler facts.
///
/// Cargo also compiles the `custom-build` binary itself. That ordinary build unit remains a physical compiler unit;
/// only the `run-custom-build` execution node supplies generated output, cfg, environment, link directives, or
/// probes to a consumer edge.
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

    let graph = OvenSelectedRustFacetGraph {
        schema_version: oven_rustc::rustc::OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
        selection: sealed.selection.clone(),
        owners: sealed.owners.clone(),
        units: units.into_values().collect(),
        exposed_roots: BTreeMap::new(),
    };
    Ok(graph)
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
) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenLegacyCargoError> {
    let graph = project_legacy_cargo_selected_graph(capture, sealed, Some(final_receipt))?;
    bind_compiler_support_root_intents(graph, authority, capture_receipt, final_receipt)
        .map_err(|error| projection_error("compiler-support selected graph", &error.to_string()))
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
            let bound = binding
                .environment
                .get(name)
                .ok_or_else(|| projection_error("selected build-script environment", "has no typed sealed binding"))?;
            match &bound.value {
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
            if &bound.observed_value != observed {
                return Err(projection_error(
                    "selected build-script environment",
                    "does not match its exact captured value",
                ));
            }
            if environment.insert(name.clone(), bound.value.clone()).is_some() {
                return Err(projection_error(
                    "selected build-script environment",
                    "is supplied by more than one build-script edge",
                ));
            }
        }
        if binding.environment.len() != facts.environment.len() {
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
        || oven_rustc::rustc::selected_graph_source_digest(&captured_members)
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
        let expected_identity = format!("registry:{}@{}", unit.package, unit.package_version);
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

/// Derive compilation domain only from a traced unit platform matched to the sealed host or target.
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
    if platform == selection.host {
        return Ok(OvenSelectedRustFacetDomain::Host);
    }
    if platform == selection.intent.target {
        return Ok(OvenSelectedRustFacetDomain::Target);
    }
    Err(projection_error(
        "selected Cargo unit platform",
        "does not match the sealed host or target",
    ))
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
        vec![OvenSelectedRustFacetSourceMember {
            path: "src/lib.rs".to_string(),
            digest: digest(b"pub fn fixture() {}\n"),
        }]
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
            target_spec: OvenSelectedRustFacetTargetSpec {
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
                root_module: "src/lib.rs".to_string(),
                edition: "2021".to_string(),
                mode: "build".to_string(),
                platform: Some("x86_64-unknown-linux-gnu".to_string()),
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

    fn sealed(
        capture: &OvenLegacyCargoSelectedUnitCapture,
    ) -> Result<OvenLegacyCargoSelectedGraphProjection, OvenLegacyCargoError> {
        let source_owner = digest(b"registry source owner");
        let toolchain_owner = selection().target_spec.source.owner;
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
            intent_owner: sealed.selection.target_spec.source.owner.clone(),
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

        let bound = project_and_bind_compiler_support_selected_graph(
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
    fn projection_attaches_only_the_exact_retained_generated_output() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        let generated_digest = selected_graph_source_digest(&[OvenSelectedRustFacetSourceMember {
            path: "bindings.rs".to_string(),
            digest: digest(b"pub const BINDING: u32 = 1;\n"),
        }])?;
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
            root_module: "build.rs".to_string(),
            edition: "2021".to_string(),
            mode: "run-custom-build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            cfg: Vec::new(),
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            sysroot_externs: Vec::new(),
            build_script: Some(super::super::OvenLegacyCargoBuildScriptFacts {
                cfgs: vec!["has_bindings".to_string()],
                environment: BTreeMap::new(),
                linked_libraries: Vec::new(),
                linked_paths: Vec::new(),
                out_dir: PathBuf::from("/transient/out"),
                output: Some(super::super::OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/bindings".to_string(),
                    digest: generated_digest.clone(),
                    members: vec![super::super::OvenLegacyCargoInspectionSourceMember {
                        path: "bindings.rs".to_string(),
                        digest: digest(b"pub const BINDING: u32 = 1;\n"),
                    }],
                }),
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

        let final_receipt = closure_final_receipt(&capture, &sealed)?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed, Some(&final_receipt))?;
        assert_eq!(graph.units[0].cfg, ["has_bindings", "target_has_atomic=\"8\""]);
        assert_eq!(graph.units[0].generated_inputs.len(), 1);
        assert_eq!(graph.units[0].generated_inputs[0].name, "bindings");
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
            root_module: "build.rs".to_string(),
            edition: "2021".to_string(),
            mode: "run-custom-build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
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
                        value: OvenSelectedRustFacetEnvironmentValue::Text {
                            value: "/transient/out".to_string(),
                        },
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
