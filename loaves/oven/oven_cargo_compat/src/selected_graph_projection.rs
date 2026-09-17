//! Projection of sealed Cargo capture facts into Oven's portable selected Rust graph.
//!
//! Cargo capture observes compiled units, but it intentionally has no authority to assign portable source owners or
//! authored feature requests. This module joins that observation to publisher-retained physical bindings and emits a
//! rootless graph. The caller must bind roots with an admitted project or compiler-support authority afterwards.

use std::collections::{BTreeMap, BTreeSet};

use oven_rustc::rustc::{
    OvenCompilerSupportRootIntentAuthority, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency,
    OvenSelectedRustFacetDomain, OvenSelectedRustFacetGeneratedInput, OvenSelectedRustFacetGraph,
    OvenSelectedRustFacetOwner, OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetPath,
    OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource, OvenSelectedRustFacetSourceKind,
    OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
    ValidatedOvenSelectedRustFacetGraph, bind_compiler_support_root_intents, selected_graph_unit_identity,
};
use oven_store::OvenReceipt;

use super::{
    OvenLegacyCargoError, OvenLegacyCargoSelectedGeneratedOutput, OvenLegacyCargoSelectedUnit,
    OvenLegacyCargoSelectedUnitCapture,
};

/// Publisher-retained physical binding for one Cargo-selected unit.
///
/// The capture owns the observed package, target, feature and edge facts. This record supplies only the portable
/// owner-relative source facts and explicit inspection role that the capture cannot safely derive from local paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenLegacyCargoSelectedGraphUnitBinding {
    /// Complete source identity already retained beneath an immutable publisher owner.
    pub source: OvenSelectedRustFacetSource,
    /// Complete source members retained under that owner; never reconstructed from a capture checkout path.
    pub source_members: Vec<OvenSelectedRustFacetSourceMember>,
    /// Physical inspection role for this selected compiler unit.
    pub role: OvenSelectedRustFacetUnitRole,
    /// Exact host or target domain retained by the publisher.
    pub domain: OvenSelectedRustFacetDomain,
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
}

