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
const INDEX_COMMIT: &str = "6ec35e0d7e2d202496e2f7a108bb5111b6a9ff87";

mod environment;
mod local;
mod native;
mod physical_edges;

pub(crate) use local::{local_native_source_selection, native_required_features};

pub use physical_edges::SdkPhysicalNativeEdge;

pub use local::{
    LocalFacetSelection, compile_local_sdk_facet, compile_local_sdk_facet_for_target, compile_local_sdk_facets,
    local_sdk_facet_source_digest, local_sdk_facet_source_digest_with, local_sdk_facet_source_inputs,
};

struct CompileContext<'a> {
    rustc: &'a Path,
    target: &'a str,
    toolchain: &'a str,
    output: &'a Path,
    store: &'a OvenStore,
    compiler_digest: String,
    profile: &'a str,
    unit_codegen: &'a [UnitCodegenSelection],
}

/// Explicit authority and physical inputs for compiling an adopted closure without Cargo.
pub struct ClosureCompileRequest<'a> {
    /// Explicit requested roots; dependencies never receive CARGO_PRIMARY_PACKAGE.
    pub primary: &'a [SdkLockedUnit],
    /// Resolution document produced by the Incan resolver.
    pub lock: &'a Path,
    /// Digest-addressed admitted archive directory.
    pub blobs: &'a Path,
    /// Private store and staging directory.
    pub output: &'a Path,
    /// Selected compiler executable.
    pub rustc: &'a Path,
    /// Index repository; its worktree is never consulted.
    pub index: &'a Path,
    /// Exact index commit used for every file read.
    pub index_commit: &'a str,
    /// Selected target triple.
    pub target: &'a str,
    /// Debug or release compilation policy.
    pub profile: &'a str,
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
    /// Exact active dependency bindings; absent only in legacy resolution documents or local facets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<SdkLockedEdge>>,
}

impl SdkLockedUnit {
    /// The binding a compiled unit is named and stored under: everything except the lock's dependency edges.
    ///
    /// An edge only records which locked unit a dependency resolved to, and that unit's own identity already enters
    /// this unit's build through its extern artifact. Keeping edges out means one unit identity names one stored
    /// payload and archive whether the lock that selected it carried edges (schema 2) or not (schema 1).
    pub fn identity_binding(&self) -> SdkLockedUnit {
        SdkLockedUnit {
            edges: None,
            ..self.clone()
        }
    }
}

/// One resolver-owned dependency alias and its exact locked destination.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SdkLockedEdge {
    /// Authored dependency alias, before Rust identifier normalization.
    pub dependency_key: String,
    /// Registry-qualified destination.
    pub loaf: String,
    /// Exact selected source version.
    pub version: String,
    /// Host or target compilation domain.
    pub domain: String,
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
    #[serde(default)]
    unit_codegen: Vec<UnitCodegenSelection>,
}

/// Exact selector and typed options projected by Incan; this adapter does not infer a profile or resolve a unit.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitCodegenSelection {
    /// Registry-qualified Loaf supplied by the declaration.
    loaf: String,
    /// Exact resolved version supplied by the declaration.
    version: String,
    /// Host or target domain supplied by the declaration.
    domain: String,
    /// Explicit compiler arguments, retained in the selected unit receipt.
    options: crate::rustc::OvenRustcCodegenOptions,
}

/// Measured compile outcomes, including refused units and their dependent units.
#[derive(Default, Serialize)]
pub struct SdkClosureReport {
    /// Units compiled during this invocation.
    pub compiled: Vec<String>,
    /// Units reused through the executor's content-checked output receipts.
    pub reused: Vec<String>,
    /// Build-script bindings without facts for the selected profile.
    pub refused: Vec<String>,
    /// Units that could not compile, with their diagnostic.
    pub failed: Vec<String>,
    /// Wall time for the entire closure attempt.
    pub seconds: f64,
}

struct PreparedUnit {
    binding: SdkLockedUnit,
    source_origin: crate::native_loaf::NativeLoafOrigin,
    about: serde_json::Value,
    primary: bool,
    /// Lock handle prevents concurrent macros or fact materialization from changing this source during compilation.
    _source_lease: Option<std::fs::File>,
    manifest: toml::Value,
    root: PathBuf,
    build_script: bool,
    fact: Option<oven_model::manifest::RustFactRecord>,
    /// The selected fact's generated files, read and digest-checked from the version's index record directory.
    fact_out: Vec<FactOutFile>,
}

/// One generated file a build fact declares, with the bytes its index record directory holds.
struct FactOutFile {
    name: String,
    bytes: Vec<u8>,
}

/// Compile all independent units of the locked SDK seed, retaining named refusal evidence.
///
/// Build-script units require exact debug facts from the pinned index. Missing facts are refused rather than
/// approximated, and independent branches continue. Matching cfg, generated-file and environment facts are applied;
/// native-link records use the admitted archive and a digest-matched supplied executable owner. Tool records require
/// retained publisher assets and are refused when those assets are absent.
pub fn prepare_sdk_seed(
    lock: &Path,
    blobs: &Path,
    output: &Path,
    rustc: &Path,
    index: &Path,
) -> Result<SdkCompiledClosure, Error> {
    let target = rustc_host_target(rustc)?;
    prepare_closure(&ClosureCompileRequest {
        primary: &[],
        lock,
        blobs,
        output,
        rustc,
        index,
        index_commit: INDEX_COMMIT,
        target: &target,
        profile: "debug",
    })
}

