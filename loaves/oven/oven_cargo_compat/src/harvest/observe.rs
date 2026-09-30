//! Conversion of captured registry units into canonical harvest proposals and refusals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use oven_model::loaf_registry::canonical_checksum;
use oven_model::manifest::{RustFactOut, is_sha256_identity};

use super::super::{
    OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoError, OvenLegacyCargoSelectedCompilerContext,
    OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture, digest_bytes,
};
use super::model::*;
use super::output::safe_relative;

// ============================================================================
// Harvesting a capture
// ============================================================================

/// The observation of one registry unit before it is folded with its equals.
struct Observation {
    fact: HarvestFact,
    source: HarvestSource,
    out_relative_root: Option<String>,
    /// Digests of the compiler probes whose only answer is the fact's `cfg` list; evidence, never part of the fact.
    compiler_probes: Vec<String>,
}

/// Turn one capture into proposals for every immediately admissible registry unit and refusals for the rest.
///
/// A unit is harvestable when it is registry-backed, compiled for the captured target, and has at most one retained
/// build-script observation. Binding-derived constants need no new fact. Before writing,
/// [`HarvestProposal::admitted_record`] proves that the candidate declares everything its script did; candidates with
/// unresolved environment/link/tool observations become typed refusals that retain those observations and their byte
/// identities. The proposal's `out` entries name every retained member by path and digest, so the reader's
/// `check_observation` compares equal on the next capture. No package can occur in both proposal and refusal channels
/// for one report.
///
/// The capture must carry its compiler context: without it no record can bind a toolchain or target, so that is an
/// error rather than a refusal of every unit.
pub fn harvest_registry_units(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    evidence: &HarvestEvidenceInputs,
    profile: &str,
) -> Result<HarvestReport, OvenLegacyCargoError> {
    let compiler = capture.compiler.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("harvest requires a capture with its compiler selection".to_string())
    })?;
    if !matches!(profile, "release" | "debug") {
        return Err(OvenLegacyCargoError::Plan(format!(
            "harvest profile must be `release` or `debug`, not `{profile}`"
        )));
    }
    let hazards = harvest_hazards(compiler, evidence);
    let mut refusals = BTreeSet::new();
    let mut observations: BTreeMap<(String, String, Vec<String>), Vec<Observation>> = BTreeMap::new();

    // ---- Classify every registry binding: refuse, or record its observation ----
    for unit in &capture.units {
        if is_build_script_unit(unit) {
            continue;
        }
        match observe_unit(capture, compiler, unit, profile, &evidence.rustc_identity) {
            Ok(observation) => {
                let key = (
                    unit.package.clone(),
                    unit.package_version.clone(),
                    observation.fact.features.clone(),
                );
                observations.entry(key).or_default().push(observation);
            }
            Err(refusal) => {
                refusals.insert(*refusal);
            }
        }
    }

    // ---- Fold equal observations of one selection; refuse a selection whose observations disagree ----
    let mut proposals = Vec::new();
    for ((package, version, _), mut group) in observations {
        let Some(first) = group.pop() else {
            continue;
        };
        if group
            .iter()
            .any(|other| other.fact != first.fact || other.source != first.source)
        {
            let refusal_observations = raw_refusal_observations(
                std::iter::once(&first.fact).chain(group.iter().map(|observation| &observation.fact)),
            );
            refusals.insert(HarvestRefusal {
                package,
                version,
                reason: HarvestRefusalReason::ConflictingObservations,
                detail: format!("{} units of one selection observed different facts", group.len() + 1),
                observations: refusal_observations,
            });
            continue;
        }
        let compiler_probes = first
            .compiler_probes
            .iter()
            .chain(group.iter().flat_map(|observation| observation.compiler_probes.iter()))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut proposal = HarvestProposal {
            project: HarvestProject { name: package, version },
            source: first.source,
            rust: HarvestRustFacts {
                facts: vec![first.fact],
            },
            evidence: HarvestEvidence {
                method: HARVEST_EVIDENCE_METHOD.to_string(),
                receipt: evidence.receipt.clone(),
                rustc_identity: evidence.rustc_identity.clone(),
                host: compiler.host.clone(),
                hazards: hazards.clone(),
                cargo_version: evidence.cargo_version.clone(),
                cargo_lock_digest: evidence.cargo_lock_digest.clone(),
                cargo_manifest_digest: evidence.cargo_manifest_digest.clone(),
                compiler_probes,
            },
            notes: evidence.notes.clone(),
            out_relative_root: first.out_relative_root,
        };
        match proposal.admitted_record() {
            Ok(record) => {
                let [fact] = proposal.rust.facts.as_mut_slice() else {
                    return Err(OvenLegacyCargoError::Plan(
                        "admitted harvest proposal lost its sole fact".to_string(),
                    ));
                };
                fact.out = record.out;
                fact.link = record.link;
                fact.tool = record.tool;
                fact.link_observations.clear();
                fact.tool_observations.clear();
                proposals.push(proposal);
            }
            Err(reason) => {
                refusals.insert(admission_refusal(&proposal, reason));
            }
        }
    }

    // ---- A registry drop never names one package in both channels ----
    let refused_packages = refusals
        .iter()
        .map(|refusal| refusal.package.clone())
        .collect::<BTreeSet<_>>();
    let mut held_back = Vec::new();
    proposals.retain(|proposal| {
        if refused_packages.contains(&proposal.project.name) {
            held_back.push((proposal.project.name.clone(), proposal.project.version.clone()));
            false
        } else {
            true
        }
    });
    for (package, version) in held_back {
        refusals.insert(HarvestRefusal {
            package,
            version,
            reason: HarvestRefusalReason::PackageHasRefusedBinding,
            detail: "another binding of this package was refused".to_string(),
            observations: HarvestRefusalObservations::default(),
        });
    }
    Ok(HarvestReport {
        proposals,
        refusals: refusals.into_iter().collect(),
        hazards,
        profile: profile.to_string(),
    })
}

