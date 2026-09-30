//! End-to-end harvest contract and persistence regression tests.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactCompileEnvironment, RustFactExecutable,
    RustFactLibrary, RustFactLibraryKind, RustFactLinkLanguage, RustFactLinkObject, RustFactOut, RustFactOutput,
    RustFactRecord, RustFactWorkObservation, RustFactWorkRecord, is_sha256_identity,
};
use oven_rustc::rustc::{OvenSelectedRustFacetCfgSnapshot, selected_graph_sha256};
use serde::Serialize;
use tempfile::tempdir;

use super::super::{
    OvenLegacyCargoBuildScriptFacts, OvenLegacyCargoBuildScriptToolProbe, OvenLegacyCargoInspectionSourceMember,
    OvenLegacyCargoSelectedCompilerContext, OvenLegacyCargoSelectedDependency, OvenLegacyCargoSelectedGeneratedOutput,
    OvenLegacyCargoSelectedRegistrySource, OvenLegacyCargoSelectedUnit, OvenLegacyCargoSelectedUnitCapture,
    OvenLegacyNativeInvocation, digest_bytes,
};
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHECKSUM: &str = "sha256:41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad";

fn evidence() -> HarvestEvidenceInputs {
    HarvestEvidenceInputs {
        receipt: Some(format!("sha256:{}", "1".repeat(64))),
        rustc_identity: format!("sha256:{}", "4".repeat(64)),
        ambient_hazards: Vec::new(),
        notes: None,
        cargo_version: "cargo 1.98.0 (fixture)".to_string(),
        cargo_lock_digest: format!("sha256:{}", "2".repeat(64)),
        cargo_manifest_digest: format!("sha256:{}", "3".repeat(64)),
    }
}

fn cfg_snapshot() -> OvenSelectedRustFacetCfgSnapshot {
    OvenSelectedRustFacetCfgSnapshot {
        flags: vec!["unix".to_string()],
        values: BTreeMap::new(),
    }
}

/// A registry library unit with no edges; the release-shaped fixtures in `selected_graph_projection` add the
/// transport root and the build-script edge on top of this same shape.
fn library(package: &str, version: &str, features: &[&str]) -> OvenLegacyCargoSelectedUnit {
    OvenLegacyCargoSelectedUnit {
        package_id: format!("registry+https://github.com/rust-lang/crates.io-index#{package}@{version}"),
        package: package.to_string(),
        package_version: version.to_string(),
        package_source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        target_name: package.to_string(),
        target_kinds: vec!["lib".to_string()],
        crate_types: vec!["lib".to_string()],
        source_path: PathBuf::from(format!("/transient/{package}/src/lib.rs")),
        artifact_paths: Vec::new(),
        root_module: "src/lib.rs".to_string(),
        edition: "2021".to_string(),
        mode: "build".to_string(),
        platform: Some("x86_64-unknown-linux-gnu".to_string()),
        target_is_explicit: Some(true),
        cfg: Vec::new(),
        compiler_arguments: Vec::new(),
        compile_environment: BTreeMap::new(),
        effective_features: features.iter().map(|feature| feature.to_string()).collect(),
        dependencies: Vec::new(),
        sysroot_externs: Vec::new(),
        build_script: None,
        registry_source: Some(OvenLegacyCargoSelectedRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: CHECKSUM.to_string(),
            digest: selected_graph_sha256(b"source"),
            root_module: "src/lib.rs".to_string(),
            members: vec![OvenLegacyCargoInspectionSourceMember {
                path: "src/lib.rs".to_string(),
                digest: selected_graph_sha256(b"pub fn fixture() {}\n"),
            }],
        }),
    }
}

/// The run-custom-build node of `library`.
fn build_script_of(library: &OvenLegacyCargoSelectedUnit) -> OvenLegacyCargoSelectedUnit {
    let mut unit = library.clone();
    unit.target_name = "build-script-build".to_string();
    unit.target_kinds = vec!["custom-build".to_string()];
    unit.crate_types = vec!["bin".to_string()];
    unit.root_module = "build.rs".to_string();
    unit.mode = "run-custom-build".to_string();
    unit
}

fn facts(cfgs: &[&str], output: Option<OvenLegacyCargoSelectedGeneratedOutput>) -> OvenLegacyCargoBuildScriptFacts {
    OvenLegacyCargoBuildScriptFacts {
        cfgs: cfgs.iter().map(|cfg| cfg.to_string()).collect(),
        environment: BTreeMap::new(),
        linked_libraries: Vec::new(),
        linked_paths: Vec::new(),
        out_dir: PathBuf::from("/transient/out"),
        output,
        publisher_work: Vec::new(),
        publisher_native_probes: Vec::new(),
        publisher_work_refusal: None,
    }
}

/// A retained output tree with stable product bytes for raw-observation contract tests.
fn retained_products() -> OvenLegacyCargoSelectedGeneratedOutput {
    OvenLegacyCargoSelectedGeneratedOutput {
        relative_root: "generated-outputs/products".to_string(),
        digest: selected_graph_sha256(b"product-tree"),
        members: vec![OvenLegacyCargoInspectionSourceMember {
            path: "libfixture.a".to_string(),
            digest: selected_graph_sha256(b"archive"),
        }],
    }
}

/// A retained generated tree shaped like one Cranelift ISLE product.
fn retained_isle_products() -> OvenLegacyCargoSelectedGeneratedOutput {
    OvenLegacyCargoSelectedGeneratedOutput {
        relative_root: "generated-outputs/isle".to_string(),
        digest: selected_graph_sha256(b"isle products"),
        members: vec![OvenLegacyCargoInspectionSourceMember {
            path: "generated/isle_opt.rs".to_string(),
            digest: selected_graph_sha256(b"generated isle source"),
        }],
    }
}

#[test]
fn harvest_contract_preserves_link_product_byte_identities() -> TestResult {
    let mut linked = facts(&[], Some(retained_products()));
    linked.linked_libraries = vec!["static=fixture".to_string()];
    linked.linked_paths = vec!["native=/transient/out".to_string()];
    let report = harvest_registry_units(
        &capture(vec![(library("fixture-sys", "1.0.0", &[]), Some(linked))]),
        &evidence(),
        "release",
    )?;
    assert!(report.proposals.is_empty());
    let refusal = report
        .refusals
        .iter()
        .find(|refusal| refusal.package == "fixture-sys" && refusal.reason == HarvestRefusalReason::LinkedLibraries)
        .ok_or("owned link work must be refused")?;
    let observation = &refusal.observations.link[0];
    assert_eq!(observation.libraries, ["static=fixture"]);
    assert_eq!(observation.search_paths[0].owner_relative_path, ".");
    assert_eq!(observation.output_tree_digest, selected_graph_sha256(b"product-tree"));
    assert_eq!(observation.products[0].digest, selected_graph_sha256(b"archive"));
    Ok(())
}

#[test]
/// A refused probe (here one of another target) keeps its invocation identity and the OUT_DIR products beside it.
fn harvest_contract_preserves_tool_probe_and_product_identities() -> TestResult {
    let unit = library("isle-meta", "1.0.0", &[]);
    let build = build_script_of(&unit);
    let mut selected = capture(vec![(unit, Some(facts(&[], Some(retained_products()))))]);
    selected
        .build_script_tool_probes
        .push(OvenLegacyCargoBuildScriptToolProbe {
            package_id: build.package_id,
            out_dir: PathBuf::from("/transient/out"),
            target_context: "x86_64-unknown-linux-gnu".to_string(),
            rustc_target: "aarch64-apple-darwin".to_string(),
            digest: selected_graph_sha256(b"probe"),
            output: None,
        });
    let report = harvest_registry_units(&selected, &evidence(), "release")?;
    assert!(report.proposals.is_empty());
    let refusal = report
        .refusals
        .iter()
        .find(|refusal| refusal.package == "isle-meta" && refusal.reason == HarvestRefusalReason::ToolProbes)
        .ok_or("owned tool work must be refused")?;
    let observation = &refusal.observations.tool[0];
    assert_eq!(observation.probe_digest, selected_graph_sha256(b"probe"));
    assert_eq!(observation.products[0].digest, selected_graph_sha256(b"archive"));
    Ok(())
}

/// Retained output containing one generated input and one compiler-probe output.
fn probed_output() -> OvenLegacyCargoSelectedGeneratedOutput {
    OvenLegacyCargoSelectedGeneratedOutput {
        relative_root: "generated-outputs/probed".to_string(),
        digest: selected_graph_sha256(b"probed tree"),
        members: vec![
            OvenLegacyCargoInspectionSourceMember {
                path: "config.rs".to_string(),
                digest: selected_graph_sha256(b"pub const PRECISION: u64 = 100;"),
            },
            OvenLegacyCargoInspectionSourceMember {
                path: "rustix_test_can_compile".to_string(),
                digest: selected_graph_sha256(b"probe metadata"),
            },
        ],
    }
}

/// Build the paired host/target selection used to prove compiler-probe folding and refusal behavior.
fn probe_only_selection(
    unit: &OvenLegacyCargoSelectedUnit,
    host_answers: &[&str],
    host_probe_target: &str,
) -> OvenLegacyCargoSelectedUnitCapture {
    let probe = |out_dir: &str, target: &str, spelling: &[u8]| OvenLegacyCargoBuildScriptToolProbe {
        package_id: unit.package_id.clone(),
        out_dir: PathBuf::from(out_dir),
        target_context: "x86_64-unknown-linux-gnu".to_string(),
        rustc_target: target.to_string(),
        digest: selected_graph_sha256(spelling),
        output: Some(crate::rustc_trace::OvenLegacyStdinProbeOutput {
            relative_path: "rustix_test_can_compile".to_string(),
            digest: selected_graph_sha256(b"probe metadata"),
        }),
    };
    let mut target_facts = facts(&["static_assertions"], Some(probed_output()));
    target_facts.out_dir = PathBuf::from("/transient/target-out");
    let mut host_facts = facts(host_answers, Some(probed_output()));
    host_facts.out_dir = PathBuf::from("/transient/host-out");
    let mut selected = capture(vec![
        (unit.clone(), Some(target_facts)),
        (unit.clone(), Some(host_facts)),
    ]);
    // An earlier probe of the target unit wrote other bytes to the same path before the last one replaced
    // them, as rustix's feature probes do; the member must match one probe's bytes, not every probe's.
    let mut earlier = probe(
        "/transient/target-out",
        "x86_64-unknown-linux-gnu",
        b"earlier probe at opt-level 3",
    );
    if let Some(output) = earlier.output.as_mut() {
        output.digest = selected_graph_sha256(b"earlier probe metadata");
    }
    selected.build_script_tool_probes.extend([
        earlier,
        probe(
            "/transient/target-out",
            "x86_64-unknown-linux-gnu",
            b"probe at opt-level 3",
        ),
        probe("/transient/host-out", host_probe_target, b"probe at opt-level 0"),
    ]);
    selected
}

