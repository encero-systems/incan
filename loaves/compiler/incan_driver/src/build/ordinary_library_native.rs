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
use oven_rustc::native_loaf::{NativeLoafClosure, NativeLoafPreparationReport, prepare_resolved_native_loafs};
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_store::store::OvenStore;
use oven_store::{OvenBuildIntent, OvenReceipt};

/// Explicit existing resolver output and native producer coordinates for one ordinary library command.
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
    reports: BTreeMap<String, NativeLoafPreparationReport>,
}

impl OrdinaryLibraryNativeProfiles {
    /// Prepare exact requested profiles through the existing native producer, without any SDK discovery.
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
        let mut reports = BTreeMap::new();
        for profile in request.profiles {
            let prepared = prepare_resolved_native_loafs(
                request.graph,
                request.index,
                request.blobs,
                request.output,
                request.rustc,
                request.target,
                profile,
            )
            .map_err(failure)?;
            observations.insert((*profile).to_string(), prepared.request_observation().map_err(failure)?);
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
        Ok(Self { metadata, reports })
    }

    /// Borrow the same original complete semantic request through selection, replay and publication handoffs.
    pub(crate) fn metadata(&self) -> &Arc<OrdinaryNativeMetadataAuthority> {
        &self.metadata
    }

    /// Observe real compile/reuse work without synthesizing successful preparation from cached output paths.
    pub(crate) fn reports(&self) -> &BTreeMap<String, NativeLoafPreparationReport> {
        &self.reports
    }

    /// Revalidate every original request and mandatory support declaration without reacquiring owners.
    pub(crate) fn verify(&self) -> CliResult<()> {
        self.metadata.verify()
    }

    /// Bind a canonical normal/dev dependency surface to every original profile before its lock is published.
    /// This verifies declared roots and their transitive physical closure without granting macro execution.
    pub(crate) fn verify_dependencies(&self, dependencies: &[DependencySpec], owner: &Path) -> CliResult<()> {
        self.verify()?;
        for observation in self.metadata.observations().values() {
            let graph = observation.graph();
            let roots = graph
                .select_dependency_roots(dependencies, owner, "target")
                .map_err(failure)?;
            graph.select(&roots).map_err(failure)?;
        }
        self.verify()
    }

    /// Check the currently admitted empty provider context without inventing Cargo support-package semantics.
    ///
    /// Mandatory native support remains bound by the complete original requests in checked metadata. This slice
    /// admits no provider/facet/dependency projections; broader semantics must supply their own ordinary authority.
    pub(crate) fn provider_semantic_dependencies(
        &self,
        plan: &incan_provider::ProviderPlan,
        requirements: &incan_provider::requirements::ProjectRequirements,
    ) -> CliResult<Vec<DependencySpec>> {
        self.metadata.support().verify()?;
        if plan.records().next().is_some()
            || !requirements.stdlib_facets.is_empty()
            || !requirements.dependencies.is_empty()
            || !requirements.sdk_dependency_rebindings.is_empty()
            || !requirements.sdk_path_dependencies.is_empty()
            || !requirements.sdk_artifact_projections.is_empty()
        {
            return Err(failure(
                "ordinary support-only provider semantics do not admit provider or dependency projections",
            ));
        }
        Ok(Vec::new())
    }

    /// Select authored dependency aliases and mandatory compiler support from the original producer graph.
    fn closure(
        &self,
        intent: &OvenBuildIntent,
        dependencies: &[DependencySpec],
        owner: &Path,
    ) -> CliResult<NativeLoafClosure> {
        let observation = self.metadata.verify_profile(intent)?;
        let graph = observation.graph();
        let support = self.metadata.support();
        let mut roots = graph
            .select_dependency_roots(support.dependencies()?, support.declaration_owner()?, "target")
            .map_err(failure)?;
        for root in graph
            .select_dependency_roots(dependencies, owner, "target")
            .map_err(failure)?
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
    /// Reuse a covering debug receipt; a real dependency delta gets its own generated constituent and native plan.
    pub(crate) fn test_envelope(
        &self,
        store: &OvenStore,
        receipt: &OvenReceipt,
        resolved: &incan_provider::dependency_resolver::ResolvedDependencies,
        owner: &Path,
    ) -> CliResult<super::PreparedOvenTestDependencyEnvelope> {
        let dependencies = crate::build_unit::promoted_oven_test_dependencies(resolved)?;
        if receipt.intent.profile != "debug" {
            return Err(failure("ordinary library test envelope requires its debug receipt"));
        }
        // ---- Complete promoted declaration and original physical-root coverage ----
        let dependency_surface_digest =
            oven_store::digest_dependency_specs(&dependencies, incan_oven_facet::provider_hooks().as_ref())
                .map_err(failure)?;
        let dependency_root_digests = super::plan_selection::oven_test_dependency_root_digests(&dependencies)?;
        let inputs = self.runtime_inputs(&receipt.intent, &[], &[], &dependencies, &dependencies, owner)?;
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
                "debug",
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
        let (receipt, plan_selection) = self.select_plan(store, &selected_receipt, &dependencies, owner)?;
        Ok(super::PreparedOvenTestDependencyEnvelope {
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