/// Project one exact physical Cargo capture into a rootless selected Rust graph.
///
/// The result deliberately has no exposed roots. Cargo's observed effective features become only per-unit compiler
/// evidence; the caller must add authored root request/default intent using a separately admitted authority before
/// validating or publishing the graph.
pub fn project_legacy_cargo_selected_graph(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
) -> Result<OvenSelectedRustFacetGraph, OvenLegacyCargoError> {
    validate_projection_compiler(capture, sealed)?;
    validate_projection_bindings(capture, sealed)?;

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
            let (cfg, generated_inputs) = projected_build_script_facts(capture, sealed, index)?;
            let crate_kind = captured_crate_kind(capture_unit)?;
            if !oven_rustc::rustc::selected_graph_unit_role_is_valid(binding.role, binding.domain, crate_kind) {
                return Err(projection_error(
                    "selected graph unit role",
                    "does not match the captured crate kind and sealed compilation domain",
                ));
            }
            validate_unit_platform(capture_unit, binding.domain, &sealed.selection)?;
            validate_registry_binding(capture_unit, binding)?;

            let mut unit = OvenSelectedRustFacetUnit {
                identity: String::new(),
                package: capture_unit.package.clone(),
                package_version: capture_unit.package_version.clone(),
                crate_name: capture_unit.target_name.replace('-', "_"),
                crate_kind,
                role: binding.role,
                domain: binding.domain,
                edition: capture_unit.edition.clone(),
                source: binding.source.clone(),
                root_module: capture_unit.root_module.clone(),
                source_members: source_members(capture_unit, binding)?,
                features: capture_unit.effective_features.clone(),
                cfg,
                environment: BTreeMap::new(),
                include_dirs: binding.include_dirs.clone(),
                exclude_dirs: binding.exclude_dirs.clone(),
                dependencies,
                generated_inputs,
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
    let graph = project_legacy_cargo_selected_graph(capture, sealed)?;
    validate_compiler_support_roles(&graph, authority)?;
    bind_compiler_support_root_intents(graph, authority, capture_receipt, final_receipt)
        .map_err(|error| projection_error("compiler-support selected graph", &error.to_string()))
}

/// Require every compiler-release authority root to name a physical compiler-support unit before root admission.
///
/// A library-shaped Cargo unit is not automatically compiler support. The separately sealed compiler declaration
/// selects the role, and this check prevents that declaration from silently rebadging an ordinary projected library.
fn validate_compiler_support_roles(
    graph: &OvenSelectedRustFacetGraph,
    authority: &OvenCompilerSupportRootIntentAuthority,
) -> Result<(), OvenLegacyCargoError> {
    for root in &authority.roots {
        let unit = graph
            .units
            .iter()
            .find(|unit| unit.identity == root.unit)
            .ok_or_else(|| projection_error("compiler-support root", "names an absent selected unit"))?;
        if unit.role != OvenSelectedRustFacetUnitRole::CompilerSupport {
            return Err(projection_error(
                "compiler-support root",
                "does not name a unit physically projected with the CompilerSupport role",
            ));
        }
    }
    Ok(())
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

/// Attach checked build-script cfg and retained generated output to one consuming physical unit.
fn projected_build_script_facts(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    sealed: &OvenLegacyCargoSelectedGraphProjection,
    consumer: usize,
) -> Result<(Vec<String>, Vec<OvenSelectedRustFacetGeneratedInput>), OvenLegacyCargoError> {
    let unit = capture
        .units
        .get(consumer)
        .ok_or_else(|| projection_error("selected unit", "is absent"))?;
    let mut cfg = unit.cfg.clone();
    if cfg.is_empty() {
        return Err(projection_error(
            "selected Cargo unit cfg",
            "is absent; raw graph projection requires the exact stable rustc trace rather than an inferred empty set",
        ));
    }
    let mut generated = Vec::new();
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
        let facts = build_unit
            .build_script
            .as_ref()
            .ok_or_else(|| projection_error("selected build-script unit", "has no structured retained facts"))?;
        if !facts.environment.is_empty() || !facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty() {
            return Err(projection_error(
                "selected build-script facts",
                "contain environment or linked-library facts that schema 3 cannot represent as a complete sealed closure",
            ));
        }
        cfg.extend(facts.cfgs.iter().cloned());
        if let Some(output) = &facts.output {
            generated.push(projected_generated_input(
                sealed,
                consumer,
                dependency.unit_index,
                output,
            )?);
        }
    }
    cfg.sort();
    cfg.dedup();
    generated.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.digest.cmp(&right.digest)));
    Ok((cfg, generated))
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
    if binding.digest != output.digest || binding.source.path != output.relative_root {
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

/// Require a registry capture to agree with its sealed source catalog without interpreting local source paths.
fn validate_registry_binding(
    unit: &OvenLegacyCargoSelectedUnit,
    binding: &OvenLegacyCargoSelectedGraphUnitBinding,
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
        if binding.source.kind != OvenSelectedRustFacetSourceKind::Registry
            || binding.source.digest != registry.digest
            || registry.root_module != unit.root_module
            || binding.source_members != captured_members
        {
            return Err(projection_error(
                "selected registry source",
                "does not match its captured digest, member catalog, kind and root module",
            ));
        }
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
            "is not one supported unambiguous schema-3 inspection output class",
        )),
    }
}

/// Verify the retained domain against Cargo's explicit target platform; absent platform evidence never defaults.
fn validate_unit_platform(
    unit: &OvenLegacyCargoSelectedUnit,
    domain: OvenSelectedRustFacetDomain,
    selection: &OvenSelectedRustFacetSelection,
) -> Result<(), OvenLegacyCargoError> {
    let platform = unit.platform.as_deref().ok_or_else(|| {
        projection_error(
            "selected Cargo unit platform",
            "is absent; projection cannot infer host or target domain",
        )
    })?;
    let expected = match domain {
        OvenSelectedRustFacetDomain::Host => selection.host.as_str(),
        OvenSelectedRustFacetDomain::Target => selection.intent.target.as_str(),
    };
    if platform != expected {
        return Err(projection_error(
            "selected Cargo unit platform",
            "does not match its sealed compilation domain",
        ));
    }
    Ok(())
}

/// Return a bounded producer refusal without exposing a local path from capture.
fn projection_error(field: &str, message: &str) -> OvenLegacyCargoError {
    OvenLegacyCargoError::Plan(format!("{field} {message}"))
}

