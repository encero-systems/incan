//! Select ordinary standard dependencies from parsed library use and original compiler-owned namespace issuers.
//!
//! Publication shares the consumer's admitted native request. Metadata inspection selects existing checked Loaves
//! only. Neither route reads an SDK inventory or substitutes ambient standard sources for missing metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use incan_frontend::ParsedModule;
use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES;
use incan_lang::lang::stdlib;
use incan_provider::FeatureSelection;
use oven_model::manifest::ProjectManifest;

use crate::build::library_dependencies::{LibraryDependencyInput, PreparedLibraryDependencies};
use crate::build::ordinary_library_native::OrdinaryLibraryNativeProfiles;
use crate::build::{OvenProjectBakeAuthorityContext, bake, reuse};
use crate::error::{CliError, CliResult};
use crate::modules::library_source_seeds;
use crate::oven_store::open_default_oven_store;
use crate::session::CompilationSession;

/// One command's selected standard closure, with original grants and a bounded publication recursion guard.
pub(super) struct StandardDependencies<'a> {
    core: Arc<TrustedStandardSourcePublication>,
    native: Option<Arc<OrdinaryLibraryNativeProfiles>>,
    target: &'a str,
    toolchain: &'a str,
    selected: BTreeMap<&'static str, LibraryDependencyInput>,
    /// Per-package transitive language dependencies; the command cache itself is never a library's closure.
    closures: BTreeMap<&'static str, BTreeSet<&'static str>>,
    visiting: BTreeSet<&'static str>,
}

impl<'a> StandardDependencies<'a> {
    /// Retain the original core source and optional publication request; absence means metadata-only selection.
    pub(super) fn new(
        core: Arc<TrustedStandardSourcePublication>,
        native: Option<Arc<OrdinaryLibraryNativeProfiles>>,
        target: &'a str,
        toolchain: &'a str,
    ) -> Self {
        Self {
            core,
            native,
            target,
            toolchain,
            selected: BTreeMap::new(),
            closures: BTreeMap::new(),
            visiting: BTreeSet::new(),
        }
    }

    /// Admit exactly the registry-owned standard packages used by the same source set library publication checks.
    ///
    /// Core is mandatory. Other packages come from the session's feature-projected module-use facts, and their
    /// transitive standard dependencies retain separate original namespace grants. Unknown roots remain unadmitted.
    pub(super) fn for_library(
        &mut self,
        manifest: &ProjectManifest,
        features: &FeatureSelection,
    ) -> CliResult<Vec<LibraryDependencyInput>> {
        let packages = self.library_packages(manifest, features)?;
        self.inputs(&packages)
    }

