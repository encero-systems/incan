//! Stage-zero SDK closure compilation from a digest-verified Loaf resolution seed.
//!
//! The seed supplies features and evaluated predicates; this boundary neither resolves again nor reads Cargo metadata.

use crate::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_proc_macro,
    rustc_host_target, rustc_identity,
};
use oven_store::source_archive::VerifiedLoafArchive;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
    OvenStoreLimits,
};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type Error = Box<dyn std::error::Error>;
const INDEX_COMMIT: &str = "452504b51712b6a3ac12a2e1b79ba69fff55e6b6";

struct CompileContext<'a> {
    rustc: &'a Path,
    target: &'a str,
    toolchain: &'a str,
    output: &'a Path,
    store: &'a OvenStore,
    compiler_digest: String,
}

/// Exact stage-zero package binding authorized by the SDK seed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SdkLockedUnit {
    /// Registry-qualified adopted Loaf name.
    pub loaf: String,
    /// Exact adopted version.
    pub version: String,
    /// Digest of uncompressed source archive bytes.
    pub archive_digest: String,
    /// Host or target compilation domain.
    pub domain: String,
    /// Complete enabled feature set.
    pub features: Vec<String>,
    /// Already evaluated dependency conditions.
    pub target_predicates: Vec<SdkLockedPredicate>,
}

/// One evaluated dependency target condition from the seed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SdkLockedPredicate {
    /// Declaration position in the adopted dependency list.
    pub declaration: usize,
    /// Authored predicate text.
    pub target: String,
    /// Evaluation for this binding.
    pub matches: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    schema: String,
    units: Vec<SdkLockedUnit>,
}

/// Measured compile outcomes, including refused units and their dependent units.
#[derive(Default, Serialize)]
pub struct SdkClosureReport {
    /// Units compiled during this invocation.
    pub compiled: Vec<String>,
    /// Units reused through the executor's content-checked output receipts.
    pub reused: Vec<String>,
    /// Build-script bindings without admitted debug facts.
    pub refused: Vec<String>,
    /// Units that could not compile, with their diagnostic.
    pub failed: Vec<String>,
    /// Wall time for the entire closure attempt.
    pub seconds: f64,
}

struct PreparedUnit {
    binding: SdkLockedUnit,
    manifest: toml::Value,
    root: PathBuf,
    build_script: bool,
    fact: Option<oven_model::manifest::RustFactRecord>,
}

