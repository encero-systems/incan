//! Incan toolchain compatibility policy for project lifecycle commands.
//!
//! `requires-incan` is an execution guard, not dependency resolution. This module keeps the pure SemVer compatibility
//! checks separate from CLI command orchestration so project-aware commands can enforce the same policy consistently.

use semver::{Prerelease, Version, VersionReq};

use crate::manifest::ProjectManifest;

/// One manifest layer that contributed a `requires-incan` constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainConstraintLayer {
    /// Human-readable source label used in diagnostics and inspection output.
    pub source: String,
    /// Raw SemVer requirement string as authored in `incan.toml`.
    pub requirement: String,
}

impl ToolchainConstraintLayer {
    /// Build one named constraint layer.
    #[must_use]
    pub fn new(source: impl Into<String>, requirement: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            requirement: requirement.into(),
        }
    }
}

/// Effective toolchain constraints after project/env overlays have been resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolchainConstraintSet {
    layers: Vec<ToolchainConstraintLayer>,
}

impl ToolchainConstraintSet {
    /// Create an empty, unconstrained set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a set from already-labeled constraint layers.
    #[must_use]
    pub fn from_layers(layers: impl IntoIterator<Item = ToolchainConstraintLayer>) -> Self {
        Self {
            layers: layers.into_iter().collect(),
        }
    }

    /// Build the project-level baseline from `[project].requires-incan`, if present.
    #[must_use]
    pub fn from_project_manifest(manifest: &ProjectManifest) -> Self {
        let mut constraints = Self::new();
        if let Some(requirement) = manifest
            .project
            .as_ref()
            .and_then(|project| project.requires_incan.as_deref())
        {
            constraints.push("project.requires-incan", requirement);
        }
        constraints
    }

    /// Add one authored constraint layer.
    pub fn push(&mut self, source: impl Into<String>, requirement: impl Into<String>) {
        self.layers.push(ToolchainConstraintLayer::new(source, requirement));
    }

    /// Extend this set with all layers from another set.
    pub fn extend(&mut self, other: ToolchainConstraintSet) {
        self.layers.extend(other.layers);
    }

    /// Borrow all contributing layers.
    #[must_use]
    pub fn layers(&self) -> &[ToolchainConstraintLayer] {
        &self.layers
    }

    /// Return whether no layer declared a constraint.
    #[must_use]
    pub fn is_unconstrained(&self) -> bool {
        self.layers.is_empty()
    }

    /// Render the effective constraint for display.
    #[must_use]
    pub fn effective_requirement_display(&self) -> String {
        if self.layers.is_empty() {
            "unconstrained".to_string()
        } else {
            self.layers
                .iter()
                .map(|layer| layer.requirement.as_str())
                .collect::<Vec<_>>()
                .join(" && ")
        }
    }

    /// Check the active compiler version from [`crate::version::INCAN_VERSION`].
    pub fn compatibility_current(&self) -> Result<ToolchainCompatibility, ToolchainConstraintError> {
        self.compatibility_with(crate::version::INCAN_VERSION)
    }