    /// Collect this library's own parsed use and selected transitives without inheriting previously cached siblings.
    fn library_packages(
        &mut self,
        manifest: &ProjectManifest,
        features: &FeatureSelection,
    ) -> CliResult<BTreeSet<&'static str>> {
        let mut packages = BTreeSet::new();
        let root = manifest
            .project_root()
            .canonicalize()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let core_root = self
            .core
            .verified_package_root()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if root != core_root {
            self.select(Arc::clone(&self.core))?;
            packages.insert(self.core.package_identity().0);
        }
        let entry = super::validate_library_entrypoint(manifest)?;
        let dependencies = self.admit(manifest, features, &packages)?;
        let session =
            super::ordinary_library_session(manifest, &entry, features, dependencies, Arc::clone(&self.core))?;
        let modules = parse_library_inputs(&entry, &session)?;
        let used = session.provider_module_paths(&modules);
        let foreign = used
            .into_iter()
            .filter(|module| !session.provider_plan().bootstrap_owns_sdk_module(module))
            .collect();
        let mut foreign_packages = packages_for_modules(&foreign);
        foreign_packages.extend(packages_for_native_imports(&modules)?);
        if let Some(own) = session.provider_plan().standard_source_publication() {
            foreign_packages.remove(own.package_identity().0);
        }
        for package in foreign_packages {
            let source = TrustedStandardSourcePublication::discover(package)
                .map_err(|error| CliError::failure(error.to_string()))?
                .ok_or_else(|| {
                    CliError::failure(format!(
                        "ordinary standard dependency `{package}` has no compiler-owned source selection"
                    ))
                })?;
            self.select(Arc::new(source))?;
            packages.insert(package);
            packages.extend(
                self.closures
                    .get(package)
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "ordinary selected package `{package}` has no retained dependency closure"
                        ))
                    })?
                    .iter()
                    .copied(),
            );
        }
        Ok(packages)
    }

    /// Retain only the original cached handoffs requested by one library's complete language closure.
    fn inputs(&self, packages: &BTreeSet<&'static str>) -> CliResult<Vec<LibraryDependencyInput>> {
        packages
            .iter()
            .map(|package| {
                self.selected.get(package).cloned().ok_or_else(|| {
                    CliError::failure(format!("ordinary dependency `{package}` lacks its selected handoff"))
                })
            })
            .collect()
    }

    /// Join the current explicit source dependency graph with original standard namespace handoffs.
    fn admit(
        &self,
        manifest: &ProjectManifest,
        features: &FeatureSelection,
        packages: &BTreeSet<&'static str>,
    ) -> CliResult<Arc<PreparedLibraryDependencies>> {
        let plan = incan_provider::PackageFeaturePlan::resolve(manifest, features)
            .map_err(|error| CliError::failure(error.to_string()))?;
        Ok(Arc::new(
            PreparedLibraryDependencies::from_source_feature_plan_with_namespaces(
                &plan,
                self.inputs(packages)?,
                self.target,
                self.toolchain,
                *open_default_oven_store()?.limits(),
            )?,
        ))
    }

    /// Select one exact package, publishing only when a native request was explicitly granted to this command.
    fn select(&mut self, source: Arc<TrustedStandardSourcePublication>) -> CliResult<()> {
        let package = source.package_identity().0;
        if self.selected.contains_key(package) {
            return Ok(());
        }
        if !self.visiting.insert(package) {
            return Err(CliError::failure(format!(
                "ordinary standard package dependency cycle at `{package}`"
            )));
        }
        let root = source
            .verified_package_root()
            .map_err(|error| CliError::failure(error.to_string()))?
            .to_path_buf();
        let manifest =
            ProjectManifest::load(&root.join("loaf.toml")).map_err(|error| CliError::failure(error.to_string()))?;
        let features = FeatureSelection::default();
        let dependencies = self.library_packages(&manifest, &features)?;
        if let Some(native) = &self.native {
            let store = open_default_oven_store()?;
            let targets = bake::discover_oven_bake_project_targets(&root)?;
            let mut authority = OvenProjectBakeAuthorityContext {
                requested_target: Some(self.target.to_string()),
                ..OvenProjectBakeAuthorityContext::default()
            };
            if reuse::try_reuse_baked_project(&root, &targets, &store, &features, Some(self.target), &mut authority)?
                .is_none()
            {
                let session = CompilationSession::discover_with_admitted_standard_source(
                    &super::validate_library_entrypoint(&manifest)?,
                    &features,
                    self.admit(&manifest, &features, &dependencies)?,
                    Arc::clone(&source),
                )?;
                let input = super::AdmittedLibraryPreparation::with_ordinary_native(session, Arc::clone(native))?;
                bake::bake_admitted_library(&input, &features, Some(self.target))?;
            }
        }
        let input = super::select_standard_checked_dependency(source)?;
        self.selected.insert(package, input);
        self.closures.insert(package, dependencies);
        self.visiting.remove(package);
        Ok(())
    }
}

