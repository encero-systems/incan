//! Command-owned source inputs for the standard-library metadata cache (#1337/#1698).
//!
//! Legacy readers retain their existing ambient discovery. Genuine source publication retains its exact capability
//! and member owners, checks freshness at checker/cache handoffs and never source-loads a foreign namespace.

use std::collections::HashMap;
use std::sync::Arc;

use oven_model::toolchain_layout::CompilerOwnedSourceMember;

use crate::provider::ProviderPlan;
use crate::provider::source_policy::TrustedStandardSourcePublication;

/// Explicit source-read policy: an absent own-source capability never implies ambient legacy permission.
#[derive(Clone, Debug, Default)]
enum StdlibSourceSelection {
    #[default]
    Legacy,
    OrdinaryCheckedOnly,
    StandardSource(Arc<TrustedStandardSourcePublication>),
}

/// Original source selection and members behind one command's parsed metadata, independent of cache entry spelling.
#[derive(Clone, Default)]
pub(super) struct StdlibSourceInputs {
    selection: StdlibSourceSelection,
    members: HashMap<String, Arc<CompilerOwnedSourceMember>>,
    failure: Option<String>,
    #[cfg(test)]
    parses: usize,
}

impl std::fmt::Debug for StdlibSourceInputs {
    /// Describe selection and retained module names without requiring original file handles to be printable.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StdlibSourceInputs")
            .field("selection", &self.selection)
            .field("members", &self.members.keys().collect::<Vec<_>>())
            .field("failure", &self.failure)
            .finish()
    }
}

impl StdlibSourceInputs {
    /// Bind the original source Arc, invalidating metadata when authority changes and verifying reused member owners.
    pub(super) fn bind(&mut self, plan: &ProviderPlan) -> Result<bool, String> {
        let next = if let Some(source) = plan.standard_source_publication() {
            StdlibSourceSelection::StandardSource(Arc::clone(source))
        } else if plan.is_admitted_library_context() {
            StdlibSourceSelection::OrdinaryCheckedOnly
        } else {
            StdlibSourceSelection::Legacy
        };
        let same = match (&self.selection, &next) {
            (StdlibSourceSelection::Legacy, StdlibSourceSelection::Legacy)
            | (StdlibSourceSelection::OrdinaryCheckedOnly, StdlibSourceSelection::OrdinaryCheckedOnly) => true,
            (StdlibSourceSelection::StandardSource(current), StdlibSourceSelection::StandardSource(next)) => {
                Arc::ptr_eq(current, next)
            }
            _ => false,
        };
        if !same {
            *self = Self {
                selection: next,
                ..Self::default()
            };
        }
        self.failure = None;
        if let Err(error) = plan
            .verify_standard_source_publication()
            .and_then(|()| self.verify_members())
        {
            self.failure = Some(error.clone());
            return Err(error);
        }
        Ok(!same)
    }

    /// Revalidate exact original declaration/member bytes at a checker or cache handoff, without reparsing them.
    pub(super) fn verify(&self) -> Result<(), String> {
        if let StdlibSourceSelection::StandardSource(source) = &self.selection {
            source.verify().map_err(|error| error.to_string())?;
        }
        self.verify_members()
    }

    /// Check original member owners without rereading an already verified declaration at the same handoff.
    fn verify_members(&self) -> Result<(), String> {
        for member in self.members.values() {
            member.verified_bytes().map_err(|error| error.to_string())?;
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        Ok(())
    }

    /// Read only an owned pinned module, retaining its original file owner; foreign metadata comes from providers.
    pub(super) fn read_module(&mut self, module: &[String]) -> Option<String> {
        let source = match &self.selection {
            StdlibSourceSelection::Legacy => {
                let relative = incan_lang::lang::stdlib::stdlib_stub_path(module)?;
                let path = super::find_stdlib_file(&relative)?;
                return std::fs::read_to_string(&path)
                    .map_err(|error| {
                        tracing::debug!(path = %path.display(), %error, "failed to read stdlib file");
                    })
                    .ok();
            }
            StdlibSourceSelection::OrdinaryCheckedOnly => return None,
            StdlibSourceSelection::StandardSource(source) => Arc::clone(source),
        };
        if module.first().map(String::as_str) != Some("std")
            || !module
                .get(1)
                .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
            || self.failure.is_some()
        {
            return None;
        }
        let key = module.join(".");
        let result = (|| {
            let member = if let Some(member) = self.members.get(&key) {
                Arc::clone(member)
            } else {
                let member = Arc::new(source.open_source_module(module).map_err(|error| error.to_string())?);
                self.members.insert(key, Arc::clone(&member));
                member
            };
            let bytes = member.verified_bytes().map_err(|error| error.to_string())?;
            String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
        })();
        match result {
            Ok(text) => Some(text),
            Err(error) => {
                self.failure = Some(error);
                None
            }
        }
    }

    /// Match package-origin source identities against pinned ownership rather than an ambient catalog/manifest.
    pub(super) fn owns_package_identity(&self, module: &[String], library: &str) -> Option<bool> {
        let source = match &self.selection {
            StdlibSourceSelection::Legacy => return None,
            StdlibSourceSelection::OrdinaryCheckedOnly => return Some(false),
            StdlibSourceSelection::StandardSource(source) => source,
        };
        let policy = module.get(1).and_then(|root| {
            incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES
                .iter()
                .find(|policy| policy.namespace_roots.contains(&root.as_str()))
        });
        Some(
            module
                .get(1)
                .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
                && policy.is_some_and(|policy| policy.package_name == library),
        )
    }

    /// Count an actual parse demanded by retained source metadata, excluding legacy readers and cached lookups.
    pub(super) fn record_parse(&mut self) {
        #[cfg(test)]
        if matches!(self.selection, StdlibSourceSelection::StandardSource(_)) {
            self.parses += 1;
        }
    }

    /// Actual retained-source parser calls for focused JEC controls.
    #[cfg(test)]
    pub(super) fn parses(&self) -> usize {
        self.parses
    }
}
