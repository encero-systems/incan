//! Ordinary library native preparation, retaining complete original producer requests for checked metadata.
//!
//! This adapter uses the existing resolver output and ordinary native producer. Physical roots remain a separate
//! projection: selecting fewer linked units never narrows the complete request used by checked metadata.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::build::library_project::metadata_replay::ordinary_native::OrdinaryNativeMetadataAuthority;
use crate::build::ordinary_support::CompilerSupportSources;
use crate::error::{CliError, CliResult};
use oven_model::manifest::DependencySpec;
#[cfg(test)]
use oven_rustc::native_loaf::NativeLoafPreparationReport;
use oven_rustc::native_loaf::{NativeLoafClosure, prepare_resolved_native_loafs_in_store};
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_store::store::OvenStore;
use oven_store::{OvenBuildIntent, OvenReceipt};

/// Explicit existing resolver output and native producer coordinates for one ordinary library command.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "#1698 ordinary library publication: only its tests call it until the ordinary command cutover does"
    )
)]
pub(crate) struct OrdinaryLibraryNativeRequest<'a> {
    pub support: Arc<CompilerSupportSources>,
    pub graph: &'a Path,
    pub index: &'a Path,
    pub blobs: &'a Path,
    pub output: &'a Path,
    pub rustc: &'a Path,
    pub target: &'a str,
    pub profiles: &'a [&'a str],
}

/// Complete original native metadata authority and measured per-profile production outcomes.
///
/// Constructed only from the actual producer. A caller still needs checked semantic demand coverage before this
/// capability can prepare library metadata or run compiler macros.
pub(crate) struct OrdinaryLibraryNativeProfiles {
    metadata: Arc<OrdinaryNativeMetadataAuthority>,
    own_facet: Option<StandardSourceNativeFacet>,
    vocabulary: Option<Arc<super::library_project::standard_vocabulary::PreparedStandardVocabulary>>,
    /// Declared native facets of this session's original ordinary standard namespace owners.
    namespace_facets: Vec<StandardSourceNativeFacet>,
    #[cfg(test)]
    reports: Arc<BTreeMap<String, NativeLoafPreparationReport>>,
}

/// A standard publisher's original source issuer paired with its declared physical Rust facet request.
#[derive(Clone)]
struct StandardSourceNativeFacet {
    publisher: Arc<incan_frontend::provider::source_policy::TrustedStandardSourcePublication>,
    dependency: DependencySpec,
}

impl StandardSourceNativeFacet {
    /// Revalidate the original issuer before returning its private, constructor-bound physical request.
    fn verified_dependency(&self) -> CliResult<&DependencySpec> {
        self.publisher.verify().map_err(failure)?;
        Ok(&self.dependency)
    }
}

