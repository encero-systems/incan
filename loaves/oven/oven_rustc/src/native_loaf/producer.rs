//! Retained source/fact projection of an explicitly admitted ordinary native set (#1337, #1698).
//!
//! Native records bind these source bytes and facts to their real producer. This projection supplies neither a
//! complete Rust semantic world nor macro execution permission. Auxiliary target groups must be admitted separately.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use oven_model::manifest::RustFactRecord;
use oven_store::digest_bytes;

use super::{
    NativeLoafError, NativeLoafGraph, Result, SelectedNativeLoaf, failed, refused, verify_child_bindings, verify_record,
};

/// Actual work attempted by this retained source-projection boundary, owned by one command.
///
/// Counts accumulate when the caller reuses a report, including work before a refusal. They exclude prior Store
/// admission and Store verification's internal I/O. This path has no native owner selector: retaining an Arc is not
/// an acquisition, so it never increments `native_owner_acquisitions`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct NativeLoafInspectionWork {
    /// Calls entering source/fact projection, including attempts refused by original-owner verification.
    pub source_projection_attempts: usize,
    /// Actual declaration file-read calls, including failed reads; inventory misses do not count as reads.
    pub manifest_read_attempts: usize,
    /// Attempts to match a valid declared generated path against the admitted native member inventory.
    pub generated_member_observation_attempts: usize,
    /// Native owner selection calls issued by this projection; the retained-only implementation issues none.
    pub native_owner_acquisitions: usize,
}

/// One source descriptor retaining the original per-unit record and native owner leases.
///
/// Coordinates describe this admitted generation. Use the containing inputs' verified handoff before inspection;
/// borrowing a descriptor alone is not a fresh integrity check or permission to load its macro output.
pub struct NativeLoafInspectionUnit {
    selected: Arc<SelectedNativeLoaf>,
    crate_name: String,
    edition: String,
    root_module: PathBuf,
    proc_macro: bool,
}

impl NativeLoafInspectionUnit {
    /// Original selected ordinary authority, including exact source, native output, intent and reproduced recipe.
    pub fn selected(&self) -> &Arc<SelectedNativeLoaf> {
        &self.selected
    }

    /// Declared Rust crate name, independent of consumer aliases and native output filenames.
    pub fn crate_name(&self) -> &str {
        &self.crate_name
    }

    /// Declared Rust edition under the retained source owner.
    pub fn edition(&self) -> &str {
        &self.edition
    }

    /// Exact inventoried source root under the retained native owner.
    pub fn root_module(&self) -> &Path {
        &self.root_module
    }

    /// Declared host procedural-macro source role; this grants no macro execution permission.
    pub fn is_proc_macro(&self) -> bool {
        self.proc_macro
    }
}

/// Command-owned source inputs for exactly one admitted set, sharing the producer's original owner Arcs.
///
/// The scope digest binds the selected record identities, not semantic-world completeness. A caller preparing
/// public ABI metadata must separately supply its complete semantic source request and auxiliary target authority.
pub struct NativeLoafInspectionInputs {
    units: BTreeMap<String, NativeLoafInspectionUnit>,
    scope_digest: String,
}

impl NativeLoafInspectionInputs {
    /// Borrow the exact selected-set descriptors in canonical immutable record order.
    pub fn units(&self) -> &BTreeMap<String, NativeLoafInspectionUnit> {
        &self.units
    }

    /// Identity of this exact retained set, including native source, intent, facts and physical edges through records.
    pub fn scope_digest(&self) -> &str {
        &self.scope_digest
    }

    /// Recheck complete original payloads and source facts at each read-only inspection handoff.
    ///
    /// This does not select new owners, write closure proofs, read an authored checkout or consult an index/Git.
    pub fn inspection_project(&self) -> Result<serde_json::Value> {
        self.inspection_project_with_work(&mut NativeLoafInspectionWork::default())
    }

