//! Typed additive package-feature graph from RFC 114: one package's declared features and the active projection a
//! command selects.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use incan_lang::lang::conventions::validate_package_feature_identifier;
use serde::Serialize;

use oven_model::manifest::{ExpandedProjectFeature, ProjectFeatureDefinition, ProjectManifest};

/// Root feature selection supplied by the project manifest and current command.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FeatureSelection {
    /// Explicit root-package features requested by the command or environment.
    pub requested: BTreeSet<String>,
    /// Suppress the package's conventional `default` feature root.
    pub no_default_features: bool,
    /// Select every feature declared by the root package.
    pub all_features: bool,
}

impl FeatureSelection {
    /// Build a selection from explicit feature names while retaining default features.
    pub fn new<I, S>(requested: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            requested: requested.into_iter().map(Into::into).collect(),
            no_default_features: false,
            all_features: false,
        }
    }
}

/// Why one package feature entered the active closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "source", rename_all = "snake_case")]
pub enum FeatureActivationReason {
    /// Conventional package default.
    Default,
    /// Explicit command or environment request.
    Requested,
    /// `--all-features` selected every declaration.
    AllFeatures,
    /// Another local package feature includes this feature.
    IncludedBy(String),
    /// One parent package dependency edge requested this feature.
    DependencyRequest {
        /// Package containing the dependency declaration.
        package: String,
        /// Dependency key used by the requesting package.
        dependency: String,
    },
}

/// Deterministic active projection for one package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ResolvedPackageFeatures {
    /// Active local package features after additive closure.
    pub active_features: BTreeSet<String>,
    /// Optional Loaf dependencies activated by the feature closure.
    pub active_optional_dependencies: BTreeSet<String>,
    /// Public feature requests grouped by active Loaf dependency key; each facet consumes its own edges.
    pub dependency_features: BTreeMap<String, BTreeSet<String>>,
    /// Stable activation reasons for inspection and lock provenance.
    pub reasons: BTreeMap<String, BTreeSet<FeatureActivationReason>>,
}

/// Validation or resolution failure in one package-owned feature graph.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeatureGraphError {
    /// A feature, dependency, component, or dependency-feature spelling is malformed.
    #[error("invalid feature member `{member}` in feature `{feature}`: {message}")]
    InvalidMember {
        /// Feature containing the invalid member.
        feature: String,
        /// Authored compact member or expanded field value.
        member: String,
        /// Validation detail.
        message: String,
    },
    /// A local feature edge or root request names no declared feature.
    #[error("unknown package feature `{feature}`{context}")]
    UnknownFeature {
        /// Missing local feature.
        feature: String,
        /// Optional edge context formatted for diagnostics.
        context: String,
    },
    /// A feature edge names no declared Loaf dependency.
    #[error("feature `{feature}` references unknown Loaf dependency `{dependency}`")]
    UnknownDependency {
        /// Feature containing the edge.
        feature: String,
        /// Missing dependency key.
        dependency: String,
    },
    /// `dep:<name>` or an expanded optional-dependency edge targets a non-optional dependency.
    #[error("feature `{feature}` activates dependency `{dependency}`, but that dependency is not optional")]
    DependencyNotOptional {
        /// Feature containing the edge.
        feature: String,
        /// Non-optional dependency key.
        dependency: String,
    },
    /// A feature closure requests a dependency feature without activating the optional dependency.
    #[error(
        "feature `{feature}` requests `{dependency}/{dependency_feature}`, but optional dependency `{dependency}` is not active"
    )]
    InactiveOptionalDependency {
        /// Feature containing the edge.
        feature: String,
        /// Inactive optional dependency key.
        dependency: String,
        /// Requested dependency-owned feature.
        dependency_feature: String,
    },
    /// The local include graph contains a cycle.
    #[error("package feature cycle: {path}")]
    Cycle {
        /// Stable arrow-separated cycle path.
        path: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NormalizedFeatureDefinition {
    local_features: BTreeSet<String>,
    optional_dependencies: BTreeSet<String>,
    dependency_features: BTreeMap<String, BTreeSet<String>>,
}

/// Validated package-owned feature graph independent of any one command selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFeatureGraph {
    definitions: BTreeMap<String, NormalizedFeatureDefinition>,
    optional_dependencies: BTreeSet<String>,
}