/// Compile a resolved closure using one pinned index and profile through the SDK executor.
pub fn prepare_closure(request: &ClosureCompileRequest<'_>) -> Result<SdkCompiledClosure, Error> {
    if !matches!(request.profile, "debug" | "release") {
        return Err("closure profile must be debug or release".into());
    }
    validate_index_commit(request.index_commit)?;
    let ClosureCompileRequest {
        lock,
        output,
        rustc,
        target,
        profile,
        ..
    } = *request;
    let started = std::time::Instant::now();
    let seed: Seed = serde_json::from_slice(&std::fs::read(lock)?)?;
    if !matches!(
        seed.schema.as_str(),
        "incan.oven.loaf-resolution/1" | "incan.oven.loaf-resolution/2"
    ) {
        return Err("unsupported SDK seed schema".into());
    }
    if seed.schema == "incan.oven.loaf-resolution/2" && seed.units.iter().any(|unit| unit.edges.is_none()) {
        return Err("resolution schema 2 requires dependency edges on every unit; re-resolve the lock".into());
    }
    if !seed.unit_codegen.is_empty() && seed.schema != "incan.oven.loaf-resolution/2" {
        return Err("SDK codegen declarations require resolution schema 2".into());
    }
    let mut declared = BTreeSet::new();
    for selection in &seed.unit_codegen {
        selection.options.validate()?;
        if !declared.insert((&selection.loaf, &selection.version, &selection.domain))
            || seed
                .units
                .iter()
                .filter(|unit| {
                    unit.loaf == selection.loaf && unit.version == selection.version && unit.domain == selection.domain
                })
                .count()
                != 1
        {
            return Err("codegen selection must uniquely name one resolved SDK unit".into());
        }
    }
    std::fs::create_dir_all(output)?;
    let toolchain = rustc_identity(rustc)?;
    let scratch = tempfile::Builder::new().prefix("sdk-source-").tempdir_in(output)?;
    let units = prepare_units(seed.units, scratch.path(), request, &toolchain)?;
    let store = OvenStore::new(
        output.join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let context = CompileContext {
        rustc,
        target,
        toolchain: &toolchain,
        output,
        store: &store,
        compiler_digest: compiler_closure_digest(rustc, target)?,
        profile,
        unit_codegen: &seed.unit_codegen,
    };
    let mut closure = compile_units(&units, &context)?;
    closure.report.seconds = started.elapsed().as_secs_f64();
    Ok(closure)
}

/// Compile independent branches in dependency order, retaining every unavailable binding in the report.
fn compile_units(units: &[PreparedUnit], context: &CompileContext<'_>) -> Result<SdkCompiledClosure, Error> {
    let mut selected_units: Vec<SdkCompiledUnit> = Vec::new();
    let mut report = SdkClosureReport::default();
    let mut pending: BTreeSet<usize> = (0..units.len()).collect();
    let mut artifacts = BTreeMap::new();
    let mut inspection_indices = BTreeMap::new();
    let mut closures: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    while !pending.is_empty() {
        let mut progressed = false;
        for index in pending.clone() {
            let unit = &units[index];
            if unit.build_script && unit.fact.is_none() {
                report.refused.push(binding_label(&unit.binding, context.profile));
                pending.remove(&index);
                progressed = true;
                continue;
            }
            let edges = active_edges(unit, units)?;
            if edges.iter().any(|(_, dependency)| pending.contains(dependency)) {
                continue;
            }
            pending.remove(&index);
            progressed = true;
            if edges.iter().any(|(_, dependency)| !artifacts.contains_key(dependency)) {
                report.failed.push(format!(
                    "{}: dependency was refused or failed",
                    binding_label(&unit.binding, context.profile)
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
            let selected_dependencies = edges
                .iter()
                .map(|(name, dependency)| {
                    inspection_indices
                        .get(dependency)
                        .and_then(|index| selected_units.get(*index))
                        .map(|selected| (name.as_str(), selected))
                        .ok_or("compiled dependency has no producer selection")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let physical_bindings = selected_native_bindings(&selected_dependencies)?;
            match compile_unit(unit, context, externs, searches, &physical_bindings) {
                Ok((path, reused, owner, reproduced_receipt)) => {
                    let physical_edges = physical_edges::capture(&owner, &reproduced_receipt, &selected_dependencies)?;
                    let dependencies = edges
                        .iter()
                        .map(|(name, dependency)| {
                            inspection_indices
                                .get(dependency)
                                .map(|index| serde_json::json!({"crate": index, "name": name}))
                                .ok_or("compiled dependency has no inspection index")
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let inspection = inspection_unit(unit, &owner.artifact_root.join("source"), dependencies)?;
                    inspection_indices.insert(index, selected_units.len());
                    selected_units.push(SdkCompiledUnit {
                        binding: unit.binding.clone(),
                        output: path.clone(),
                        owner,
                        inspection,
                        physical_edges,
                        reproduced_receipt,
                    });
                    artifacts.insert(index, path);
                    closures.insert(index, closure);
                    if reused {
                        report.reused.push(binding_label(&unit.binding, context.profile));
                    } else {
                        report.compiled.push(binding_label(&unit.binding, context.profile));
                    }
                }
                Err(error) => report
                    .failed
                    .push(format!("{}: {error}", binding_label(&unit.binding, context.profile))),
            }
        }
        if !progressed {
            return Err("cycle in locked SDK closure".into());
        }
    }
    Ok(SdkCompiledClosure {
        report,
        units: selected_units,
        auxiliary_targets: BTreeMap::new(),
    })
}

/// Freeze source inspection against the same selected facet, facts and active dependency indices as compilation.
fn inspection_unit(
    unit: &PreparedUnit,
    source: &Path,
    dependencies: Vec<serde_json::Value>,
) -> Result<serde_json::Value, Error> {
    let facet = unit.manifest.get("rust").ok_or("compiled unit has no Rust facet")?;
    let root = facet
        .get("source")
        .and_then(|source| source.get("root"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    let mut environment = environment::package_environment(unit)?;
    // Macro queries inspect the retained source owner, not the publisher's temporary/stable staging projection.
    environment.insert("CARGO_MANIFEST_DIR".to_string(), source.to_string_lossy().into_owned());
    let mut cfg = unit
        .binding
        .features
        .iter()
        .map(|feature| format!("feature=\"{feature}\""))
        .collect::<Vec<_>>();
    if let Some(fact) = &unit.fact {
        cfg.extend(fact.cfg.iter().cloned());
        let out = source.join(".oven-out");
        if !fact.out.is_empty() {
            environment.insert("OUT_DIR".to_string(), out.to_string_lossy().into_owned());
        }
        for entry in &fact.environment {
            let value = match (&entry.literal, &entry.out) {
                (Some(value), None) => value.clone(),
                (None, Some(relative)) => out.join(relative).to_string_lossy().into_owned(),
                _ => return Err("compiled fact has invalid environment".into()),
            };
            environment.insert(entry.name.clone(), value);
        }
    }
    cfg.sort();
    cfg.dedup();
    Ok(serde_json::json!({
        "display_name": facet.get("name").and_then(toml::Value::as_str).ok_or("compiled unit has no Rust name")?,
        "root_module": source.join(root),
        "edition": facet.get("edition").and_then(toml::Value::as_str).ok_or("compiled unit has no Rust edition")?,
        "deps": dependencies, "cfg": cfg, "env": environment,
        "is_workspace_member": false,
        "is_proc_macro": facet.get("type").and_then(toml::Value::as_str) == Some("proc-macro"),
    }))
}

/// Verify archives before parsing their adopted manifests and create fresh source-only roots.
fn prepare_units(
    bindings: Vec<SdkLockedUnit>,
    scratch: &Path,
    request: &ClosureCompileRequest<'_>,
    toolchain: &str,
) -> Result<Vec<PreparedUnit>, Error> {
    let ClosureCompileRequest {
        blobs,
        index,
        index_commit,
        target,
        profile,
        ..
    } = *request;
    let about = environment::adopted_about(index, index_commit, &bindings)?;
    // A host-domain unit's build script ran for the machine that compiles it, so its fact is keyed by the compiler's
    // host triple even when the closure cross-compiles its target-domain units.
    let host = rustc_host_target(request.rustc)?;
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
            let fact_target = if binding.domain == "host" {
                host.as_str()
            } else {
                target
            };
            let fact = index_fact(index, index_commit, &binding, fact_target, toolchain, profile)?;
            let fact_out = match &fact {
                Some(fact) => fact_out_files(index, index_commit, &binding, fact)?,
                None => Vec::new(),
            };
            let metadata = about
                .get(&binding.archive_digest)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let primary = request.primary.iter().any(|root| {
                root.loaf == binding.loaf && root.version == binding.version && root.domain == binding.domain
            });
            let (root, lease) = environment::stable_sources(&root, &binding, &metadata, primary, fact.as_ref())?;
            Ok(PreparedUnit {
                source_origin: crate::native_loaf::NativeLoafOrigin::Registry,
                about: metadata,
                primary,
                _source_lease: Some(lease),
                build_script: declares_build_script(&manifest) || archive.has_build_script(),
                binding,
                manifest,
                root,
                fact,
                fact_out,
            })
        })
        .collect()
}

/// Render every exact binding dimension required for a missing-fact refusal.
fn binding_label(unit: &SdkLockedUnit, profile: &str) -> String {
    format!(
        "({}, {}, {profile}, {:?}, {})",
        unit.loaf, unit.version, unit.features, unit.domain
    )
}

/// Follow resolver-owned edges, or uniquely match legacy declarations using locked features and predicates.
fn active_edges(unit: &PreparedUnit, units: &[PreparedUnit]) -> Result<Vec<(String, usize)>, Error> {
    if let Some(edges) = &unit.binding.edges {
        return locked_edges(unit, edges, units);
    }
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
                    "{} dependency {name}: legacy lock expected one locked unit, found {}; re-resolve the lock",
                    unit.binding.loaf,
                    candidates.len()
                )
                .into());
            }
            let (index, _) = candidates[0];
            edges.push((extern_alias(name, &units[index]), index));
        }
    }
    Ok(edges)
}

/// The crate name a dependent uses for one dependency, as rustc's `--extern` must spell it.
///
/// A dependency key that only repeats the package name is not a rename, so the dependent sees the dependency's
/// library name (its `[rust] name`, e.g. `debug_unreachable` for the `new_debug_unreachable` package). A key that
/// differs from the package name is a rename and is used as written. Hyphens become underscores either way.
fn extern_alias(dependency_key: &str, target: &PreparedUnit) -> String {
    let package = target.binding.loaf.rsplit('/').next().unwrap_or(&target.binding.loaf);
    let library = target
        .manifest
        .get("rust")
        .and_then(|rust| rust.get("name"))
        .and_then(toml::Value::as_str);
    let alias = match library {
        Some(library) if dependency_key == package => library,
        _ => dependency_key,
    };
    alias.replace('-', "_")
}

/// Follow recorded destinations without repeating semver selection; reject missing or conflicting bindings.
fn locked_edges(
    unit: &PreparedUnit,
    edges: &[SdkLockedEdge],
    units: &[PreparedUnit],
) -> Result<Vec<(String, usize)>, Error> {
    let mut selected = BTreeMap::new();
    for edge in edges {
        let candidates: Vec<_> = units
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.binding.loaf == edge.loaf
                    && candidate.binding.version == edge.version
                    && candidate.binding.domain == edge.domain
            })
            .collect();
        if candidates.len() != 1 {
            return Err(format!(
                "{} dependency {}: recorded {} {} ({}) has {} locked destinations; re-resolve the lock",
                unit.binding.loaf,
                edge.dependency_key,
                edge.loaf,
                edge.version,
                edge.domain,
                candidates.len()
            )
            .into());
        }
        let index = candidates[0].0;
        let alias = extern_alias(&edge.dependency_key, &units[index]);
        if selected
            .insert(alias.clone(), index)
            .is_some_and(|previous| previous != index)
        {
            return Err(format!(
                "{} dependency {alias}: conflicting recorded destinations",
                unit.binding.loaf
            )
            .into());
        }
    }
    Ok(selected.into_iter().collect())
}

/// Compile one admitted script-free unit through the existing receipt-bound library executor.
fn compile_unit(
    unit: &PreparedUnit,
    context: &CompileContext<'_>,
    externs: Vec<(String, PathBuf)>,
    searches: Vec<PathBuf>,
    physical_bindings: &[crate::native_loaf::NativeLoafPhysicalBinding],
) -> Result<(PathBuf, bool, OvenStoreExecutionPayload, oven_store::OvenReceipt), Error> {
    let _source_lease = unit
        ._source_lease
        .as_ref()
        .map(environment::SourceLease::acquire)
        .transpose()?;
    let rustc = context.rustc;
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
    let mut receipt = unit_receipt(unit, context, &source)?;
    receipt = oven_store::receipt_with_build_unit_input(
        &receipt,
        crate::native_loaf::ORIGIN_INPUT,
        unit.source_origin.as_str(),
    )?;
    receipt = oven_store::receipt_with_build_unit_input(
        &receipt,
        crate::native_loaf::SOURCE_INPUT,
        crate::native_loaf::source_binding_input(&ordinary_native_source(&unit.binding))?,
    )?;
    receipt = oven_store::receipt_with_build_unit_input(
        &receipt,
        crate::native_loaf::EDGES_INPUT,
        crate::native_loaf::physical_edges_input(physical_bindings)?,
    )?;
    let (mut plan, artifacts) = unit_plan(unit, &receipt.intent, externs, searches)?;
    receipt = environment::bind_environment(&receipt, &plan.compile_environment)?;
    let native = native::prepare(unit, context, &receipt)?;
    for product in &native {
        plan.native_search_paths.push(product.owner.artifact_root.clone());
        receipt = oven_store::receipt_with_build_unit_input(
            &receipt,
            format!("sdk-native:{}", product.name),
            &product.owner.manifest.receipt_identity,
        )?;
    }
    if !native.is_empty() {
        receipt = oven_store::receipt_with_build_unit_input(
            &receipt,
            "sdk-native-libraries",
            serde_json::to_string(&native.iter().map(|product| &product.name).collect::<Vec<_>>())?,
        )?;
    }
    for (name, digest) in &plan.caller_owned_library_digests {
        receipt = oven_store::receipt_with_build_unit_input(&receipt, format!("extern:{name}"), digest)?;
    }
    let key = receipt.identity.clone();
    let directory = output.join(key.replace(':', "-"));
    std::fs::create_dir_all(&directory)?;
    let proc_macro = facet.get("type").and_then(toml::Value::as_str) == Some("proc-macro");
    let path = directory.join(if proc_macro {
        crate::rustc::linking::proc_macro_file_name(name, &rustc_host_target(context.rustc)?)
    } else {
        format!("lib{name}.rlib")
    });
    let relative = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid output name")?
        .to_string();
    let domain = format!("sdk-source-unit-{}", unit.binding.domain);
    if let Some(owner) = select_native_unit(store, &receipt, &domain)? {
        return Ok((owner.artifact_root.join(&relative), true, owner, receipt));
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
    let owner = publish_unit(unit, store, receipt.clone(), domain, result.output, &relative)?;
    Ok((owner.artifact_root.join(relative), result.reused, owner, receipt))
}

/// Reuse an exact native unit locally or import its verified receipt-bound bytes from configured mirrors.
///
/// A mirror supplies cache bytes only: source, features, toolchain, environment, native facts and dependency outputs
/// have already selected the receipt. An incompatible entry never replaces compilation, and the selected payload
/// remains leased through every consumer of the enclosing closure.
fn select_native_unit(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    domain: &str,
) -> Result<Option<OvenStoreExecutionPayload>, Error> {
    let matches = |manifest: &oven_store::store::OvenArtifactManifest| {
        manifest.kind == OvenArtifactKind::Engine
            && manifest.domain == domain
            && manifest.receipt_identity == receipt.identity
    };
    let mut selected = store.select_payloads_matching_for_execution(matches)?;
    if selected.is_empty() {
        oven_store::store_mirror::import_matching_from_mirrors(
            store,
            &oven_store::store_mirror::configured_mirrors(|name| std::env::var_os(name)),
            Some(receipt),
            matches,
        )?;
        selected = store.select_payloads_matching_for_execution(matches)?;
    }
    let owner = selected.pop();
    if let Some(owner) = &owner {
        let _verified = store.select(&owner.manifest.identity)?;
    }
    Ok(owner)
}

/// Bind a unit's archive and exact fact record to the selected compilation policy.
fn unit_receipt(
    unit: &PreparedUnit,
    context: &CompileContext<'_>,
    source: &Path,
) -> Result<oven_store::OvenReceipt, Error> {
    // A host-domain unit is compiled for the machine running the compiler, so its receipt names the host triple even
    // when the closure cross-compiles; for a same-host closure both are equal and identities are unchanged.
    let host = crate::rustc::rustc_host_target(context.rustc)?;
    let compiled_for = if unit.binding.domain == "host" {
        host.as_str()
    } else {
        context.target
    };
    let mut receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &unit.root,
            &unit.binding.loaf,
            &unit.binding.version,
            compiled_for,
            context.toolchain,
            context.profile,
            unit.binding.features.clone(),
        )
        .with_generated_source("sdk-root", source)
        .with_build_unit_input("sdk-source-archive", &unit.binding.archive_digest)
        .with_build_unit_input("domain", &unit.binding.domain)
        .with_build_unit_input("sdk-compile-policy", "source-sealed-portable-v2")
        .with_build_unit_input("compiler-host", crate::rustc::rustc_host_target(context.rustc)?)
        .with_build_unit_input(
            "compiler-commit",
            crate::rustc::rustc_commit_hash(context.rustc).ok_or("compiler has no commit hash")?,
        )
        .with_build_unit_input("compiler-binary", &context.compiler_digest),
    )?;
    if let Some(selection) = context.unit_codegen.iter().find(|selection| {
        selection.loaf == unit.binding.loaf
            && selection.version == unit.binding.version
            && selection.domain == unit.binding.domain
    }) {
        if unit.build_script {
            return Err(
                "SDK codegen override requires a build fact bound to those options for a build-script unit".into(),
            );
        }
        receipt = oven_store::receipt_with_build_unit_input(
            &receipt,
            "sdk-codegen-options",
            serde_json::to_string(&selection.options)?,
        )?;
    }
    if let Some(fact) = &unit.fact {
        receipt = oven_store::receipt_with_build_unit_input(&receipt, "sdk-build-fact", serde_json::to_string(fact)?)?;
    }
    if unit
        .manifest
        .get("rust")
        .and_then(|facet| facet.get("type"))
        .and_then(toml::Value::as_str)
        == Some("proc-macro")
        && let Some(link) = crate::rustc::linking::pinned_link(context.rustc, context.target)?
    {
        receipt = oven_store::receipt_with_build_unit_input(&receipt, "link-closure", link.identity)?;
    }
    Ok(receipt)
}