/// Compile all independent units of the locked SDK seed, retaining named refusal evidence.
///
/// Build-script units require exact debug facts from the pinned index. Missing facts are refused rather than
/// approximated, and independent branches continue. Matching cfg, generated-file and environment facts are applied;
/// link/tool facts require retained publisher assets and are refused at execution when those assets are absent.
pub fn prepare_sdk_seed(
    lock: &Path,
    blobs: &Path,
    output: &Path,
    rustc: &Path,
    index: &Path,
) -> Result<SdkCompiledClosure, Error> {
    let started = std::time::Instant::now();
    let seed: Seed = serde_json::from_slice(&std::fs::read(lock)?)?;
    if seed.schema != "incan.oven.loaf-resolution/1" {
        return Err("unsupported SDK seed schema".into());
    }
    std::fs::create_dir_all(output)?;
    let target = rustc_host_target(rustc)?;
    let toolchain = rustc_identity(rustc)?;
    let scratch = tempfile::Builder::new().prefix("sdk-source-").tempdir_in(output)?;
    let units = prepare_units(seed.units, blobs, scratch.path(), index, &target, &toolchain)?;
    let store = OvenStore::new(
        output.join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let context = CompileContext {
        rustc,
        target: &target,
        toolchain: &toolchain,
        output,
        store: &store,
        compiler_digest: compiler_closure_digest(rustc)?,
    };
    let mut selected_units = Vec::new();
    let mut report = SdkClosureReport::default();
    let mut pending: BTreeSet<usize> = (0..units.len()).collect();
    let mut artifacts = BTreeMap::new();
    let mut closures: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    while !pending.is_empty() {
        let mut progressed = false;
        for index in pending.clone() {
            let unit = &units[index];
            if unit.build_script && unit.fact.is_none() {
                report.refused.push(binding_label(&unit.binding));
                pending.remove(&index);
                progressed = true;
                continue;
            }
            let edges = active_edges(unit, &units)?;
            if edges.iter().any(|(_, dependency)| pending.contains(dependency)) {
                continue;
            }
            pending.remove(&index);
            progressed = true;
            if edges.iter().any(|(_, dependency)| !artifacts.contains_key(dependency)) {
                report.failed.push(format!(
                    "{}: dependency was refused or failed",
                    binding_label(&unit.binding)
                ));
                continue;
            }
            let externs = edges
                .iter()
                .filter_map(|(name, dependency)| {
                    artifacts
                        .get(dependency)
                        .map(|path: &PathBuf| (name.clone(), path.clone()))
                })
                .collect();
            let mut closure = BTreeSet::new();
            for (_, dependency) in &edges {
                closure.insert(*dependency);
                if let Some(transitive) = closures.get(dependency) {
                    closure.extend(transitive);
                }
            }
            let searches = closure
                .iter()
                .filter_map(|index| {
                    artifacts
                        .get(index)
                        .and_then(|path| path.parent())
                        .map(Path::to_path_buf)
                })
                .collect();
            match compile_unit(unit, &context, externs, searches) {
                Ok((path, reused, owner)) => {
                    selected_units.push(SdkCompiledUnit {
                        binding: unit.binding.clone(),
                        output: path.clone(),
                        owner,
                    });
                    artifacts.insert(index, path);
                    closures.insert(index, closure);
                    if reused {
                        report.reused.push(binding_label(&unit.binding));
                    } else {
                        report.compiled.push(binding_label(&unit.binding));
                    }
                }
                Err(error) => report.failed.push(format!("{}: {error}", binding_label(&unit.binding))),
            }
        }
        if !progressed {
            return Err("cycle in locked SDK closure".into());
        }
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(SdkCompiledClosure {
        report,
        units: selected_units,
    })
}

/// Verify archives before parsing their adopted manifests and create fresh source-only roots.
fn prepare_units(
    bindings: Vec<SdkLockedUnit>,
    blobs: &Path,
    scratch: &Path,
    index: &Path,
    target: &str,
    toolchain: &str,
) -> Result<Vec<PreparedUnit>, Error> {
    bindings
        .into_iter()
        .enumerate()
        .map(|(ordinal, binding)| {
            if !matches!(binding.domain.as_str(), "host" | "target") {
                return Err("invalid SDK unit domain".into());
            }
            let hex = binding
                .archive_digest
                .strip_prefix("sha256:")
                .ok_or("invalid archive digest")?;
            if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("invalid archive digest".into());
            }
            let archive = VerifiedLoafArchive::read(&blobs.join(format!("{hex}.tar")), &binding.archive_digest)?;
            let manifest: toml::Value = toml::from_str(std::str::from_utf8(archive.manifest()?)?)?;
            if manifest["project"]["name"].as_str() != Some(&binding.loaf)
                || manifest["project"]["version"].as_str() != Some(&binding.version)
            {
                return Err("archive manifest differs from locked package".into());
            }
            let root = scratch.join(ordinal.to_string());
            archive.materialize(&root)?;
            let fact = index_fact(index, &binding, target, toolchain)?;
            Ok(PreparedUnit {
                binding,
                manifest,
                root,
                build_script: archive.has_build_script(),
                fact,
            })
        })
        .collect()
}

/// Render every exact binding dimension required for a missing-fact refusal.
fn binding_label(unit: &SdkLockedUnit) -> String {
    format!(
        "({}, {}, debug, {:?}, {})",
        unit.loaf, unit.version, unit.features, unit.domain
    )
}

/// Select active dependency declarations using the seed's features, predicates and host partition.
fn active_edges(unit: &PreparedUnit, units: &[PreparedUnit]) -> Result<Vec<(String, usize)>, Error> {
    let mut enabled: BTreeSet<String> = unit.binding.features.iter().cloned().collect();
    let mut optional = BTreeSet::new();
    loop {
        let before = enabled.len();
        for feature in enabled.clone() {
            if let Some(values) = unit
                .manifest
                .get("project")
                .and_then(|value| value.get("features"))
                .and_then(|value| value.get(&feature))
                .and_then(toml::Value::as_array)
            {
                for value in values.iter().filter_map(toml::Value::as_str) {
                    if let Some(name) = value.strip_prefix("dep:") {
                        optional.insert(name.to_string());
                    } else if let Some((name, _)) = value.split_once('/') {
                        if !name.ends_with('?') {
                            optional.insert(name.to_string());
                        }
                    } else {
                        enabled.insert(value.to_string());
                        optional.insert(value.to_string());
                    }
                }
            }
        }
        if enabled.len() == before {
            break;
        }
    }
    let mut edges = Vec::new();
    let Some(dependencies) = unit.manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(edges);
    };
    for (name, values) in dependencies {
        let declarations = values
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(values));
        for declaration in declarations {
            if declaration.get("optional").and_then(toml::Value::as_bool) == Some(true)
                && !optional.contains(name)
                && !enabled.contains(name)
            {
                continue;
            }
            if let Some(predicate) = declaration.get("target").and_then(toml::Value::as_str) {
                let evaluation = unit
                    .binding
                    .target_predicates
                    .iter()
                    .find(|entry| entry.target == predicate)
                    .ok_or("dependency predicate absent from seed")?;
                if !evaluation.matches {
                    continue;
                }
            }
            let loaf = declaration
                .get("loaf")
                .and_then(toml::Value::as_str)
                .ok_or("dependency has no Loaf identity")?;
            let requirement = semver::VersionReq::parse(
                declaration
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .ok_or("dependency has no version")?,
            )?;
            let candidates: Vec<_> = units
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    let proc_macro = candidate
                        .manifest
                        .get("rust")
                        .and_then(|rust| rust.get("type"))
                        .and_then(toml::Value::as_str)
                        == Some("proc-macro");
                    let domain = if proc_macro { "host" } else { &unit.binding.domain };
                    candidate.binding.loaf == loaf
                        && candidate.binding.domain == domain
                        && semver::Version::parse(&candidate.binding.version)
                            .is_ok_and(|version| requirement.matches(&version))
                })
                .collect();
            if candidates.len() != 1 {
                return Err(format!(
                    "{} dependency {name}: expected one locked unit, found {}",
                    unit.binding.loaf,
                    candidates.len()
                )
                .into());
            }
            let (index, _) = candidates[0];
            edges.push((name.replace('-', "_"), index));
        }
    }
    Ok(edges)
}