    /// Check compatibility against one SemVer version string.
    pub fn compatibility_with(&self, active_version: &str) -> Result<ToolchainCompatibility, ToolchainConstraintError> {
        let active =
            Version::parse(active_version).map_err(|source| ToolchainConstraintError::InvalidActiveVersion {
                version: active_version.to_string(),
                message: source.to_string(),
            })?;

        let parsed = self
            .layers
            .iter()
            .map(|layer| {
                VersionReq::parse(&layer.requirement).map_err(|source| ToolchainConstraintError::InvalidRequirement {
                    source: layer.source.clone(),
                    requirement: layer.requirement.clone(),
                    message: source.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let satisfied = parsed
            .iter()
            .all(|requirement| requirement_matches_active_toolchain(requirement, &active));

        Ok(ToolchainCompatibility {
            active_version: active_version.to_string(),
            effective_requirement: (!self.layers.is_empty()).then(|| self.effective_requirement_display()),
            satisfied,
            layers: self.layers.clone(),
        })
    }

    /// Fail if the active compiler version does not satisfy this effective constraint.
    pub fn enforce_current(&self) -> Result<(), ToolchainConstraintError> {
        let compatibility = self.compatibility_current()?;
        if compatibility.satisfied {
            Ok(())
        } else {
            Err(ToolchainConstraintError::Unsatisfied(compatibility))
        }
    }
}

/// Compatibility result for inspection commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainCompatibility {
    /// Active compiler version string.
    pub active_version: String,
    /// Effective constraint display string, or `None` when unconstrained.
    pub effective_requirement: Option<String>,
    /// Whether the active compiler satisfies every contributing layer.
    pub satisfied: bool,
    /// Contributing project/env layers.
    pub layers: Vec<ToolchainConstraintLayer>,
}

/// Errors emitted while parsing or enforcing toolchain constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainConstraintError {
    /// The compiler's own package version is not a valid SemVer version.
    InvalidActiveVersion {
        /// Invalid active version string.
        version: String,
        /// SemVer parser message.
        message: String,
    },
    /// One manifest-authored `requires-incan` value is malformed.
    InvalidRequirement {
        /// Manifest layer label.
        source: String,
        /// Authored requirement string.
        requirement: String,
        /// SemVer parser message.
        message: String,
    },
    /// The active compiler does not satisfy the effective requirement.
    Unsatisfied(ToolchainCompatibility),
}

impl std::fmt::Display for ToolchainConstraintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidActiveVersion { version, message } => {
                write!(f, "invalid active Incan version `{version}`: {message}")
            }
            Self::InvalidRequirement {
                source,
                requirement,
                message,
            } => {
                write!(
                    f,
                    "invalid requires-incan constraint `{requirement}` from {source}: {message}"
                )
            }
            Self::Unsatisfied(compatibility) => {
                writeln!(
                    f,
                    "active Incan toolchain {} does not satisfy requires-incan {}",
                    compatibility.active_version,
                    compatibility
                        .effective_requirement
                        .as_deref()
                        .unwrap_or("unconstrained")
                )?;
                writeln!(f)?;
                writeln!(f, "contributing constraints:")?;
                for layer in &compatibility.layers {
                    writeln!(f, "  - {}: {}", layer.source, layer.requirement)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ToolchainConstraintError {}

/// Return whether a requirement matches the active toolchain.
///
/// Incan dev builds are published as SemVer prereleases such as `0.3.0-dev.48`. For lifecycle compatibility, a dev
/// build is allowed to satisfy a range that admits its release-core version (`0.3.0`) so projects can write a normal
/// release-line constraint such as `>=0.3,<0.4` while the line is still in development.
fn requirement_matches_active_toolchain(requirement: &VersionReq, active: &Version) -> bool {
    if requirement.matches(active) {
        return true;
    }
    if active.pre.is_empty() {
        return false;
    }

    let mut release_core = active.clone();
    release_core.pre = Prerelease::EMPTY;
    requirement.matches(&release_core)
}

#[cfg(test)]
mod tests {
    use super::{ToolchainConstraintError, ToolchainConstraintSet};

    #[test]
    fn unconstrained_set_is_satisfied() -> Result<(), Box<dyn std::error::Error>> {
        let compatibility = ToolchainConstraintSet::new().compatibility_with("0.3.0-dev.48")?;

        assert!(compatibility.satisfied);
        assert_eq!(compatibility.effective_requirement, None);
        Ok(())
    }

    #[test]
    fn release_line_requirement_matches_dev_toolchain() -> Result<(), Box<dyn std::error::Error>> {
        let mut constraints = ToolchainConstraintSet::new();
        constraints.push("project.requires-incan", ">=0.3,<0.4");

        let compatibility = constraints.compatibility_with("0.3.0-dev.48")?;

        assert!(compatibility.satisfied);
        assert_eq!(compatibility.effective_requirement.as_deref(), Some(">=0.3,<0.4"));
        Ok(())
    }

    #[test]
    fn all_layers_must_match_active_toolchain() -> Result<(), Box<dyn std::error::Error>> {
        let mut constraints = ToolchainConstraintSet::new();
        constraints.push("project.requires-incan", ">=0.3,<0.5");
        constraints.push("env.release.requires-incan", ">=0.4,<0.5");

        let compatibility = constraints.compatibility_with("0.3.0-dev.48")?;

        assert!(!compatibility.satisfied);
        assert_eq!(
            compatibility.effective_requirement.as_deref(),
            Some(">=0.3,<0.5 && >=0.4,<0.5")
        );
        Ok(())
    }

    #[test]
    fn malformed_layer_is_reported_with_source() {
        let mut constraints = ToolchainConstraintSet::new();
        constraints.push("project.requires-incan", "not semver");

        let error = match constraints.compatibility_with("0.3.0-dev.48") {
            Ok(compatibility) => panic!("expected malformed constraint, got {compatibility:?}"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            ToolchainConstraintError::InvalidRequirement { ref source, .. }
                if source == "project.requires-incan"
        ));
    }
}