#[test]
/// A build script's compiler probes propose their answers as `cfg` with the probe digests as evidence. A probe's
/// own output left in OUT_DIR is neither `out` nor a product, while a file the script wrote stays `out`. Units
/// that asked differently but answered the same fold into one fact; different answers still conflict, and a
/// probe of another target stays tool work.
fn harvest_contract_proposes_probe_only_answers_as_cfg() -> TestResult {
    let unit = library("rustix", "1.1.5", &["std"]);

    let report = harvest_registry_units(
        &probe_only_selection(&unit, &["static_assertions"], "x86_64-unknown-linux-gnu"),
        &evidence(),
        "release",
    )?;
    assert!(report.refusals.iter().all(|refusal| refusal.package != "rustix"));
    let proposal = report
        .proposals
        .iter()
        .find(|proposal| proposal.project.name == "rustix")
        .ok_or("probe-only answers must be proposed")?;
    assert_eq!(proposal.rust.facts[0].cfg, ["static_assertions"]);
    assert!(proposal.rust.facts[0].tool.is_empty());
    assert_eq!(
        proposal.rust.facts[0]
            .out
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>(),
        ["config.rs"],
        "the probe's own output is not a generated input; the script's file is"
    );
    let mut probes = vec![
        selected_graph_sha256(b"earlier probe at opt-level 3"),
        selected_graph_sha256(b"probe at opt-level 3"),
        selected_graph_sha256(b"probe at opt-level 0"),
    ];
    probes.sort();
    assert_eq!(proposal.evidence.compiler_probes, probes);
    let written = serde_json::to_value(proposal)?;
    assert_eq!(written["evidence"]["compiler_probes"].as_array().map(Vec::len), Some(3));
    assert!(written["rust"]["facts"][0].get("tool").is_none());

    let disagreeing = harvest_registry_units(
        &probe_only_selection(&unit, &["other_answer"], "x86_64-unknown-linux-gnu"),
        &evidence(),
        "release",
    )?;
    assert!(
        disagreeing
            .refusals
            .iter()
            .any(|refusal| refusal.package == "rustix"
                && refusal.reason == HarvestRefusalReason::ConflictingObservations)
    );

    let foreign = harvest_registry_units(
        &probe_only_selection(&unit, &["static_assertions"], "aarch64-apple-darwin"),
        &evidence(),
        "release",
    )?;
    assert!(
        foreign
            .proposals
            .iter()
            .all(|proposal| proposal.project.name != "rustix")
    );
    assert!(foreign.refusals.iter().any(|refusal| refusal.package == "rustix"));

    // A file at a probe's output path holding other bytes was rewritten by the script: refuse, never choose.
    let mut rewritten = probe_only_selection(&unit, &["static_assertions"], "x86_64-unknown-linux-gnu");
    for probe in &mut rewritten.build_script_tool_probes {
        if let Some(output) = probe.output.as_mut() {
            output.digest = selected_graph_sha256(b"what the probe wrote before the script replaced it");
        }
    }
    let rewritten = harvest_registry_units(&rewritten, &evidence(), "release")?;
    assert!(
        rewritten
            .proposals
            .iter()
            .all(|proposal| proposal.project.name != "rustix")
    );
    assert!(rewritten.refusals.iter().any(|refusal| refusal.package == "rustix"
        && refusal.reason == HarvestRefusalReason::ConflictingObservations
        && refusal.detail.contains("rewritten by the script")));
    Ok(())
}

