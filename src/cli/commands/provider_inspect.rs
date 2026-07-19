//! Machine-readable SDK provider and public package-feature inspection from RFC 114.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::manifest::ProjectManifest;
use crate::provider::{
    ComponentSelectionReason, FeatureActivationReason, FeatureSelection, ProviderParticipation, ProviderPlan,
    ProviderProvenance,
};

use super::common::{CompilationSession, collect_modules_detailed_with_session, resolve_project_root};

/// Human or JSON output for provider and feature inspection.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderInspectionFormat {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
struct ProvidersReport {
    schema_version: u32,
    sdk: Option<SdkReport>,
    providers: Vec<ProviderReport>,
}

#[derive(Debug, Serialize)]
struct SdkReport {
    identity: String,
    profile: String,
    components: Vec<ComponentReport>,
}

#[derive(Debug, Serialize)]
struct ComponentReport {
    id: String,
    version: String,
    mandatory: bool,
    available: bool,
    enabled: bool,
    dependencies: BTreeSet<String>,
    reason: Option<ComponentSelectionReason>,
}

#[derive(Debug, Serialize)]
struct ProviderReport {
    identity: String,
    available: bool,
    enabled: bool,
    used: bool,
    participation: ProviderParticipation,
    provenance: ProviderProvenance,
    namespace_claims: BTreeSet<Vec<String>>,
    used_modules: BTreeSet<Vec<String>>,
    active_features: BTreeSet<String>,
    provider_dependencies: Vec<crate::library_manifest::ProviderDependencyMetadata>,
    implementation_facets: Vec<String>,
    manifest_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct FeaturesReport {
    schema_version: u32,
    packages: Vec<PackageFeaturesReport>,
    edges: Vec<FeatureEdgeReport>,
    conditioned_facts: Vec<ConditionedFactReport>,
}

#[derive(Debug, Serialize)]
struct PackageFeaturesReport {
    package: String,
    project_root: PathBuf,
    active_features: BTreeSet<String>,
    active_optional_dependencies: BTreeSet<String>,
    dependency_features: BTreeMap<String, BTreeSet<String>>,
    required_sdk_components: BTreeSet<String>,
    reasons: BTreeMap<String, BTreeSet<FeatureActivationReason>>,
}

#[derive(Debug, Serialize)]
struct FeatureEdgeReport {
    from: PathBuf,
    dependency_key: String,
    to: PathBuf,
    requested_features: BTreeSet<String>,
    default_features: bool,
    optional: bool,
}

#[derive(Debug, Serialize)]
struct ConditionedFactReport {
    provider: String,
    kind: crate::library_manifest::ProviderFactKind,
    identity: String,
    required_features: BTreeSet<String>,
    active: bool,
}

/// Inspect the active SDK component catalog and compilation provider participation.
pub fn inspect_providers(
    path: &Path,
    format: ProviderInspectionFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let context = inspection_context(path, feature_selection, sdk_profile_override)?;
    let provider_plan = context.session.provider_plan_for_modules(&context.modules)?;
    let sdk = context
        .session
        .sdk_inventory
        .as_ref()
        .zip(context.session.sdk_components.as_ref())
        .map(|(inventory, resolved)| SdkReport {
            identity: inventory.identity(),
            profile: resolved.profile.clone(),
            components: inventory
                .components
                .values()
                .map(|component| ComponentReport {
                    id: component.id.clone(),
                    version: component.version.clone(),
                    mandatory: component.mandatory,
                    available: component.available,
                    enabled: resolved.enabled.contains(&component.id),
                    dependencies: component.dependencies.clone(),
                    reason: resolved.reasons.get(&component.id).cloned(),
                })
                .collect(),
        });
    let report = ProvidersReport {
        schema_version: 1,
        sdk,
        providers: provider_reports(&provider_plan),
    };
    emit_provider_report(&report, format)?;
    Ok(ExitCode::SUCCESS)
}

/// Inspect package feature roots, unified closure, dependency edges, and conditioned provider facts.
pub fn inspect_features(
    path: &Path,
    format: ProviderInspectionFormat,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let context = inspection_context(path, feature_selection, sdk_profile_override)?;
    let provider_plan = context.session.provider_plan_for_modules(&context.modules)?;
    let packages = context
        .session
        .package_feature_plan
        .iter()
        .flat_map(|plan| plan.packages())
        .map(|package| PackageFeaturesReport {
            package: package.package_name.clone(),
            project_root: package.project_root.clone(),
            active_features: package.features.active_features.clone(),
            active_optional_dependencies: package.features.active_optional_dependencies.clone(),
            dependency_features: package.features.dependency_features.clone(),
            required_sdk_components: package.features.required_sdk_components.clone(),
            reasons: package.features.reasons.clone(),
        })
        .collect();
    let edges = context
        .session
        .package_feature_plan
        .iter()
        .flat_map(|plan| plan.edges())
        .map(|edge| FeatureEdgeReport {
            from: edge.from.clone(),
            dependency_key: edge.dependency_key.clone(),
            to: edge.to.clone(),
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        })
        .collect();
    let report = FeaturesReport {
        schema_version: 1,
        packages,
        edges,
        conditioned_facts: conditioned_fact_reports(&provider_plan),
    };
    emit_feature_report(&report, format)?;
    Ok(ExitCode::SUCCESS)
}

struct InspectionContext {
    session: CompilationSession,
    modules: Vec<ParsedModule>,
}

/// Discover the shared compilation session and any source modules needed to inspect one file or project directory.
fn inspection_context(
    path: &Path,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<InspectionContext> {
    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };
    let project_root = resolve_project_root(&input);
    let manifest = ProjectManifest::discover(&project_root).map_err(|error| CliError::failure(error.to_string()))?;
    let entry = inspection_entrypoint(&input, manifest.as_ref());
    let session_path = entry.as_deref().unwrap_or(input.as_path());
    let session = CompilationSession::discover_with_selections(session_path, feature_selection, sdk_profile_override)?;
    let modules = entry
        .as_ref()
        .map(|entry| {
            collect_modules_detailed_with_session(entry.clone(), &session)
                .map_err(|failure| CliError::failure(failure.render_human()))
        })
        .transpose()?
        .unwrap_or_default();
    Ok(InspectionContext { session, modules })
}

/// Select an explicit source file or the conventional project entrypoint used to collect inspection-time usage facts.
fn inspection_entrypoint(input: &Path, manifest: Option<&ProjectManifest>) -> Option<PathBuf> {
    if input.is_file() {
        return Some(input.to_path_buf());
    }
    let root = manifest.map(ProjectManifest::project_root).unwrap_or(input);
    [root.join("src/main.incn"), root.join("src/lib.incn")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Project provider identities, availability, participation, provenance, and implementation facts into stable reports.
fn provider_reports(plan: &ProviderPlan) -> Vec<ProviderReport> {
    plan.records()
        .map(|provider| ProviderReport {
            identity: provider.identity.stable_key(),
            available: provider.available,
            enabled: provider.enabled,
            used: plan.participation(provider) == ProviderParticipation::Used,
            participation: plan.participation(provider),
            provenance: provider.provenance.clone(),
            namespace_claims: provider.namespace_claims.clone(),
            used_modules: plan.used_modules(provider),
            active_features: provider.identity.feature_projection.clone(),
            provider_dependencies: provider
                .manifest
                .iter()
                .flat_map(|manifest| {
                    manifest
                        .contract_metadata
                        .provider
                        .provider_dependencies
                        .iter()
                        .cloned()
                })
                .collect(),
            implementation_facets: provider
                .implementation_facets
                .iter()
                .map(|facet| facet.id.clone())
                .collect(),
            manifest_path: provider
                .artifact
                .as_ref()
                .map(|artifact| artifact.manifest_path.clone()),
        })
        .collect()
}

/// Project every feature-conditioned provider fact and whether the provider's active feature set enables that fact.
fn conditioned_fact_reports(plan: &ProviderPlan) -> Vec<ConditionedFactReport> {
    plan.records()
        .flat_map(|provider| {
            let active_features = &provider.identity.feature_projection;
            provider.manifest.iter().flat_map(move |manifest| {
                manifest
                    .contract_metadata
                    .provider
                    .fact_requirements
                    .iter()
                    .map(move |fact| ConditionedFactReport {
                        provider: provider.identity.stable_key(),
                        kind: fact.kind,
                        identity: fact.identity.clone(),
                        required_features: fact.required_features.clone(),
                        active: fact.required_features.is_subset(active_features),
                    })
            })
        })
        .collect()
}

/// Render provider inspection as deterministic JSON or as the concise human-readable provider and component summary.
fn emit_provider_report(report: &ProvidersReport, format: ProviderInspectionFormat) -> CliResult<()> {
    match format {
        ProviderInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize provider inspection: {error}")))?
        ),
        ProviderInspectionFormat::Text => {
            if let Some(sdk) = &report.sdk {
                println!("SDK: {} (profile {})", sdk.identity, sdk.profile);
                for component in &sdk.components {
                    let state = if !component.available {
                        "unavailable"
                    } else if component.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    println!("  {}: {state}", component.id);
                }
            } else {
                println!("SDK: legacy installation without a component inventory");
            }
            for provider in &report.providers {
                println!("Provider: {} ({:?})", provider.identity, provider.participation);
            }
        }
    }
    Ok(())
}

/// Render feature inspection as deterministic JSON or as the concise human-readable package activation summary.
fn emit_feature_report(report: &FeaturesReport, format: ProviderInspectionFormat) -> CliResult<()> {
    match format {
        ProviderInspectionFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize feature inspection: {error}")))?
        ),
        ProviderInspectionFormat::Text => {
            if report.packages.is_empty() {
                println!("No package feature graph is active.");
            }
            for package in &report.packages {
                println!(
                    "Package {}: {}",
                    package.package,
                    package.active_features.iter().cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }
    }
    Ok(())
}
