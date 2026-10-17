//! Ordinary Incan source coverage before checking and inside the resulting checked metadata contract.
//!
//! Incan declaration shapes do not determine whether native inputs are complete. Imports and interop demands do.
//! These facts allow the real checker to process normal source, and never certify checking or grant macro execution.

use std::collections::{BTreeMap, BTreeSet};

use incan_frontend::ParsedModule;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_lang::lang::generated_support::SUPPORT_CRATES_EVERY_PROGRAM_LINKS;
use incan_lang::lang::stdlib;
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
    /// Exact own-facet or declared namespace-companion extern declarations; absent old coverage grants no paths.
    #[serde(default)]
    rust_extern_paths: BTreeSet<String>,
    /// Original report labels mapped to actual inspected backing items; absent old facts retain identity coverage.
    #[serde(default)]
    rust_extern_abi_paths: Option<BTreeMap<String, String>>,
    /// Successful checker lookups retained only after complete manifest ABI validation, including transitive types.
    #[serde(default)]
    checked_abi_queries: BTreeSet<String>,
    /// Actual collected derive paths; absent older coverage cannot authorize macro requests.
    #[serde(default)]
    rust_derive_paths: BTreeSet<String>,
    /// Source-only derive demands backed by the original manifest declaration; absent old facts grant none.
    /// Native root, host macro, compiler and physical-output admission remain separate execution requirements.
    #[serde(default)]
    declared_rust_derive_paths: BTreeSet<String>,
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
            rust_extern_paths: BTreeSet::new(),
            rust_extern_abi_paths: Some(BTreeMap::new()),
            checked_abi_queries: BTreeSet::new(),
            rust_derive_paths: crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths(modules)
                .into_iter()
                .collect(),
            declared_rust_derive_paths: BTreeSet::new(),
            native_crates: BTreeSet::new(),
            own_namespace_roots: BTreeSet::new(),
            unsupported_native_source: BTreeSet::new(),
        };
        let mut own_native_companions = BTreeSet::new();
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
            let declaration = std::str::from_utf8(
                owner
                    .verified_declaration_bytes()
                    .map_err(|error| invalid(error.to_string()))?,
            )
            .map_err(|error| invalid(error.to_string()))?;
            let pinned = ProjectManifest::from_str(declaration, &project.project_root().join("loaf.toml"))
                .map_err(|error| invalid(error.to_string()))?;
            // Path declarations are classified as Rust or Incan by observing the child's source tree. Own-source
            // coverage needs only the original declared name; actual native admission still verifies that child.
            own_native_companions.extend(
                pinned
                    .rust_dependencies()
                    .keys()
                    .chain(pinned.library_dependencies().keys())
                    .filter(|name| namespace_path_companion(&source.own_namespace_roots, name))
                    .cloned(),
            );
        }
        let own_native_facet = plan
            .standard_source_publication()
            .map(|owner| {
                owner
                    .verified_native_facet_name()
                    .map_err(|error| invalid(error.to_string()))
            })
            .transpose()?
            .flatten();
        for context in crate::build::rust_extern::collect_rust_extern_contexts(modules) {
            if let Some(root) = context.rust_module_path.split("::").next()
                && (own_native_facet.as_deref() == Some(root) || own_native_companions.contains(root))
            {
                source.native_crates.insert(root.to_string());
                let label = format!("{}::{}", context.rust_module_path, context.item_name);
                let abi = crate::build::rust_extern::ordinary_rust_extern_abi_path(&context, modules, plan)?;
                source.native_imports.insert(abi.clone());
                source.rust_extern_paths.insert(label.clone());
                if let Some(paths) = source.rust_extern_abi_paths.as_mut()
                    && let Some(previous) = paths.insert(label, abi.clone())
                    && previous != abi
                {
                    return Err(invalid(
                        "ordinary extern report label has conflicting backing ABI items",
                    ));
                }
            } else {
                source.unsupported_native_source.insert("rust.extern".to_string());
            }
        }
        let declared = project.rust_dependencies();
        #[cfg(feature = "rust_inspect")]
        {
            let declared_derive_roots = modules
                .iter()
                .flat_map(|module| {
                    crate::modules::collect_declared_rust_derive_dependency_uses(module, Some(project), false)
                })
                .filter(|import| declared.contains_key(&import.crate_name))
                .map(|import| import.crate_name.replace('-', "_"))
                .collect::<BTreeSet<_>>();
            source.declared_rust_derive_paths.extend(
                source
                    .rust_derive_paths
                    .iter()
                    .filter(|path| {
                        path.split_once("::")
                            .is_some_and(|(root, _)| declared_derive_roots.contains(root))
                    })
                    .cloned(),
            );
        }
        let admitted_facets = plan
            .active_sdk_records()
            .filter(|record| {
                matches!(
                    record.provenance,
                    incan_provider::ProviderProvenance::StandardPublication { .. }
                )
            })
            .map(|record| {
                let namespace = incan_frontend::provider::namespaces::SelectedProviderNamespace::from_plan(
                    plan,
                    &record.identity.stable_key(),
                )
                .map_err(invalid)?;
                Ok(namespace
                    .record()
                    .namespace_claims
                    .iter()
                    .filter_map(|module| stdlib::stdlib_facet_for(module).map(str::to_string))
                    .collect::<BTreeSet<_>>())
            })
            .collect::<CliResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();
        for module in modules {
            if module.ast.rust_module_path.as_ref().is_some_and(|path| {
                path.node.split("::").next().is_none_or(|root| {
                    own_native_facet.as_deref() != Some(root) && !own_native_companions.contains(root)
                })
            }) {
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
                                || mandatory_native_facet(crate_name)
                                || admitted_facets.contains(crate_name)
                                || own_native_facet.as_ref() == Some(crate_name)) =>
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

    /// Bind successful checked lookups after their complete manifest ABI has been independently validated.
    /// Source-only capture leaves this empty: declared imports alone cannot invent a transitive metadata demand.
    pub(super) fn bind_checked_abi_queries(
        &mut self,
        queries: &BTreeSet<String>,
        captured_queries: &BTreeSet<String>,
    ) -> CliResult<()> {
        if !queries.is_subset(captured_queries) {
            return Err(invalid(
                "checked ABI queries differ from captured publication requirements",
            ));
        }
        self.checked_abi_queries = queries.clone();
        Ok(())
    }

    /// Check exact imports and own namespace use without inferring completeness from a physical native subset.
    pub(super) fn require_inspection(
        &self,
        facts: &NativeDemandFacts,
        admitted_modules: &BTreeSet<Vec<String>>,
        admitted_dependencies: &BTreeSet<String>,
        vocabulary: Option<&serde_json::Value>,
    ) -> CliResult<()> {
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
            || !self.checked_abi_queries.is_subset(&facts.rust_abi_queries)
            || facts.rust_abi_queries.iter().any(|path| {
                path.split_once("::").is_none_or(|(root, _)| {
                    !matches!(root, "std" | "core" | "alloc")
                        && !self.native_crates.contains(root)
                        && !self.checked_abi_queries.contains(path)
                })
            })
            || !self.unsupported_native_source.is_empty()
            || !facts.public_dependency_modules.is_subset(admitted_modules)
            || facts.rust_derive_probe_paths != self.rust_derive_paths
            || !self.declared_rust_derive_paths.is_subset(&self.rust_derive_paths)
            || self.rust_derive_paths.iter().any(|path| {
                !self.declared_rust_derive_paths.contains(path)
                    && (self.own_namespace_roots.is_empty()
                        || path
                            .split_once("::")
                            .is_none_or(|(root, _)| !self.native_crates.contains(root)))
            })
            || match (&facts.vocab_manifest, vocabulary) {
                (ManifestDemand::Absent, None) => false,
                (ManifestDemand::Declared(actual), Some(expected)) => actual != expected,
                _ => true,
            }
            || !matches!(facts.c_manifest, ManifestDemand::Absent)
            || facts.dependency_aliases != *admitted_dependencies
            || facts.physical_projections
            || facts.stdlib_facets.iter().any(|facet| {
                !mandatory_native_facet(facet)
                    && !self.native_crates.contains(facet)
                    && !admitted_modules
                        .iter()
                        .any(|module| stdlib::stdlib_facet_for(module) == Some(facet.as_str()))
                    && !self.own_namespace_roots.iter().any(|root| {
                        stdlib::stdlib_facet_for(&[stdlib::STDLIB_ROOT.to_string(), root.clone()])
                            == Some(facet.as_str())
                    })
            })
            || facts.used_module_paths.iter().any(|path| {
                !stdlib::is_typechecker_only_stdlib(path)
                    && !admitted_modules.contains(path)
                    && (path.first().map(String::as_str) != Some("std")
                        || path.get(1).is_none_or(|root| !self.own_namespace_roots.contains(root)))
            })
        {
            return Err(invalid(format!(
                "ordinary loaded source lacks complete native import or own-namespace coverage: {}",
                serde_json::json!({
                    "native_imports": self.native_imports, "native_crates": self.native_crates,
                    "abi_queries": facts.rust_abi_queries, "unsupported_native_source": self.unsupported_native_source,
                    "checked_abi_queries": self.checked_abi_queries, "admitted_modules": admitted_modules,
                    "used_modules": facts.used_module_paths, "own_namespace_roots": self.own_namespace_roots,
                    "derive_probes": facts.rust_derive_probe_paths, "declared_derive_probes": self.declared_rust_derive_paths,
                    "facets": facts.stdlib_facets,
                    "dependencies": facts.dependency_aliases, "provider_count": facts.provider_contracts.len(),
                    "admitted_dependencies": admitted_dependencies, "source_modules": facts.source_modules,
                    "public_dependencies": facts.public_dependency_modules, "physical_projections": facts.physical_projections,
                    "vocabulary": !matches!(facts.vocab_manifest, ManifestDemand::Absent),
                    "c_interop": !matches!(facts.c_manifest, ManifestDemand::Absent),
                })
            )));
        }
        Ok(())
    }

    /// Match reported extern labels to exact owned declarations and independently captured ABI queries.
    pub(super) fn require_extern_paths(&self, paths: &[String], facts: &NativeDemandFacts) -> CliResult<()> {
        let selected = paths.iter().cloned().collect::<BTreeSet<_>>();
        let abi = match &self.rust_extern_abi_paths {
            Some(bindings) => {
                if bindings.keys().cloned().collect::<BTreeSet<_>>() != self.rust_extern_paths {
                    return Err(invalid(
                        "ordinary extern ABI binding keys differ from captured report labels",
                    ));
                }
                bindings.values().cloned().collect::<BTreeSet<_>>()
            }
            None => self.rust_extern_paths.clone(),
        };
        if selected.len() != paths.len()
            || selected != self.rust_extern_paths
            || !abi.is_subset(&self.native_imports)
            || !abi.is_subset(&facts.rust_abi_queries)
        {
            return Err(invalid(
                "ordinary Rust extern labels differ from captured owned ABI coverage",
            ));
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
            .filter(|root| {
                root.as_str() != "std"
                    && !stdlib::facets::is_facet(root)
                    && !namespace_path_companion(&self.own_namespace_roots, root)
            })
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

/// Match a path companion of the captured own namespace using the same registry classification as dependency
/// collection. Capture additionally requires that crate in the original publisher declaration. This only identifies
/// source demands; it cannot select a native owner, satisfy an ABI query or grant macro execution.
fn namespace_path_companion(roots: &BTreeSet<String>, crate_name: &str) -> bool {
    stdlib::is_path_extra_crate_dep(crate_name)
        && roots
            .iter()
            .filter_map(|root| stdlib::find_namespace(root))
            .any(|namespace| {
                namespace
                    .extra_crate_deps
                    .iter()
                    .any(|dependency| dependency.crate_name == crate_name)
            })
}

/// Identify implicit physical facets from the existing generated-support registry, excluding proc-macro crates.
pub(super) fn mandatory_native_facet(name: &str) -> bool {
    incan_lang::lang::stdlib::facets::is_facet(name) && SUPPORT_CRATES_EVERY_PROGRAM_LINKS.contains(&name)
}