/// Compile one admitted script-free unit through the existing receipt-bound library executor.
fn compile_unit(
    unit: &PreparedUnit,
    context: &CompileContext<'_>,
    externs: Vec<(String, PathBuf)>,
    searches: Vec<PathBuf>,
) -> Result<(PathBuf, bool, OvenStoreExecutionPayload), Error> {
    let rustc = context.rustc;
    let target = context.target;
    let toolchain = context.toolchain;
    let output = context.output;
    let store = context.store;
    let facet = unit.manifest.get("rust").ok_or("missing Rust facet")?;
    let name = facet
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or("missing Rust name")?;
    let edition = facet
        .get("edition")
        .and_then(toml::Value::as_str)
        .ok_or("missing Rust edition")?;
    let relative = facet
        .get("source")
        .and_then(|value| value.get("root"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    if Path::new(relative)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("invalid Rust source root".into());
    }
    let source = unit.root.join(relative);
    let mut receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &unit.root,
            &unit.binding.loaf,
            &unit.binding.version,
            target,
            toolchain,
            "debug",
            unit.binding.features.clone(),
        )
        .with_generated_source("sdk-root", &source)
        .with_build_unit_input("sdk-source-archive", &unit.binding.archive_digest)
        .with_build_unit_input("domain", &unit.binding.domain)
        .with_build_unit_input("sdk-compile-policy", "source-sealed-v1")
        .with_build_unit_input("compiler-binary", &context.compiler_digest),
    )?;
    if let Some(fact) = &unit.fact {
        receipt = oven_store::receipt_with_build_unit_input(&receipt, "sdk-build-fact", serde_json::to_string(fact)?)?;
    }
    let (plan, artifacts) = unit_plan(unit, &receipt.intent, externs, searches)?;
    for (name, digest) in &plan.caller_owned_library_digests {
        receipt = oven_store::receipt_with_build_unit_input(&receipt, format!("extern:{name}"), digest)?;
    }
    let key = receipt.identity.clone();
    let directory = output.join(key.replace(':', "-"));
    std::fs::create_dir_all(&directory)?;
    let proc_macro = facet.get("type").and_then(toml::Value::as_str) == Some("proc-macro");
    let path = directory.join(if proc_macro {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.rlib")
    });
    let relative = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid output name")?
        .to_string();
    let domain = format!("sdk-source-unit-{}", unit.binding.domain);
    let mut selected = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::Engine
            && manifest.domain == domain
            && manifest.receipt_identity == receipt.identity
    })?;
    if let Some(owner) = selected.pop() {
        let _verified = store.select(&owner.manifest.identity)?;
        return Ok((owner.artifact_root.join(&relative), true, owner));
    }
    let request = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: &unit.root,
        artifact_plan: Some(&plan),
        rustc,
        source: &source,
        output: &path,
        crate_name: name,
        edition,
        source_evidence_key: "sdk-root",
        prefer_dynamic: false,
        features: &unit.binding.features,
    };
    let result = if proc_macro {
        bake_trusted_direct_rustc_proc_macro(&request)?
    } else {
        bake_trusted_direct_rustc_library(&request)?
    };
    let mut files = source_materializations(&unit.root, &unit.root)?;
    files.push(OvenArtifactMaterializedFile {
        source_path: result.output,
        relative_path: relative.clone(),
    });
    let entry = store.publish(&OvenArtifactPublishRequest {
        receipt,
        domain,
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&unit.binding)?,
        materialized_files: files,
        materialized_directories: Vec::new(),
    })?;
    let owner = store
        .select_payloads_for_execution(&[entry.identity])?
        .pop()
        .ok_or("published SDK unit was not selected")?;
    Ok((owner.artifact_root.join(relative), result.reused, owner))
}

