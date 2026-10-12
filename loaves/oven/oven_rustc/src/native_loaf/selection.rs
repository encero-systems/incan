//! Declaration-authorized root projection over producer-authenticated native records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oven_model::manifest::{DependencySource, DependencySpec};
use oven_model::observed_inputs::InputObservations;

use super::{
    NativeLoafError, NativeLoafGraph, NativeLoafOrigin, NativeLoafRoot, NativeLoafSource, Result, SelectedNativeLoaf,
    failed, refused,
};

/// Match already active declarations without resolving, compiling, or granting undeclared graph namespaces.
pub(super) fn select_roots(
    graph: &NativeLoafGraph,
    dependencies: &[DependencySpec],
    declaration_owner: &Path,
    domain: &str,
) -> Result<Vec<NativeLoafRoot>> {
    select_roots_with_sources(
        graph,
        dependencies,
        declaration_owner,
        domain,
        &mut LocalSources::default(),
    )
}

/// Existing local source selections retained by the serial owner under replacement-sensitive observations.
#[derive(Default)]
pub(super) struct LocalSources {
    selections: BTreeMap<PathBuf, ObservedSelection>,
    checked: BTreeSet<PathBuf>,
    /// Actual mapping/hash demands; metadata-only retained checks do not increment this counter.
    pub(super) loads: usize,
}

/// The publisher's original portable mapping and the exact filesystem inputs consulted to produce it.
struct ObservedSelection {
    selected: (toml::Value, String),
    inputs: InputObservations,
}

impl LocalSources {
    /// Begin one current-input reconciliation without discarding prior source selections.
    pub(super) fn begin_request(&mut self) {
        self.loads = 0;
        self.checked.clear();
    }

    /// Reuse a reconciled selection, returning to the canonical publisher mapper on any metadata change.
    /// Unsupported identities never authorize reuse across commands; repeated demands within one command still share
    /// their freshly mapped result. Final reconciliation rejects inputs changed during selection or handoff.
    pub(super) fn get(&mut self, path: &Path) -> Result<&(toml::Value, String)> {
        let path = path.canonicalize().map_err(failed)?;
        if !self.checked.contains(&path) {
            let retained = self
                .selections
                .get(&path)
                .is_some_and(|entry| entry.inputs.supports_retention() && entry.inputs.current());
            if !retained {
                let mut inputs = InputObservations::default();
                let selected = crate::sdk_closure::local_native_source_selection_with_observer(&path, &mut |input| {
                    inputs.observe(input)
                })
                .map_err(NativeLoafError::Failed)?;
                if inputs.supports_retention() && !inputs.current() {
                    return Err(refused("local source changed during canonical selection"));
                }
                self.loads += 1;
                self.selections
                    .insert(path.clone(), ObservedSelection { selected, inputs });
            }
            self.checked.insert(path.clone());
        }
        self.selections
            .get(&path)
            .map(|entry| &entry.selected)
            .ok_or_else(|| refused("current local source selection is missing"))
    }

    /// Reconcile every selection actually used by this command before retaining its source authority.
    pub(super) fn finish_request(&self) -> Result<()> {
        for path in &self.checked {
            let entry = self
                .selections
                .get(path)
                .ok_or_else(|| refused("checked local source selection missing"))?;
            if entry.inputs.supports_retention() && !entry.inputs.current() {
                return Err(refused("local source changed during native selection"));
            }
        }
        Ok(())
    }
}

/// Project validated roots in canonical alias order using the command-owned current source mapping.
pub(super) fn select_roots_with_sources(
    graph: &NativeLoafGraph,
    dependencies: &[DependencySpec],
    declaration_owner: &Path,
    domain: &str,
    local: &mut LocalSources,
) -> Result<Vec<NativeLoafRoot>> {
    if !matches!(domain, "host" | "target") {
        return Err(refused("declared native roots require host or target domain"));
    }
    let mut roots = Vec::new();
    for demand in declared_root_demands(dependencies, declaration_owner, local)? {
        let mut matches = Vec::new();
        for unit in graph.units.values() {
            if unit.record.source.loaf != demand.loaf {
                continue;
            }
            let origin = super::source_origin(&unit.record.recipe)?;
            if origin != demand.origin || !demand.matches_version(&unit.record.source.version)? {
                continue;
            }
            let root = unit.declared_root(&demand.alias)?;
            if demand.matches(&unit.record.source, origin, &source_manifest(unit)?, domain)? {
                matches.push(root);
            }
        }
        match matches.len() {
            1 => roots.extend(matches),
            0 => {
                return Err(refused(&format!(
                    "native dependency {} has no current declared source binding",
                    demand.dependency.crate_name
                )));
            }
            _ => {
                return Err(refused(&format!(
                    "native dependency {} has ambiguous declared source bindings",
                    demand.dependency.crate_name
                )));
            }
        }
    }
    roots.sort_by(|left, right| left.alias.cmp(&right.alias));
    Ok(roots)
}