/// Convert an in-memory admission refusal into the complete on-disk refusal evidence the registry can inspect.
pub(super) fn admission_refusal(proposal: &HarvestProposal, refusal: HarvestAdmissionRefusal) -> HarvestRefusal {
    let observations = raw_refusal_observations(proposal.rust.facts.iter());
    let (reason, detail) = match refusal {
        HarvestAdmissionRefusal::FactCount => (
            HarvestRefusalReason::ConflictingObservations,
            "proposal did not contain exactly one fact".to_string(),
        ),
        HarvestAdmissionRefusal::UnresolvedEnvironmentInputs => {
            let names = observations
                .environment
                .iter()
                .map(|input| input.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (HarvestRefusalReason::EnvironmentObserved, names)
        }
        HarvestAdmissionRefusal::UnresolvedLinkObservations => {
            if let Some(detail) = observations
                .link
                .iter()
                .find_map(|observation| observation.conversion_refusal.clone())
            {
                (HarvestRefusalReason::LinkedLibraries, detail)
            } else {
                let libraries = observations
                    .link
                    .iter()
                    .flat_map(|observation| observation.libraries.iter().map(String::as_str))
                    .collect::<Vec<_>>();
                if libraries.is_empty() {
                    let paths = observations
                        .link
                        .iter()
                        .map(|observation| observation.search_paths.len())
                        .sum::<usize>();
                    (
                        HarvestRefusalReason::LinkedPaths,
                        format!("{paths} link search path(s)"),
                    )
                } else {
                    (HarvestRefusalReason::LinkedLibraries, libraries.join(", "))
                }
            }
        }
        HarvestAdmissionRefusal::UnresolvedToolObservations => (
            HarvestRefusalReason::ToolProbes,
            format!("{} compiler probe(s)", observations.tool.len()),
        ),
        HarvestAdmissionRefusal::ConflictingObservations => (
            HarvestRefusalReason::ConflictingObservations,
            "typed publisher output disagrees with retained out bytes".to_string(),
        ),
    };
    HarvestRefusal {
        package: proposal.project.name.clone(),
        version: proposal.project.version.clone(),
        reason,
        detail,
        observations,
    }
}

/// Collect deterministic raw evidence from every candidate fact represented by one typed refusal.
fn raw_refusal_observations<'a>(facts: impl IntoIterator<Item = &'a HarvestFact>) -> HarvestRefusalObservations {
    let mut observations = HarvestRefusalObservations::default();
    for fact in facts {
        observations.environment.extend(fact.environment_inputs.iter().cloned());
        observations.link.extend(fact.link_observations.iter().cloned());
        observations.tool.extend(fact.tool_observations.iter().cloned());
    }
    observations.environment.sort();
    observations.environment.dedup();
    observations.link.sort();
    observations.link.dedup();
    observations.tool.sort();
    observations.tool.dedup();
    observations
}