/// Read one file from the pinned index commit without consulting its mutable worktree.
fn index_file(index: &Path, relative: &str) -> Result<Vec<u8>, Error> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(index)
        .args(["show", &format!("{INDEX_COMMIT}:{relative}")])
        .output()?;
    if !output.status.success() {
        return Err(format!("pinned index file unavailable: {relative}").into());
    }
    Ok(output.stdout)
}

/// Select a fact only after verifying the pinned index archive association and all four binding dimensions.
fn index_fact(
    index: &Path,
    binding: &SdkLockedUnit,
    target: &str,
    toolchain: &str,
) -> Result<Option<oven_model::manifest::RustFactRecord>, Error> {
    let bytes = index_file(index, &format!("index/{}", binding.loaf))?;
    let lines = std::str::from_utf8(&bytes)?;
    let mut entries = Vec::new();
    for line in lines.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("vers").and_then(serde_json::Value::as_str) == Some(&binding.version) {
            entries.push(value);
        }
    }
    if entries.len() != 1 {
        return Err("pinned index version is absent or ambiguous".into());
    }
    let entry = &entries[0];
    if entry.get("cksum").and_then(serde_json::Value::as_str) != Some(&binding.archive_digest) {
        return Err("pinned index disagrees with locked archive digest".into());
    }
    let relative = entry
        .get("manifest")
        .and_then(serde_json::Value::as_str)
        .ok_or("pinned index manifest is absent")?;
    let manifest: toml::Value = toml::from_str(std::str::from_utf8(&index_file(index, relative)?)?)?;
    let mut selected = Vec::new();
    if let Some(facts) = manifest
        .get("rust")
        .and_then(|rust| rust.get("facts"))
        .and_then(toml::Value::as_array)
    {
        for value in facts {
            let fact: oven_model::manifest::RustFactRecord = value.clone().try_into()?;
            let mut features = fact.features.clone();
            features.sort();
            let mut enabled = binding.features.clone();
            enabled.sort();
            if fact.toolchain == toolchain && fact.target == target && fact.profile == "debug" && features == enabled {
                selected.push(fact);
            }
        }
    }
    if selected.len() > 1 {
        return Err("ambiguous exact build facts".into());
    }
    Ok(selected.pop())
}