impl OrdinaryLibraryNativeProfiles {
    /// Prepare requested profiles in the shared command Store, retaining complete requests without SDK discovery.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "#1698 ordinary library publication: only its tests call it until the ordinary command cutover does"
        )
    )]
    pub(crate) fn prepare(request: OrdinaryLibraryNativeRequest<'_>) -> CliResult<Self> {
        if request.profiles.is_empty()
            || request
                .profiles
                .iter()
                .any(|profile| !matches!(*profile, "debug" | "release"))
        {
            return Err(failure(
                "ordinary library native profiles must explicitly select debug or release",
            ));
        }
        if request.profiles.iter().collect::<std::collections::BTreeSet<_>>().len() != request.profiles.len() {
            return Err(failure("ordinary library native profile is duplicated"));
        }
        request.support.verify()?;
        let toolchain = oven_rustc::rustc::rustc_identity(request.rustc).map_err(failure)?;
        let mut observations = BTreeMap::new();
        #[cfg(test)]
        let mut reports = BTreeMap::new();
        let store = crate::oven_store::open_default_oven_store()?;
        for profile in request.profiles {
            let prepared = prepare_resolved_native_loafs_in_store(
                request.graph,
                request.index,
                request.blobs,
                request.output,
                request.rustc,
                request.target,
                profile,
                &store,
            )
            .map_err(failure)?;
            observations.insert((*profile).to_string(), prepared.request_observation().map_err(failure)?);
            #[cfg(test)]
            reports.insert(
                (*profile).to_string(),
                NativeLoafPreparationReport {
                    index_reads: prepared.report().index_reads,
                    compiled: prepared.report().compiled.clone(),
                    reused: prepared.report().reused.clone(),
                    seconds: prepared.report().seconds,
                },
            );
        }
        let metadata = Arc::new(OrdinaryNativeMetadataAuthority::new(
            request.support,
            observations,
            request.rustc,
            request.target,
            &toolchain,
        )?);
        Ok(Self {
            metadata,
            own_facet: None,
            vocabulary: None,
            namespace_facets: Vec::new(),
            #[cfg(test)]
            reports: Arc::new(reports),
        })
    }

    /// Bind this session's own standard Rust facet without preparing another graph or adding a logical self-edge.
    ///
    /// Only the original standard source issuer supplies this implicit physical root. Normal consumer aliases and
    /// checked Incan package edges stay unchanged; every profile still selects the facet from the full native request.
    pub(crate) fn for_session(self: &Arc<Self>, session: &crate::session::CompilationSession) -> CliResult<Arc<Self>> {
        let mut namespace_facets = Vec::new();
        for record in session.provider_plan().active_sdk_records() {
            if !matches!(
                record.provenance,
                incan_provider::ProviderProvenance::StandardPublication { .. }
            ) {
                continue;
            }
            let namespace = incan_frontend::provider::namespaces::SelectedProviderNamespace::from_plan(
                session.provider_plan(),
                &record.identity.stable_key(),
            )
            .map_err(failure)?;
            let publisher = Arc::new(
                incan_frontend::provider::source_policy::TrustedStandardSourcePublication::discover(
                    &record.identity.name,
                )
                .map_err(failure)?
                .ok_or_else(|| failure("ordinary namespace has no original standard source"))?,
            );
            let root = publisher.verified_package_root().map_err(failure)?;
            if namespace
                .record()
                .artifact
                .as_ref()
                .and_then(|artifact| {
                    incan_frontend::library_manifest_index::dependency_project_root(&artifact.crate_root)
                })
                .as_deref()
                != Some(root)
            {
                return Err(failure(
                    "ordinary namespace native source differs from its original publication",
                ));
            }
            let Some(name) = publisher.verified_native_facet_name().map_err(failure)? else {
                continue;
            };
            let (package, version) = publisher.package_identity();
            let dependency = DependencySpec {
                crate_name: name,
                package: Some(package.to_string()),
                version: Some(format!("={version}")),
                features: record.identity.feature_projection.iter().cloned().collect(),
                default_features: false,
                optional: false,
                source: oven_model::manifest::DependencySource::Path {
                    path: root.to_path_buf(),
                },
            };
            namespace_facets.push(StandardSourceNativeFacet { publisher, dependency });
        }
        if session.provider_plan().standard_source_publication().is_none()
            && self.own_facet.is_none()
            && namespace_facets.is_empty()
            && self.namespace_facets.is_empty()
        {
            return Ok(Arc::clone(self));
        }
        let own_facet = session
            .provider_plan()
            .standard_source_publication()
            .map(|publisher| {
                let Some(name) = publisher.verified_native_facet_name().map_err(failure)? else {
                    return Ok(None);
                };
                let root = publisher.verified_package_root().map_err(failure)?;
                let manifest_root = session
                    .manifest
                    .as_ref()
                    .ok_or_else(|| failure("standard native source session has no manifest"))?
                    .project_root()
                    .canonicalize()
                    .map_err(failure)?;
                if root != manifest_root {
                    return Err(failure("standard native source facet belongs to another session"));
                }
                let (package, version) = publisher.package_identity();
                let features = session
                    .package_feature_plan()
                    .as_ref()
                    .and_then(|plan| plan.root_package())
                    .ok_or_else(|| failure("standard native source session has no feature selection"))?
                    .features
                    .active_features
                    .iter()
                    .cloned()
                    .collect();
                Ok::<_, CliError>(Some(StandardSourceNativeFacet {
                    publisher: Arc::clone(publisher),
                    dependency: DependencySpec {
                        crate_name: name,
                        package: Some(package.to_string()),
                        version: Some(format!("={version}")),
                        features,
                        default_features: true,
                        optional: false,
                        source: oven_model::manifest::DependencySource::Path {
                            path: root.to_path_buf(),
                        },
                    },
                }))
            })
            .transpose()?
            .flatten();
        Ok(Arc::new(Self {
            metadata: Arc::clone(&self.metadata),
            own_facet,
            vocabulary: self.vocabulary.clone(),
            namespace_facets,
            #[cfg(test)]
            reports: Arc::clone(&self.reports),
        }))
    }

    /// Keep the explicit vocabulary producer beside original native profiles without replacing their request owners.
    pub(super) fn retaining_vocabulary(
        &self,
        vocabulary: Arc<super::library_project::standard_vocabulary::PreparedStandardVocabulary>,
    ) -> CliResult<Arc<Self>> {
        self.verify()?;
        vocabulary.verify()?;
        if self.vocabulary.is_some() {
            return Err(failure("ordinary context already retains a vocabulary producer"));
        }
        Ok(Arc::new(Self {
            metadata: Arc::clone(&self.metadata),
            own_facet: self.own_facet.clone(),
            namespace_facets: self.namespace_facets.clone(),
            vocabulary: Some(vocabulary),
            #[cfg(test)]
            reports: Arc::clone(&self.reports),
        }))
    }

    /// Borrow the originally supplied vocabulary capability for checked-demand and canonical lock handoffs.
    pub(crate) fn prepared_vocabulary(
        &self,
    ) -> Option<&Arc<super::library_project::standard_vocabulary::PreparedStandardVocabulary>> {
        self.vocabulary.as_ref()
    }

    /// Borrow the same original complete semantic request through selection, replay and publication handoffs.
    pub(crate) fn metadata(&self) -> &Arc<OrdinaryNativeMetadataAuthority> {
        &self.metadata
    }

    /// Observe real compile/reuse work without synthesizing successful preparation from cached output paths.
    #[cfg(test)]
    pub(crate) fn reports(&self) -> &BTreeMap<String, NativeLoafPreparationReport> {
        &self.reports
    }

    /// Revalidate every original request and mandatory support declaration without reacquiring owners.
    #[track_caller]
    pub(crate) fn verify(&self) -> CliResult<()> {
        self.metadata.verify()?;
        if let Some(vocabulary) = &self.vocabulary {
            vocabulary.verify()?;
        }
        for own in self.own_facet.iter().chain(&self.namespace_facets) {
            own.verified_dependency()?;
        }
        Ok(())
    }

    /// Bind a canonical normal/dev dependency surface to every original profile before its lock is published.
    /// Root selection checks current declarations and selected physical owners, but grants no execution or publication
    /// authority. Verify the complete original requests once after that tentative projection, including profiles and
    /// units the declarations do not select, before returning success.
    pub(crate) fn verify_dependencies(&self, dependencies: &[DependencySpec], owner: &Path) -> CliResult<()> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::verify_ordinary_dependency_projection", dependencies = dependencies.len(), profiles = self.metadata.observations().len()).entered();
        #[cfg(test)]
        let requests_before =
            super::library_project::metadata_replay::ordinary_native::metadata_request_verifications();
        for observation in self.metadata.observations().values() {
            let graph = observation.graph();
            let roots = graph
                .select_dependency_roots(dependencies, owner, "target")
                .map_err(|error| {
                    failure(format!(
                        "cannot verify ordinary declared dependencies {dependencies:?} for {}: {error}",
                        owner.display()
                    ))
                })?;
            graph.select(&roots).map_err(failure)?;
        }
        self.verify()?;
        #[cfg(test)]
        assert_eq!(
            super::library_project::metadata_replay::ordinary_native::metadata_request_verifications()
                - requests_before,
            self.metadata.observations().len(),
            "dependency projection must validate each complete original profile once before returning",
        );
        Ok(())
    }

    /// Check provider semantics against the original mandatory support declarations.
    ///
    /// A source import can explicitly name an already mandatory native facet. This does not introduce another
    /// dependency or grant a namespace: the full requests already bind its original compiler-owned source.
    /// Other facets require original checked dependency requirements or the retained standard publisher's own
    /// compiler-pinned declaration. Native selection still verifies each demanded physical facet independently.
    pub(crate) fn provider_semantic_dependencies(
        &self,
        plan: &incan_provider::ProviderPlan,
        requirements: &incan_provider::requirements::ProjectRequirements,
        admitted: Option<&super::library_dependencies::PreparedLibraryDependencies>,
    ) -> CliResult<Vec<DependencySpec>> {
        let permitted = match admitted {
            Some(dependencies) => dependencies.projected_requirements(plan)?,
            None if plan.records().next().is_none() => incan_provider::requirements::ProjectRequirements::default(),
            None => {
                return Err(failure(
                    "ordinary provider semantics require original checked dependency owners",
                ));
            }
        };
        let support = self.metadata.support().dependencies()?;
        let own_facet = plan
            .standard_source_publication()
            .map(|source| source.verified_native_facet_name().map_err(failure))
            .transpose()?
            .flatten();
        let unadmitted_facets = requirements
            .stdlib_facets
            .iter()
            .filter(|facet| {
                !incan_lang::lang::stdlib::facets::is_facet(facet)
                    || !(support.iter().any(|dependency| dependency.crate_name == **facet)
                        || permitted.stdlib_facets.contains(facet)
                        || self
                            .namespace_facets
                            .iter()
                            .any(|source| source.dependency.crate_name == **facet)
                        || own_facet.as_ref() == Some(*facet))
            })
            .collect::<Vec<_>>();
        let matches = incan_provider::requirements::dependency_specs_match;
        let additional = requirements
            .dependencies
            .iter()
            .filter(|required| !permitted.dependencies.iter().any(|allowed| matches(required, allowed)))
            .cloned()
            .collect::<Vec<_>>();
        let mut dependency_projection_matches = permitted.dependencies.iter().all(|allowed| {
            requirements
                .dependencies
                .iter()
                .any(|required| matches(required, allowed))
        });
        if !additional.is_empty() {
            if let Some(source) = plan.standard_source_publication() {
                let root = source.verified_package_root().map_err(failure)?;
                let declaration =
                    std::str::from_utf8(source.verified_declaration_bytes().map_err(failure)?).map_err(failure)?;
                let manifest = oven_model::manifest::ProjectManifest::from_str(declaration, &root.join("loaf.toml"))
                    .map_err(failure)?;
                let declared = manifest.rust_dependency_values();
                dependency_projection_matches &= additional
                    .iter()
                    .all(|required| declared.iter().any(|allowed| matches(required, allowed)));
                // A source requirement such as serde still needs the exact original native root and complete
                // producer request. The pinned declaration alone grants no physical or semantic ABI authority.
                if dependency_projection_matches {
                    self.verify_dependencies(&additional, root)?;
                }
            } else {
                dependency_projection_matches = false;
            }
        }
        let legacy_projection_empty = requirements.sdk_dependency_rebindings.is_empty()
            && requirements.sdk_path_dependencies.is_empty()
            && requirements.sdk_artifact_projections.is_empty();
        if !unadmitted_facets.is_empty() || !dependency_projection_matches || !legacy_projection_empty {
            return Err(failure(format!(
                "ordinary mandatory support does not cover the requested provider, facet or dependency projections: unadmitted facets {unadmitted_facets:?}, dependency projection matches {dependency_projection_matches}, legacy projection empty {legacy_projection_empty}, additional source dependencies {additional:?}"
            )));
        }
        Ok(Vec::new())
    }

    /// Bind source inspection to explicit dependencies, original publisher path companions, and mandatory support.
    /// The generic import collector omits registered companions. Only the verified publisher declaration restores
    /// their exact requests here; a query name or forward native membership cannot grant a new root alias.
    pub(crate) fn inspection_dependencies(
        &self,
        declared: &[DependencySpec],
        owner: &Path,
    ) -> CliResult<Vec<DependencySpec>> {
        let (profile, _) = self.metadata.inspection_request()?;
        let intent = OvenBuildIntent {
            target: self.metadata.target().to_string(),
            toolchain: oven_rustc::rustc::rustc_identity(self.metadata.rustc()).map_err(failure)?,
            profile: profile.to_string(),
            features: Vec::new(),
        };

        let mut dependencies = self.source_dependencies(declared)?;

        // ---- Exact physical selection before implicit support aliases ----
        self.closure(&intent, &dependencies, owner)?;
        for support in self.metadata.support().dependencies()? {
            if !dependencies
                .iter()
                .any(|dependency| dependency.crate_name == support.crate_name)
            {
                dependencies.push(support.clone());
            }
        }
        for own in self.own_facet.iter().chain(&self.namespace_facets) {
            let own = own.verified_dependency()?;
            if !dependencies
                .iter()
                .any(|dependency| dependency.crate_name == own.crate_name)
            {
                dependencies.push(own.clone());
            }
        }
        Ok(dependencies)
    }

    /// Restore original publisher path companions omitted by the generic import collector.
    /// Inspection and executable closures must use the same pinned declaration and namespace registry. This only
    /// projects requests: callers still validate every physical root against the original native producer graph.
    fn source_dependencies(&self, declared: &[DependencySpec]) -> CliResult<Vec<DependencySpec>> {
        let mut dependencies = declared.to_vec();

        // ---- Original publisher declaration ----
        if let Some(own) = &self.own_facet {
            let publisher = &own.publisher;
            let root = publisher.verified_package_root().map_err(failure)?;
            let declaration =
                std::str::from_utf8(publisher.verified_declaration_bytes().map_err(failure)?).map_err(failure)?;
            let manifest = oven_model::manifest::ProjectManifest::from_str(declaration, &root.join("loaf.toml"))
                .map_err(failure)?;

            // ---- Declared namespace companions and exact alias consistency ----
            for companion in manifest.rust_dependency_values().into_iter().filter(|dependency| {
                incan_lang::lang::stdlib::is_path_extra_crate_dep(&dependency.crate_name)
                    && publisher.namespace_roots().iter().any(|root| {
                        incan_lang::lang::stdlib::find_namespace(root).is_some_and(|namespace| {
                            namespace
                                .extra_crate_deps
                                .iter()
                                .any(|companion| companion.crate_name == dependency.crate_name)
                        })
                    })
            }) {
                if let Some(existing) = dependencies
                    .iter()
                    .find(|dependency| dependency.crate_name == companion.crate_name)
                {
                    if !incan_provider::requirements::dependency_specs_match(existing, &companion) {
                        return Err(failure(format!(
                            "ordinary dependency `{}` conflicts with its original publisher declaration",
                            companion.crate_name
                        )));
                    }
                } else {
                    dependencies.push(companion);
                }
            }
        }

        Ok(dependencies)
    }

    /// Select authored aliases, original publisher companions, and mandatory support from the original graph.
    fn closure(
        &self,
        intent: &OvenBuildIntent,
        dependencies: &[DependencySpec],
        owner: &Path,
    ) -> CliResult<NativeLoafClosure> {
        // ---- Original declaration and mandatory physical roots ----
        let dependencies = self.source_dependencies(dependencies)?;
        let observation = self.metadata.verify_profile(intent).map_err(|error| {
            failure(format!(
                "cannot verify ordinary native profile for {}: {error}",
                owner.display()
            ))
        })?;
        let graph = observation.graph();
        let support = self.metadata.support();
        let mut roots = graph
            .select_dependency_roots(support.dependencies()?, support.declaration_owner()?, "target")
            .map_err(|error| failure(format!("cannot select mandatory ordinary support: {error}")))?;
        for own in self.own_facet.iter().chain(&self.namespace_facets) {
            let own = own.verified_dependency()?;
            for root in graph
                .select_dependency_roots(std::slice::from_ref(own), owner, "target")
                .map_err(failure)?
            {
                if let Some(original) = roots.iter().find(|original| original.alias == root.alias) {
                    if original.record_identity != root.record_identity {
                        return Err(failure(
                            "standard publisher's own native facet conflicts with mandatory support",
                        ));
                    }
                } else {
                    roots.push(root);
                }
            }
        }

        // ---- Exact authored and publisher companion aliases ----
        for root in graph
            .select_dependency_roots(&dependencies, owner, "target")
            .map_err(|error| {
                failure(format!(
                    "cannot select ordinary native declarations {dependencies:?} for {}: {error}",
                    owner.display()
                ))
            })?
        {
            if let Some(original) = roots.iter().find(|original| original.alias == root.alias) {
                if original.record_identity != root.record_identity {
                    return Err(failure(format!(
                        "ordinary library dependency `{}` conflicts with mandatory support",
                        root.alias
                    )));
                }
            } else {
                roots.push(root);
            }
        }
        graph.select(&roots).map_err(failure)
    }

    /// Project per-profile physical inputs through the compiler-bound Incan identity engine.
    pub(crate) fn runtime_inputs(
        &self,
        intent: &OvenBuildIntent,
        providers: &[String],
        facets: &[String],
        dependencies: &[DependencySpec],
        physical_dependencies: &[DependencySpec],
        owner: &Path,
    ) -> CliResult<BTreeMap<String, String>> {
        let closure = self.closure(intent, physical_dependencies, owner)?;
        let inputs = super::native_runtime_inputs::ordinary_runtime_inputs(
            &closure,
            intent,
            self.metadata.rustc(),
            providers,
            facets,
            dependencies,
        )?;
        self.metadata.verify_profile(intent)?;
        Ok(inputs)
    }

    /// Bind an actual current consumer receipt to the same original physical roots used for its runtime inputs.
    pub(crate) fn select_plan(
        &self,
        store: &OvenStore,
        receipt: &OvenReceipt,
        dependencies: &[DependencySpec],
        owner: &Path,
    ) -> CliResult<(OvenReceipt, OvenDirectRustcPlanSelection)> {
        let closure = self.closure(&receipt.intent, dependencies, owner)?;
        let selected = super::native_loaf_plan::select_native_loaf_plan(store, receipt, &closure)?;
        self.metadata.verify_profile(&receipt.intent)?;
        Ok(selected)
    }

    /// Retain the exact promoted normal/dev surface through original native roots and per-root source digests.
    /// Reuse a covering requested receipt; a real dependency delta gets its own constituent and native plan.
    pub(crate) fn test_envelope(
        &self,
        store: &OvenStore,
        receipt: &OvenReceipt,
        resolved: &incan_provider::dependency_resolver::ResolvedDependencies,
        owner: &Path,
        session: &crate::session::CompilationSession,
    ) -> CliResult<super::PreparedOvenTestDependencyEnvelope> {
        let dependencies = crate::build_unit::promoted_oven_test_dependencies(resolved)?;
        let owners = session
            .admitted_library_dependencies()
            .ok_or_else(|| failure("ordinary test envelope lacks checked package owners"))?;
        let physical = owners.physical_native_dependencies(session.provider_plan(), &dependencies)?;
        // ---- Complete promoted declaration and original physical-root coverage ----
        let dependency_surface_digest =
            oven_store::digest_dependency_specs(&dependencies, incan_oven_facet::provider_hooks().as_ref())
                .map_err(failure)?;
        let dependency_root_digests = super::plan_selection::oven_test_dependency_root_digests(&dependencies)?;
        let inputs = self.runtime_inputs(&receipt.intent, &[], &[], &dependencies, &physical, owner)?;
        // ---- Reuse the actual library receipt only when every native input already covers the test surface ----
        let selected_receipt = if inputs
            .iter()
            .all(|(key, value)| receipt.sources.build_unit_inputs.get(key) == Some(value))
        {
            receipt.clone()
        } else {
            let generated = owner.join("target/incan/oven/test-dependency-envelope");
            let mut generator =
                crate::backend::ProjectGenerator::new(&generated, "incan_test_dependency_envelope", true);
            generator.set_package_metadata(Some(incan_lang::version::INCAN_VERSION.to_string()), None);
            generator.set_dependencies(dependencies.clone());
            generator.set_dev_dependencies(Vec::new());
            generator.generate("fn main() {}\n").map_err(failure)?;
            let mut request = oven_store::OvenGeneratedProjectRequest::new(
                owner,
                "incan-test-dependency-envelope",
                incan_lang::version::INCAN_VERSION,
                receipt.intent.target.clone(),
                receipt.intent.toolchain.clone(),
                receipt.intent.profile.clone(),
                receipt.intent.features.clone(),
            )
            .with_generated_source("generated-root", generator.crate_root_path())
            .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
            for (key, value) in &receipt.sources.build_unit_inputs {
                request = request.with_build_unit_input(key.clone(), value.clone());
            }
            request = request.with_build_unit_input(
                "project-owner-identity",
                super::output_selection::baked_project_owner_identity(owner)?,
            );
            for (key, value) in inputs {
                request = request.with_build_unit_input(key, value);
            }
            oven_store::receipt_generated_project(&request).map_err(failure)?
        };
        // ---- Select and retain the same genuine producer graph for future generated tests ----
        let closure = self.closure(&selected_receipt.intent, &physical, owner)?;
        let (receipt, plan_selection) =
            super::native_loaf_plan::select_native_loaf_plan(store, &selected_receipt, &closure)?;
        self.metadata.verify_profile(&receipt.intent)?;
        Ok(super::PreparedOvenTestDependencyEnvelope {
            native_roots: Some(closure.roots().clone()),
            receipt,
            dependency_surface_digest,
            dependencies,
            dependency_root_digests,
            provider_entries: Vec::new(),
            plan_selection,
        })
    }
}

/// Preserve producer and original-owner refusal diagnostics in the library command domain.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