/// Observe one unit, or say why it cannot be proposed.
fn observe_unit(
    capture: &OvenLegacyCargoSelectedUnitCapture,
    compiler: &OvenLegacyCargoSelectedCompilerContext,
    unit: &OvenLegacyCargoSelectedUnit,
    profile: &str,
    rustc_executable_identity: &str,
) -> Result<Observation, Box<HarvestRefusal>> {
    let refuse = |reason: HarvestRefusalReason, detail: String| {
        Box::new(HarvestRefusal {
            package: unit.package.clone(),
            version: unit.package_version.clone(),
            reason,
            detail,
            observations: HarvestRefusalObservations::default(),
        })
    };
    if is_build_script_unit(unit) {
        // Named by what it is, never by its position: Cargo orders the unit graph differently between runs, and a
        // refusal list must be the same bytes for the same closure.
        return Err(refuse(
            HarvestRefusalReason::BuildScriptUnit,
            "run-custom-build execution node; the package's library unit is harvested on its own".to_string(),
        ));
    }
    let Some(registry_source) = unit.registry_source.as_ref() else {
        return Err(refuse(
            HarvestRefusalReason::NotRegistryBacked,
            unit.package_source
                .clone()
                .unwrap_or_else(|| "no package source".to_string()),
        ));
    };
    // Cargo's lock spells the checksum bare; the registry binds it as a `sha256:` identity.
    let checksum = canonical_checksum(&registry_source.checksum).ok_or_else(|| {
        refuse(
            HarvestRefusalReason::MalformedChecksum,
            "registry checksum is not a sha256 identity".to_string(),
        )
    })?;
    if let Some(platform) = unit.platform.as_deref()
        && platform != compiler.target
    {
        return Err(refuse(
            HarvestRefusalReason::TargetMismatch,
            format!("compiled for `{platform}`, capture target is `{}`", compiler.target),
        ));
    }

    let build_script = observe_build_script(capture, unit).map_err(|(reason, detail)| refuse(reason, detail))?;
    let facts = build_script.facts;
    let build_unit = build_script.build_unit;

    // ---- The fact, in the reader's shape ----
    let mut features = unit.effective_features.clone();
    features.sort();
    features.dedup();
    let mut cfg = facts.map(|facts| facts.cfgs.clone()).unwrap_or_default();
    cfg.sort();
    cfg.dedup();
    let output = facts.and_then(|facts| facts.output.as_ref());
    if output.is_some_and(|output| !is_sha256_identity(&output.digest)) {
        return Err(refuse(
            HarvestRefusalReason::MalformedOutput,
            "retained output tree has no sha256 digest".to_string(),
        ));
    }
    // The compiler probes this unit's build script ran, and the files that carried their answers. A probe's own
    // output is not a generated input: nothing the package compiles reads it, so it is neither `out` nor a product.
    // It is recognized only by the path and bytes the capture recorded when the probe finished; a file at that path
    // holding other bytes was rewritten by the script, and the harvest refuses rather than guess which it is.
    let unit_probes = match (facts, build_unit) {
        (Some(facts), Some(build_unit)) => capture
            .build_script_tool_probes
            .iter()
            .filter(|probe| probe.package_id == build_unit.package_id && probe.out_dir == facts.out_dir)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    // Several probes may write one path in turn (rustix probes every feature into `rustix_test_can_compile`), so a
    // path maps to every set of bytes a probe left there.
    let mut probe_outputs = BTreeMap::<&str, BTreeSet<&str>>::new();
    for probe_output in unit_probes.iter().filter_map(|probe| probe.output.as_ref()) {
        probe_outputs
            .entry(probe_output.relative_path.as_str())
            .or_default()
            .insert(probe_output.digest.as_str());
    }
    if let Some(facts) = facts {
        for probe_output in facts
            .publisher_native_probes
            .iter()
            .filter_map(|probe| probe.output.as_ref())
        {
            probe_outputs
                .entry(probe_output.relative_path.as_str())
                .or_default()
                .insert(probe_output.digest.as_str());
        }
    }
    let members = output.map(|output| output.members.as_slice()).unwrap_or_default();
    if let Some(member) = members.iter().find(|member| {
        probe_outputs
            .get(member.path.as_str())
            .is_some_and(|digests| !digests.contains(member.digest.as_str()))
    }) {
        return Err(refuse(
            HarvestRefusalReason::ConflictingObservations,
            format!(
                "OUT_DIR member `{}` is a probe's output rewritten by the script",
                member.path
            ),
        ));
    }
    let generated_members = members
        .iter()
        .filter(|member| !probe_outputs.contains_key(member.path.as_str()));
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for member in generated_members.clone() {
        if safe_relative(&member.path).is_none() || !is_sha256_identity(&member.digest) {
            return Err(refuse(
                HarvestRefusalReason::MalformedOutput,
                format!(
                    "retained member `{}` is not a plain relative path with a sha256 digest",
                    member.path
                ),
            ));
        }
        if !names.insert(member.path.as_str()) {
            return Err(refuse(
                HarvestRefusalReason::MalformedOutput,
                format!("retained member `{}` is inventoried twice", member.path),
            ));
        }
        out.push(RustFactOut {
            name: member.path.clone(),
            path: format!("{HARVEST_OUT_DIRECTORY}/{}", member.path),
            digest: member.digest.clone(),
        });
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    let mut products = generated_members
        .map(|member| HarvestObservedProduct {
            owner_relative_path: member.path.clone(),
            digest: member.digest.clone(),
        })
        .collect::<Vec<_>>();
    products.sort();

    let environment_inputs = observe_environment_inputs(facts, &features, profile, &compiler.target_cfg, &products)
        .map_err(|(reason, detail)| refuse(reason, detail))?;

    // ---- Native-link paths retain ownership, never their publisher staging coordinates ----
    let mut link_observations = Vec::new();
    if let Some(facts) = facts
        && (!facts.linked_libraries.is_empty() || !facts.linked_paths.is_empty())
    {
        let mut libraries = facts.linked_libraries.clone();
        libraries.sort();
        libraries.dedup();
        let mut search_paths = Vec::new();
        for value in &facts.linked_paths {
            let (kind, path) = value.split_once('=').unwrap_or(("all", value));
            let Some(owner_relative_path) = owner_relative_path(Path::new(path), &facts.out_dir) else {
                return Err(refuse(
                    HarvestRefusalReason::LinkedPaths,
                    "link search path is not owned by the retained output".to_string(),
                ));
            };
            search_paths.push(HarvestLinkSearchPath {
                kind: kind.to_string(),
                owner_relative_path,
            });
        }
        search_paths.sort();
        search_paths.dedup();
        let output_tree_digest = output.map(|output| output.digest.clone()).ok_or_else(|| {
            refuse(
                HarvestRefusalReason::OutputNotRetained,
                "link products were not retained".to_string(),
            )
        })?;
        link_observations.push(HarvestLinkObservation {
            libraries,
            search_paths,
            output_tree_digest,
            products: products.clone(),
            name: None,
            executable: None,
            objects: Vec::new(),
            environment: Vec::new(),
            sources: Vec::new(),
            library: None,
            conversion_refusal: facts.publisher_work_refusal.clone(),
        });
    }
    let publisher_links = facts
        .into_iter()
        .flat_map(|facts| facts.publisher_work.iter())
        .filter(|work| matches!(work.role, oven_model::manifest::RustFactProducerRole::Link))
        .collect::<Vec<_>>();
    if let [observation] = link_observations.as_slice()
        && !publisher_links.is_empty()
    {
        let template = observation.clone();
        link_observations = publisher_links
            .iter()
            .map(|work| {
                let mut observation = template.clone();
                observation.libraries = work
                    .library
                    .as_ref()
                    .map(|library| vec![format!("static={}", library.name)])
                    .unwrap_or_default();
                observation.name = Some(work.name.clone());
                observation.executable = work.executable.clone();
                observation.objects = work.objects.clone();
                observation.environment = work.environment.clone();
                observation.sources = work.inputs.clone();
                observation.library = work.library.clone();
                observation.conversion_refusal = None;
                observation
            })
            .collect();
    }

    // ---- Tool probes retain their target domains, invocation identity and generated product identities ----
    let mut tool_observations = Vec::new();
    let output_tree_digest = output.map(|output| output.digest.clone());
    for probe in &unit_probes {
        let Some(output_tree_digest) = output_tree_digest.clone() else {
            return Err(refuse(
                HarvestRefusalReason::OutputNotRetained,
                "tool products were not retained".to_string(),
            ));
        };
        tool_observations.push(HarvestToolObservation {
            target_context: probe.target_context.clone(),
            rustc_target: probe.rustc_target.clone(),
            probe_digest: probe.digest.clone(),
            executable_identity: rustc_executable_identity.to_string(),
            output_tree_digest,
            products: products.clone(),
            name: None,
            executable: None,
            arguments: Vec::new(),
            environment: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        });
    }

    // ---- A compiler probe answers only through `cfg` ----
    // RFC 119 records a probe's answer, not the probe. A compiler probe is a bounded, non-linking rustc invocation
    // that emits only metadata or IR, so its answer is a function of the compiler and the crate source, which the
    // record already binds by `toolchain`, `target` and `source.checksum`, and it produces no input the package
    // compiles. The probe digests therefore stay evidence, and two units whose probes were spelled differently (a host
    // unit's opt-level) but answered the same fold into one fact. Whatever else the script left in OUT_DIR is its
    // ordinary `out`, as for a script that probes nothing. A probe of another target is refused until a record binds
    // that target.
    let mut compiler_probes = Vec::new();
    if tool_observations
        .iter()
        .all(|observation| observation.rustc_target == compiler.target)
    {
        compiler_probes = tool_observations
            .drain(..)
            .map(|observation| observation.probe_digest)
            .collect();
    }
    if let Some(facts) = facts {
        compiler_probes.extend(facts.publisher_native_probes.iter().map(|probe| probe.digest.clone()));
    }
    if let (Some(facts), Some(output_tree_digest)) = (facts, output.map(|output| output.digest.clone())) {
        for work in facts
            .publisher_work
            .iter()
            .filter(|work| matches!(work.role, oven_model::manifest::RustFactProducerRole::Tool))
        {
            let encoded = serde_json::to_vec(work).map_err(|error| {
                refuse(
                    HarvestRefusalReason::ToolProbes,
                    format!("tool observation could not be encoded: {error}"),
                )
            })?;
            tool_observations.push(HarvestToolObservation {
                target_context: compiler.host.clone(),
                rustc_target: work.target.clone(),
                probe_digest: digest_bytes(&encoded),
                executable_identity: work
                    .executable
                    .as_ref()
                    .map(|executable| executable.digest.clone())
                    .unwrap_or_default(),
                output_tree_digest: output_tree_digest.clone(),
                products: products.clone(),
                name: Some(work.name.clone()),
                executable: work.executable.clone(),
                arguments: work.arguments.clone(),
                environment: work.environment.clone(),
                inputs: work.inputs.clone(),
                outputs: work.outputs.clone(),
            });
        }
    }
    tool_observations.sort();
    tool_observations.dedup();
    Ok(Observation {
        fact: HarvestFact {
            toolchain: compiler.toolchain.clone(),
            target: compiler.target.clone(),
            profile: profile.to_string(),
            features,
            cfg,
            out,
            environment_inputs,
            link_observations,
            tool_observations,
            link: Vec::new(),
            tool: Vec::new(),
        },
        source: HarvestSource {
            registry: registry_index_of(&registry_source.registry),
            checksum,
        },
        out_relative_root: output.map(|output| output.relative_root.clone()),
        compiler_probes,
    })
}

/// Optional retained build-script facts and their execution unit.
struct BuildScriptObservation<'a> {
    /// Retained facts attached to the consumer edge or build-script unit.
    facts: Option<&'a OvenLegacyCargoBuildScriptFacts>,
    /// Execution unit used to associate captured tool probes.
    build_unit: Option<&'a OvenLegacyCargoSelectedUnit>,
}

/// Resolve the optional retained build-script observation for one consumer unit.
fn observe_build_script<'a>(
    capture: &'a OvenLegacyCargoSelectedUnitCapture,
    unit: &'a OvenLegacyCargoSelectedUnit,
) -> Result<BuildScriptObservation<'a>, (HarvestRefusalReason, String)> {
    let mut script_edges = unit.dependencies.iter().filter_map(|dependency| {
        capture
            .units
            .get(dependency.unit_index)
            .filter(|candidate| is_build_script_unit(candidate))
            .map(|build_unit| (dependency, build_unit))
    });
    let edge = script_edges.next();
    if script_edges.next().is_some() {
        return Err((
            HarvestRefusalReason::MultipleBuildScriptEdges,
            "more than one run-custom-build edge".to_string(),
        ));
    }
    let Some((dependency, build_unit)) = edge else {
        return Ok(BuildScriptObservation {
            facts: None,
            build_unit: None,
        });
    };
    let facts = dependency
        .build_script
        .as_ref()
        .or(build_unit.build_script.as_ref())
        .ok_or_else(|| {
            (
                HarvestRefusalReason::OutputNotRetained,
                "build-script edge has no retained facts".to_string(),
            )
        })?;
    if facts.output.is_none() {
        return Err((
            HarvestRefusalReason::OutputNotRetained,
            "OUT_DIR was not inventoried".to_string(),
        ));
    }
    Ok(BuildScriptObservation {
        facts: Some(facts),
        build_unit: Some(build_unit),
    })
}