/// Resolved feature state for one concrete package instance in the dependency graph.
#[derive(Debug, Clone)]

/// Locate the exact authored feature member responsible for one graph failure when it came from the manifest.
fn feature_error_location(path: &Path, error: &FeatureGraphError) -> String {
    let (owner, candidates) = feature_error_anchor(error);
    feature_value_location(path, owner, &candidates)
}

/// Locate one authored package-feature value in its source manifest, when source is available.
pub fn feature_value_location(path: &Path, owner: Option<&str>, candidates: &[String]) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut section = String::new();
    let mut compact_owner_active = false;
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed.trim_matches(['[', ']']).to_string();
            compact_owner_active = false;
            continue;
        }
        if !section.starts_with("project.features") {
            continue;
        }
        if section == "project.features"
            && let Some((authored_feature, _)) = trimmed.split_once('=')
        {
            compact_owner_active = owner.is_none_or(|owner| authored_feature.trim() == owner);
        }
        let in_owner_section = owner.is_none_or(|owner| {
            section == "project.features" && compact_owner_active || section == format!("project.features.{owner}")
        });
        if !in_owner_section {
            continue;
        }
        for candidate in candidates {
            let quoted = format!("\"{candidate}\"");
            if let Some(column) = line.find(&quoted) {
                return format!(":{}:{}", line_index + 1, column + 1);
            }
        }
    }
    String::new()
}

/// Return the owning feature and authored value spellings most likely to identify one exact manifest item.
fn feature_error_anchor(error: &FeatureGraphError) -> (Option<&str>, Vec<String>) {
    match error {
        FeatureGraphError::InvalidMember { feature, member, .. } => (Some(feature), vec![member.clone()]),
        FeatureGraphError::UnknownFeature { feature, context } => {
            let owner = context.split('`').nth(1).filter(|owner| !owner.is_empty());
            (owner, vec![feature.clone()])
        }
        FeatureGraphError::UnknownDependency { feature, dependency }
        | FeatureGraphError::DependencyNotOptional { feature, dependency } => {
            (Some(feature), vec![format!("dep:{dependency}"), dependency.clone()])
        }
        FeatureGraphError::InactiveOptionalDependency {
            feature,
            dependency,
            dependency_feature,
        } => (
            Some(feature),
            vec![format!("{dependency}/{dependency_feature}"), dependency_feature.clone()],
        ),
        FeatureGraphError::Cycle { path } => {
            let mut cycle = path.split(" -> ");
            let owner = cycle.next();
            let included = cycle.next().unwrap_or_default();
            (owner, vec![included.to_string()])
        }
    }
}