/// Apply selected compile environment facts without executing a build script or importing an ambient output root.
fn apply_fact(unit: &PreparedUnit, plan: &mut OvenRustcArtifactPlan) -> Result<(), Error> {
    let Some(fact) = &unit.fact else {
        return Ok(());
    };
    if !fact.link.is_empty() || !fact.tool.is_empty() {
        return Err(
            "exact fact requires retained publisher link/tool assets; none were supplied to the SDK seed compiler"
                .into(),
        );
    }
    let out = unit.root.join(".oven-out");
    if !fact.out.is_empty() {
        std::fs::create_dir(&out)?;
    }
    for member in &fact.out {
        for path in [&member.name, &member.path] {
            if Path::new(path)
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("invalid generated fact path".into());
            }
        }
        let bytes = std::fs::read(unit.root.join(&member.path))?;
        if digest_bytes(&bytes) != member.digest {
            return Err("generated fact digest mismatch".into());
        }
        let destination = out.join(&member.name);
        std::fs::create_dir_all(destination.parent().ok_or("generated fact has no parent")?)?;
        std::fs::write(destination, bytes)?;
    }
    if !fact.out.is_empty() {
        plan.compile_environment
            .insert("OUT_DIR".to_string(), out.to_string_lossy().into_owned());
    }
    for environment in &fact.environment {
        let value = match (&environment.literal, &environment.out) {
            (Some(literal), None) => literal.clone(),
            (None, Some(relative)) if relative == "." => out.to_string_lossy().into_owned(),
            (None, Some(relative))
                if Path::new(relative)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))) =>
            {
                out.join(relative).to_string_lossy().into_owned()
            }
            _ => return Err("invalid fact environment".into()),
        };
        plan.compile_environment.insert(environment.name.clone(), value);
    }
    Ok(())
}