/// Publish the admitted source root and finished library atomically, then retain its execution lease.
fn publish_unit(
    unit: &PreparedUnit,
    store: &OvenStore,
    receipt: oven_store::OvenReceipt,
    domain: String,
    output: PathBuf,
    relative: &str,
) -> Result<OvenStoreExecutionPayload, Error> {
    let mut files = source_materializations(&unit.root, &unit.root)?;
    files.push(OvenArtifactMaterializedFile {
        source_path: output,
        relative_path: relative.to_string(),
    });
    let entry = store.publish(&OvenArtifactPublishRequest {
        receipt,
        domain,
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&unit.binding.identity_binding())?,
        materialized_files: files,
        materialized_directories: Vec::new(),
    })?;
    let owner = store
        .select_payloads_for_execution(&[entry.identity])?
        .pop()
        .ok_or("published SDK unit was not selected")?;
    Ok(owner)
}

/// Whether an adopted manifest states that its package has a build script (RFC 119 `[rust] build-script`).
///
/// The upstream script may live anywhere its manifest declared, so its presence is a stated fact, not something a
/// root `build.rs` check can discover; a unit that declares one compiles only with a recorded fact for its binding.
fn declares_build_script(manifest: &toml::Value) -> bool {
    manifest
        .get("rust")
        .and_then(|rust| rust.get("build-script"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Read a fact's generated files from the version's record directory at the pinned index commit.
///
/// RFC 119 `out` paths are owner-relative to the record that declares the fact (`<loaf>/<version>/`), not to the
/// source archive, so the bytes come from the index and are checked against the declared digest before use.
fn fact_out_files(
    index: &Path,
    index_commit: &str,
    binding: &SdkLockedUnit,
    fact: &oven_model::manifest::RustFactRecord,
) -> Result<Vec<FactOutFile>, Error> {
    let mut files = Vec::new();
    for member in &fact.out {
        for path in [&member.name, &member.path] {
            if Path::new(path)
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("invalid generated fact path".into());
            }
        }
        let bytes = index_file(
            index,
            index_commit,
            &format!("{}/{}/{}", binding.loaf, binding.version, member.path),
        )?;
        if digest_bytes(&bytes) != member.digest {
            return Err("generated fact digest mismatch".into());
        }
        files.push(FactOutFile {
            name: member.name.clone(),
            bytes,
        });
    }
    Ok(files)
}

/// Read one file from the pinned index commit without consulting its mutable worktree.
fn index_file(index: &Path, index_commit: &str, relative: &str) -> Result<Vec<u8>, Error> {
    validate_index_commit(index_commit)?;
    if Path::new(relative).components().any(|component| {
        !matches!(component, std::path::Component::Normal(_))
            || matches!(
                component.as_os_str().to_str(),
                Some("Cargo.toml" | "Cargo.toml.orig" | "Cargo.lock")
            )
    }) {
        return Err("index read must name an owner-relative non-Cargo file".into());
    }
    let kind = std::process::Command::new("git")
        .arg("-C")
        .arg(index)
        .args(["cat-file", "-t", index_commit])
        .output()?;
    if !kind.status.success() || kind.stdout != b"commit\n" {
        return Err("index pin must identify an existing Git commit object".into());
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(index)
        .args(["show", &format!("{index_commit}:{relative}")])
        .output()?;
    if !output.status.success() {
        return Err(format!("pinned index file unavailable: {relative}").into());
    }
    Ok(output.stdout)
}

/// Refuse mutable revisions and abbreviated commits before consulting any index file.
fn validate_index_commit(index_commit: &str) -> Result<(), Error> {
    if index_commit.len() != 40 || !index_commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("index commit must be a full hexadecimal commit identity".into());
    }
    Ok(())
}

/// Select a fact only after verifying the pinned index archive association and all four binding dimensions.
fn index_fact(
    index: &Path,
    index_commit: &str,
    binding: &SdkLockedUnit,
    target: &str,
    toolchain: &str,
    profile: &str,
) -> Result<Option<oven_model::manifest::RustFactRecord>, Error> {
    let entry = index_entry(index, index_commit, binding)?;
    let relative = entry
        .get("manifest")
        .and_then(serde_json::Value::as_str)
        .ok_or("pinned index manifest is absent")?;
    let manifest: toml::Value = toml::from_str(std::str::from_utf8(&index_file(index, index_commit, relative)?)?)?;
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
            if fact.toolchain == toolchain && fact.target == target && fact.profile == profile && features == enabled {
                selected.push(fact);
            }
        }
    }
    if selected.len() > 1 {
        return Err("ambiguous exact build facts".into());
    }
    Ok(selected.pop())
}