/// Parse the existing publication source set without collecting ambient standard source or projecting models.
fn parse_library_inputs(entry: &Path, session: &CompilationSession) -> CliResult<Vec<ParsedModule>> {
    library_source_seeds(entry, session)?
        .into_iter()
        .map(|(path, name, path_segments)| {
            let source = crate::project::read_source(&path.to_string_lossy())?;
            let ast = session
                .parse_source_for_collection(&path, &source)
                .and_then(|parsed| session.project_parsed_program(parsed))
                .map_err(|errors| {
                    CliError::failure(format!(
                        "ordinary standard dependency import collection failed for {}: {errors:?}",
                        path.display()
                    ))
                })?;
            Ok(ParsedModule {
                name,
                path_segments,
                file_path: path,
                source,
                ast,
            })
        })
        .collect()
}

/// Select package owners of explicit native facet imports through their compiler-pinned authored declarations.
fn packages_for_native_imports(modules: &[ParsedModule]) -> CliResult<BTreeSet<&'static str>> {
    use incan_frontend::ast::{Declaration, ImportKind};

    let facets = modules
        .iter()
        .flat_map(|module| &module.ast.declarations)
        .filter_map(|declaration| {
            let Declaration::Import(import) = &declaration.node else {
                return None;
            };
            let crate_name = match &import.kind {
                ImportKind::RustFrom { crate_name, .. } | ImportKind::RustCrate { crate_name, .. } => crate_name,
                _ => return None,
            };
            stdlib::facets::is_facet(crate_name).then_some(crate_name.as_str())
        })
        .collect::<BTreeSet<_>>();
    let mut packages = BTreeSet::new();
    for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
        let manifest = ProjectManifest::from_str(
            policy.declaration,
            &Path::new(policy.source_directory).join("loaf.toml"),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        if manifest
            .rust_facet
            .and_then(|facet| facet.name)
            .is_some_and(|name| facets.contains(name.as_str()))
        {
            packages.insert(policy.package_name);
        }
    }
    Ok(packages)
}

/// Map canonical module-use facts through the compiler registry; frontend-only and unknown roots select no package.
fn packages_for_modules(modules: &BTreeSet<Vec<String>>) -> BTreeSet<&'static str> {
    modules
        .iter()
        .filter_map(|module| {
            if module.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT)
                || stdlib::is_typechecker_only_stdlib(module)
            {
                return None;
            }
            let root = module.get(1)?;
            STANDARD_PACKAGE_NAMESPACE_POLICIES
                .iter()
                .find(|policy| policy.namespace_roots.contains(&root.as_str()))
                .map(|policy| policy.package_name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{packages_for_modules, packages_for_native_imports};
    use std::collections::BTreeSet;

    /// Package selection follows namespace ownership, coalesces aliases and never invents an unknown owner.
    #[test]
    fn ordinary_standard_imports_select_registry_packages() -> Result<(), Box<dyn std::error::Error>> {
        let modules = [
            ["std", "async", "task"],
            ["std", "async", "channel"],
            ["std", "io", "unused"],
            ["std", "testing", "test"],
            ["std", "rust", "Send"],
            ["std", "unknown", "item"],
        ]
        .into_iter()
        .map(|path| path.into_iter().map(str::to_string).collect())
        .collect();
        assert_eq!(
            packages_for_modules(&modules),
            BTreeSet::from(["incan_stdlib_async", "incan_stdlib_system", "incan_stdlib_testing",])
        );
        let text = "from rust::incan_std_data::json import parse\nfrom rust::incan_std_async::task import RuntimeFuture\nfrom rust::foreign import item\n";
        let tokens = incan_frontend::lexer::lex(text).map_err(|errors| format!("lex: {errors:?}"))?;
        let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
        let parsed = incan_frontend::ParsedModule {
            name: "lib".into(),
            path_segments: vec!["lib".into()],
            file_path: "src/lib.incn".into(),
            source: text.into(),
            ast,
        };
        assert_eq!(
            packages_for_native_imports(&[parsed])?,
            BTreeSet::from(["incan_stdlib_async", "incan_stdlib_data"])
        );
        Ok(())
    }
}
