//! Checked Rust emission shared by explicit debug emission and compiler-development bootstrap.
//!
//! This boundary returns generated source, not permission to execute it. Native producers must separately bind their
//! source receipt and retain an admitted execution plan. It never selects an unbound ordinary build route.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::backend::{BackendExecutionReceipt, IrCodegen, ProjectGenerator, digest_output};
use crate::error::{CliError, CliResult};
use crate::modules::collect_modules_detailed_with_session;
use crate::session::CompilationSession;
use incan_frontend::ParsedModule;
use incan_provider::compiled_sdk::CompiledSdkModules;

/// Collected source and its original semantic authority, kept together until checked emission finishes.
pub struct CheckedRustSource {
    pub(crate) session: CompilationSession,
    pub(crate) modules: Vec<ParsedModule>,
}

/// Actual emitted bytes and the backend receipt recording which checked sources produced them.
pub struct CheckedRustEmission {
    /// Debug Rust root only; callers needing a runnable crate must separately materialize its module tree.
    pub source: String,
    /// Checked backend selection and emitted-source identity; native execution remains a separate boundary.
    pub backend_receipt: BackendExecutionReceipt,
}

impl CheckedRustSource {
    /// Collect one explicit emission request through the same full-artifact session used by debug emission.
    pub fn discover(entrypoint: &Path) -> CliResult<Self> {
        let entrypoint = std::path::absolute(entrypoint).map_err(|error| CliError::failure(error.to_string()))?;
        let session = CompilationSession::discover_with_feature_selection(
            &entrypoint,
            &incan_provider::FeatureSelection::default(),
        )?;
        let modules = collect_modules_detailed_with_session(entrypoint, &session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
        Ok(Self { session, modules })
    }

    /// Analyze once and emit through the existing IR backend, retaining the caller's prepared inspection authority.
    /// The explicit debugger passes no inspection workspace; a bootstrap producer retains its selected source owners
    /// through this call. Neither path may substitute ambient source for the checked module cache.
    pub fn emit(
        &self,
        strict: bool,
        #[cfg(feature = "rust_inspect")] inspection: Option<&Path>,
    ) -> CliResult<CheckedRustEmission> {
        let (main, _) = self
            .modules
            .split_last()
            .ok_or_else(|| CliError::failure("No modules found"))?;
        let codegen = self.checked_codegen(
            strict,
            #[cfg(feature = "rust_inspect")]
            inspection,
        )?;
        let source = codegen
            .try_generate(&main.ast)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        let backend_receipt = super::backend_selection::finalize_backend_receipt(
            &super::backend_selection::select_build_backend(&self.modules),
            digest_output(&[source.as_str()]),
        )?;
        Ok(CheckedRustEmission {
            source,
            backend_receipt,
        })
    }

    /// Materialize the complete checked executable project through the ordinary IR project generator.
    ///
    /// Debug emission deliberately returns only the root. A native bootstrap must also write every source-backed
    /// dependency and the selected providers' compatibility facade before binding its generated-source receipt.
    /// The caller supplies the already resolved Rust dependencies; this boundary never resolves or executes them.
    pub(super) fn generate_project(
        &self,
        output: &Path,
        name: &str,
        dependencies: &[oven_model::manifest::DependencySpec],
        #[cfg(feature = "rust_inspect")] inspection: Option<&Path>,
    ) -> CliResult<BackendExecutionReceipt> {
        let (main, modules) = self
            .modules
            .split_last()
            .ok_or_else(|| CliError::failure("No modules found"))?;
        let providers = self.session.provider_plan_for_modules(&self.modules)?;
        let compiled = CompiledSdkModules::from_provider_plan(&providers);
        let module_paths = modules
            .iter()
            .filter(|module| !compiled.contains_emission_path(&module.path_segments))
            .map(|module| module.path_segments.clone())
            .collect::<Vec<_>>();
        let mut codegen = self.checked_codegen(
            false,
            #[cfg(feature = "rust_inspect")]
            inspection,
        )?;
        codegen.set_preserve_dependency_public_items(!module_paths.is_empty());
        codegen.set_registry_package_identity(Some(name.to_string()));
        codegen.set_root_source_module_name(
            main.file_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string),
        );
        let requirements = incan_provider::requirements::collect_project_requirements(
            &self.modules,
            &self.session.library_manifest_index,
        )?;
        let mut generator = ProjectGenerator::new(output, name, true);
        generator.set_provider_plan(&providers);
        generator.set_sdk_path_dependencies(requirements.sdk_path_dependencies);
        generator.set_stdlib_facets(requirements.stdlib_facets);
        generator.set_dependencies(dependencies.to_vec());
        generator.set_include_dev_dependencies(false);
        let manifest = self.session.manifest.as_ref();
        if let Some(project) = manifest.and_then(|manifest| manifest.project.as_ref()) {
            generator.set_package_metadata(project.version.clone(), project.license.clone());
        }
        generator.set_rust_edition(Some(
            manifest
                .and_then(|manifest| manifest.rust_edition())
                .unwrap_or("2024")
                .to_string(),
        ));
        let (root, rust_modules) = codegen
            .try_generate_multi_file_nested(&main.ast, &module_paths)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate_nested(&root, &rust_modules)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
        super::backend_selection::finalize_backend_receipt(
            &super::backend_selection::select_build_backend(&self.modules),
            super::rust_extern::multi_file_output_identity(&root, &rust_modules),
        )
    }