/// Read exactly one index version and verify its association with the admitted archive.
fn index_entry(index: &Path, index_commit: &str, binding: &SdkLockedUnit) -> Result<serde_json::Value, Error> {
    let bytes = index_file(index, index_commit, &format!("index/{}", binding.loaf))?;
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
    let entry = entries.pop().ok_or("pinned index version is absent")?;
    if entry.get("cksum").and_then(serde_json::Value::as_str) != Some(&binding.archive_digest) {
        return Err("pinned index disagrees with locked archive digest".into());
    }
    Ok(entry)
}

/// Check that a supplied resolution honors the requested root's local and default feature demands at the same pin.
pub fn validate_locked_root_features(
    index: &Path,
    index_commit: &str,
    binding: &SdkLockedUnit,
    requested: &[String],
    default_features: bool,
) -> Result<(), Error> {
    validate_index_commit(index_commit)?;
    let entry = index_entry(index, index_commit, binding)?;
    let features = entry
        .get("features")
        .and_then(serde_json::Value::as_object)
        .ok_or("index feature table is absent")?;
    for feature in requested {
        if !features.contains_key(feature) || !binding.features.contains(feature) {
            return Err(format!(
                "{}: requested root feature {feature} absent from index or lock",
                binding.loaf
            )
            .into());
        }
    }
    if default_features
        && features.contains_key("default")
        && !binding.features.iter().any(|feature| feature == "default")
    {
        return Err(format!("{}: requested root default features absent from lock", binding.loaf).into());
    }
    Ok(())
}