#[cfg(test)]
mod tests {
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
            host_cfg: OvenSelectedRustFacetCfgSnapshot {
                flags: Vec::new(),
                values: BTreeMap::new(),
            },
            target_cfg: OvenSelectedRustFacetCfgSnapshot {
                flags: Vec::new(),
                values: BTreeMap::new(),
            },
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
            compiler: Some(super::super::OvenLegacyCargoSelectedCompilerContext {
                host: "x86_64-unknown-linux-gnu".to_string(),
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.98.0".to_string(),
                rustc_identity: "rustc 1.98.0".to_string(),
                host_cfg: OvenSelectedRustFacetCfgSnapshot {
                    flags: Vec::new(),
                    values: BTreeMap::new(),
                },
                target_cfg: OvenSelectedRustFacetCfgSnapshot {
                    flags: Vec::new(),
                    values: BTreeMap::new(),
                },
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
                effective_features: vec!["derive".to_string()],
                dependencies: Vec::new(),
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
                        identity: "registry:https://example.invalid/index#serde@1.0.0".to_string(),
                        owner: source_owner.clone(),
                        root: ".".to_string(),
                        digest: source_digest,
                    },
                    source_members,
                    role: OvenSelectedRustFacetUnitRole::CompilerSupport,
                    domain: OvenSelectedRustFacetDomain::Target,
                    include_dirs: vec![OvenSelectedRustFacetPath {
                        owner: source_owner,
                        path: ".".to_string(),
                    }],
                    exclude_dirs: Vec::new(),
                },
            )]),
            generated: BTreeMap::new(),
        })
    }

    #[test]
    fn projection_retains_observed_compiler_support_features_without_creating_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let graph = project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?)?;
        assert!(graph.exposed_roots.is_empty());
        assert_eq!(graph.units[0].role, OvenSelectedRustFacetUnitRole::CompilerSupport);
        assert_eq!(graph.units[0].features, ["derive"]);
        Ok(())
    }

    #[test]
    fn projection_refuses_a_changed_compiler_cfg_snapshot() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let mut sealed = sealed(&capture)?;
        sealed.selection.target_cfg.flags.push("unix".to_string());
        assert!(project_legacy_cargo_selected_graph(&capture, &sealed).is_err());
        Ok(())
    }

    #[test]
    fn compiler_support_projection_binds_only_the_sealed_root_authority() -> Result<(), Box<dyn std::error::Error>> {
        let capture = capture()?;
        let sealed = sealed(&capture)?;
        let raw = project_legacy_cargo_selected_graph(&capture, &sealed)?;
        let directory = tempdir()?;
        let capture_receipt = receipt_generated_project(&OvenGeneratedProjectRequest::new(
            directory.path(),
            "compiler-release-fixture",
            "1.0.0",
            "x86_64-unknown-linux-gnu",
            "rustc 1.98.0",
            "release",
            Vec::new(),
        ))?;
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
        let generated_digest = digest(b"generated output");
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
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
            effective_features: Vec::new(),
            dependencies: Vec::new(),
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

        let graph = project_legacy_cargo_selected_graph(&capture, &sealed)?;
        assert_eq!(graph.units[0].cfg, ["has_bindings"]);
        assert_eq!(graph.units[0].generated_inputs.len(), 1);
        assert_eq!(graph.units[0].generated_inputs[0].name, "bindings");
        Ok(())
    }

    #[test]
    fn projection_refuses_unrepresentable_link_facts() -> Result<(), Box<dyn std::error::Error>> {
        let mut capture = capture()?;
        capture.units[0]
            .dependencies
            .push(super::super::OvenLegacyCargoSelectedDependency {
                unit_index: 1,
                extern_crate_name: None,
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
            effective_features: Vec::new(),
            dependencies: Vec::new(),
            build_script: Some(super::super::OvenLegacyCargoBuildScriptFacts {
                cfgs: Vec::new(),
                environment: BTreeMap::new(),
                linked_libraries: vec!["static=fixture".to_string()],
                linked_paths: Vec::new(),
                out_dir: PathBuf::from("/transient/out"),
                output: None,
            }),
            registry_source: None,
        });
        let error = match project_legacy_cargo_selected_graph(&capture, &sealed(&capture)?) {
            Ok(_) => return Err(std::io::Error::other("link facts should refuse").into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("linked-library"));
        Ok(())
    }
}