    /// Perform the same verified handoff while reporting actual work, including attempts before a refusal.
    pub fn inspection_project_with_work(&self, work: &mut NativeLoafInspectionWork) -> Result<serde_json::Value> {
        let selected = self
            .units
            .iter()
            .map(|(identity, unit)| (identity.clone(), Arc::clone(&unit.selected)))
            .collect::<BTreeMap<_, _>>();
        let indices = selected
            .keys()
            .enumerate()
            .map(|(index, identity)| (identity.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut crates = Vec::new();
        for unit in self.units.values() {
            verify_child_bindings(&unit.selected.record, &selected)?;
            let source = source_projection(&unit.selected, work)?;
            if source.crate_name != unit.crate_name
                || source.edition != unit.edition
                || source.root_module != unit.root_module
                || source.proc_macro != unit.proc_macro
            {
                return Err(refused("retained native source descriptor changed"));
            }
            let mut dependencies = BTreeMap::new();
            for edge in &unit.selected.record.dependencies {
                let index = indices
                    .get(edge.record_identity.as_str())
                    .ok_or_else(|| refused("inspection physical destination is not admitted"))?;
                if dependencies.insert(&edge.alias, index).is_some() {
                    return Err(refused("inspection physical extern alias is duplicated"));
                }
            }
            let dependencies = dependencies
                .into_iter()
                .map(|(name, index)| serde_json::json!({"crate": index, "name": name}))
                .collect();
            crates.push(
                crate::sdk_closure::inspection_source_unit(
                    &source.manifest,
                    &unit.selected.record.source.features,
                    source.environment,
                    source.fact.as_ref(),
                    &unit.selected.native_owner.artifact_root.join("source"),
                    dependencies,
                )
                .map_err(NativeLoafError::Failed)?,
            );
        }
        Ok(serde_json::json!({"crates": crates}))
    }
}

/// Retain an already admitted ordinary set; no Store acquisition or SDK inventory supplies source authority.
pub(super) fn from_graph(
    graph: &NativeLoafGraph,
    work: &mut NativeLoafInspectionWork,
) -> Result<NativeLoafInspectionInputs> {
    let mut units = BTreeMap::new();
    let mut targets = BTreeSet::new();
    for (identity, selected) in &graph.units {
        verify_child_bindings(&selected.record, &graph.units)?;
        let source = source_projection(selected, work)?;
        if selected.record.source.domain == "target" {
            targets.insert(&selected.record.recipe.intent.target);
        }
        units.insert(
            identity.clone(),
            NativeLoafInspectionUnit {
                selected: Arc::clone(selected),
                crate_name: source.crate_name,
                edition: source.edition,
                root_module: source.root_module,
                proc_macro: source.proc_macro,
            },
        );
    }
    if targets.len() > 1 {
        return Err(refused(
            "inspection auxiliary targets require separate retained source groups",
        ));
    }
    let scope_digest = digest_bytes(&serde_json::to_vec(&units.keys().collect::<Vec<_>>()).map_err(failed)?);
    Ok(NativeLoafInspectionInputs { units, scope_digest })
}

/// Checked per-handoff data; the source declaration and sealed recipe are the only projection authorities.
struct SourceProjection {
    manifest: toml::Value,
    environment: BTreeMap<String, String>,
    fact: Option<RustFactRecord>,
    crate_name: String,
    edition: String,
    root_module: PathBuf,
    proc_macro: bool,
}

/// Verify original complete inventories, then reconstruct source/fact inputs without a live producer shortcut.
fn source_projection(unit: &SelectedNativeLoaf, work: &mut NativeLoafInspectionWork) -> Result<SourceProjection> {
    work.source_projection_attempts += 1;
    // Full admitted verification is intentional even for transitional domains with writable closure-proof caches.
    verify_record(&unit.record, &unit.record_owner, &unit.native_owner, true)?;
    let manifest = super::selection::source_manifest_with_reader(unit, &mut |path| {
        work.manifest_read_attempts += 1;
        std::fs::read(path).map_err(failed)
    })?;
    let project = manifest
        .get("project")
        .ok_or_else(|| refused("inspection source has no project"))?;
    if string(project, "name")? != unit.record.source.loaf || string(project, "version")? != unit.record.source.version
    {
        return Err(refused(
            "inspection declaration differs from the native producer source",
        ));
    }
    let facet = manifest
        .get("rust")
        .ok_or_else(|| refused("inspection source has no Rust facet"))?;
    let crate_name = string(facet, "name")?.to_string();
    if !rust_name(&crate_name) {
        return Err(refused("inspection source has invalid declared Rust crate name"));
    }
    let edition = string(facet, "edition")?.to_string();
    if !matches!(edition.as_str(), "2015" | "2018" | "2021" | "2024") {
        return Err(refused("inspection source has unsupported declared Rust edition"));
    }
    let proc_macro = match string(facet, "type")? {
        "lib" => false,
        "proc-macro" => true,
        _ => {
            return Err(refused(
                "inspection source requires a declared library or procedural macro",
            ));
        }
    };
    let root = facet
        .get("source")
        .and_then(|source| source.get("root"))
        .map(|root| {
            root.as_str()
                .ok_or_else(|| refused("inspection source root is not text"))
        })
        .transpose()?
        .unwrap_or("src/lib.rs");
    let root_module = source_member(unit, root)?;
    let inputs = &unit.record.recipe.sources.build_unit_inputs;
    for name in [
        "compiler-host",
        "compiler-commit",
        "compiler-binary",
        "native-compiler-executable",
        "sdk-compile-policy",
    ] {
        if inputs.get(name).is_none_or(String::is_empty) {
            return Err(refused("inspection native producer lacks sealed compiler authority"));
        }
    }
    match unit.record.source.domain.as_str() {
        "target" if !proc_macro => {}
        "host" if inputs.get("compiler-host") == Some(&unit.record.recipe.intent.target) => {}
        _ => {
            return Err(refused(
                "inspection source role contradicts native producer domain or host",
            ));
        }
    }
    let environment: BTreeMap<String, String> = serde_json::from_str(
        inputs
            .get("sdk-compile-environment")
            .ok_or_else(|| refused("inspection native producer has no sealed environment"))?,
    )
    .map_err(failed)?;
    for name in [
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "CARGO_PKG_NAME",
        "CARGO_PKG_VERSION",
        "CARGO_CRATE_NAME",
    ] {
        if !environment.contains_key(name) {
            return Err(refused("inspection native producer environment is incomplete"));
        }
    }
    let fact: Option<RustFactRecord> = inputs
        .get("sdk-build-fact")
        .map(|value| serde_json::from_str(value).map_err(failed))
        .transpose()?;
    if facet.get("build-script").and_then(toml::Value::as_bool) == Some(true) && fact.is_none() {
        return Err(refused("inspection build-script source lacks its sealed declared fact"));
    }
    if let Some(fact) = &fact {
        validate_fact(unit, fact, work)?;
    }
    Ok(SourceProjection {
        manifest,
        environment,
        fact,
        crate_name,
        edition,
        root_module,
        proc_macro,
    })
}

/// Check generated inputs against the producer's exact selection and admitted immutable generated members.
fn validate_fact(unit: &SelectedNativeLoaf, fact: &RustFactRecord, work: &mut NativeLoafInspectionWork) -> Result<()> {
    let intent = &unit.record.recipe.intent;
    let mut features = fact.features.clone();
    features.sort();
    features.dedup();
    let mut selected = intent.features.clone();
    selected.sort();
    selected.dedup();
    if fact.toolchain != intent.toolchain
        || fact.target != intent.target
        || fact.profile != intent.profile
        || features != selected
    {
        return Err(refused("inspection fact differs from exact native producer intent"));
    }
    let mut outputs = BTreeSet::new();
    for output in &fact.out {
        if !plain_relative(&output.name) || !plain_relative(&output.path) || !outputs.insert(&output.name) {
            return Err(refused("inspection generated fact has invalid or duplicate paths"));
        }
        let relative = format!(".oven-out/{}", output.name);
        work.generated_member_observation_attempts += 1;
        source_member(unit, &relative)?;
        let member = unit
            .native_owner
            .admitted_materialized_files()
            .iter()
            .find(|member| member.relative_path == format!("source/{relative}"))
            .ok_or_else(|| refused("inspection generated member is not inventoried"))?;
        if member.digest != output.digest {
            return Err(refused("inspection generated member differs from its sealed fact"));
        }
    }
    let mut names = BTreeSet::new();
    for entry in &fact.environment {
        if entry.name.is_empty() || !names.insert(&entry.name) {
            return Err(refused(
                "inspection fact environment repeats an empty or duplicate name",
            ));
        }
        match (&entry.literal, &entry.out) {
            (Some(_), None) => {}
            (None, Some(path)) if path == "." || plain_relative(path) => {}
            _ => return Err(refused("inspection fact environment has invalid path or value form")),
        }
    }
    Ok(())
}

/// Require an exact regular-file member in the original native source inventory, with no path inference.
fn source_member(unit: &SelectedNativeLoaf, relative: &str) -> Result<PathBuf> {
    if !plain_relative(relative) {
        return Err(refused("inspection source member is not a portable relative path"));
    }
    let relative = format!("source/{relative}");
    if !unit
        .native_owner
        .admitted_materialized_files()
        .iter()
        .any(|member| member.relative_path == relative)
    {
        return Err(refused(
            "inspection source member is not inventoried by the native owner",
        ));
    }
    Ok(unit.native_owner.artifact_root.join(relative))
}

/// Admit portable owner-relative members without traversal, empty names or foreign separators.
fn plain_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Require the Rust declaration name rather than infer an extern name from output spelling.
fn rust_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Read a required declaration text field without accepting another TOML value kind.
fn string<'a>(value: &'a toml::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| refused("inspection declaration lacks required text field"))
}

#[cfg(test)]
mod tests;