/// Apply selected compile environment facts without executing a build script or importing an ambient output root.
fn apply_fact(unit: &PreparedUnit, plan: &mut OvenRustcArtifactPlan) -> Result<(), Error> {
    let Some(fact) = &unit.fact else {
        return Ok(());
    };
    if !fact.tool.is_empty() {
        return Err(
            "exact fact requires retained publisher tool assets; none were supplied to the closure compiler".into(),
        );
    }
    let out = unit.root.join(".oven-out");
    if !unit.fact_out.is_empty() {
        std::fs::create_dir_all(&out)?;
    }
    for file in &unit.fact_out {
        let destination = out.join(&file.name);
        std::fs::create_dir_all(destination.parent().ok_or("generated fact has no parent")?)?;
        std::fs::write(destination, &file.bytes)?;
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
    let environment = environment::package_environment(unit)?;
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
            .insert(name.clone(), oven_store::store::digest_regular_file(path)?.1);
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
    /// Cross-target closures keyed by target triple, such as the wasm32-wasip1 inputs of vocabulary desugarers.
    auxiliary_targets: BTreeMap<String, SdkCompiledClosure>,
}

/// One admitted archive and compiled output held in the immutable Oven store.
pub struct SdkCompiledUnit {
    /// Complete selected source binding.
    binding: SdkLockedUnit,
    /// Store-owned library or procedural-macro output path.
    output: PathBuf,
    owner: OvenStoreExecutionPayload,
    /// Frozen direct-inspection record using the same active edges, features and facts as compilation.
    inspection: serde_json::Value,
    /// Exact physical inputs checked against the source-current reproduced compilation recipe.
    physical_edges: Vec<SdkPhysicalNativeEdge>,
    /// Exact source-current recipe reproduced on both compile and reuse paths; Engine owners do not retain it.
    reproduced_receipt: oven_store::OvenReceipt,
}

/// Portable coordinates of one receipt-bound native SDK output inside its retained store entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkNativeArtifact {
    /// Complete selected package, domain, feature, and archive binding.
    pub binding: SdkLockedUnit,
    /// Immutable Oven entry that owns both source and native output.
    pub store_identity: String,
    /// Receipt authorizing the native compilation.
    pub receipt_identity: String,
    /// Native library path relative to the entry's artifact root.
    pub relative_path: String,
    /// Digest of the native bytes recorded by the immutable entry manifest.
    pub digest: String,
}