#[test]
fn harvest_contract_declares_out_dir_relative_environment() -> TestResult {
    let mut observed = facts(&[], Some(retained_products()));
    observed
        .environment
        .insert("FIXTURE_ARCHIVE".to_string(), "/transient/out/libfixture.a".to_string());
    let report = harvest_registry_units(
        &capture(vec![(library("fixture", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let proposal = report
        .proposals
        .iter()
        .find(|proposal| proposal.project.name == "fixture")
        .ok_or("retained environment input was not proposed")?;
    assert_eq!(
        proposal.rust.facts[0].environment,
        [RustFactCompileEnvironment {
            name: "FIXTURE_ARCHIVE".to_string(),
            literal: None,
            out: Some("libfixture.a".to_string()),
        }]
    );
    assert!(proposal.admitted_record().is_ok());
    Ok(())
}

#[test]
fn harvest_contract_declares_literal_environment_values() -> TestResult {
    let mut observed = facts(&[], Some(retained_products()));
    observed
        .environment
        .insert("SECRET".to_string(), "ambient-value".to_string());
    let report = harvest_registry_units(
        &capture(vec![(library("fixture", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let proposal = report.proposals.first().ok_or("literal environment was not proposed")?;
    assert_eq!(proposal.rust.facts[0].environment[0].name, "SECRET");
    assert_eq!(
        proposal.rust.facts[0].environment[0].literal.as_deref(),
        Some("ambient-value")
    );
    Ok(())
}

#[test]
fn harvest_contract_proves_binding_derived_environment_constants() -> TestResult {
    let mut observed = facts(&[], Some(retained_products()));
    observed.environment = BTreeMap::from([
        ("CFG_CARGO_FEATURES".to_string(), "[\"arch\", \"default\"]".to_string()),
        ("CFG_OPT_LEVEL".to_string(), "3".to_string()),
        ("CFG_TARGET_FEATURES".to_string(), "[\"neon\", \"sha2\"]".to_string()),
    ]);
    let mut selected = capture(vec![(library("libm", "0.2.16", &["default", "arch"]), Some(observed))]);
    let compiler = selected.compiler.as_mut().ok_or("fixture compiler context missing")?;
    compiler
        .target_cfg
        .values
        .insert("target_feature".to_string(), vec!["sha2,neon".to_string()]);

    let report = harvest_registry_units(&selected, &evidence(), "release")?;
    let proposal = report
        .proposals
        .iter()
        .find(|proposal| proposal.project.name == "libm")
        .ok_or("libm binding-derived constants must harvest")?;
    assert_eq!(proposal.rust.facts[0].environment.len(), 3);
    assert!(proposal.admitted_record().is_ok());
    assert!(
        serde_json::to_string(proposal)?.contains("CFG_TARGET_FEATURES"),
        "proved constants must remain identity-bearing compile environment"
    );
    Ok(())
}

#[test]
fn harvest_contract_refuses_a_binding_derived_constant_that_disagrees() -> TestResult {
    for (name, value) in [
        ("CFG_CARGO_FEATURES", "[\"wrong\"]"),
        ("CFG_OPT_LEVEL", "0"),
        ("CFG_TARGET_FEATURES", "[\"wrong\"]"),
    ] {
        let mut observed = facts(&[], Some(retained_products()));
        observed.environment.insert(name.to_string(), value.to_string());
        let report = harvest_registry_units(
            &capture(vec![(library("libm", "0.2.16", &["arch", "default"]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        assert!(report.proposals.is_empty());
        assert!(report.refusals.iter().any(|refusal| {
            refusal.reason == HarvestRefusalReason::BindingDerivedEnvironmentMismatch && refusal.detail == name
        }));
    }
    Ok(())
}

#[test]
fn harvest_contract_emits_explicit_empty_facts() -> TestResult {
    let report = harvest_registry_units(
        &capture(vec![(library("empty", "1.0.0", &[]), None)]),
        &evidence(),
        "release",
    )?;
    assert_eq!(report.proposals.len(), 1);
    assert_eq!(
        report.proposals[0].rust.facts[0].effect_classes(),
        [HarvestEffectClass::Empty]
    );
    Ok(())
}

#[test]
fn admitted_proposal_converts_cfg_out_and_refuses_raw_work() -> TestResult {
    let clean = harvest_registry_units(
        &capture(vec![(library("empty", "1.0.0", &[]), None)]),
        &evidence(),
        "release",
    )?;
    let admitted = clean.proposals[0].admitted_record()?;
    assert!(admitted.cfg.is_empty() && admitted.out.is_empty());
    assert_eq!(admitted.harvested_from, evidence().receipt);

    let mut linked = facts(&[], Some(retained_products()));
    linked.linked_libraries = vec!["static=fixture".to_string()];
    let raw = harvest_registry_units(
        &capture(vec![(library("fixture-sys", "1.0.0", &[]), Some(linked))]),
        &evidence(),
        "release",
    )?;
    let link = raw
        .refusals
        .iter()
        .find(|refusal| refusal.package == "fixture-sys")
        .ok_or("link refusal missing")?
        .observations
        .link
        .clone();
    let mut raw_proposal = clean.proposals[0].clone();
    raw_proposal.rust.facts[0].link_observations = link;
    assert_eq!(
        raw_proposal.admitted_record().err(),
        Some(HarvestAdmissionRefusal::UnresolvedLinkObservations)
    );

    let mut raw_proposal = clean.proposals[0].clone();
    raw_proposal.rust.facts[0].environment_inputs = vec![HarvestEnvironmentInput {
        name: "FIXTURE_ARCHIVE".to_string(),
        owner_relative_path: "libfixture.a".to_string(),
        digest: selected_graph_sha256(b"archive"),
    }];
    assert_eq!(
        raw_proposal.admitted_record().err(),
        Some(HarvestAdmissionRefusal::UnresolvedEnvironmentInputs)
    );
    Ok(())
}

/// Complete native work shaped like the bundled C archives emitted by blake3 and zstd-sys.
fn complete_link_observation(name: &str, source_path: &str) -> HarvestLinkObservation {
    let digest = selected_graph_sha256(name.as_bytes());
    HarvestLinkObservation {
        libraries: vec![format!("static={name}")],
        search_paths: vec![HarvestLinkSearchPath {
            kind: "native".to_string(),
            owner_relative_path: ".".to_string(),
        }],
        output_tree_digest: selected_graph_sha256(format!("{name}-products").as_bytes()),
        products: vec![HarvestObservedProduct {
            owner_relative_path: format!("lib{name}.a"),
            digest: selected_graph_sha256(format!("lib{name}.a").as_bytes()),
        }],
        name: Some(name.to_string()),
        executable: Some(RustFactExecutable {
            name: "clang".to_string(),
            owner: selected_graph_sha256(b"publisher-toolchain"),
            path: "bin/clang".to_string(),
            digest: selected_graph_sha256(b"clang"),
        }),
        objects: vec![RustFactLinkObject {
            name: format!("{name}.o"),
            language: oven_model::manifest::RustFactLinkLanguage::C,
            arguments: vec![
                RustFactArgument::Input {
                    input: "sources".to_string(),
                },
                RustFactArgument::Output {
                    output: format!("{name}.o"),
                },
            ],
        }],
        environment: Vec::new(),
        sources: vec![RustFactArtifact {
            name: "sources".to_string(),
            kind: RustFactArtifactKind::Tree,
            path: source_path.to_string(),
            digest,
            members: vec![oven_model::manifest::RustFactArtifactMember {
                path: "fixture.c".to_string(),
                digest: selected_graph_sha256(b"fixture source"),
            }],
        }],
        library: Some(RustFactLibrary {
            name: name.to_string(),
            kind: RustFactLibraryKind::Static,
        }),
        conversion_refusal: None,
    }
}

/// Native-trace fixture with its compiler owner, crate source, sysroot, and captured invocations.
type NativeTraceFixture = (
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    PathBuf,
    Vec<OvenLegacyNativeInvocation>,
);

/// Build one native-trace fixture with a compiler owner, crate source, OUT_DIR objects, and exact archive.
fn native_trace_fixture(sources: &[&str]) -> Result<NativeTraceFixture, Box<dyn std::error::Error>> {
    let root = tempdir()?;
    let owner = root.path().join("toolchain");
    let compiler = owner.join("usr/bin/clang");
    let resource = owner.join("usr/lib/clang/21");
    let sysroot = owner.join("SDKs/Fixture.sdk");
    let package = root.path().join("package");
    let out = root.path().join("out");
    fs::create_dir_all(compiler.parent().ok_or("compiler has no parent")?)?;
    fs::create_dir_all(&resource)?;
    fs::create_dir_all(&sysroot)?;
    fs::create_dir_all(package.join("include"))?;
    fs::create_dir_all(&out)?;
    fs::write(&compiler, b"fixture compiler")?;
    fs::write(package.join("include/fixture.h"), b"#define FIXTURE 1\n")?;
    let environment = BTreeMap::from([
        ("CARGO_MANIFEST_DIR".to_string(), package.to_string_lossy().into_owned()),
        ("CARGO_PKG_NAME".to_string(), "fixture-sys".to_string()),
        ("CARGO_PKG_VERSION".to_string(), "1.0.0".to_string()),
        ("OUT_DIR".to_string(), out.to_string_lossy().into_owned()),
        ("TARGET".to_string(), "aarch64-apple-darwin".to_string()),
    ]);
    let mut invocations = Vec::new();
    let mut members = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let source_path = package.join(source);
        fs::create_dir_all(source_path.parent().ok_or("source has no parent")?)?;
        fs::write(
            &source_path,
            format!("int fixture_{index}(void) {{ return {index}; }}\n"),
        )?;
        let object = out.join(format!("0123456789abcdef-{}.o", source.replace(['/', '.'], "_")));
        fs::write(&object, format!("object-{index}"))?;
        members.push(object.clone());
        invocations.push(OvenLegacyNativeInvocation {
            reason: "incan-native-compile-invocation".to_string(),
            executable: compiler.to_string_lossy().into_owned(),
            working_directory: package.to_string_lossy().into_owned(),
            arguments: vec![
                "-DTRACE_FIXTURE=1".to_string(),
                "-I".to_string(),
                package.join("include").to_string_lossy().into_owned(),
                "-isysroot".to_string(),
                sysroot.to_string_lossy().into_owned(),
                "-c".to_string(),
                source_path.to_string_lossy().into_owned(),
                "-o".to_string(),
                object.to_string_lossy().into_owned(),
            ],
            environment: environment.clone(),
            output: None,
        });
    }
    members.sort_by_key(|path| stable_object_name(path).unwrap_or_default());
    let archive = out.join("libfixture.a");
    fs::write(&archive, b"fixture archive")?;
    let mut archive_arguments = vec!["cq".to_string(), archive.to_string_lossy().into_owned()];
    archive_arguments.extend(members.iter().map(|path| path.to_string_lossy().into_owned()));
    invocations.push(OvenLegacyNativeInvocation {
        reason: "incan-native-archive-invocation".to_string(),
        executable: owner.join("usr/bin/ar").to_string_lossy().into_owned(),
        working_directory: out.to_string_lossy().into_owned(),
        arguments: archive_arguments,
        environment,
        output: None,
    });
    Ok((root, compiler, resource, sysroot, invocations))
}

#[test]
fn native_trace_converts_blake3_and_zstd_shaped_compiles() -> TestResult {
    for sources in [vec!["c/blake3_neon.c"], vec!["zstd/a.c", "zstd/b.c", "zstd/asm.S"]] {
        let (_root, compiler, resource, sysroot, invocations) = native_trace_fixture(&sources)?;
        let compilers = [NativeCompiler {
            executable: &compiler,
            resource_dir: &resource,
            sysroot: &sysroot,
        }];
        let conversion = native_link_work_from_observations(&invocations, &["static=fixture".to_string()], &compilers)?;
        let work = conversion.work.first().ok_or("native link work missing")?;
        assert_eq!(work.objects.len(), sources.len());
        assert!(work.inputs.iter().any(|input| input.kind == RustFactArtifactKind::Tree));
        assert!(work.objects.iter().all(|object| {
            object
                .arguments
                .iter()
                .any(|argument| matches!(argument, RustFactArgument::Owner { .. }))
        }));
        assert!(work.objects.iter().all(|object| {
            object
                .arguments
                .iter()
                .any(|argument| matches!(argument, RustFactArgument::Literal { literal } if literal == "-resource-dir"))
        }));
        let record = RustFactWorkRecord::try_from_observation(work.clone())?;
        assert!(matches!(record, RustFactWorkRecord::Link(_)));
        let mut observed = facts(&[], Some(retained_products()));
        observed.linked_libraries = vec!["static=fixture".to_string()];
        observed.linked_paths = vec!["native=/transient/out".to_string()];
        observed.publisher_work = vec![work.clone()];
        let report = harvest_registry_units(
            &capture(vec![(library("native-fixture", "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        let proposal = report
            .proposals
            .first()
            .ok_or("converted native binding was not proposed")?;
        assert_eq!(proposal.admitted_record()?.link[0].library.name, "fixture");
    }
    Ok(())
}

/// Convert lzma-style root and nested include directories into complete tree sources.
#[test]
fn native_trace_converts_crate_root_and_nested_include_trees() -> TestResult {
    let (_root, compiler, resource, sysroot, mut invocations) = native_trace_fixture(&["src/lzma.c"])?;
    let manifest_dir = PathBuf::from(
        invocations
            .first()
            .and_then(|invocation| invocation.environment.get("CARGO_MANIFEST_DIR"))
            .ok_or("CARGO_MANIFEST_DIR missing")?,
    );
    fs::write(manifest_dir.join("config.h"), b"#define HAVE_CONFIG_H 1\n")?;
    let compile = invocations.first_mut().ok_or("compile missing")?;
    compile
        .arguments
        .splice(0..0, ["-I".to_string(), manifest_dir.to_string_lossy().into_owned()]);
    let compilers = [NativeCompiler {
        executable: &compiler,
        resource_dir: &resource,
        sysroot: &sysroot,
    }];

    let conversion = native_link_work_from_observations(&invocations, &["static=fixture".to_string()], &compilers)?;
    let work = conversion.work.first().ok_or("native link work missing")?;

    assert!(work.inputs.iter().any(|input| {
        input.kind == RustFactArtifactKind::Tree
            && input.path == "."
            && input.members.iter().any(|member| member.path == "config.h")
    }));
    assert!(work.inputs.iter().any(|input| {
        input.kind == RustFactArtifactKind::Tree
            && input.path == "include"
            && input.members.iter().any(|member| member.path == "fixture.h")
    }));
    Ok(())
}

/// Keep cc-rs flag checks as probe evidence while replaying only the object admitted to the archive.
#[test]
fn native_trace_keeps_flag_probe_evidence_out_of_link_objects() -> TestResult {
    let (_root, compiler, resource, sysroot, mut invocations) = native_trace_fixture(&["c/blake3_neon.c"])?;
    let environment = invocations.first().ok_or("compile missing")?.environment.clone();
    let out = PathBuf::from(environment.get("OUT_DIR").ok_or("OUT_DIR missing")?);
    let probe_source = out.join("flag_check.c");
    let probe_output = out.join("flag_check.o");
    fs::write(&probe_source, b"int main(void) { return 0; }\n")?;
    fs::write(&probe_output, b"probe object")?;
    invocations.insert(
        0,
        OvenLegacyNativeInvocation {
            reason: "incan-native-compile-invocation".to_string(),
            executable: compiler.to_string_lossy().into_owned(),
            working_directory: out.to_string_lossy().into_owned(),
            arguments: vec![
                "-Werror".to_string(),
                "-c".to_string(),
                probe_source.to_string_lossy().into_owned(),
                "-o".to_string(),
                probe_output.to_string_lossy().into_owned(),
            ],
            environment,
            output: Some(super::super::native_trace::OvenLegacyNativeProbeOutput {
                relative_path: "flag_check.o".to_string(),
                digest: digest_bytes(b"probe object"),
            }),
        },
    );
    let compilers = [NativeCompiler {
        executable: &compiler,
        resource_dir: &resource,
        sysroot: &sysroot,
    }];

    let conversion = native_link_work_from_observations(&invocations, &["static=fixture".to_string()], &compilers)?;

    assert_eq!(conversion.work.first().ok_or("link work missing")?.objects.len(), 1);
    assert_eq!(conversion.probes.len(), 1);
    assert!(is_sha256_identity(&conversion.probes[0].digest));
    assert_eq!(
        conversion.probes[0]
            .output
            .as_ref()
            .map(|output| output.relative_path.as_str()),
        Some("flag_check.o")
    );
    let mut observed = facts(
        &[],
        Some(OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/native-probe".to_string(),
            digest: selected_graph_sha256(b"native probe tree"),
            members: vec![
                OvenLegacyCargoInspectionSourceMember {
                    path: "libfixture.a".to_string(),
                    digest: selected_graph_sha256(b"archive"),
                },
                OvenLegacyCargoInspectionSourceMember {
                    path: "flag_check.o".to_string(),
                    digest: digest_bytes(b"probe object"),
                },
            ],
        }),
    );
    observed.out_dir = out;
    observed.linked_libraries = vec!["static=fixture".to_string()];
    observed.linked_paths = vec![format!("native={}", observed.out_dir.display())];
    observed.publisher_work = conversion.work;
    observed.publisher_native_probes = conversion.probes.clone();
    let report = harvest_registry_units(
        &capture(vec![(library("native-probe", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let proposal = report.proposals.first().ok_or("native probe proposal missing")?;
    assert_eq!(proposal.evidence.compiler_probes, [conversion.probes[0].digest.clone()]);
    assert!(
        proposal
            .admitted_record()?
            .out
            .iter()
            .all(|output| output.name != "flag_check.o")
    );
    Ok(())
}

/// Reconstruct one archive from repeated `ar cq` chunks followed by the index-only `ar s` operation.
#[test]
fn native_trace_converts_chunked_archive_operations() -> TestResult {
    let (_root, compiler, resource, sysroot, mut invocations) =
        native_trace_fixture(&["zstd/a.c", "zstd/b.c", "zstd/c.c"])?;
    let archive = invocations.pop().ok_or("archive missing")?;
    let archive_path = archive.arguments.get(1).ok_or("archive path missing")?.clone();
    let members = archive.arguments[2..].to_vec();
    for chunk in members.chunks(2) {
        let mut invocation = archive.clone();
        invocation.arguments = vec!["cq".to_string(), archive_path.clone()];
        invocation.arguments.extend_from_slice(chunk);
        invocations.push(invocation);
    }
    let mut index = archive;
    index.arguments = vec!["s".to_string(), archive_path];
    invocations.push(index);
    let compilers = [NativeCompiler {
        executable: &compiler,
        resource_dir: &resource,
        sysroot: &sysroot,
    }];

    let conversion = native_link_work_from_observations(&invocations, &["static=fixture".to_string()], &compilers)?;

    assert_eq!(conversion.work.first().ok_or("link work missing")?.objects.len(), 3);
    Ok(())
}

/// Bind a C++ object to the declared C++ executable and language instead of the C driver.
#[test]
fn native_trace_names_the_declared_cxx_driver_for_cpp_objects() -> TestResult {
    let (_root, compiler, resource, sysroot, mut invocations) = native_trace_fixture(&["c/fixture.cpp"])?;
    let cxx = compiler.parent().ok_or("compiler has no parent")?.join("clang++");
    fs::write(&cxx, b"fixture C++ compiler")?;
    for invocation in invocations
        .iter_mut()
        .filter(|invocation| invocation.reason == "incan-native-compile-invocation")
    {
        invocation.executable = cxx.to_string_lossy().into_owned();
    }
    let compilers = [
        NativeCompiler {
            executable: &compiler,
            resource_dir: &resource,
            sysroot: &sysroot,
        },
        NativeCompiler {
            executable: &cxx,
            resource_dir: &resource,
            sysroot: &sysroot,
        },
    ];

    let conversion = native_link_work_from_observations(&invocations, &["static=fixture".to_string()], &compilers)?;
    let work = conversion.work.first().ok_or("C++ link work missing")?;

    assert_eq!(
        work.executable.as_ref().map(|executable| executable.name.as_str()),
        Some("clang++")
    );
    assert_eq!(
        work.objects.first().map(|object| &object.language),
        Some(&RustFactLinkLanguage::Cpp)
    );
    Ok(())
}

/// Refuse every mismatch between observed compiles, rebuilt archive membership, and emitted link libraries.
#[test]
fn native_trace_refuses_each_archive_membership_mismatch() -> TestResult {
    let (root, compiler, resource, sysroot, invocations) = native_trace_fixture(&["c/first.c", "c/second.c"])?;
    let linked = ["static=fixture".to_string()];
    let compilers = [NativeCompiler {
        executable: &compiler,
        resource_dir: &resource,
        sysroot: &sysroot,
    }];

    let mut unarchived = invocations.clone();
    unarchived.last_mut().ok_or("archive missing")?.arguments.pop();
    let reason = native_link_work_from_observations(&unarchived, &linked, &compilers)
        .err()
        .ok_or("compiled but unarchived object was accepted")?;
    assert!(reason.contains("was never archived"));

    let mut uncompiled = invocations.clone();
    let extra = root.path().join("extra.o");
    fs::write(&extra, b"extra")?;
    uncompiled
        .last_mut()
        .ok_or("archive missing")?
        .arguments
        .push(extra.to_string_lossy().into_owned());
    let reason = native_link_work_from_observations(&uncompiled, &linked, &compilers)
        .err()
        .ok_or("archived but uncompiled object was accepted")?;
    assert!(reason.contains("was never compiled"));

    let reason = native_link_work_from_observations(&invocations, &["static=different".to_string()], &compilers)
        .err()
        .ok_or("archive without matching link library was accepted")?;
    assert!(reason.contains("has no matching `static=<name>` link library"));
    Ok(())
}

#[test]
fn native_trace_refuses_absolute_generated_and_missing_archive_inputs() -> TestResult {
    let (root, compiler, resource, sysroot, invocations) = native_trace_fixture(&["c/fixture.c"])?;
    let linked = ["static=fixture".to_string()];
    let compilers = [NativeCompiler {
        executable: &compiler,
        resource_dir: &resource,
        sysroot: &sysroot,
    }];

    let mut absolute = invocations.clone();
    absolute[0]
        .arguments
        .insert(0, root.path().join("outside.h").to_string_lossy().into_owned());
    fs::write(root.path().join("outside.h"), b"outside")?;
    let reason = native_link_work_from_observations(&absolute, &linked, &compilers)
        .err()
        .ok_or("absolute non-owner input was accepted")?;
    assert!(reason.contains("outside the crate and compiler owner"));

    let mut generated = invocations.clone();
    let out = PathBuf::from(generated[0].environment.get("OUT_DIR").ok_or("OUT_DIR missing")?);
    let generated_source = out.join("generated.c");
    fs::write(&generated_source, b"generated")?;
    let source_index = generated[0]
        .arguments
        .iter()
        .position(|argument| argument == "-c")
        .ok_or("-c missing")?
        + 1;
    generated[0].arguments[source_index] = generated_source.to_string_lossy().into_owned();
    let generated_reason = native_link_work_from_observations(&generated, &linked, &compilers)
        .err()
        .ok_or("generated input was accepted")?;
    assert!(generated_reason.contains("generated inside OUT_DIR"));

    let mut observed = facts(&[], Some(retained_products()));
    observed.linked_libraries = linked.to_vec();
    observed.linked_paths = vec!["native=/transient/out".to_string()];
    observed.publisher_work_refusal = Some(generated_reason.clone());
    let report = harvest_registry_units(
        &capture(vec![(library("native-fixture", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let refusal = report
        .refusals
        .iter()
        .find(|refusal| refusal.package == "native-fixture")
        .ok_or("native conversion refusal was not retained")?;
    assert_eq!(refusal.detail, generated_reason);
    assert!(!refusal.detail.contains(root.path().to_string_lossy().as_ref()));

    let mut missing = invocations;
    missing.last_mut().ok_or("archive missing")?.arguments.pop();
    let reason = native_link_work_from_observations(&missing, &linked, &compilers)
        .err()
        .ok_or("missing archive member was accepted")?;
    assert!(
        reason.contains("exactly one archive")
            || reason.contains("archive invocation did not name")
            || reason.contains("archive members")
            || reason.contains("was never archived")
    );
    Ok(())
}

/// Complete generator work shaped like Cranelift's ISLE source-to-Rust generation.
fn complete_isle_observation() -> HarvestToolObservation {
    HarvestToolObservation {
        target_context: "x86_64-unknown-linux-gnu".to_string(),
        rustc_target: "x86_64-unknown-linux-gnu".to_string(),
        probe_digest: selected_graph_sha256(b"isle invocation"),
        executable_identity: selected_graph_sha256(b"isle executable"),
        output_tree_digest: selected_graph_sha256(b"isle products"),
        products: vec![HarvestObservedProduct {
            owner_relative_path: "generated/isle_opt.rs".to_string(),
            digest: selected_graph_sha256(b"generated isle source"),
        }],
        name: Some("isle".to_string()),
        executable: Some(RustFactExecutable {
            name: "isle".to_string(),
            owner: selected_graph_sha256(b"cranelift-isle provider"),
            path: "bin/isle".to_string(),
            digest: selected_graph_sha256(b"isle executable"),
        }),
        arguments: vec![
            RustFactArgument::Input {
                input: "isle-source".to_string(),
            },
            RustFactArgument::Output {
                output: "generated-rust".to_string(),
            },
        ],
        environment: Vec::new(),
        inputs: vec![RustFactArtifact {
            name: "isle-source".to_string(),
            kind: RustFactArtifactKind::File,
            path: "src/opts.isle".to_string(),
            digest: selected_graph_sha256(b"isle source"),
            members: Vec::new(),
        }],
        outputs: vec![RustFactOutput {
            name: "generated-rust".to_string(),
            kind: RustFactArtifactKind::File,
            path: "generated/isle_opt.rs".to_string(),
        }],
    }
}

#[test]
fn admitted_proposal_converts_blake3_and_zstd_shaped_link_observations() -> TestResult {
    for (name, source_path) in [("blake3", "c"), ("zstd", "zstd/lib")] {
        let complete = complete_link_observation(name, source_path);
        let link = link_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
        let mut observed = facts(&[], Some(retained_products()));
        observed.linked_libraries = vec![format!("static={name}")];
        observed.linked_paths = vec!["native=/transient/out".to_string()];
        observed.publisher_work = vec![RustFactWorkObservation::from(link)];
        let report = harvest_registry_units(
            &capture(vec![(library(name, "1.0.0", &[]), Some(observed))]),
            &evidence(),
            "release",
        )?;
        let proposal = report
            .proposals
            .first()
            .ok_or("complete link observation was not proposed")?;
        let admitted = proposal.admitted_record()?;
        assert_eq!(admitted.link.len(), 1);
        assert_eq!(admitted.link[0].library.name, name);
        assert_eq!(admitted.link[0].sources[0].path, source_path);
        assert!(proposal.rust.facts[0].link_observations.is_empty());
        assert_eq!(proposal.rust.facts[0].link.len(), 1);
        assert!(proposal.rust.facts[0].out.is_empty());
        let encoded = serde_json::to_value(proposal)?;
        assert!(encoded["rust"]["facts"][0]["link"][0]["sources"][0]["digest"].is_string());
        assert!(encoded["rust"]["facts"][0].get("link_observations").is_none());
    }
    Ok(())
}

#[test]
fn admitted_proposal_converts_cranelift_isle_shaped_tool_observation() -> TestResult {
    let complete = complete_isle_observation();
    let tool = tool_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
    let mut observed = facts(&[], Some(retained_isle_products()));
    observed.publisher_work = vec![RustFactWorkObservation::from(tool)];
    let report = harvest_registry_units(
        &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let proposal = report
        .proposals
        .first()
        .ok_or("complete ISLE observation was not proposed")?;
    let admitted = proposal.admitted_record()?;
    assert_eq!(admitted.tool.len(), 1);
    assert_eq!(admitted.tool[0].name, "isle");
    assert_eq!(admitted.tool[0].outputs[0].path, "generated/isle_opt.rs");
    assert!(proposal.rust.facts[0].tool_observations.is_empty());
    assert_eq!(proposal.rust.facts[0].tool.len(), 1);
    assert!(proposal.rust.facts[0].out.is_empty());
    let encoded = serde_json::to_value(proposal)?;
    assert!(encoded["rust"]["facts"][0]["tool"][0]["executable"]["digest"].is_string());
    assert!(
        encoded["rust"]["facts"][0]["tool"][0]["outputs"][0]
            .get("digest")
            .is_none()
    );
    assert!(encoded["rust"]["facts"][0].get("tool_observations").is_none());
    let output = tempdir()?;
    let retained = tempdir()?;
    let written = write_harvest_report(&report, output.path(), retained.path())?;
    assert!(written.iter().all(|path| !path.to_string_lossy().contains("/out/")));
    Ok(())
}

#[test]
fn complete_tool_output_is_not_also_retained_as_out() -> TestResult {
    let complete = complete_isle_observation();
    let tool = tool_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
    let mut observed = facts(&[], Some(retained_isle_products()));
    observed.publisher_work = vec![RustFactWorkObservation::from(tool)];
    let report = harvest_registry_units(
        &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), Some(observed))]),
        &evidence(),
        "release",
    )?;
    let fact = &report
        .proposals
        .first()
        .ok_or("complete ISLE observation was not proposed")?
        .rust
        .facts[0];
    assert!(fact.out.is_empty());
    assert_eq!(fact.tool[0].outputs[0].path, "generated/isle_opt.rs");
    Ok(())
}

#[test]
fn disagreeing_tool_output_and_out_bytes_are_conflicting_observations() -> TestResult {
    let clean = harvest_registry_units(
        &capture(vec![(library("cranelift-codegen", "1.0.0", &[]), None)]),
        &evidence(),
        "release",
    )?;
    let mut proposal = clean.proposals[0].clone();
    proposal.rust.facts[0].out = vec![RustFactOut {
        name: "generated/isle_opt.rs".to_string(),
        path: "out/generated/isle_opt.rs".to_string(),
        digest: selected_graph_sha256(b"generated isle source"),
    }];
    let mut observation = complete_isle_observation();
    observation.products[0].digest = selected_graph_sha256(b"different");
    proposal.rust.facts[0].tool_observations = vec![observation];
    let reason = proposal
        .admitted_record()
        .err()
        .ok_or("disagreeing tool/out bytes were admitted")?;
    assert_eq!(reason, HarvestAdmissionRefusal::ConflictingObservations);
    assert_eq!(
        admission_refusal(&proposal, reason).reason,
        HarvestRefusalReason::ConflictingObservations
    );
    Ok(())
}

#[test]
fn incomplete_work_refuses_independently_of_profile() -> TestResult {
    let complete = complete_link_observation("zstd", "zstd/lib");
    let link = link_record_from_observation(&complete, "x86_64-unknown-linux-gnu")?;
    let mut incomplete = RustFactWorkObservation::from(link);
    incomplete.executable = None;
    let mut observed = facts(&[], Some(retained_products()));
    observed.linked_libraries = vec!["static=zstd".to_string()];
    observed.linked_paths = vec!["native=/transient/out".to_string()];
    observed.publisher_work = vec![incomplete];
    let selected = capture(vec![(library("zstd-sys", "1.0.0", &[]), Some(observed))]);
    let release = harvest_registry_units(&selected, &evidence(), "release")?;
    let debug = harvest_registry_units(&selected, &evidence(), "debug")?;
    assert!(release.proposals.is_empty() && debug.proposals.is_empty());
    assert_eq!(release.refusals, debug.refusals);
    assert!(release.refusals.iter().any(|refusal| {
        refusal.package == "zstd-sys"
            && refusal.reason == HarvestRefusalReason::LinkedLibraries
            && !refusal.observations.link.is_empty()
    }));
    Ok(())
}

/// One fixture unit and, when present, the facts of the build-script edge feeding it.
type FixtureUnit = (OvenLegacyCargoSelectedUnit, Option<OvenLegacyCargoBuildScriptFacts>);

/// A capture of a transport root over `library` units; each unit with `Some(facts)` gets a build-script edge.
fn capture(units: Vec<FixtureUnit>) -> OvenLegacyCargoSelectedUnitCapture {
    let mut root = library("oven_release_stdlib", "0.1.0", &[]);
    root.package_id = "path+file:///fixture#oven_release_stdlib@0.1.0".to_string();
    root.package_source = None;
    root.registry_source = None;
    let mut all = vec![root];
    for (unit, facts) in units {
        let index = all.len();
        let mut unit = unit;
        if let Some(facts) = facts {
            let script = build_script_of(&unit);
            unit.dependencies.push(OvenLegacyCargoSelectedDependency {
                unit_index: index + 1,
                extern_crate_name: None,
                build_script: Some(facts),
            });
            all.push(unit);
            all.push(script);
        } else {
            all.push(unit);
        }
        let alias = all[index].package.replace('-', "_");
        all[0].dependencies.push(OvenLegacyCargoSelectedDependency {
            unit_index: index,
            extern_crate_name: Some(alias),
            build_script: None,
        });
    }
    OvenLegacyCargoSelectedUnitCapture {
        roots: vec![0],
        units: all,
        rustc_invocations_observed: true,
        build_script_tool_probes: Vec::new(),
        compiler: Some(OvenLegacyCargoSelectedCompilerContext {
            host: "x86_64-unknown-linux-gnu".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            toolchain: "rustc 1.98.0 (fixture)".to_string(),
            rustc_identity: "rustc 1.98.0 (fixture)".to_string(),
            host_cfg: cfg_snapshot(),
            target_cfg: cfg_snapshot(),
        }),
    }
}

fn reasons(report: &HarvestReport) -> Vec<(&str, HarvestRefusalReason)> {
    report
        .refusals
        .iter()
        .map(|refusal| (refusal.package.as_str(), refusal.reason))
        .collect()
}

#[test]
fn a_cfg_only_script_proposes_its_answers_and_nothing_else() -> TestResult {
    let capture = capture(vec![(
        library("libm", "0.2.16", &["default", "arch"]),
        Some(facts(
            &["optimizations_enabled", "arch_enabled", "arch_enabled"],
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/empty".to_string(),
                digest: selected_graph_sha256(b"empty"),
                members: Vec::new(),
            }),
        )),
    )]);
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    assert_eq!(report.proposals.len(), 1);
    let proposal = &report.proposals[0];
    assert_eq!(proposal.project.name, "libm");
    assert_eq!(proposal.source.registry, "https://github.com/rust-lang/crates.io-index");
    assert_eq!(proposal.source.checksum, CHECKSUM);
    let fact = &proposal.rust.facts[0];
    assert_eq!(fact.features, ["arch", "default"], "features are sorted and unique");
    assert_eq!(
        fact.cfg,
        ["arch_enabled", "optimizations_enabled"],
        "cfg is sorted and unique"
    );
    assert!(fact.out.is_empty());
    assert_eq!(fact.toolchain, "rustc 1.98.0 (fixture)");
    assert_eq!(fact.profile, "release");
    assert_eq!(proposal.evidence.method, HARVEST_EVIDENCE_METHOD);
    assert_eq!(proposal.evidence.receipt, evidence().receipt);
    assert_eq!(proposal.evidence.rustc_identity, evidence().rustc_identity);
    assert_eq!(proposal.evidence.host, "x86_64-unknown-linux-gnu");
    assert!(proposal.evidence.hazards.is_empty());
    assert!(proposal.notes.is_none());
    // The transport root is refused; the build-script node is evidence owned by the library binding.
    assert_eq!(
        reasons(&report),
        [("oven_release_stdlib", HarvestRefusalReason::NotRegistryBacked)]
    );
    Ok(())
}

#[test]
fn a_unit_without_a_build_script_proposes_an_empty_cfg_fact() -> TestResult {
    let mut bare = library("serde", "1.0.228", &["std"]);
    if let Some(source) = bare.registry_source.as_mut() {
        source.checksum = CHECKSUM.trim_start_matches("sha256:").to_string();
    }
    let bare_capture = capture(vec![(bare, None)]);
    let report = harvest_registry_units(&bare_capture, &evidence(), "debug")?;
    assert_eq!(report.proposals.len(), 1);
    let fact = &report.proposals[0].rust.facts[0];
    assert!(fact.cfg.is_empty() && fact.out.is_empty());
    assert_eq!(fact.profile, "debug");
    assert_eq!(
        report.proposals[0].source.checksum, CHECKSUM,
        "Cargo's bare lock checksum is proposed as the sha256 identity the registry binds"
    );
    let mut malformed = library("odd", "1.0.0", &[]);
    if let Some(source) = malformed.registry_source.as_mut() {
        source.checksum = "not-a-digest".to_string();
    }
    let report = harvest_registry_units(&capture(vec![(malformed, None)]), &evidence(), "debug")?;
    assert!(report.proposals.is_empty());
    assert_eq!(reasons(&report)[0], ("odd", HarvestRefusalReason::MalformedChecksum));
    Ok(())
}

#[test]
fn out_bearing_scripts_name_nested_members_exactly_as_retained() -> TestResult {
    let capture = capture(vec![(
        library("serde_core", "1.0.228", &["std"]),
        Some(facts(
            &[],
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/abc".to_string(),
                digest: selected_graph_sha256(b"tree"),
                members: vec![
                    OvenLegacyCargoInspectionSourceMember {
                        path: "private.rs".to_string(),
                        digest: selected_graph_sha256(b"private"),
                    },
                    OvenLegacyCargoInspectionSourceMember {
                        path: "nested/dir/generated.rs".to_string(),
                        digest: selected_graph_sha256(b"generated"),
                    },
                ],
            }),
        )),
    )]);
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    let fact = &report.proposals[0].rust.facts[0];
    assert_eq!(
        fact.out
            .iter()
            .map(|out| (out.name.as_str(), out.path.as_str()))
            .collect::<Vec<_>>(),
        [
            ("nested/dir/generated.rs", "out/nested/dir/generated.rs"),
            ("private.rs", "out/private.rs"),
        ],
        "out.name is the member path the reader compares, out.path is proposal-relative"
    );
    assert_eq!(
        report.proposals[0].out_relative_root.as_deref(),
        Some("generated-outputs/abc")
    );
    Ok(())
}

#[test]
fn scripts_outside_the_record_vocabulary_are_refused_by_reason() -> TestResult {
    let mut linked = facts(&[], Some(retained_products()));
    linked.linked_libraries = vec!["static=zstd".to_string()];
    let mut paths = facts(&[], Some(retained_products()));
    paths.linked_paths = vec!["/transient/out".to_string()];
    let mut environment = facts(&[], Some(retained_products()));
    environment
        .environment
        .insert("SECRET".to_string(), "ambient-value".to_string());
    let mut probed = facts(&[], Some(retained_products()));
    probed.out_dir = PathBuf::from("/transient/probed-out");
    let unretained = facts(&["answer"], None);
    let mut capture = capture(vec![
        (library("zstd-sys", "2.0.0", &[]), Some(linked)),
        (library("openssl-sys", "0.9.0", &[]), Some(paths)),
        (library("libm", "0.2.16", &[]), Some(environment)),
        (library("proc-macro2", "1.0.106", &[]), Some(probed)),
        (library("late", "1.0.0", &[]), Some(unretained)),
    ]);
    let probe_unit = capture
        .units
        .iter()
        .find(|unit| unit.package == "proc-macro2" && unit.mode == "run-custom-build")
        .ok_or("fixture must hold the proc-macro2 build script")?;
    capture
        .build_script_tool_probes
        .push(OvenLegacyCargoBuildScriptToolProbe {
            package_id: probe_unit.package_id.clone(),
            out_dir: PathBuf::from("/transient/probed-out"),
            target_context: "x86_64-unknown-linux-gnu".to_string(),
            rustc_target: "aarch64-apple-darwin".to_string(),
            digest: selected_graph_sha256(b"probe"),
            output: None,
        });
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    assert!(report.proposals.iter().any(|proposal| proposal.project.name == "libm"));
    let refused = report
        .refusals
        .iter()
        .filter(|refusal| refusal.reason != HarvestRefusalReason::BuildScriptUnit)
        .map(|refusal| (refusal.package.as_str(), refusal.reason, refusal.detail.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        refused,
        [
            (
                "late",
                HarvestRefusalReason::OutputNotRetained,
                "OUT_DIR was not inventoried"
            ),
            (
                "openssl-sys",
                HarvestRefusalReason::LinkedPaths,
                "1 link search path(s)"
            ),
            (
                "oven_release_stdlib",
                HarvestRefusalReason::NotRegistryBacked,
                "no package source"
            ),
            ("proc-macro2", HarvestRefusalReason::ToolProbes, "1 compiler probe(s)"),
            ("zstd-sys", HarvestRefusalReason::LinkedLibraries, "static=zstd"),
        ]
    );
    Ok(())
}

#[test]
fn harvest_contract_never_emits_one_package_as_both_proposal_and_refusal() -> TestResult {
    let mut linked = facts(&[], Some(retained_products()));
    linked.linked_libraries = vec!["static=mixed".to_string()];
    let report = harvest_registry_units(
        &capture(vec![
            (library("mixed", "1.0.0", &["clean"]), None),
            (library("mixed", "1.0.0", &["native"]), Some(linked)),
            (library("plain", "1.0.0", &[]), None),
        ]),
        &evidence(),
        "release",
    )?;
    let proposed = report
        .proposals
        .iter()
        .map(|proposal| proposal.project.name.as_str())
        .collect::<BTreeSet<_>>();
    let refused = report
        .refusals
        .iter()
        .map(|refusal| refusal.package.as_str())
        .collect::<BTreeSet<_>>();
    assert!(proposed.is_disjoint(&refused));
    assert!(proposed.contains("plain"));
    assert!(!proposed.contains("mixed"));
    assert!(refused.contains("mixed"));

    let output = tempdir()?;
    let retained = tempdir()?;
    let written = write_harvest_report(&report, output.path(), retained.path())?;
    assert!(written.contains(&output.path().join("plain-1.0.0-release/proposal.json")));
    assert!(
        !written
            .iter()
            .any(|path| path.to_string_lossy().contains("mixed-1.0.0-release/proposal.json"))
    );
    let refusals: Vec<HarvestRefusal> =
        serde_json::from_slice(&fs::read(output.path().join("refusals-release.json"))?)?;
    let refused_on_disk = refusals
        .iter()
        .map(|refusal| refusal.package.as_str())
        .collect::<BTreeSet<_>>();
    assert!(proposed.is_disjoint(&refused_on_disk));
    Ok(())
}

#[test]
fn a_unit_off_the_captured_target_and_a_missing_compiler_are_refused() -> TestResult {
    let mut host_only = library("host-only", "1.0.0", &[]);
    host_only.platform = Some("aarch64-apple-darwin".to_string());
    let mut capture = capture(vec![(host_only, None)]);
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    assert_eq!(reasons(&report)[0], ("host-only", HarvestRefusalReason::TargetMismatch));
    capture.compiler = None;
    assert!(harvest_registry_units(&capture, &evidence(), "release").is_err());
    assert!(harvest_registry_units(&capture, &evidence(), "bench").is_err());
    Ok(())
}

#[test]
fn proposals_are_ordered_by_package_and_version_and_equal_units_fold() -> TestResult {
    let capture = capture(vec![
        (library("zeta", "1.0.0", &[]), None),
        (library("alpha", "2.0.0", &["b", "a"]), None),
        (library("alpha", "1.0.0", &[]), None),
        (library("alpha", "2.0.0", &["a", "b"]), None),
    ]);
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    assert_eq!(
        proposal_directory_names(&report)?,
        ["alpha-1.0.0-release", "alpha-2.0.0-release", "zeta-1.0.0-release"],
        "one proposal per selection, sorted; the two alpha 2.0.0 units are one selection"
    );
    Ok(())
}

/// Cargo orders the unit graph differently between runs; the report, refusals included, must not.
#[test]
fn harvest_contract_is_byte_identical_after_reordered_capture() -> TestResult {
    let empty_output = || {
        Some(OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/empty".to_string(),
            digest: selected_graph_sha256(b"empty"),
            members: Vec::new(),
        })
    };
    let forward = capture(vec![
        (library("alpha", "1.0.0", &[]), Some(facts(&["one"], empty_output()))),
        (library("beta", "1.0.0", &[]), None),
        (library("gamma", "1.0.0", &[]), Some(facts(&["two"], empty_output()))),
    ]);
    let backward = capture(vec![
        (library("gamma", "1.0.0", &[]), Some(facts(&["two"], empty_output()))),
        (library("beta", "1.0.0", &[]), None),
        (library("alpha", "1.0.0", &[]), Some(facts(&["one"], empty_output()))),
    ]);
    let first = harvest_registry_units(&forward, &evidence(), "release")?;
    let second = harvest_registry_units(&backward, &evidence(), "release")?;
    assert_eq!(
        serde_json::to_vec(&first.proposals)?,
        serde_json::to_vec(&second.proposals)?
    );
    assert_eq!(
        serde_json::to_vec(&first.refusals)?,
        serde_json::to_vec(&second.refusals)?
    );
    assert!(
        first
            .refusals
            .iter()
            .all(|refusal| refusal.reason != HarvestRefusalReason::BuildScriptUnit)
    );
    Ok(())
}

#[test]
fn harvest_contract_folds_equal_and_refuses_conflicting_observations() -> TestResult {
    let empty_output = || {
        Some(OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/empty".to_string(),
            digest: selected_graph_sha256(b"empty"),
            members: Vec::new(),
        })
    };
    let mut one = facts(&["one"], empty_output());
    one.linked_libraries = vec!["static=one".to_string()];
    let mut two = facts(&["two"], empty_output());
    two.linked_libraries = vec!["static=two".to_string()];
    let disagreeing = capture(vec![
        (library("alpha", "1.0.0", &["x"]), Some(one)),
        (library("alpha", "1.0.0", &["x"]), Some(two)),
    ]);
    let report = harvest_registry_units(&disagreeing, &evidence(), "release")?;
    assert!(report.proposals.is_empty());
    let conflict = report
        .refusals
        .iter()
        .find(|refusal| refusal.reason == HarvestRefusalReason::ConflictingObservations)
        .ok_or("missing conflicting-observations refusal")?;
    assert_eq!(conflict.observations.link.len(), 2);

    let split = capture(vec![
        (library("syn", "2.0.0", &["full"]), None),
        (library("syn", "2.0.0", &["derive"]), None),
        (library("quote", "1.0.0", &[]), None),
    ]);
    let report = harvest_registry_units(&split, &evidence(), "release")?;
    let names = proposal_directory_names(&report)?;
    assert_eq!(names[0], "quote-1.0.0-release");
    assert!(names[1].starts_with("syn-2.0.0-release-") && names[2].starts_with("syn-2.0.0-release-"));
    assert_ne!(names[1], names[2]);
    assert_eq!(names[1].len(), "syn-2.0.0-release-".len() + 12);
    Ok(())
}

/// A proposal admitted into a registry checkout must be the record the reader adopts for the very capture it
/// came from, and that record must agree with the observation `check_observation` compares it to.
#[test]
fn an_admitted_proposal_is_adopted_by_the_reader_for_its_own_capture() -> TestResult {
    let retained = tempdir()?;
    fs::create_dir_all(retained.path().join("generated-outputs/abc/nested"))?;
    fs::write(
        retained.path().join("generated-outputs/abc/private.rs"),
        b"pub mod private {}\n",
    )?;
    fs::write(
        retained.path().join("generated-outputs/abc/nested/generated.rs"),
        b"pub mod generated {}\n",
    )?;
    let facts = facts(
        &["if_docsrs"],
        Some(OvenLegacyCargoSelectedGeneratedOutput {
            relative_root: "generated-outputs/abc".to_string(),
            digest: selected_graph_sha256(b"tree"),
            members: vec![
                OvenLegacyCargoInspectionSourceMember {
                    path: "private.rs".to_string(),
                    digest: digest_bytes(b"pub mod private {}\n"),
                },
                OvenLegacyCargoInspectionSourceMember {
                    path: "nested/generated.rs".to_string(),
                    digest: digest_bytes(b"pub mod generated {}\n"),
                },
            ],
        }),
    );
    let capture = capture(vec![(
        library("serde_core", "1.0.228", &["std", "alloc"]),
        Some(facts.clone()),
    )]);
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    let proposal = &report.proposals[0];

    // ---- Admit the proposal the way incan-pub renders it: loaf.toml beside its out/ files, one index line ----
    let registry_root = tempdir()?;
    let record_dir = registry_root.path().join("crates-io/serde_core/1.0.228");
    write_harvest_report(&report, registry_root.path().join("harvest").as_path(), retained.path())?;
    fs::create_dir_all(&record_dir)?;
    for out in &proposal.rust.facts[0].out {
        let source = registry_root
            .path()
            .join("harvest/serde_core-1.0.228-release")
            .join(&out.path);
        let destination = record_dir.join(&out.path);
        fs::create_dir_all(destination.parent().ok_or("out path has a parent")?)?;
        fs::copy(source, destination)?;
    }
    #[derive(Serialize)]
    struct Rendered<'a> {
        project: &'a HarvestProject,
        source: &'a HarvestSource,
        rust: RenderedRust,
    }
    #[derive(Serialize)]
    struct RenderedRust {
        facts: Vec<RustFactRecord>,
    }
    fs::write(
        record_dir.join("loaf.toml"),
        toml::to_string(&Rendered {
            project: &proposal.project,
            source: &proposal.source,
            rust: RenderedRust {
                facts: vec![proposal.admitted_record()?],
            },
        })?,
    )?;
    let index_dir = registry_root.path().join("index/se/rd");
    fs::create_dir_all(&index_dir)?;
    fs::write(
        index_dir.join("serde_core"),
        format!(
            "{{\"cksum\":\"{CHECKSUM}\",\"manifest\":\"crates-io/serde_core/1.0.228/loaf.toml\",\"name\":\"serde_core\",\"source\":\"crates-io\",\"vers\":\"1.0.228\"}}\n"
        ),
    )?;

    // ---- The reader adopts the library unit and its declaration agrees with the observation ----
    let registry = oven_model::loaf_registry::LoafRegistry::open(registry_root.path())?;
    let authority = super::super::LoafRegistryAuthority::resolve(&capture, &registry, "release")?;
    let adoption = authority.adoption(1).ok_or("the harvested unit must be adopted")?;
    assert_eq!(adoption.checksum, CHECKSUM);
    super::super::LoafRegistryAuthority::check_observation(adoption, Some(&facts), false)?;
    assert!(
        super::super::LoafRegistryAuthority::resolve(&capture, &registry, "debug")?.is_empty(),
        "the record binds the harvested profile only"
    );
    Ok(())
}

/// Every hazard token admission refuses on is recorded, never refused on, and spelled exactly.
#[test]
fn hazards_record_the_publisher_environment_without_refusing() -> TestResult {
    let mut nightly = capture(vec![(library("serde", "1.0.228", &[]), None)]);
    if let Some(compiler) = nightly.compiler.as_mut() {
        compiler.toolchain = "rustc 1.99.0-nightly (abcdef123 2026-03-24)".to_string();
        compiler.rustc_identity = compiler.toolchain.clone();
    }
    let mut hazardous = evidence();
    hazardous.ambient_hazards = vec![HARVEST_HAZARD_RUSTC_BOOTSTRAP.to_string()];
    hazardous.cargo_version = "cargo 1.99.0-nightly (123abc 2026-03-24)".to_string();
    let report = harvest_registry_units(&nightly, &hazardous, "release")?;
    assert_eq!(
        report.proposals.len(),
        1,
        "a hazard never refuses; the registry decides"
    );
    assert_eq!(report.hazards, report.proposals[0].evidence.hazards);
    assert_eq!(
        report.proposals[0].evidence.hazards,
        ["RUSTC_BOOTSTRAP", "nightly-rustc"],
        "sorted, unique, and spelled as admission expects"
    );
    // The publisher Cargo is nightly by design (`--unit-graph`); it compiles nothing, so it is provenance in
    // `cargo_version`, not a hazard.
    let mut stable_rustc_nightly_cargo = evidence();
    stable_rustc_nightly_cargo.cargo_version = "cargo 1.99.0-nightly (123abc 2026-03-24)".to_string();
    let capture = capture(vec![(library("serde", "1.0.228", &[]), None)]);
    let report = harvest_registry_units(&capture, &stable_rustc_nightly_cargo, "release")?;
    assert!(report.proposals[0].evidence.hazards.is_empty());
    assert!(report.proposals[0].evidence.cargo_version.contains("nightly"));
    assert_eq!(
        ambient_harvest_hazards().len(),
        usize::from(std::env::var_os("RUSTC_BOOTSTRAP").is_some())
    );
    Ok(())
}

/// `evidence.receipt` is written in the one spelling admission accepts or not at all; `notes` only when known.
#[test]
fn receipt_and_notes_are_written_only_in_admissible_shapes() -> TestResult {
    let capture = capture(vec![(library("serde", "1.0.228", &[]), None)]);
    let identity = HarvestPublisherIdentity::new(
        "receipt-without-a-digest",
        &format!("sha256:{}", "4".repeat(64)),
        Vec::new(),
        Some(harvest_notes_for_checkout("8d40e1d3e5c43139a11b406dd0ba6e092efac492")),
    );
    assert!(identity.receipt.is_none(), "another spelling is dropped, not rewritten");
    let mut inputs = evidence();
    inputs.receipt = identity.receipt;
    inputs.notes = identity.notes;
    let report = harvest_registry_units(&capture, &inputs, "release")?;
    let json = serde_json::to_value(&report.proposals[0])?;
    assert!(json["evidence"].get("receipt").is_none());
    assert_eq!(json["notes"], "harvested from the release capture at 8d40e1d");
    let kept = HarvestPublisherIdentity::new(&format!("sha256:{}", "1".repeat(64)), "x", Vec::new(), None);
    assert!(kept.receipt.is_some());
    let json = serde_json::to_value(&harvest_registry_units(&capture, &evidence(), "release")?.proposals[0])?;
    assert_eq!(json["evidence"]["receipt"], format!("sha256:{}", "1".repeat(64)));
    assert!(json.get("notes").is_none());
    Ok(())
}

/// A fact carries exactly the record vocabulary, and an `out` entry names a committed file safely and once.
#[test]
fn a_fact_carries_exactly_the_record_keys_and_safe_unique_outputs() -> TestResult {
    let capture = capture(vec![(
        library("serde_core", "1.0.228", &[]),
        Some(facts(
            &["answer"],
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/abc".to_string(),
                digest: selected_graph_sha256(b"tree"),
                members: vec![OvenLegacyCargoInspectionSourceMember {
                    path: "private.rs".to_string(),
                    digest: selected_graph_sha256(b"private"),
                }],
            }),
        )),
    )]);
    let json = serde_json::to_value(&harvest_registry_units(&capture, &evidence(), "release")?.proposals[0])?;
    let keys = json["rust"]["facts"][0]
        .as_object()
        .ok_or("fact must be an object")?
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        ["cfg", "features", "out", "profile", "target", "toolchain"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    );
    assert_eq!(json["rust"]["facts"][0]["out"][0]["path"], "out/private.rs");

    let escaping = capture_with_members(vec![OvenLegacyCargoInspectionSourceMember {
        path: "../escape.rs".to_string(),
        digest: selected_graph_sha256(b"x"),
    }]);
    let report = harvest_registry_units(&escaping, &evidence(), "release")?;
    assert!(report.proposals.is_empty());
    assert!(
        report
            .refusals
            .iter()
            .any(|refusal| refusal.reason == HarvestRefusalReason::MalformedOutput)
    );
    let repeated = capture_with_members(vec![
        OvenLegacyCargoInspectionSourceMember {
            path: "twice.rs".to_string(),
            digest: selected_graph_sha256(b"a"),
        },
        OvenLegacyCargoInspectionSourceMember {
            path: "twice.rs".to_string(),
            digest: selected_graph_sha256(b"b"),
        },
    ]);
    let report = harvest_registry_units(&repeated, &evidence(), "release")?;
    assert!(report.proposals.is_empty());
    assert!(
        report
            .refusals
            .iter()
            .any(|refusal| refusal.reason == HarvestRefusalReason::MalformedOutput && refusal.detail.contains("twice"))
    );
    Ok(())
}

/// A registry library whose script retained exactly `members`.
fn capture_with_members(members: Vec<OvenLegacyCargoInspectionSourceMember>) -> OvenLegacyCargoSelectedUnitCapture {
    capture(vec![(
        library("odd", "1.0.0", &[]),
        Some(facts(
            &[],
            Some(OvenLegacyCargoSelectedGeneratedOutput {
                relative_root: "generated-outputs/odd".to_string(),
                digest: selected_graph_sha256(b"odd"),
                members,
            }),
        )),
    )])
}

/// Materialize the retained members and capture used by the canonical report persistence test.
fn canonical_report_fixture(retained: &Path) -> Result<OvenLegacyCargoSelectedUnitCapture, std::io::Error> {
    let member_root = retained.join("generated-outputs/abc/nested");
    fs::create_dir_all(&member_root)?;
    fs::write(member_root.join("generated.rs"), b"pub mod generated {}\n")?;
    fs::write(
        retained.join("generated-outputs/abc/private.rs"),
        b"pub mod private {}\n",
    )?;
    Ok(capture(vec![
        (
            library("serde_core", "1.0.228", &["std"]),
            Some(facts(
                &["if_docsrs"],
                Some(OvenLegacyCargoSelectedGeneratedOutput {
                    relative_root: "generated-outputs/abc".to_string(),
                    digest: selected_graph_sha256(b"tree"),
                    members: vec![
                        OvenLegacyCargoInspectionSourceMember {
                            path: "private.rs".to_string(),
                            digest: digest_bytes(b"pub mod private {}\n"),
                        },
                        OvenLegacyCargoInspectionSourceMember {
                            path: "nested/generated.rs".to_string(),
                            digest: digest_bytes(b"pub mod generated {}\n"),
                        },
                    ],
                }),
            )),
        ),
        (library("quote", "1.0.0", &[]), None),
    ]))
}

#[test]
fn the_written_report_is_canonical_and_idempotent() -> TestResult {
    let retained = tempdir()?;
    let capture = canonical_report_fixture(retained.path())?;
    let report = harvest_registry_units(&capture, &evidence(), "release")?;
    let output = tempdir()?;
    let written = write_harvest_report(&report, output.path(), retained.path())?;
    assert_eq!(
        written,
        [
            output.path().join("quote-1.0.0-release/proposal.json"),
            output.path().join("serde_core-1.0.228-release/out/nested/generated.rs"),
            output.path().join("serde_core-1.0.228-release/out/private.rs"),
            output.path().join("serde_core-1.0.228-release/proposal.json"),
            output.path().join("refusals-release.json"),
        ]
    );
    let proposal_text = fs::read_to_string(output.path().join("serde_core-1.0.228-release/proposal.json"))?;
    let proposal: serde_json::Value = serde_json::from_str(&proposal_text)?;
    let keys = proposal
        .as_object()
        .ok_or("proposal must be an object")?
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        ["evidence", "project", "rust", "source"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
        "the proposal carries exactly the admitted top-level fields"
    );
    let evidence_position = proposal_text
        .find("\"evidence\"")
        .ok_or("proposal has no evidence field")?;
    let project_position = proposal_text
        .find("\"project\"")
        .ok_or("proposal has no project field")?;
    let rust_position = proposal_text.find("\"rust\"").ok_or("proposal has no rust field")?;
    let source_position = proposal_text.find("\"source\"").ok_or("proposal has no source field")?;
    assert!(
        evidence_position < project_position && project_position < rust_position && rust_position < source_position,
        "canonical proposal fields are sorted in the written bytes"
    );
    assert!(proposal_text.ends_with('\n'));
    assert_eq!(proposal["rust"]["facts"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        proposal["rust"]["facts"][0]["out"][0]["path"],
        "out/nested/generated.rs"
    );
    assert!(
        proposal["rust"]["facts"][0].get("harvested-from").is_none()
            && proposal["rust"]["facts"][0].get("link").is_none(),
        "the proposal never carries the keys admission fills or reserves"
    );
    assert_eq!(proposal["evidence"]["method"], HARVEST_EVIDENCE_METHOD);
    assert_eq!(proposal["evidence"]["hazards"], serde_json::json!([]));
    assert_eq!(proposal["evidence"]["host"], "x86_64-unknown-linux-gnu");
    let round_trip: HarvestProposal = serde_json::from_str(&proposal_text)?;
    assert_eq!(round_trip.rust, report.proposals[1].rust);
    assert_eq!(
        fs::read(output.path().join("serde_core-1.0.228-release/out/private.rs"))?,
        b"pub mod private {}\n"
    );
    let refusals: Vec<HarvestRefusal> =
        serde_json::from_slice(&fs::read(output.path().join("refusals-release.json"))?)?;
    assert_eq!(refusals, report.refusals);
    assert!(
        refusals.iter().all(|refusal| refusal.package != "serde_core"),
        "serde_core's build-script node is supporting evidence for its proposal, never a refusal of the package"
    );
    assert_eq!(
        serde_json::to_value(HarvestRefusalReason::BuildScriptUnit)?,
        "build-script-unit",
        "reasons are kebab-case on the wire"
    );

    // ---- Idempotence: the same report rewrites nothing and refuses a changed answer ----
    let again = write_harvest_report(&report, output.path(), retained.path())?;
    assert_eq!(again, written);
    // ---- The other profile's harvest shares the directory without contending for the refusal list ----
    let debug = harvest_registry_units(&capture, &evidence(), "debug")?;
    let debug_written = write_harvest_report(&debug, output.path(), retained.path())?;
    assert!(debug_written.contains(&output.path().join("serde_core-1.0.228-debug/proposal.json")));
    assert!(debug_written.contains(&output.path().join("refusals-debug.json")));
    // ---- Provenance moves with every commit; the binding does not, and that is what idempotence is over ----
    let mut later = report.clone();
    later.proposals[0].notes = Some("harvested from the release capture at 1234567".to_string());
    later.proposals[0].evidence.receipt = Some(format!("sha256:{}", "9".repeat(64)));
    let rewritten = write_harvest_report(&later, output.path(), retained.path())?;
    assert_eq!(rewritten, written);
    assert_eq!(
        fs::read_to_string(output.path().join("quote-1.0.0-release/proposal.json"))?,
        fs::read_to_string(output.path().join("quote-1.0.0-release/proposal.json"))?,
        "the earlier observation stays as written"
    );
    // The changed answer also retains one more member; the refusal must come before that member is copied, so
    // the directory stays exactly what the earlier harvest wrote.
    fs::write(
        retained.path().join("generated-outputs/abc/extra.rs"),
        b"pub mod extra {}\n",
    )?;
    let mut changed = report.clone();
    changed.proposals[1].rust.facts[0].cfg.push("new_answer".to_string());
    changed.proposals[1].rust.facts[0].out.push(RustFactOut {
        name: "extra.rs".to_string(),
        path: "out/extra.rs".to_string(),
        digest: digest_bytes(b"pub mod extra {}\n"),
    });
    let refused = write_harvest_report(&changed, output.path(), retained.path());
    assert!(
        refused
            .as_ref()
            .err()
            .is_some_and(|error| error.to_string().contains("binds different facts")),
        "a changed answer for the same directory is refused: {:?}",
        refused.as_ref().err().map(ToString::to_string)
    );
    assert!(
        !output.path().join("serde_core-1.0.228-release/out/extra.rs").exists(),
        "a refused rewrite copies no member the earlier proposal does not name"
    );

    // ---- A retained member that no longer matches its digest is refused before anything is copied ----
    fs::write(retained.path().join("generated-outputs/abc/private.rs"), b"tampered\n")?;
    let fresh = tempdir()?;
    assert!(write_harvest_report(&report, fresh.path(), retained.path()).is_err());
    Ok(())
}