    /// Prepare the single checked IR emitter used by both root-only debug output and complete native projects.
    /// Provider symbol modules remain distinct from emitted source modules, and all lowering consumes the original
    /// session analysis/cache rather than reacquiring ambient source authority.
    fn checked_codegen(
        &self,
        strict: bool,
        #[cfg(feature = "rust_inspect")] inspection: Option<&Path>,
    ) -> CliResult<IrCodegen<'_>> {
        let (main, dependencies) = self
            .modules
            .split_last()
            .ok_or_else(|| CliError::failure("No modules found"))?;
        let provider_plan = self.session.provider_plan_for_modules(&self.modules)?;
        let compiled = CompiledSdkModules::from_provider_plan(&provider_plan);
        let mut codegen = IrCodegen::new();
        codegen.set_strict_generated_lints(strict);
        if let Some(manifest) = &self.session.manifest {
            codegen.set_declared_crate_names(manifest.declared_rust_crate_names());
        }
        codegen.set_provider_plan(Arc::clone(&provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(inspection) = inspection {
            codegen.set_rust_inspect_manifest_dir(inspection.to_path_buf());
        }
        for module in dependencies {
            if compiled.contains_emission_path(&module.path_segments) {
                codegen.add_dependency_symbol_module_with_path_segments(
                    &module.name,
                    &module.ast,
                    module.path_segments.clone(),
                );
            } else {
                codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
            }
        }
        let analysis = self
            .session
            .analyze_modules(
                &self.modules,
                #[cfg(feature = "rust_inspect")]
                inspection,
            )
            .map_err(|failure| CliError::failure(failure.render_human()))?;
        let type_info = |module: &ParsedModule| {
            analysis.type_info_for_path(&module.file_path).cloned().ok_or_else(|| {
                CliError::failure(format!("missing session analysis for {}", module.file_path.display()))
            })
        };
        let dependency_types = dependencies
            .iter()
            .map(|module| Ok((module.path_segments.clone(), type_info(module)?)))
            .collect::<CliResult<HashMap<_, _>>>()?;
        codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
        codegen.set_prechecked_type_info(type_info(main)?, dependency_types);
        Ok(codegen)
    }
}

#[cfg(test)]
mod tests {
    use super::CheckedRustSource;