impl SdkCompiledUnit {
    /// Borrow verified physical inputs; an empty slice is proven by the reproduced recipe with no extern inputs.
    ///
    /// These producer records are not a published graph authority and do not authorize consumer closure selection
    /// until a source-generation-bound graph seals them (#1337, #1698).
    pub fn physical_edges(&self) -> &[SdkPhysicalNativeEdge] {
        &self.physical_edges
    }

    /// Export the native output's admitted coordinates without rereading its source or native bytes.
    pub fn native_artifact(&self) -> Result<SdkNativeArtifact, Error> {
        let relative = self
            .output
            .strip_prefix(&self.owner.artifact_root)?
            .to_string_lossy()
            .replace('\\', "/");
        let file = self
            .owner
            .admitted_materialized_files()
            .iter()
            .find(|file| file.relative_path == relative)
            .ok_or("native SDK output is absent from its admitted artifact manifest")?;
        Ok(SdkNativeArtifact {
            binding: self.binding.clone(),
            store_identity: self.owner.manifest.identity.clone(),
            receipt_identity: self.compiled_identity().to_string(),
            relative_path: relative,
            digest: file.digest.clone(),
        })
    }
    /// Receipt identity binding source, dependencies, facts, compiler and profile.
    pub fn compiled_identity(&self) -> &str {
        &self.owner.manifest.receipt_identity
    }

    /// Content-addressed Oven store entry holding this unit's compiled output, the coordinate an asset archive packs.
    pub fn entry_identity(&self) -> &str {
        &self.owner.manifest.identity
    }
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
    /// Publish ordinary per-unit physical records and transfer original native leases without an SDK inventory.
    ///
    /// This migration bridge consumes only a complete native producer graph. Exact captured physical edges, not
    /// catalog edges, select child records. Records retain each original native recipe and source binding; they
    /// grant no semantic or macro completeness. Auxiliary target closures remain distinct through their identities.
    pub fn into_native_loafs(self, store: &OvenStore) -> Result<crate::native_loaf::NativeLoafGraph, Error> {
        self.require_complete()?;
        let mut graph = crate::native_loaf::NativeLoafGraph::default();
        publish_native_loaf_units(self, store, &mut graph)?;
        Ok(graph)
    }