/// Rebind script environment observations to selected-binding constants or retained products.
fn observe_environment_inputs(
    facts: Option<&OvenLegacyCargoBuildScriptFacts>,
    features: &[String],
    profile: &str,
    target_cfg: &oven_rustc::rustc::OvenSelectedRustFacetCfgSnapshot,
    products: &[HarvestObservedProduct],
) -> Result<Vec<HarvestEnvironmentInput>, (HarvestRefusalReason, String)> {
    let mut environment_inputs = Vec::new();
    if let Some(facts) = facts {
        for (name, value) in &facts.environment {
            if let Some(expected) = binding_derived_environment_value(name, features, profile, target_cfg) {
                if value != &expected {
                    return Err((HarvestRefusalReason::BindingDerivedEnvironmentMismatch, name.clone()));
                }
                continue;
            }
            let Some(owner_relative_path) = owner_relative_path(Path::new(value), &facts.out_dir) else {
                return Err((HarvestRefusalReason::EnvironmentObserved, name.clone()));
            };
            let Some(product) = products
                .iter()
                .find(|product| product.owner_relative_path == owner_relative_path)
            else {
                return Err((HarvestRefusalReason::EnvironmentObserved, name.clone()));
            };
            environment_inputs.push(HarvestEnvironmentInput {
                name: name.clone(),
                owner_relative_path,
                digest: product.digest.clone(),
            });
        }
    }
    environment_inputs.sort();
    Ok(environment_inputs)
}