    /// Complete native generation retains sibling modules and binds their emitted bytes, while debug output stays
    /// root-only. The same checked analysis rejects malformed dependencies before a new project is materialized.
    #[test]
    fn checked_native_project_materializes_and_binds_sibling_modules() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let project = directory.path().join("source");
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname = \"cold_emission_control\"\nversion = \"0.1.0\"\n",
        )?;
        let entry = project.join("src/main.incn");
        let sibling = project.join("src/helper.incn");
        std::fs::write(
            &entry,
            "from helper import answer\n\ndef main() -> None:\n    print(answer())\n",
        )?;
        std::fs::write(&sibling, "pub def answer() -> int:\n    return 41\n")?;
        let output = directory.path().join("generated");
        let checked = CheckedRustSource::discover(&entry)?;
        let debug = checked.emit(
            false,
            #[cfg(feature = "rust_inspect")]
            None,
        )?;
        assert!(!output.exists(), "debug emission must not materialize a crate");
        assert!(!debug.source.contains(" as answer;"));
        let first = checked.generate_project(
            &output,
            "cold_emission_control",
            &[],
            #[cfg(feature = "rust_inspect")]
            None,
        )?;
        let root = std::fs::read_to_string(output.join("src/main.rs"))?;
        let dependency = std::fs::read_to_string(output.join("src/helper.rs"))?;
        assert!(root.contains("mod helper;"), "missing sibling declaration: {root}");
        assert!(
            dependency.contains("pub fn ") && dependency.contains(" as answer;"),
            "missing sibling definition or public alias: {dependency}"
        );
        assert!(dependency.contains("41"));
        std::fs::write(&sibling, "pub def answer() -> int:\n    return 42\n")?;
        let changed = CheckedRustSource::discover(&entry)?.generate_project(
            &output,
            "cold_emission_control",
            &[],
            #[cfg(feature = "rust_inspect")]
            None,
        )?;
        assert_ne!(
            first.output_identity, changed.output_identity,
            "a dependency edit must change the emitted project identity"
        );
        assert!(std::fs::read_to_string(output.join("src/helper.rs"))?.contains("42"));
        std::fs::write(&sibling, "pub def answer() -> int:\n    return \"wrong\"\n")?;
        let refused = directory.path().join("refused");
        assert!(
            CheckedRustSource::discover(&entry)?
                .generate_project(
                    &refused,
                    "cold_emission_control",
                    &[],
                    #[cfg(feature = "rust_inspect")]
                    None
                )
                .is_err()
        );
        assert!(
            !refused.exists(),
            "unchecked dependency must not reach project generation"
        );
        Ok(())
    }

    /// A fresh dedicated source root permits excluded compiler outputs but rejects changed source topology.
    #[cfg(unix)]
    #[test]
    fn cold_source_observer_keeps_outputs_outside_authored_generation() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let project = directory.path().canonicalize()?;
        std::fs::create_dir(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname = \"cold_source_outputs\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(project.join("src/main.incn"), "def main() -> None:\n    print(41)\n")?;
        let mut source_inputs = crate::build::ProjectSourceAuthorityDigester::observing_authored_sources();
        source_inputs.digest(&project)?;
        source_inputs.ensure_original_sources_current()?;
        std::fs::create_dir_all(project.join("target/lib"))?;
        std::fs::write(project.join("target/lib/generated"), "not authored")?;
        source_inputs.ensure_original_sources_current()?;
        std::fs::write(project.join("src/added.incn"), "VALUE: int = 1\n")?;
        std::fs::remove_file(project.join("src/added.incn"))?;
        assert!(source_inputs.ensure_original_sources_current().is_err());
        Ok(())
    }

    /// Original authored observations reject a transient collected program even after its source bytes are restored.
    #[cfg(unix)]
    #[test]
    fn cold_source_observer_rejects_restored_authored_program() -> Result<(), Box<dyn std::error::Error>> {
        use crate::build::source_authority::{
            digest_baked_project_source_authority, digest_project_development_dependencies,
        };
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        let project = root.join("source");
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(
            project.join("loaf.toml"),
            "[project]\nname = \"authored_source_probe\"\nversion = \"0.1.0\"\n",
        )?;
        let entry = project.join("src/main.incn");
        let original = "def main() -> None:\n    print(41)\n";
        let transient = "def main() -> None:\n    print(42)\n";
        std::fs::write(&entry, original)?;
        let mut source_inputs = crate::build::ProjectSourceAuthorityDigester::observing_authored_sources();
        let source_before = source_inputs.digest(&project)?;
        source_inputs.ensure_original_sources_current()?;
        let dependencies_before = digest_project_development_dependencies(&project)?;
        std::fs::write(&entry, transient)?;
        let checked = CheckedRustSource::discover(&entry)?;
        assert_eq!(checked.modules.last().ok_or("missing checked root")?.source, transient);
        let generated = root.join("generated");
        checked.generate_project(
            &generated,
            "authored_source_probe",
            &[],
            #[cfg(feature = "rust_inspect")]
            None,
        )?;
        let emitted = std::fs::read_to_string(generated.join("src/main.rs"))?;
        std::fs::write(&entry, original)?;
        assert_eq!(source_before, digest_baked_project_source_authority(&project)?);
        assert_eq!(dependencies_before, digest_project_development_dependencies(&project)?);
        assert!(
            emitted.contains("42"),
            "transient AST did not reach emission: {emitted}"
        );
        assert!(
            !emitted.contains("41"),
            "original value unexpectedly emitted: {emitted}"
        );
        let refusal = source_inputs
            .ensure_original_sources_current()
            .err()
            .ok_or("restored authored program was admitted")?;
        assert!(
            refusal.message.contains("original authored inputs changed"),
            "{}",
            refusal.message
        );
        Ok(())
    }
}