    /// Project the successfully compiled subgraph while retaining its source leases in this closure.
    ///
    /// The graph includes the exact resolved host/target edges. Refused units are absent, and callers must retain and
    /// report the refusal list before publishing a component whose declared roots include those units.
    pub fn inspection_project(&self) -> serde_json::Value {
        serde_json::json!({ "crates": self.units.iter().map(|unit| &unit.inspection).collect::<Vec<_>>() })
    }
    /// Borrow the compiler measurement without weakening publication readiness.
    pub fn report(&self) -> &SdkClosureReport {
        &self.report
    }
    /// Borrow all retained units without releasing or retargeting their leases.
    pub fn units(&self) -> &[SdkCompiledUnit] {
        &self.units
    }
    /// Retain a complete cross-target closure beside this one for the same publication transaction.
    ///
    /// The auxiliary closure keeps its own leases. A target may be attached once, and an incomplete closure is refused
    /// so a desugarer can never be built from a partially compiled input set.
    pub fn attach_auxiliary_target(&mut self, target: &str, closure: SdkCompiledClosure) -> Result<(), Error> {
        closure.require_complete()?;
        if self.auxiliary_targets.contains_key(target) {
            return Err(format!("auxiliary target `{target}` is already attached").into());
        }
        self.auxiliary_targets.insert(target.to_string(), closure);
        Ok(())
    }
    /// Borrow the retained cross-target closures, keyed by target triple.
    pub fn auxiliary_targets(&self) -> &BTreeMap<String, SdkCompiledClosure> {
        &self.auxiliary_targets
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

/// Transfer authoritative producer units into ordinary durable records in physical dependency order.
fn publish_native_loaf_units(
    closure: SdkCompiledClosure,
    store: &OvenStore,
    graph: &mut crate::native_loaf::NativeLoafGraph,
) -> Result<(), Error> {
    use crate::native_loaf::{NativeLoafDependency, NativeLoafReference};
    closure.require_complete()?;
    for unit in closure.units {
        let artifact = unit.native_artifact()?;
        let mut dependencies = Vec::new();
        for edge in &unit.physical_edges {
            let source = ordinary_native_source(&edge.destination().binding);
            let child = graph
                .units()
                .values()
                .find(|child| {
                    child.record().native.identity == edge.destination().store_identity
                        && child.record().native.receipt_identity == edge.destination().receipt_identity
                        && child.record().native.relative_path == edge.destination().relative_path
                        && child.record().native.digest == edge.destination().digest
                        && child.record().source == source
                })
                .ok_or("ordinary native record is missing its exact producer-selected dependency")?;
            let child_owner_store = child
                .output()?
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or("ordinary native child has no Store coordinate")?
                .to_path_buf();
            if child_owner_store != edge.store() {
                return Err("ordinary native dependency Store differs from its producer-selected destination".into());
            }
            dependencies.push(NativeLoafDependency {
                alias: edge.alias().to_string(),
                record_identity: child.identity().to_string(),
                native: child.record().native.clone(),
                source: child.record().source.clone(),
            });
        }
        // The native owner's payload carries the exact source selection. Do not promote mutable catalog edges.
        let admitted: SdkLockedUnit = serde_json::from_slice(&unit.owner.payload)?;
        if serde_json::to_value(admitted.identity_binding())? != serde_json::to_value(unit.binding.identity_binding())?
            || unit
                .reproduced_receipt
                .sources
                .build_unit_inputs
                .get("sdk-source-archive")
                != Some(&unit.binding.archive_digest)
            || unit.reproduced_receipt.sources.build_unit_inputs.get("domain") != Some(&unit.binding.domain)
        {
            return Err("ordinary native source differs from its actual producer binding".into());
        }
        let native = NativeLoafReference {
            identity: artifact.store_identity,
            receipt_identity: artifact.receipt_identity,
            domain: unit.owner.manifest.domain.clone(),
            relative_path: artifact.relative_path,
            digest: artifact.digest,
        };
        graph.publish(
            store,
            ordinary_native_source(&unit.binding),
            native,
            unit.reproduced_receipt,
            std::sync::Arc::new(unit.owner),
            dependencies,
        )?;
    }
    for (_, auxiliary) in closure.auxiliary_targets {
        publish_native_loaf_units(auxiliary, store, graph)?;
    }
    Ok(())
}

/// Preserve the producer's complete source and evaluated predicates while dropping unauthenticated catalog edges.
fn ordinary_native_source(binding: &SdkLockedUnit) -> crate::native_loaf::NativeLoafSource {
    crate::native_loaf::NativeLoafSource {
        loaf: binding.loaf.clone(),
        version: binding.version.clone(),
        archive_digest: binding.archive_digest.clone(),
        domain: binding.domain.clone(),
        features: binding.features.clone(),
        target_predicates: binding
            .target_predicates
            .iter()
            .map(|predicate| crate::native_loaf::NativeLoafPredicate {
                declaration: predicate.declaration,
                target: predicate.target.clone(),
                matches: predicate.matches,
            })
            .collect(),
    }
}

/// Capture complete selected child coordinates before their parent's native recipe is published.
fn selected_native_bindings(
    selected: &[(&str, &SdkCompiledUnit)],
) -> Result<Vec<crate::native_loaf::NativeLoafPhysicalBinding>, Error> {
    selected
        .iter()
        .map(|(alias, unit)| {
            let edge = physical_edges::record(alias, unit)?;
            let artifact = edge.destination();
            Ok(crate::native_loaf::NativeLoafPhysicalBinding {
                alias: alias.to_string(),
                source: ordinary_native_source(&artifact.binding),
                native: crate::native_loaf::NativeLoafReference {
                    identity: artifact.store_identity.clone(),
                    receipt_identity: artifact.receipt_identity.clone(),
                    domain: unit.owner.manifest.domain.clone(),
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                },
            })
        })
        .collect()
}

/// Hash exactly the target standard library a unit links: the files the `rust-std` component manifest lists.
///
/// Optional components install into the same directories (`rustc-dev` alone adds hundreds of compiler-internal
/// libraries beside std), so walking the directory would make a unit's identity depend on which components a machine
/// happens to have. The `rust-std-<target>` manifest names the std set every install of the release shares; each
/// listed file is hashed by its sysroot-relative path and bytes, and a missing or escaping entry refuses.
fn compiler_closure_digest(rustc: &Path, target: &str) -> Result<String, Error> {
    let rustc = std::fs::canonicalize(rustc)?;
    let root = rustc.parent().and_then(Path::parent).ok_or("compiler has no sysroot")?;
    let manifest = root.join("lib/rustlib").join(format!("manifest-rust-std-{target}"));
    let listing = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("compiler has no rust-std manifest for {target}: {error}"))?;
    let mut records = BTreeMap::new();
    for line in listing.lines() {
        let Some(relative) = line.strip_prefix("file:") else {
            continue;
        };
        records.insert(relative.to_string(), std_library_digest(root, relative)?);
    }
    if records.is_empty() {
        return Err(format!("rust-std manifest for {target} lists no files").into());
    }
    Ok(digest_bytes(&serde_json::to_vec(&records)?))
}

/// Digest one manifest-listed std file, refusing a path that leaves the sysroot or is not a regular file.
fn std_library_digest(root: &Path, relative: &str) -> Result<String, Error> {
    if Path::new(relative)
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("rust-std manifest entry is not sysroot-relative: {relative}").into());
    }
    let resolved = std::fs::canonicalize(root.join(relative))?;
    if !resolved.starts_with(root) || !std::fs::metadata(&resolved)?.is_file() {
        return Err(format!("rust-std manifest entry is not a sysroot file: {relative}").into());
    }
    Ok(digest_bytes(&std::fs::read(resolved)?))
}

