//! Explicit compiler-owned standard publication transport for cold toolchains (#1337, #1698).
//!
//! Authored Incan cannot perform its first ordinary invocation before core is published. This bootstrap host
//! choreography delegates all package policy, native selection, metadata admission and publication to existing
//! owners. It is never an ordinary-command fallback or a caller-supplied standard namespace grant.

use crate::build::OvenProjectBakeReport;
use crate::build::native_loaf_plan::ConfiguredNativeLoafInputs;
use crate::build::ordinary_library_native::{OrdinaryLibraryNativeProfiles, OrdinaryLibraryNativeRequest};
use crate::build::ordinary_support::CompilerSupportSources;
use crate::error::{CliError, CliResult};
use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES;
use incan_provider::FeatureSelection;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_rustc::native_loaf::{NativeLoafConsumerRequest, project_declared_native_loaf_request};
use std::path::Path;
use std::sync::Arc;

/// Prepare all registered standard packages from executable-bound sources and one explicit native family graph.
///
/// The current configured graph supplies candidate resolution facts; only registered default package demands and
/// mandatory compiler support are projected into `native_graph_output`. Each profile is genuinely prepared once.
/// Package publication uses those original owners. Later ordinary commands keep their existing configured graph;
/// checked package admission does not require its producer request to equal the consumer's native request. A caller
/// explicitly selecting this narrower graph still refuses newly requested dependencies outside its closure.
/// No environment variables are changed, no SDK catalog substitutes for a missing owner, and a failed package
/// stops readiness without hiding earlier completed publications.
pub fn prepare_standard_packages(native_graph_output: &Path) -> CliResult<Vec<OvenProjectBakeReport>> {
    // ---- Original registry-owned source and dependency demands ----
    let inputs = ConfiguredNativeLoafInputs::discover()?
        .ok_or_else(|| failure("standard preparation requires an explicitly configured native graph"))?;
    let support = Arc::new(
        CompilerSupportSources::discover()?
            .ok_or_else(|| failure("standard preparation requires compiler-owned support sources"))?,
    );
    let mut sources = Vec::new();
    let mut dependencies = support.dependencies()?.to_vec();
    for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
        let source = TrustedStandardSourcePublication::discover(policy.package_name)
            .map_err(failure)?
            .ok_or_else(|| {
                failure(format!(
                    "standard package {} has no compiler-owned source",
                    policy.package_name
                ))
            })?;
        let root = source.verified_package_root().map_err(failure)?;
        let manifest = ProjectManifest::load(&root.join("loaf.toml")).map_err(failure)?;
        append_package_demands(&source, &manifest, &mut dependencies)?;
        source.verify().map_err(failure)?;
        sources.push((Arc::new(source), manifest));
    }

    // ---- One canonical native family and genuine profile preparations ----
    let rustc = oven_rustc::rustc::resolve_active_rustc().map_err(failure)?;
    let target = oven_rustc::rustc::rustc_host_target(&rustc).map_err(failure)?;
    let toolchain = oven_rustc::rustc::rustc_identity(&rustc).map_err(failure)?;
    let output = native_graph_output
        .parent()
        .ok_or_else(|| failure("standard graph output has no parent"))?
        .join("native-preparation");
    let request = NativeLoafConsumerRequest {
        graph: &inputs.graph,
        index: &inputs.index,
        blobs: &inputs.blobs,
        output: &output,
        rustc: &rustc,
        target: &target,
        profile: "debug",
        dependencies: &dependencies,
        declaration_owner: support.declaration_owner()?,
        domain: "target",
    };
    let mut projection = project_declared_native_loaf_request(&request, native_graph_output).map_err(failure)?;
    let native = Arc::new(OrdinaryLibraryNativeProfiles::prepare(OrdinaryLibraryNativeRequest {
        support: Arc::clone(&support),
        graph: projection.graph(),
        index: &inputs.index,
        blobs: &inputs.blobs,
        output: &output,
        rustc: &rustc,
        target: &target,
        profiles: &super::explicit_bake_profiles(),
    })?);
    projection.verify(&request).map_err(failure)?;

    // ---- Ordinary package publication, retaining any separately produced vocabulary ----
    let mut reports = Vec::new();
    for (source, manifest) in &sources {
        source.verify().map_err(failure)?;
        let mut input = super::AdmittedLibraryPreparation::from_prepared_ordinary_native(
            manifest,
            &FeatureSelection::default(),
            &target,
            &toolchain,
            Arc::clone(&native),
        )?;
        if manifest.vocab().is_some() {
            input = input.with_prepared_vocabulary(Arc::new(
                super::standard_vocabulary::PreparedStandardVocabulary::prepare(
                    Arc::clone(source),
                    manifest,
                    support.verified_standard_source_root()?,
                    projection.graph(),
                    &inputs,
                    &rustc,
                    &output,
                )?,
            ))?;
        }
        reports.push(crate::build::bake::bake_admitted_library(
            &input,
            &FeatureSelection::default(),
            Some(&target),
        )?);
        source.verify().map_err(failure)?;
        projection.verify(&request).map_err(failure)?;
    }
    support.verify()?;
    for (source, _) in &sources {
        source.verify().map_err(failure)?;
    }
    Ok(reports)
}

