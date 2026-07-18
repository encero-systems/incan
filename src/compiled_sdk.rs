//! Compiler-owned view of canonical modules published by active SDK providers.
//!
//! Provider publication persists namespace claims in checked artifacts. Consumers derive ownership from the shared
//! provider plan instead of maintaining a second Rust inventory.

use std::collections::BTreeSet;

use crate::provider::ProviderPlan;
use incan_core::lang::stdlib;

/// Canonical module paths supplied by compiled SDK providers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompiledSdkModules {
    relative_paths: BTreeSet<Vec<String>>,
}

impl CompiledSdkModules {
    /// Build an inventory from module paths relative to the artifact crate root.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn from_relative_paths(paths: impl IntoIterator<Item = Vec<String>>) -> Self {
        Self {
            relative_paths: paths.into_iter().collect(),
        }
    }

    /// Derive SDK-owned source and emission paths from the shared provider plan.
    #[must_use]
    pub(crate) fn from_provider_plan(plan: &ProviderPlan) -> Self {
        let relative_paths = plan
            .active_std_module_paths()
            .into_iter()
            .filter(|path| path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT))
            .map(|path| path.into_iter().skip(1).collect())
            .collect();
        Self { relative_paths }
    }

    /// Return whether a public `std.*` source path is supplied by the artifact.
    #[must_use]
    pub(crate) fn contains_source_path(&self, path: &[String]) -> bool {
        path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) && self.relative_paths.contains(&path[1..])
    }

    /// Return whether an emitted `__incan_std.*` path is supplied by the artifact.
    #[must_use]
    pub(crate) fn contains_emission_path(&self, path: &[String]) -> bool {
        path.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE)
            && self.relative_paths.contains(&path[1..])
    }

    /// Iterate over artifact-owned paths relative to the public `std` namespace.
    pub(crate) fn relative_paths(&self) -> impl Iterator<Item = &[String]> {
        self.relative_paths.iter().map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::CompiledSdkModules;

    #[test]
    fn inventory_projects_provider_namespace_claims() {
        let modules = CompiledSdkModules::from_relative_paths([
            vec!["fs".to_string(), "locking".to_string()],
            vec!["traits".to_string()],
        ]);

        assert!(modules.contains_source_path(&["std".to_string(), "fs".to_string(), "locking".to_string()]));
        assert!(modules.contains_emission_path(&["__incan_std".to_string(), "traits".to_string(),]));
        assert!(!modules.contains_source_path(&["std".to_string(), "main".to_string()]));
    }
}
