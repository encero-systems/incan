//! Ordinary Incan source coverage before checking and inside the resulting checked metadata contract.
//!
//! Incan declaration shapes do not determine whether native inputs are complete. Imports and interop demands do.
//! These facts allow the real checker to process normal source, and never certify checking or grant macro execution.

use std::collections::BTreeSet;

use incan_frontend::ParsedModule;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_lang::lang::generated_support::SUPPORT_CRATES_EVERY_PROGRAM_LINKS;
use incan_provider::ProviderPlan;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};

use super::{ManifestDemand, NativeDemandFacts, invalid};
use crate::error::CliResult;

/// Compiler-captured imports and own-source scope, associated with the complete loaded module facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OrdinarySourceDemands {
    schema_version: u32,
    native_imports: BTreeSet<String>,
    native_crates: BTreeSet<String>,
    own_namespace_roots: BTreeSet<String>,
    unsupported_native_source: BTreeSet<String>,
}

impl OrdinarySourceDemands {
    /// Record actual source imports; only a retained genuine source publication can supply own namespace roots.
    pub(super) fn capture(project: &ProjectManifest, modules: &[ParsedModule], plan: &ProviderPlan) -> CliResult<Self> {
        let mut source = Self {
            schema_version: 1,
            native_imports: BTreeSet::new(),
            native_crates: BTreeSet::new(),
            own_namespace_roots: BTreeSet::new(),
            unsupported_native_source: BTreeSet::new(),
        };
        if let Some(owner) = plan.standard_source_publication() {
            if owner
                .verified_package_root()
                .map_err(|error| invalid(error.to_string()))?
                != project
                    .project_root()
                    .canonicalize()
                    .map_err(|error| invalid(error.to_string()))?
            {
                return Err(invalid("ordinary own-source namespace belongs to a different project"));
            }
            source
                .own_namespace_roots
                .extend(owner.namespace_roots().iter().map(|root| (*root).to_string()));
        }
        if !crate::build::rust_extern::collect_rust_extern_contexts(modules).is_empty() {
            source.unsupported_native_source.insert("rust.extern".to_string());
        }
        let declared = project.rust_dependencies();
        for module in modules {
            if module.ast.rust_module_path.is_some() {
                source.unsupported_native_source.insert("rust.module".to_string());
            }
            for declaration in &module.ast.declarations {
                match &declaration.node {
                    Declaration::Import(import) => match &import.kind {
                        ImportKind::RustFrom {
                            crate_name,
                            path,
                            items,
                            version,
                            features,
                        } if version.is_none()
                            && features.is_empty()
                            && !items.is_empty()
                            && (matches!(crate_name.as_str(), "std" | "core" | "alloc")
                                || declared.contains_key(crate_name)
                                || mandatory_native_facet(crate_name)) =>
                        {
                            source.native_crates.insert(crate_name.clone());
                            for item in items {
                                source.native_imports.insert(
                                    std::iter::once(crate_name.as_str())
                                        .chain(path.iter().map(String::as_str))
                                        .chain(std::iter::once(item.name.as_str()))
                                        .collect::<Vec<_>>()
                                        .join("::"),
                                );
                            }
                        }
                        ImportKind::RustFrom { .. } | ImportKind::RustCrate { .. } => {
                            source
                                .unsupported_native_source
                                .insert("native import without complete item coverage".to_string());
                        }
                        _ => (),
                    },
                    Declaration::VocabBlock(_) => {
                        source
                            .unsupported_native_source
                            .insert("vocabulary execution".to_string());
                    }
                    _ => (),
                }
            }
        }
        Ok(source)
    }

    /// Check exact imports and own namespace use without inferring completeness from a physical native subset.
    pub(super) fn require_inspection(&self, facts: &NativeDemandFacts) -> CliResult<()> {
        facts.validate_scalar_coverage()?;
        let roots = self
            .native_imports
            .iter()
            .filter_map(|path| path.split_once("::").map(|(root, _)| root.to_string()))
            .collect::<BTreeSet<_>>();
        if self.schema_version != 1
            || facts.schema_version != 1
            || facts.source_modules.is_empty()
            || roots != self.native_crates
            || !self.native_imports.is_subset(&facts.rust_abi_queries)
            || facts.rust_abi_queries.iter().any(|path| {
                path.split_once("::").is_none_or(|(root, _)| {
                    !matches!(root, "std" | "core" | "alloc") && !self.native_crates.contains(root)
                })
            })
            || !self.unsupported_native_source.is_empty()
            || !facts.public_dependency_modules.is_empty()
            || !facts.provider_contracts.is_empty()
            || !facts.rust_derive_probe_paths.is_empty()
            || !matches!(facts.vocab_manifest, ManifestDemand::Absent)
            || !matches!(facts.c_manifest, ManifestDemand::Absent)
            || !facts.dependency_aliases.is_empty()
            || facts.physical_projections
            || facts.stdlib_facets.iter().any(|facet| !mandatory_native_facet(facet))
            || facts.used_module_paths.iter().any(|path| {
                path.first().map(String::as_str) != Some("std")
                    || path.get(1).is_none_or(|root| !self.own_namespace_roots.contains(root))
            })
        {
            return Err(invalid(format!(
                "ordinary loaded source lacks complete native import or own-namespace coverage: {}",
                serde_json::json!({
                    "native_imports": self.native_imports, "native_crates": self.native_crates,
                    "abi_queries": facts.rust_abi_queries, "unsupported_native_source": self.unsupported_native_source,
                    "used_modules": facts.used_module_paths, "own_namespace_roots": self.own_namespace_roots,
                    "derive_probes": facts.rust_derive_probe_paths, "facets": facts.stdlib_facets,
                    "dependencies": facts.dependency_aliases, "provider_count": facts.provider_contracts.len(),
                    "public_dependencies": facts.public_dependency_modules, "physical_projections": facts.physical_projections,
                    "vocabulary": !matches!(facts.vocab_manifest, ManifestDemand::Absent),
                    "c_interop": !matches!(facts.c_manifest, ManifestDemand::Absent),
                })
            )));
        }
        Ok(())
    }

    /// Match captured non-facet imports to the checked contract instead of allowing serialized root substitution.
    pub(super) fn validate_contract(&self, source_inline_crates: &BTreeSet<String>) -> CliResult<()> {
        // Legacy preparations can retain explicitly unsupported input facts. They cannot consume the ordinary gate.
        if !self.unsupported_native_source.is_empty() {
            return Ok(());
        }
        let imported = self
            .native_crates
            .iter()
            .filter(|root| root.as_str() != "std" && !incan_lang::lang::stdlib::facets::is_facet(root))
            .cloned()
            .collect::<BTreeSet<_>>();
        if imported != *source_inline_crates {
            return Err(invalid(
                "ordinary native imports disagree with checked source crate roots",
            ));
        }
        Ok(())
    }
}

/// Identify implicit physical facets from the existing generated-support registry, excluding proc-macro crates.
pub(super) fn mandatory_native_facet(name: &str) -> bool {
    incan_lang::lang::stdlib::facets::is_facet(name) && SUPPORT_CRATES_EVERY_PROGRAM_LINKS.contains(&name)
}