impl PackageFeatureGraph {
    /// Validate one package's declared feature graph from its manifest.
    pub fn from_manifest(manifest: &ProjectManifest) -> Result<Self, FeatureGraphError> {
        // Public features belong to Loaves regardless of their implementation language. Keep this declaration
        // projection separate from provider traversal: native facets consume their own resolved feature edges.
        // An activation is optional only when every authored target alternative declares it optional.
        let dependencies = manifest
            .library_dependencies()
            .iter()
            .map(|(name, dependency)| (name.as_str(), dependency.optional))
            .chain(manifest.loaf_dependencies.iter().map(|(name, alternatives)| {
                (
                    name.as_str(),
                    alternatives.iter().all(|dependency| dependency.spec.optional),
                )
            }))
            .collect::<BTreeMap<_, _>>();
        let optional_dependencies = dependencies
            .iter()
            .filter(|(_, optional)| **optional)
            .map(|(name, _)| (*name).to_string())
            .collect();
        let mut definitions = BTreeMap::new();

        for (name, definition) in manifest.project_features() {
            validate_identifier(name).map_err(|message| FeatureGraphError::InvalidMember {
                feature: name.clone(),
                member: name.clone(),
                message,
            })?;
            definitions.insert(name.clone(), normalize_definition(name, definition)?);
        }

        for (feature, definition) in &definitions {
            for included in &definition.local_features {
                if !definitions.contains_key(included) {
                    return Err(FeatureGraphError::UnknownFeature {
                        feature: included.clone(),
                        context: format!(" referenced by `{feature}`"),
                    });
                }
            }
            for dependency in &definition.optional_dependencies {
                let Some(optional) = dependencies.get(dependency.as_str()) else {
                    return Err(FeatureGraphError::UnknownDependency {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                };
                if !optional {
                    return Err(FeatureGraphError::DependencyNotOptional {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
            for dependency in definition.dependency_features.keys() {
                if !dependencies.contains_key(dependency.as_str()) {
                    return Err(FeatureGraphError::UnknownDependency {
                        feature: feature.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        validate_acyclic(&definitions)?;
        Ok(Self {
            definitions,
            optional_dependencies,
        })
    }

    /// Resolve one additive active feature projection.
    pub fn resolve(&self, selection: &FeatureSelection) -> Result<ResolvedPackageFeatures, FeatureGraphError> {
        let mut resolved = ResolvedPackageFeatures::default();
        let mut pending = Vec::new();

        if !selection.no_default_features && self.definitions.contains_key("default") {
            add_root(&mut resolved, &mut pending, "default", FeatureActivationReason::Default);
        }
        for requested in &selection.requested {
            if !self.definitions.contains_key(requested) {
                return Err(FeatureGraphError::UnknownFeature {
                    feature: requested.clone(),
                    context: " requested by the current command".to_string(),
                });
            }
            add_root(
                &mut resolved,
                &mut pending,
                requested,
                FeatureActivationReason::Requested,
            );
        }
        if selection.all_features {
            for feature in self.definitions.keys() {
                add_root(
                    &mut resolved,
                    &mut pending,
                    feature,
                    FeatureActivationReason::AllFeatures,
                );
            }
        }

        while let Some(feature) = pending.pop() {
            let Some(definition) = self.definitions.get(&feature) else {
                continue;
            };
            resolved
                .active_optional_dependencies
                .extend(definition.optional_dependencies.iter().cloned());
            for (dependency, features) in &definition.dependency_features {
                resolved
                    .dependency_features
                    .entry(dependency.clone())
                    .or_default()
                    .extend(features.iter().cloned());
            }
            for included in &definition.local_features {
                resolved
                    .reasons
                    .entry(included.clone())
                    .or_default()
                    .insert(FeatureActivationReason::IncludedBy(feature.clone()));
                if resolved.active_features.insert(included.clone()) {
                    pending.push(included.clone());
                }
            }
        }

        for active_feature in &resolved.active_features {
            let Some(definition) = self.definitions.get(active_feature) else {
                continue;
            };
            for (dependency, dependency_features) in &definition.dependency_features {
                if self.optional_dependencies.contains(dependency)
                    && !resolved.active_optional_dependencies.contains(dependency)
                {
                    let dependency_feature = dependency_features.iter().next().cloned().unwrap_or_default();
                    return Err(FeatureGraphError::InactiveOptionalDependency {
                        feature: active_feature.clone(),
                        dependency: dependency.clone(),
                        dependency_feature,
                    });
                }
            }
        }

        Ok(resolved)
    }

    /// Return every declared package feature in stable order.
    pub fn declared_features(&self) -> impl Iterator<Item = &str> {
        self.definitions.keys().map(String::as_str)
    }

}

/// Add one root feature and its reason to the pending closure.
fn add_root(
    resolved: &mut ResolvedPackageFeatures,
    pending: &mut Vec<String>,
    feature: &str,
    reason: FeatureActivationReason,
) {
    resolved.reasons.entry(feature.to_string()).or_default().insert(reason);
    if resolved.active_features.insert(feature.to_string()) {
        pending.push(feature.to_string());
    }
}

/// Normalize either authored feature representation into typed edge sets.
fn normalize_definition(
    feature: &str,
    definition: &ProjectFeatureDefinition,
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    match definition {
        ProjectFeatureDefinition::Compact(members) => normalize_compact_definition(feature, members),
        ProjectFeatureDefinition::Expanded(expanded) => normalize_expanded_definition(feature, expanded),
    }
}

/// Parse the compact feature-member reference language.
fn normalize_compact_definition(
    feature: &str,
    members: &[String],
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    let mut normalized = NormalizedFeatureDefinition::default();
    for member in members {
        let member = member.trim();
        if let Some(dependency) = member.strip_prefix("dep:") {
            validate_member_identifier(feature, member, dependency)?;
            normalized.optional_dependencies.insert(dependency.to_string());
        } else if let Some((dependency, dependency_feature)) = member.split_once('/') {
            if dependency_feature.contains('/') {
                return Err(invalid_member(feature, member, "expected exactly one `/` separator"));
            }
            validate_member_identifier(feature, member, dependency)?;
            validate_member_identifier(feature, member, dependency_feature)?;
            normalized
                .dependency_features
                .entry(dependency.to_string())
                .or_default()
                .insert(dependency_feature.to_string());
        } else {
            validate_member_identifier(feature, member, member)?;
            normalized.local_features.insert(member.to_string());
        }
    }
    Ok(normalized)
}

/// Normalize the expanded typed feature table.
fn normalize_expanded_definition(
    feature: &str,
    expanded: &ExpandedProjectFeature,
) -> Result<NormalizedFeatureDefinition, FeatureGraphError> {
    let mut normalized = NormalizedFeatureDefinition::default();
    for included in &expanded.includes {
        validate_member_identifier(feature, included, included)?;
        normalized.local_features.insert(included.clone());
    }
    for dependency in &expanded.optional_dependencies {
        validate_member_identifier(feature, dependency, dependency)?;
        normalized.optional_dependencies.insert(dependency.clone());
    }
    for (dependency, features) in &expanded.dependency_features {
        validate_member_identifier(feature, dependency, dependency)?;
        for dependency_feature in features {
            validate_member_identifier(feature, dependency_feature, dependency_feature)?;
            normalized
                .dependency_features
                .entry(dependency.clone())
                .or_default()
                .insert(dependency_feature.clone());
        }
    }
    Ok(normalized)
}

/// Validate one identifier embedded in a feature member and retain authored context on failure.
fn validate_member_identifier(feature: &str, member: &str, identifier: &str) -> Result<(), FeatureGraphError> {
    validate_identifier(identifier).map_err(|message| invalid_member(feature, member, message))
}

/// Build one invalid-member error while preserving the authored feature and member.
fn invalid_member(feature: &str, member: &str, message: impl Into<String>) -> FeatureGraphError {
    FeatureGraphError::InvalidMember {
        feature: feature.to_string(),
        member: member.to_string(),
        message: message.into(),
    }
}

/// Validate the RFC 114 feature, dependency, and component identifier subset.
fn validate_identifier(identifier: &str) -> Result<(), String> {
    validate_package_feature_identifier(identifier).map_err(str::to_string)
}

/// Reject cycles in the complete local feature graph, including inactive declarations.
fn validate_acyclic(definitions: &BTreeMap<String, NormalizedFeatureDefinition>) -> Result<(), FeatureGraphError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    for feature in definitions.keys() {
        visit_feature(feature, definitions, &mut visiting, &mut visited, &mut stack)?;
    }
    Ok(())
}

/// Depth-first cycle validation for one local feature.
fn visit_feature(
    feature: &str,
    definitions: &BTreeMap<String, NormalizedFeatureDefinition>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), FeatureGraphError> {
    if visited.contains(feature) {
        return Ok(());
    }
    if visiting.contains(feature) {
        let start = stack.iter().position(|entry| entry == feature).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(feature.to_string());
        return Err(FeatureGraphError::Cycle {
            path: cycle.join(" -> "),
        });
    }

    visiting.insert(feature.to_string());
    stack.push(feature.to_string());
    if let Some(definition) = definitions.get(feature) {
        for included in &definition.local_features {
            visit_feature(included, definitions, visiting, visited, stack)?;
        }
    }
    stack.pop();
    visiting.remove(feature);
    visited.insert(feature.to_string());
    Ok(())
}