#[cfg(all(test, unix))]
pub(crate) mod digest_reuse_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A stated build script requires a fact even when no `build.rs` sits at the package root.
    #[test]
    fn stated_build_script_is_honored_wherever_the_script_lives() -> Result<(), Error> {
        let stated: toml::Value = toml::from_str("[rust]\nname = \"rustversion\"\nbuild-script = true\n")?;
        let absent: toml::Value = toml::from_str("[rust]\nname = \"itoa\"\n")?;
        let denied: toml::Value = toml::from_str("[rust]\nname = \"itoa\"\nbuild-script = false\n")?;
        assert!(declares_build_script(&stated));
        assert!(!declares_build_script(&absent));
        assert!(!declares_build_script(&denied));
        Ok(())
    }

    /// Component catalogs and additional targets cannot change the selected sysroot library identity.
    #[test]
    fn compiler_digest_excludes_optional_components() -> Result<(), Error> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("bin"))?;
        std::fs::create_dir_all(root.path().join("lib/rustlib/selected/lib"))?;
        let rustc = root.path().join("bin/rustc");
        std::fs::write(&rustc, b"compiler")?;
        let library = root.path().join("lib/rustlib/selected/lib/libstd.rlib");
        std::fs::write(&library, b"selected library")?;
        std::fs::write(
            root.path().join("lib/rustlib/manifest-rust-std-selected"),
            b"file:lib/rustlib/selected/lib/libstd.rlib\n",
        )?;
        let first = compiler_closure_digest(&rustc, "selected")?;
        std::fs::create_dir_all(root.path().join("lib/rustlib/extra/lib"))?;
        std::fs::write(root.path().join("lib/rustlib/extra/lib/libstd.rlib"), b"extra target")?;
        std::fs::write(
            root.path().join("lib/rustlib/components"),
            b"rust-src\nclippy\nrustc-dev\n",
        )?;
        // rustc-dev installs compiler-internal libraries beside std; they are not part of what a unit links.
        std::fs::write(
            root.path().join("lib/rustlib/selected/lib/librustc_driver.rlib"),
            b"compiler internals",
        )?;
        assert_eq!(first, compiler_closure_digest(&rustc, "selected")?);
        std::fs::write(&library, b"changed selected library")?;
        assert_ne!(first, compiler_closure_digest(&rustc, "selected")?);
        Ok(())
    }

    /// Run a local fixture Git command without changing the user's index repository or identity configuration.
    pub(super) fn fixture_git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>, Error> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()?;
        if !output.status.success() {
            return Err(format!("fixture git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        Ok(output.stdout)
    }

    /// Committed index bytes must win over mutable worktree bytes; tree objects and Cargo paths are not pins.
    #[test]
    fn index_reads_are_commit_bound_and_exclude_cargo() -> Result<(), Error> {
        let root = tempfile::tempdir()?;
        fixture_git(root.path(), &["init", "--quiet"])?;
        std::fs::write(root.path().join("record.txt"), b"committed")?;
        fixture_git(root.path(), &["add", "record.txt"])?;
        fixture_git(
            root.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "chore - 1698 pinned index fixture",
            ],
        )?;
        let revision = fixture_git(root.path(), &["rev-parse", "HEAD"])?;
        let commit = std::str::from_utf8(&revision)?.trim();
        std::fs::write(root.path().join("record.txt"), b"worktree")?;
        assert_eq!(index_file(root.path(), commit, "record.txt")?, b"committed");
        assert_eq!(std::fs::read(root.path().join("record.txt"))?, b"worktree");
        let tree = fixture_git(root.path(), &["rev-parse", "HEAD^{tree}"])?;
        assert!(index_file(root.path(), std::str::from_utf8(&tree)?.trim(), "record.txt").is_err());
        assert!(index_file(root.path(), commit, "Cargo.toml").is_err());
        assert!(index_file(root.path(), "HEAD", "record.txt").is_err());
        Ok(())
    }

    /// Parse a script-free seed unit for graph-selection regression tests.
    pub(super) fn unit(loaf: &str, domain: &str, manifest: &str, features: &[&str]) -> Result<PreparedUnit, Error> {
        Ok(PreparedUnit {
            source_origin: crate::native_loaf::NativeLoafOrigin::Registry,
            binding: SdkLockedUnit {
                loaf: loaf.to_string(),
                version: "1.0.0".to_string(),
                archive_digest: digest_bytes(b"source"),
                domain: domain.to_string(),
                features: features.iter().map(|feature| (*feature).to_string()).collect(),
                target_predicates: Vec::new(),
                edges: None,
            },
            about: serde_json::Value::Null,
            primary: false,
            _source_lease: None,
            manifest: toml::from_str(manifest)?,
            root: PathBuf::new(),
            build_script: false,
            fact: None,
            fact_out: Vec::new(),
        })
    }

    /// Recorded destinations distinguish compatible versions; legacy ambiguity requires fresh resolution.
    #[test]
    fn recorded_edges_disambiguate_versions_and_reject_missing_destinations() -> Result<(), Error> {
        let mut root = unit(
            "crates-io/root",
            "host",
            "[dependencies]\nsyn={loaf='crates-io/syn',version='>=2, <4'}",
            &[],
        )?;
        let mut older = unit("crates-io/syn", "host", "", &[])?;
        older.binding.version = "2.0.119".into();
        let mut newer = unit("crates-io/syn", "host", "", &[])?;
        newer.binding.version = "3.0.6".into();
        let mut units = vec![root, older, newer];
        let error = match active_edges(&units[0], &units) {
            Ok(_) => return Err("ambiguous legacy lock was accepted".into()),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("dependency syn"));
        assert!(error.contains("re-resolve"));
        units[0].binding.edges = Some(vec![SdkLockedEdge {
            dependency_key: "syn".into(),
            loaf: "crates-io/syn".into(),
            version: "3.0.6".into(),
            domain: "host".into(),
        }]);
        assert_eq!(active_edges(&units[0], &units)?, vec![("syn".into(), 2)]);
        root = units.remove(0);
        units.pop();
        assert!(active_edges(&root, &units).is_err());
        root.binding.edges = Some(Vec::new());
        assert!(active_edges(&root, &units)?.is_empty());
        Ok(())
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
            auxiliary_targets: BTreeMap::new(),
        };
        assert!(closure.require_complete().is_err());
    }

    /// Inspection retains the selected Rust source, alias indices and feature set without rereading declarations.
    #[test]
    fn inspection_uses_selected_source_and_edges() -> Result<(), Error> {
        let unit = unit(
            "crates-io/example",
            "host",
            "[rust]\nname='example_macro'\ntype='proc-macro'\nedition='2021'\n[rust.source]\nroot='src/macro.rs'\n",
            &["selected"],
        )?;
        let graph = inspection_unit(
            &unit,
            Path::new("/sealed/source"),
            vec![serde_json::json!({"crate": 3, "name": "renamed"})],
        )?;
        assert_eq!(graph["root_module"], "/sealed/source/src/macro.rs");
        assert_eq!(graph["is_proc_macro"], true);
        assert_eq!(graph["deps"][0]["crate"], 3);
        assert_eq!(graph["deps"][0]["name"], "renamed");
        assert_eq!(graph["cfg"][0], "feature=\"selected\"");
        assert_eq!(graph["env"]["CARGO_PKG_VERSION"], "1.0.0");
        Ok(())
    }
}