/// Derive the recognized script-emitted constants whose bytes are already fixed by the selected binding.
fn binding_derived_environment_value(
    name: &str,
    features: &[String],
    profile: &str,
    target_cfg: &oven_rustc::rustc::OvenSelectedRustFacetCfgSnapshot,
) -> Option<String> {
    match name {
        "CFG_CARGO_FEATURES" => Some(format!("{features:?}")),
        "CFG_OPT_LEVEL" => Some(if profile == "release" { "3" } else { "0" }.to_string()),
        "CFG_TARGET_FEATURES" => {
            let mut target_features = target_cfg
                .values
                .get("target_feature")
                .into_iter()
                .flatten()
                .flat_map(|value| value.split(','))
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            target_features.sort();
            target_features.dedup();
            Some(format!("{target_features:?}"))
        }
        _ => None,
    }
}

/// Rebind one path below `owner` to a portable owner-relative coordinate.
fn owner_relative_path(path: &Path, owner: &Path) -> Option<String> {
    let relative = path.strip_prefix(owner).ok()?;
    if relative.as_os_str().is_empty() {
        return Some(".".to_string());
    }
    let value = relative.to_str()?;
    safe_relative(value).map(|safe| safe.to_string_lossy().into_owned())
}

/// The execution node that supplies build-script facts to a consumer edge; the same test the projection applies.
fn is_build_script_unit(unit: &OvenLegacyCargoSelectedUnit) -> bool {
    unit.mode == "run-custom-build" && unit.target_kinds.iter().any(|kind| kind == "custom-build")
}

/// The registry index a Cargo source id names: `registry+https://…` becomes `https://…`.
///
/// The registry records `source.registry` as the index URL; Cargo's source-kind prefix is transport spelling.
fn registry_index_of(source: &str) -> String {
    source.strip_prefix("registry+").unwrap_or(source).to_string()
}