/// One authored root demand shared by cold source projection and retained physical owner selection.
pub(super) struct DeclaredRootDemand<'a> {
    pub(super) alias: String,
    pub(super) loaf: String,
    pub(super) origin: NativeLoafOrigin,
    dependency: &'a DependencySpec,
    requirement: Option<semver::VersionReq>,
    authored: Option<(toml::Value, String)>,
}

/// Canonicalize authored aliases and path generations before inspecting any candidate physical output.
pub(super) fn declared_root_demands<'a>(
    dependencies: &'a [DependencySpec],
    declaration_owner: &Path,
    local: &mut LocalSources,
) -> Result<Vec<DeclaredRootDemand<'a>>> {
    let owner = declaration_owner.canonicalize().map_err(failed)?;
    if !owner.is_dir() {
        return Err(refused("native declaration owner must be a directory"));
    }
    let mut aliases = BTreeSet::new();
    let mut demands = Vec::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        if alias.is_empty() || !aliases.insert(alias.clone()) {
            return Err(refused("declared native roots repeat an empty or normalized alias"));
        }
        let requirement = dependency
            .version
            .as_deref()
            .map(semver::VersionReq::parse)
            .transpose()
            .map_err(failed)?;
        let (origin, loaf, authored) = match &dependency.source {
            DependencySource::Registry => (
                NativeLoafOrigin::Registry,
                format!(
                    "crates-io/{}",
                    dependency.package.as_deref().unwrap_or(&dependency.crate_name)
                ),
                None,
            ),
            DependencySource::Path { path } => {
                // Parsed absolute declarations already contain their manifest owner. Raw relative paths are
                // explicitly owner-relative; callers parsing a relative manifest must canonicalize it first.
                let path = owner.join(path).canonicalize().map_err(failed)?;
                let selected = local.get(&path)?;
                let loaf = project_string(&selected.0, "name")?;
                if dependency.package.as_deref().is_some_and(|package| package != loaf) {
                    return Err(refused(
                        "local native declaration package contradicts its current source",
                    ));
                }
                (NativeLoafOrigin::Local, loaf.to_string(), Some(selected.clone()))
            }
            DependencySource::Git { .. } => {
                return Err(refused(
                    "git native declarations require an authenticated commit producer",
                ));
            }
        };
        demands.push(DeclaredRootDemand {
            alias,
            loaf,
            origin,
            dependency,
            requirement,
            authored,
        });
    }
    Ok(demands)
}

impl DeclaredRootDemand<'_> {
    /// Reject incompatible coordinates before reading or hashing a candidate's source declaration.
    pub(super) fn matches_version(&self, version: &str) -> Result<bool> {
        let version = semver::Version::parse(version).map_err(failed)?;
        Ok(self
            .requirement
            .as_ref()
            .is_none_or(|requirement| requirement.matches(&version)))
    }

    /// Apply the original source/version/domain/default-feature contract without granting native ownership.
    pub(super) fn matches(
        &self,
        source: &NativeLoafSource,
        origin: NativeLoafOrigin,
        manifest: &toml::Value,
        domain: &str,
    ) -> Result<bool> {
        if !matches!(domain, "host" | "target") {
            return Err(refused("declared native roots require host or target domain"));
        }
        if source.loaf != self.loaf || origin != self.origin {
            return Ok(false);
        }
        if !self.matches_version(&source.version)? {
            return Ok(false);
        }
        if project_string(manifest, "name")? != source.loaf || project_string(manifest, "version")? != source.version {
            return Err(refused(
                "native source manifest contradicts its producer-bound selection",
            ));
        }
        if let Some((current, digest)) = &self.authored
            && (digest != &source.archive_digest || current != manifest)
        {
            return Ok(false);
        }
        let selected_domain = if manifest
            .get("rust")
            .and_then(|rust| rust.get("type"))
            .and_then(toml::Value::as_str)
            == Some("proc-macro")
        {
            "host"
        } else {
            domain
        };
        let required = crate::sdk_closure::native_required_features(
            manifest,
            &self.dependency.features,
            self.dependency.default_features,
        )
        .map_err(NativeLoafError::Failed)?;
        Ok(source.domain == selected_domain && required.iter().all(|feature| source.features.contains(feature)))
    }
}

