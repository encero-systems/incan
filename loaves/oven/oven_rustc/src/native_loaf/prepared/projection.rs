//! Root cold physical preparation in current declarations, retaining the original complete candidate world.

use super::{Current, NativeLoafConsumerRequest};
use crate::native_loaf::preparation::{NativeLoafPreparationRequest, prepare_native_loafs_in_store};
use crate::native_loaf::selection::declared_root_demands;
use crate::native_loaf::{
    NativeLoafError, NativeLoafOrigin, NativeLoafPreparation, NativeLoafSource, Result, failed, refused,
};
use crate::sdk_closure::current_inputs;
use oven_store::store::OvenStore;
use std::collections::BTreeSet;

/// Candidate coordinates supply selection facts only; they confer no native, inspection or macro authority.
#[derive(Clone, Copy)]
enum Origin {
    Registry(usize),
    Local(usize),
}

/// Lazily mapped source facts from the original graph, independent of its eventual compiled native generation.
struct Candidate {
    origin: Origin,
    loaf: String,
    version: String,
    domain: String,
    features: Vec<String>,
    manifest: Option<toml::Value>,
    source_digest: Option<String>,
}

/// Root a cold consumer before compiling, then discard the projected request's inspection association.
///
/// The complete producer API remains unchanged: it still observes every unit it is supplied. Here its private
/// request contains only the source-authorized physical closure. Returning those owners cannot grant complete
/// semantic authority for the consumer's broader graph. The caller rechecks its original request key afterward.
pub(super) fn prepare(
    request: &NativeLoafConsumerRequest<'_>,
    store: &OvenStore,
    current: &mut Current,
) -> Result<NativeLoafPreparation> {
    let mut candidates = catalog(current)?;
    let selected = selected_candidates(request, current, &mut candidates)?;
    let registry = selected
        .iter()
        .filter_map(|index| match candidates[*index].origin {
            Origin::Registry(index) => Some(index),
            Origin::Local(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let units = registry
        .iter()
        .map(|index| &current.seed.units[*index])
        .collect::<Vec<_>>();
    let codegen = current.seed.unit_codegen.iter().filter(|selection| units.iter().any(|unit| {
        unit.loaf == selection.loaf && unit.version == selection.version && unit.domain == selection.domain
    })).map(|selection| serde_json::json!({"loaf":selection.loaf,"version":selection.version,"domain":selection.domain,"options":selection.options})).collect::<Vec<_>>();
    let facets = selected
        .iter()
        .filter_map(|index| match candidates[*index].origin {
            Origin::Local(index) => Some(current.graph.facets[index].clone()),
            Origin::Registry(_) => None,
        })
        .collect::<Vec<_>>();
    std::fs::create_dir_all(request.output).map_err(failed)?;
    let temporary = tempfile::Builder::new()
        .prefix("native-consumer-")
        .tempdir_in(request.output)
        .map_err(failed)?;
    let lock = temporary.path().join("lock.json");
    std::fs::write(
        &lock,
        serde_json::to_vec(&serde_json::json!({"schema":current.seed.schema,"units":units,"unit_codegen":codegen}))
            .map_err(failed)?,
    )
    .map_err(failed)?;
    let mut prepared = prepare_native_loafs_in_store(
        &NativeLoafPreparationRequest {
            lock: &lock,
            blobs: request.blobs,
            output: request.output,
            rustc: request.rustc,
            index: request.index,
            index_commit: &current.graph.index_commit,
            target: request.target,
            profile: request.profile,
            facet_owner: &current.owner,
            facets: &facets,
        },
        store,
    )?;
    prepared.observation = None;
    Ok(prepared)
}

/// Enumerate original package identities without hashing unrelated local source trees or reading their archives.
fn catalog(current: &Current) -> Result<Vec<Candidate>> {
    let mut candidates = current
        .seed
        .units
        .iter()
        .enumerate()
        .map(|(index, unit)| Candidate {
            origin: Origin::Registry(index),
            loaf: unit.loaf.clone(),
            version: unit.version.clone(),
            domain: unit.domain.clone(),
            features: unit.features.clone(),
            manifest: None,
            source_digest: Some(unit.archive_digest.clone()),
        })
        .collect::<Vec<_>>();
    for (index, facet) in current.graph.facets.iter().enumerate() {
        let manifest: toml::Value = toml::from_str(
            &std::fs::read_to_string(current.owner.join(&facet.project).join("loaf.toml")).map_err(failed)?,
        )
        .map_err(failed)?;
        let project = manifest
            .get("project")
            .ok_or_else(|| refused("current local candidate has no project"))?;
        let field = |key| {
            project
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| refused("current local candidate lacks exact project identity"))
        };
        candidates.push(Candidate {
            origin: Origin::Local(index),
            loaf: field("name")?,
            version: field("version")?,
            domain: facet.domain.clone(),
            features: Vec::new(),
            manifest: None,
            source_digest: None,
        });
    }
    Ok(candidates)
}

/// Apply the shared root contract and original producer edge selectors before acquiring any native owner.
fn selected_candidates(
    request: &NativeLoafConsumerRequest<'_>,
    current: &mut Current,
    candidates: &mut [Candidate],
) -> Result<BTreeSet<usize>> {
    if current.seed.schema != "incan.oven.loaf-resolution/2" {
        return Err(refused(
            "cold native consumer projection requires authenticated resolution edges",
        ));
    }
    let demands = declared_root_demands(request.dependencies, request.declaration_owner, &mut current.local)?;
    let mut pending = Vec::new();
    for demand in demands {
        let mut matching = Vec::new();
        for (index, candidate) in candidates.iter_mut().enumerate() {
            if candidate.loaf != demand.loaf
                || candidate.native_origin() != demand.origin
                || !demand.matches_version(&candidate.version)?
            {
                continue;
            }
            candidate.load(request, current)?;
            if demand.matches(
                &candidate.source(current)?,
                candidate.native_origin(),
                candidate.manifest()?,
                request.domain,
            )? {
                matching.push(index);
            }
        }
        match matching.as_slice() {
            [index] => pending.push(*index),
            [] => {
                return Err(refused(&format!(
                    "native dependency {} has no current declared source binding",
                    demand.alias
                )));
            }
            _ => {
                return Err(refused(&format!(
                    "native dependency {} has ambiguous declared source bindings",
                    demand.alias
                )));
            }
        }
    }
    let mut selected = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if !selected.insert(index) {
            continue;
        }
        candidates[index].load(request, current)?;
        match candidates[index].origin {
            Origin::Registry(registry) => {
                let edges = current.seed.units[registry]
                    .edges
                    .as_ref()
                    .ok_or_else(|| refused("current native resolution has no edges"))?;
                for edge in edges {
                    let bindings = current
                        .seed
                        .units
                        .iter()
                        .enumerate()
                        .filter(|(_, unit)| {
                            unit.loaf == edge.loaf && unit.version == edge.version && unit.domain == edge.domain
                        })
                        .map(|(index, _)| index)
                        .collect::<Vec<_>>();
                    match bindings.as_slice() {
                        [index] => pending.push(*index),
                        _ => return Err(refused("current native resolution edge is missing or ambiguous")),
                    }
                }
            }
            Origin::Local(_) => {
                let domain = candidates[index].domain.clone();
                let demands = current_inputs::local_dependency_demands(
                    candidates[index].manifest()?,
                    &candidates[index].features,
                )
                .map_err(NativeLoafError::Failed)?;
                for demand in demands {
                    let mut indices = Vec::new();
                    for (index, candidate) in candidates.iter_mut().enumerate() {
                        if candidate.loaf == demand.loaf {
                            candidate.load(request, current)?;
                            indices.push(index);
                        }
                    }
                    let bindings = indices
                        .iter()
                        .map(|index| candidates[*index].binding())
                        .collect::<Result<Vec<_>>>()?;
                    let child = current_inputs::select_local_dependency(&demand, &domain, &bindings)
                        .map_err(NativeLoafError::Failed)?;
                    pending.push(indices[child]);
                }
            }
        }
    }
    Ok(selected)
}

impl Candidate {
    /// Preserve archive-versus-current-source provenance while matching authored root declarations.
    fn native_origin(&self) -> NativeLoafOrigin {
        match self.origin {
            Origin::Registry(_) => NativeLoafOrigin::Registry,
            Origin::Local(_) => NativeLoafOrigin::Local,
        }
    }

    /// Read only demanded source facts, sharing actual local source observations with the caller's replay checks.
    fn load(&mut self, request: &NativeLoafConsumerRequest<'_>, current: &mut Current) -> Result<()> {
        if self.manifest.is_some() {
            return Ok(());
        }
        let manifest = match self.origin {
            Origin::Registry(index) => {
                current_inputs::registry_source_manifest(request.blobs, &current.seed.units[index])
                    .map_err(NativeLoafError::Failed)?
            }
            Origin::Local(index) => {
                let facet = &current.graph.facets[index];
                let (manifest, digest) = current.local.get(&current.owner.join(&facet.project))?;
                self.source_digest = Some(digest.clone());
                self.features = crate::sdk_closure::native_required_features(manifest, &facet.features, false)
                    .map_err(NativeLoafError::Failed)?
                    .into_iter()
                    .collect();
                manifest.clone()
            }
        };
        self.manifest = Some(manifest);
        Ok(())
    }

    /// Require real current source mapping before asking any producer selector to consume its facts.
    fn manifest(&self) -> Result<&toml::Value> {
        self.manifest
            .as_ref()
            .ok_or_else(|| refused("native candidate source manifest is unavailable"))
    }

    /// Reproduce exactly the source binding that the unchanged native producer will seal after compilation.
    fn source(&self, current: &Current) -> Result<NativeLoafSource> {
        match self.origin {
            Origin::Registry(index) => Ok(crate::sdk_closure::ordinary_native_source(&current.seed.units[index])),
            Origin::Local(_) => Ok(NativeLoafSource {
                loaf: self.loaf.clone(),
                version: self.version.clone(),
                domain: self.domain.clone(),
                features: self.features.clone(),
                target_predicates: Vec::new(),
                archive_digest: self
                    .source_digest
                    .clone()
                    .ok_or_else(|| refused("native candidate current source generation is unavailable"))?,
            }),
        }
    }

    /// Borrow the same domain/version/feature facts used by native production and unchanged current-input replay.
    fn binding(&self) -> Result<current_inputs::LocalDependencyCandidate<'_>> {
        Ok(current_inputs::LocalDependencyCandidate {
            loaf: &self.loaf,
            version: &self.version,
            domain: &self.domain,
            features: &self.features,
            proc_macro: self
                .manifest()?
                .get("rust")
                .and_then(|rust| rust.get("type"))
                .and_then(toml::Value::as_str)
                == Some("proc-macro"),
        })
    }
}
