//! Declaration-authorized root projection over producer-authenticated native records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oven_model::manifest::{DependencySource, DependencySpec};

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

/// Command-local mapped source observations shared by current-graph validation and declaration projection.
#[derive(Default)]
pub(super) struct LocalSources {
    selections: BTreeMap<PathBuf, (toml::Value, String)>,
    /// Actual mapping/hash demands; cache hits do not increment this counter.
    pub(super) loads: usize,
}

impl LocalSources {
    /// Reproduce each canonical current source mapping once during this command, never across commands.
    pub(super) fn get(&mut self, path: &Path) -> Result<&(toml::Value, String)> {
        let path = path.canonicalize().map_err(failed)?;
        if !self.selections.contains_key(&path) {
            let selected = crate::sdk_closure::local_native_source_selection(&path).map_err(NativeLoafError::Failed)?;
            self.loads += 1;
            self.selections.insert(path.clone(), selected);
        }
        self.selections
            .get(&path)
            .ok_or_else(|| refused("current local source selection is missing"))
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
            unit.verify()?;
            if demand.matches(&unit.record.source, origin, &source_manifest(unit)?, domain)? {
                matches.push(unit);
            }
        }
        match matches.as_slice() {
            [unit] => roots.push(unit.declared_root(&demand.alias)?),
            [] => {
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
        if let Some((current, digest)) = &self.authored {
            if digest != &source.archive_digest || current != manifest {
                return Ok(false);
            }
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

/// Read only an exact inventoried source declaration and verify the bytes used for semantic root projection.
pub(crate) fn source_manifest(unit: &SelectedNativeLoaf) -> Result<toml::Value> {
    source_manifest_with_reader(unit, &mut |path| std::fs::read(path).map_err(failed))
}

/// Share exact declaration admission while allowing command-owned observation of actual file-read attempts.
pub(super) fn source_manifest_with_reader(
    unit: &SelectedNativeLoaf,
    read: &mut dyn FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<toml::Value> {
    let member = unit
        .native_owner
        .admitted_materialized_files()
        .iter()
        .find(|member| member.relative_path == "source/loaf.toml")
        .ok_or_else(|| refused("native owner has no admitted source declaration"))?;
    let path = unit.native_owner.artifact_root.join(&member.relative_path);
    if !std::fs::symlink_metadata(&path).map_err(failed)?.is_file() {
        return Err(refused("native source declaration is not a plain file"));
    }
    let bytes = read(&path)?;
    if bytes.len() as u64 != member.logical_bytes || oven_store::digest_bytes(&bytes) != member.digest {
        return Err(refused("native source declaration changed after admission"));
    }
    toml::from_str(std::str::from_utf8(&bytes).map_err(failed)?).map_err(failed)
}

/// Require exact authored project fields instead of inferring them from filenames or Rust crate aliases.
fn project_string<'a>(manifest: &'a toml::Value, key: &str) -> Result<&'a str> {
    manifest
        .get("project")
        .and_then(|project| project.get(key))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| refused("native source declaration lacks exact project identity"))
}