/// Reconcile the exact inventoried declaration and reuse its parsed metadata under the original Loaf owner.
pub(crate) fn source_manifest(unit: &SelectedNativeLoaf) -> Result<toml::Value> {
    source_manifest_with_reader(unit, &mut |path| std::fs::read(path).map_err(failed))
}

/// Parse one admitted declaration per retained owner; the reader records actual reads, not retained lookups.
///
/// A retained syntax tree is usable only while the existing file observer verifies its immutable member digest.
/// Changed bytes, replacements, missing files and source-root links never inherit the previous declaration.
pub(super) fn source_manifest_with_reader(
    unit: &SelectedNativeLoaf,
    read: &mut dyn FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<toml::Value> {
    if let Some(manifest) = unit.source_declaration.get() {
        verify_native_source_declaration(&unit.native_owner)?;
        tracing::trace!(target: "incan::invocation", decision = "reuse_native_declaration", input = %unit.identity());
        return Ok(manifest.clone());
    }
    let bytes = source_declaration_bytes(&unit.native_owner, read)?;
    let manifest = toml::from_str(std::str::from_utf8(&bytes).map_err(failed)?).map_err(failed)?;
    let _already_initialized = unit.source_declaration.set(manifest);
    unit.source_declaration
        .get()
        .cloned()
        .ok_or_else(|| refused("native source declaration was not retained"))
}

/// Retain declaration integrity on native handoffs using the existing replacement-sensitive file observer.
pub(crate) fn verify_native_source_declaration(owner: &oven_store::store::OvenStoreExecutionPayload) -> Result<()> {
    if let Some(member) = owner
        .admitted_materialized_files()
        .iter()
        .find(|member| member.relative_path == "source/loaf.toml")
    {
        let (length, digest) = owner
            .digest_materialized_member(&member.relative_path)
            .map_err(failed)?;
        if length != member.logical_bytes || digest != member.digest {
            return Err(refused("native source declaration changed after admission"));
        }
    }
    Ok(())
}

/// Read the exact declaration and match its admitted byte digest before retaining any parsed metadata.
fn source_declaration_bytes(
    owner: &oven_store::store::OvenStoreExecutionPayload,
    read: &mut dyn FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let (path, member) = source_declaration_member(owner)?;
    let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_rustc::read_native_source_declaration", input = %path.display()).entered();
    let bytes = read(&path)?;
    if u64::try_from(bytes.len()).ok() != Some(member.logical_bytes)
        || oven_store::digest_bytes(&bytes) != member.digest
    {
        return Err(refused("native source declaration changed after admission"));
    }
    tracing::trace!(target: "incan::invocation", declaration_bytes_read = member.logical_bytes, declaration_reads = 1_u64);
    Ok(bytes)
}

/// Require the original plain source directory and exact inventoried non-link declaration before observation.
fn source_declaration_member(
    owner: &oven_store::store::OvenStoreExecutionPayload,
) -> Result<(PathBuf, &oven_store::store::OvenArtifactMaterializedFileManifest)> {
    let source_root = owner.artifact_root.join("source");
    let source_metadata = std::fs::symlink_metadata(&source_root).map_err(failed)?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(refused(
            "native source declaration requires its original plain source directory",
        ));
    }
    let member = owner
        .admitted_materialized_files()
        .iter()
        .find(|member| member.relative_path == "source/loaf.toml")
        .ok_or_else(|| refused("native owner has no admitted source declaration"))?;
    let path = owner.artifact_root.join(&member.relative_path);
    if !std::fs::symlink_metadata(&path).map_err(failed)?.is_file() {
        return Err(refused("native source declaration is not a plain file"));
    }
    Ok((path, member))
}

/// Require exact authored project fields instead of inferring them from filenames or Rust crate aliases.
fn project_string<'a>(manifest: &'a toml::Value, key: &str) -> Result<&'a str> {
    manifest
        .get("project")
        .and_then(|project| project.get(key))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| refused("native source declaration lacks exact project identity"))
}
