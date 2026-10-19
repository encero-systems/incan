//! The promoted test dependencies the lock and the test runner agree on.
//!
//! Every command that resolves a project or runs its tests computes these the same way; they are here so `lock` and
//! `test` cannot drift.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{CliError, CliResult};
use incan_provider::dependency_resolver::ResolvedDependencies;
use oven_model::manifest::DependencySpec;

/// Promote the complete canonical normal/dev surface into one generated-test dependency set.
///
/// Cargo unifies features and default-feature activation for duplicate package edges. The synthetic envelope mirrors
/// that behavior once at explicit bake time while preserving the dependency key, package rename, source, and version.
/// Every selected edge becomes non-optional because a generated native test may use any dependency reachable from the
/// checked project/test graph.
pub fn promoted_oven_test_dependencies(resolved: &ResolvedDependencies) -> CliResult<Vec<DependencySpec>> {
    let mut promoted = Vec::new();
    for candidate in resolved.dependencies.iter().chain(&resolved.dev_dependencies) {
        if let Some(existing) = promoted
            .iter_mut()
            .find(|dependency: &&mut DependencySpec| dependency.crate_name == candidate.crate_name)
        {
            if existing.version != candidate.version
                || existing.source != candidate.source
                || existing.package != candidate.package
            {
                return Err(CliError::failure(format!(
                    "test dependency `{}` conflicts between the canonical normal and dev surfaces",
                    candidate.crate_name
                )));
            }
            existing.features.extend(candidate.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
            existing.default_features |= candidate.default_features;
            existing.optional = false;
            continue;
        }
        let mut candidate = candidate.clone().normalized();
        candidate.optional = false;
        promoted.push(candidate);
    }
    promoted.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(promoted)
}