/// Combine actual package-local requests without unifying aliases or manufacturing dependency feature choices.
/// Generated aliases disambiguate package-local names only; exact package/version/path/features remain unchanged.
fn append_package_demands(
    source: &TrustedStandardSourcePublication,
    manifest: &ProjectManifest,
    dependencies: &mut Vec<DependencySpec>,
) -> CliResult<()> {
    let (package, version) = source.package_identity();
    let root = source.verified_package_root().map_err(failure)?;
    for mut dependency in manifest.rust_dependency_values() {
        if dependency.optional {
            return Err(failure(
                "standard toolchain preparation requires explicit activation for optional native declarations",
            ));
        }
        dependency.package = Some(dependency.package.unwrap_or_else(|| dependency.crate_name.clone()));
        dependency.crate_name = format!("{package}_dependency_{}", dependency.crate_name);
        if let DependencySource::Path { path } = &mut dependency.source {
            *path = root.join(&*path);
        }
        dependencies.push(dependency);
    }
    if let Some(facet) = source.verified_native_facet_name().map_err(failure)? {
        let plan =
            incan_provider::PackageFeaturePlan::resolve(manifest, &FeatureSelection::default()).map_err(failure)?;
        let selected = plan
            .root_package()
            .ok_or_else(|| failure("standard package has no resolved feature projection"))?;
        dependencies.push(DependencySpec {
            crate_name: format!("{package}_facet_{facet}"),
            package: Some(package.to_string()),
            version: Some(format!("={version}")),
            features: selected.features.active_features.iter().cloned().collect(),
            default_features: true,
            optional: false,
            source: DependencySource::Path {
                path: root.to_path_buf(),
            },
        });
    }
    Ok(())
}

/// Preserve canonical admission and publication errors in the existing CLI error domain.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES;
    use oven_model::manifest::{DependencySource, ProjectManifest};
    use oven_rustc::native_loaf::NativeLoafFacet;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Follow real standard manifests and their local declarations, checking the explicit producer's candidate world.
    /// This does not grant any physical authority: canonical projection and actual publication remain separate gates.
    fn check_standard_native_candidates(root: &Path, facets: &[NativeLoafFacet]) -> TestResult {
        // ---- Standard source declarations, including packages without their own Rust facet ----
        let owner = root.join("workspaces/oven");
        let mut pending = STANDARD_PACKAGE_NAMESPACE_POLICIES
            .iter()
            .map(|policy| root.join("loaves/stdlib").join(policy.source_directory))
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(project) = pending.pop() {
            let project = project.canonicalize()?;
            if !visited.insert(project.clone()) {
                continue;
            }
            let manifest = ProjectManifest::load(&project.join("loaf.toml"))?;

            // ---- Every declared native owner has one explicit source/domain candidate ----
            if let Some(native) = &manifest.rust_facet {
                let domain = if native.kind.as_deref() == Some("proc-macro") {
                    "host"
                } else {
                    "target"
                };
                let mut matching = Vec::new();
                for facet in facets {
                    if owner.join(&facet.project).canonicalize()? == project {
                        matching.push(facet);
                    }
                }
                let [selected] = matching.as_slice() else {
                    return Err(format!(
                        "standard native source {} has {} explicit candidates",
                        project.display(),
                        matching.len()
                    )
                    .into());
                };
                if selected.domain != domain {
                    return Err(
                        format!("standard native source {} requires {domain} domain", project.display()).into(),
                    );
                }
            }

            // ---- Authored local dependencies supply companions; no hardcoded package list ----
            for dependency in manifest.rust_dependency_values() {
                if let DependencySource::Path { path } = dependency.source {
                    pending.push(project.join(path));
                }
            }
        }
        Ok(())
    }

    /// The real compiler graph covers all registry-owned standard native facets and their authored local companions.
    /// Missing-source and wrong-domain mutations reproduce the preparation refusal without compiling a second SDK.
    #[test]
    fn standard_readiness_graph_covers_authored_native_sources() -> TestResult {
        let root = incan_test_support::repo_root();
        let graph: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("workspaces/oven/compiler-suite-native.json"))?)?;
        let facets: Vec<NativeLoafFacet> = serde_json::from_value(graph["facets"].clone())?;
        check_standard_native_candidates(&root, &facets)?;
        for (index, facet) in facets
            .iter()
            .enumerate()
            .filter(|(_, facet)| facet.project.starts_with("../../loaves/stdlib"))
        {
            let mut missing = facets.clone();
            missing.remove(index);
            assert!(
                check_standard_native_candidates(&root, &missing).is_err(),
                "missing {:?}",
                facet.project
            );
            let mut wrong_domain = facets.clone();
            wrong_domain[index].domain = if facet.domain == "host" { "target" } else { "host" }.to_string();
            assert!(
                check_standard_native_candidates(&root, &wrong_domain).is_err(),
                "wrong domain {:?}",
                facet.project
            );
        }
        Ok(())
    }
}