/// Construct the exact active dependency plan and package environment for one adopted unit.
fn unit_plan(
    unit: &PreparedUnit,
    intent: &oven_store::OvenBuildIntent,
    externs: Vec<(String, PathBuf)>,
    searches: Vec<PathBuf>,
) -> Result<(OvenRustcArtifactPlan, OvenRustcArtifactManifest), Error> {
    let version = semver::Version::parse(&unit.binding.version)?;
    let environment = BTreeMap::from([
        (
            "CARGO_PKG_NAME".to_string(),
            unit.binding.loaf.trim_start_matches("crates-io/").to_string(),
        ),
        ("CARGO_PKG_VERSION".to_string(), unit.binding.version.clone()),
        ("CARGO_PKG_VERSION_MAJOR".to_string(), version.major.to_string()),
        ("CARGO_PKG_VERSION_MINOR".to_string(), version.minor.to_string()),
        ("CARGO_PKG_VERSION_PATCH".to_string(), version.patch.to_string()),
    ]);
    let mut plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs,
        compile_environment: environment.clone(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    for (name, path) in &plan.externs {
        plan.caller_owned_library_digests
            .insert(name.clone(), digest_bytes(&std::fs::read(path)?));
        if let Some(parent) = path.parent() {
            plan.dependency_search_paths.push(parent.to_path_buf());
        }
    }
    plan.dependency_search_paths.extend(searches);
    apply_fact(unit, &mut plan)?;
    let artifacts = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_externs: BTreeMap::new(),
        entrypoint_dependency_search_paths: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: environment,
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    Ok((plan, artifacts))
}

/// Measure one seed compile, dropping the retained execution leases after reporting.
pub fn compile_sdk_seed(
    lock: &Path,
    blobs: &Path,
    output: &Path,
    rustc: &Path,
    index: &Path,
) -> Result<SdkClosureReport, Error> {
    Ok(prepare_sdk_seed(lock, blobs, output, rustc, index)?.report)
}

/// Collect source files already admitted from an archive, rejecting later links and special files.
fn source_materializations(root: &Path, current: &Path) -> Result<Vec<OvenArtifactMaterializedFile>, Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files.extend(source_materializations(root, &path)?);
        } else if kind.is_file() {
            files.push(OvenArtifactMaterializedFile {
                source_path: path.clone(),
                relative_path: format!("source/{}", path.strip_prefix(root)?.to_string_lossy()),
            });
        } else {
            return Err("admitted source tree contains a link or special file".into());
        }
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

/// Independently compiled units and leases retained for SDK component linking and inspection.
pub struct SdkCompiledClosure {
    /// Measurements and binding-specific refusal evidence.
    report: SdkClosureReport,
    /// Verified source and output selections, retained under execution leases.
    units: Vec<SdkCompiledUnit>,
}

/// One admitted archive and compiled output held in the immutable Oven store.
pub struct SdkCompiledUnit {
    /// Complete selected source binding.
    binding: SdkLockedUnit,
    /// Store-owned library or procedural-macro output path.
    output: PathBuf,
    owner: OvenStoreExecutionPayload,
}

impl SdkCompiledUnit {
    /// Borrow the immutable selected binding.
    pub fn binding(&self) -> &SdkLockedUnit {
        &self.binding
    }
    /// Borrow the library artifact while its store lease is held.
    pub fn output(&self) -> &Path {
        &self.output
    }
    /// Return the source-only immutable root retained by this unit's execution lease.
    pub fn source_root(&self) -> PathBuf {
        self.owner.artifact_root.join("source")
    }
}

impl SdkCompiledClosure {
    /// Borrow the compiler measurement without weakening publication readiness.
    pub fn report(&self) -> &SdkClosureReport {
        &self.report
    }
    /// Borrow all retained units without releasing or retargeting their leases.
    pub fn units(&self) -> &[SdkCompiledUnit] {
        &self.units
    }
    /// Refuse publication of inspection or SDK inventory authority while any locked unit is unavailable.
    pub fn require_complete(&self) -> Result<(), Error> {
        if self.report.refused.is_empty() && self.report.failed.is_empty() {
            return Ok(());
        }
        Err(format!(
            "SDK closure cannot be sealed: {} missing exact facts; {} unavailable dependents or compile failures\n{}",
            self.report.refused.len(),
            self.report.failed.len(),
            self.report.refused.join("\n")
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a script-free seed unit for graph-selection regression tests.
    fn unit(loaf: &str, domain: &str, manifest: &str, features: &[&str]) -> Result<PreparedUnit, Error> {
        Ok(PreparedUnit {
            binding: SdkLockedUnit {
                loaf: loaf.to_string(),
                version: "1.0.0".to_string(),
                archive_digest: digest_bytes(b"source"),
                domain: domain.to_string(),
                features: features.iter().map(|feature| (*feature).to_string()).collect(),
                target_predicates: Vec::new(),
            },
            manifest: toml::from_str(manifest)?,
            root: PathBuf::new(),
            build_script: false,
            fact: None,
        })
    }

    /// Optional aliases are activated by feature expansion, while proc macros select only host units.
    #[test]
    fn active_graph_preserves_aliases_and_host_partition() -> Result<(), Error> {
        let root = unit(
            "crates-io/root",
            "target",
            "[project.features]\nderive=['dep:macro-alias']\n[dependencies]\nmacro-alias={loaf='crates-io/macros',version='1',optional=true}\nunused={loaf='crates-io/absent',version='1',optional=true}\n",
            &["derive"],
        )?;
        let host = unit("crates-io/macros", "host", "[rust]\ntype='proc-macro'\n", &[])?;
        let target = unit("crates-io/macros", "target", "[rust]\ntype='proc-macro'\n", &[])?;
        let units = vec![root, host, target];
        assert_eq!(active_edges(&units[0], &units)?, vec![("macro_alias".to_string(), 1)]);
        Ok(())
    }

    /// A predicate absent from the seed is an error; a false evaluated edge never requires its package.
    #[test]
    fn target_edges_require_locked_evaluations() -> Result<(), Error> {
        let mut root = unit(
            "crates-io/root",
            "target",
            "[dependencies]\nforeign={loaf='crates-io/absent',version='1',target='cfg(windows)'}\n",
            &[],
        )?;
        assert!(active_edges(&root, &[]).is_err());
        root.binding.target_predicates.push(SdkLockedPredicate {
            declaration: 0,
            target: "cfg(windows)".to_string(),
            matches: false,
        });
        assert!(active_edges(&root, &[])?.is_empty());
        Ok(())
    }

    /// Partial closures cannot publish complete SDK source authority even when independent units compiled.
    #[test]
    fn missing_facts_prevent_complete_sealing() {
        let mut report = SdkClosureReport::default();
        report
            .refused
            .push("(crates-io/missing, 1.0.0, debug, [], target)".to_string());
        let closure = SdkCompiledClosure {
            report,
            units: Vec::new(),
        };
        assert!(closure.require_complete().is_err());
    }
}

/// Hash the SDK's pinned compiler libraries, preserving internal symlink aliases as logical content records.
///
/// Installed Apple toolchains include a `libLLVM.dylib` alias. Its target must remain inside the selected sysroot;
/// hashing its resolved bytes under the alias coordinate covers what the loader reads without accepting an ambient
/// library. Rust package manifests and source trees are not compiler inputs and are not traversed here.
fn compiler_closure_digest(rustc: &Path) -> Result<String, Error> {
    let rustc = std::fs::canonicalize(rustc)?;
    let root = rustc.parent().and_then(Path::parent).ok_or("compiler has no sysroot")?;
    let mut records = BTreeMap::from([("bin/rustc".to_string(), digest_bytes(&std::fs::read(&rustc)?))]);
    compiler_library_records(root, &root.join("lib"), &mut BTreeSet::new(), &mut records)?;
    Ok(digest_bytes(&serde_json::to_vec(&records)?))
}

/// Collect compiler library bytes while rejecting external symlinks, cycles and special files.
fn compiler_library_records(
    root: &Path,
    current: &Path,
    active: &mut BTreeSet<PathBuf>,
    records: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    let resolved = std::fs::canonicalize(current)?;
    if !resolved.starts_with(root) {
        return Err("compiler library alias escapes the selected sysroot".into());
    }
    let metadata = std::fs::metadata(&resolved)?;
    if metadata.is_dir() {
        if !active.insert(resolved.clone()) {
            return Err("cycle in compiler library aliases".into());
        }
        for entry in std::fs::read_dir(&resolved)? {
            let name = entry?.file_name();
            if matches!(name.to_str(), Some("src" | "Cargo.toml" | "Cargo.lock")) {
                continue;
            }
            compiler_library_records(root, &current.join(name), active, records)?;
        }
        active.remove(&resolved);
    } else if metadata.is_file() {
        records.insert(
            current.strip_prefix(root)?.to_string_lossy().into_owned(),
            digest_bytes(&std::fs::read(resolved)?),
        );
    } else {
        return Err("compiler closure contains a special file".into());
    }
    Ok(())
}
