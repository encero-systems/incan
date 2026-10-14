//! Completed test source generation retained by the existing reconciled artifact session.
//!
//! This skips only analysis and emission. Every caller still constructs a fresh receipt, admits the native plan,
//! verifies the executable and executes tests. Inspection workspaces retain their canonical preparation path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use incan_frontend::ParsedModule;
use incan_frontend::library_manifest::visit_provider_artifact_inputs;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use oven_model::observed_inputs::InputObservations;

use crate::testing::types::TestInfo;

/// Exact already-collected inputs to the existing test analysis and generator, within one session generation.
pub struct TestGenerationRequest<'a> {
    /// Ordered parsed modules, including transformed test ASTs and diagnostic source text.
    pub modules: &'a [ParsedModule],
    /// Original tests and conftests, before their runner transformation.
    pub authored_sources: &'a [(PathBuf, String)],
    /// Ordered selected cases, including generated arguments, fixtures and asynchronous calls.
    pub tests: &'a [TestInfo],
    /// Current canonical provider and runtime identities, prepared before analysis.
    pub build_unit_inputs: &'a BTreeMap<String, String>,
    /// Selected compiler executable, observed physically as well as compared by path.
    pub rustc: &'a Path,
    /// Exact target of the generated harness.
    pub target: &'a str,
    /// Exact toolchain identity of the generated harness.
    pub toolchain: &'a str,
}

impl TestGenerationRequest<'_> {
    /// Observe consulted sources before analysis; mutations during preparation prevent installing retained state.
    pub fn observe(&self) -> InputObservations {
        let mut inputs = InputObservations::default();
        inputs.observe(self.rustc);
        for module in self.modules {
            inputs.observe(&module.file_path);
        }
        for (path, _) in self.authored_sources {
            inputs.observe(path);
        }
        inputs
    }
}

/// Completed generated files, owned by the canonical lock/provider session and shared with its parallel workers.
/// No test outcomes, mutable fixtures, new resolver or persisted cache belong here.
#[derive(Default)]
pub struct RetainedTestGeneration {
    projects: Mutex<BTreeMap<PathBuf, GeneratedTestProject>>,
}

/// Exact prior generation and replacement-sensitive observations of the files it produced.
struct GeneratedTestProject {
    modules: Vec<ParsedModule>,
    authored_sources: Vec<(PathBuf, String)>,
    tests: Vec<TestInfo>,
    build_unit_inputs: BTreeMap<String, String>,
    rustc: PathBuf,
    target: String,
    toolchain: String,
    inputs: InputObservations,
    outputs: InputObservations,
    source_metadata: StdlibAstCache,
}

impl GeneratedTestProject {
    /// Compare typed analysis inputs rather than assuming equal source text implies equal transformed ASTs.
    fn matches(&self, request: &TestGenerationRequest<'_>) -> bool {
        self.rustc == request.rustc
            && self.target == request.target
            && self.toolchain == request.toolchain
            && self.tests == request.tests
            && self.authored_sources == request.authored_sources
            && &self.build_unit_inputs == request.build_unit_inputs
            && self.modules.len() == request.modules.len()
            && self.modules.iter().zip(request.modules).all(|(left, right)| {
                left.name == right.name
                    && left.path_segments == right.path_segments
                    && left.file_path == right.file_path
                    && left.source == right.source
                    && left.ast == right.ast
            })
            && self.source_metadata.supports_generation_reuse()
            && self.inputs.supports_retention()
            && self.outputs.supports_retention()
            && self.inputs.current()
            && self.outputs.current()
    }
}

impl RetainedTestGeneration {
    /// Reuse only unchanged generated files for exactly the same checked request; failures keep canonical work.
    pub fn reuses(&self, output: &Path, request: &TestGenerationRequest<'_>) -> bool {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::retained_test_generation").entered();
        let Ok(mut projects) = self.projects.lock() else {
            return false;
        };
        let reused = projects.get(output).is_some_and(|project| project.matches(request));
        if !reused {
            projects.remove(output);
        }
        tracing::trace!(target: "incan::invocation", decision = if reused { "reuse_test_generation" } else { "prepare_test_generation" });
        reused
    }

    /// Retain successful generation after canonical native admission and compilation have completed.
    ///
    /// The source observation must precede analysis. Generated file and directory observations use the existing
    /// input observer and provider source visitor; linked or unsupported outputs simply retain nothing.
    pub fn remember(
        &self,
        output: &Path,
        request: &TestGenerationRequest<'_>,
        inputs: InputObservations,
        source_metadata: StdlibAstCache,
    ) {
        if !source_metadata.supports_generation_reuse() || !inputs.supports_retention() || !inputs.current() {
            return;
        }
        let mut outputs = InputObservations::default();
        outputs.observe(&output.join("Cargo.toml"));
        outputs.observe(&output.join("Cargo.lock"));
        if visit_provider_artifact_inputs(&output.join("src"), &mut |path| outputs.observe(path)).is_err()
            || !outputs.supports_retention()
            || !outputs.current()
            || !inputs.current()
        {
            return;
        }
        let Ok(mut projects) = self.projects.lock() else {
            return;
        };
        projects.insert(
            output.to_path_buf(),
            GeneratedTestProject {
                modules: request.modules.to_vec(),
                authored_sources: request.authored_sources.to_vec(),
                tests: request.tests.to_vec(),
                build_unit_inputs: request.build_unit_inputs.clone(),
                rustc: request.rustc.to_path_buf(),
                target: request.target.to_string(),
                toolchain: request.toolchain.to_string(),
                inputs,
                outputs,
                source_metadata,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use incan_frontend::ParsedModule;
    use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;

    use super::{RetainedTestGeneration, TestGenerationRequest};
    use crate::testing::types::TestInfo;

    /// Minimal physical generation with independent source, compiler, generated tree and runtime data.
    struct Fixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
        output: PathBuf,
        rustc: PathBuf,
        modules: Vec<ParsedModule>,
        authored: Vec<(PathBuf, String)>,
        tests: Vec<TestInfo>,
        identities: BTreeMap<String, String>,
    }

    impl Fixture {
        /// Keep the fixture's paths canonical so the existing observer can retain their actual object identities.
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let root = std::fs::canonicalize(directory.path())?;
            let output = root.join("generated");
            std::fs::create_dir_all(output.join("src"))?;
            std::fs::write(output.join("src/main.rs"), "fn main() {}")?;
            std::fs::write(output.join("Cargo.toml"), "[package]\nname = 'test'")?;
            let source = root.join("test.incn");
            std::fs::write(&source, "def test_value() -> None:\n    pass\n")?;
            let text = std::fs::read_to_string(&source)?;
            let rustc = root.join("rustc");
            std::fs::write(&rustc, "compiler")?;
            let test = TestInfo {
                file_path: source.clone(),
                function_name: "test_value".into(),
                is_async: false,
                markers: Vec::new(),
                required_fixtures: Vec::new(),
                parameter_names: Vec::new(),
                timeout: None,
                parametrize_call: None,
            };
            Ok(Self {
                _directory: directory,
                root,
                output,
                rustc,
                modules: vec![ParsedModule {
                    name: "test".into(),
                    path_segments: vec!["test".into()],
                    file_path: source.clone(),
                    source: text.clone(),
                    ast: Default::default(),
                }],
                authored: vec![(source, text)],
                tests: vec![test],
                identities: BTreeMap::from([("provider".into(), "first".into())]),
            })
        }

        /// Borrow the exact typed request that analysis and code generation received.
        fn request(&self) -> TestGenerationRequest<'_> {
            TestGenerationRequest {
                modules: &self.modules,
                authored_sources: &self.authored,
                tests: &self.tests,
                build_unit_inputs: &self.identities,
                rustc: &self.rustc,
                target: "target",
                toolchain: "toolchain",
            }
        }

        /// Install one completed canonical preparation, preserving its pre-analysis observation.
        fn retain(&self, owner: &RetainedTestGeneration) {
            let request = self.request();
            owner.remember(&self.output, &request, request.observe(), StdlibAstCache::default());
        }
    }

    /// Codegen cache clones share source provenance; even an unsuccessful ambient lookup prevents retained reuse.
    #[test]
    fn ambient_metadata_lookup_prevents_generation_retention() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let owner = RetainedTestGeneration::default();
        let metadata = StdlibAstCache::default();
        let mut codegen_metadata = metadata.clone();
        assert!(metadata.supports_generation_reuse());
        // A registered module reaches ambient discovery; unknown names are rejected before any source lookup.
        codegen_metadata.lookup_function_symbol(&["std".into(), "math".into()], "missing");
        assert!(!metadata.supports_generation_reuse());
        let request = fixture.request();
        owner.remember(&fixture.output, &request, request.observe(), metadata);
        assert!(!owner.reuses(&fixture.output, &request));
        Ok(())
    }

    /// Runtime data is deliberately outside compiled preparation; the subsequent test process reads it afresh.
    #[cfg(unix)]
    #[test]
    fn unchanged_generation_ignores_runtime_data() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let owner = RetainedTestGeneration::default();
        fixture.retain(&owner);
        assert!(owner.reuses(&fixture.output, &fixture.request()));
        std::fs::write(fixture.root.join("runtime.txt"), "different test outcome")?;
        assert!(owner.reuses(&fixture.output, &fixture.request()));
        Ok(())
    }

    /// Equal authored text cannot hide a changed transformed AST, selected case or provider identity.
    #[cfg(unix)]
    #[test]
    fn generation_requires_exact_analysis_and_selection_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = Fixture::new()?;
        let owner = RetainedTestGeneration::default();
        fixture.retain(&owner);
        fixture.modules[0].ast.source_path = Some("different semantic context".into());
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        fixture.retain(&owner);
        fixture.tests[0].function_name = "another_case".into();
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        fixture.retain(&owner);
        fixture.identities.insert("provider".into(), "replacement".into());
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        fixture.retain(&owner);
        let mut request = fixture.request();
        request.target = "another-target";
        assert!(!owner.reuses(&fixture.output, &request));
        fixture.retain(&owner);
        let mut request = fixture.request();
        request.toolchain = "another-toolchain";
        assert!(!owner.reuses(&fixture.output, &request));
        Ok(())
    }

    /// Changed generated bytes, new generated modules and compiler replacement invalidate the original generation.
    #[cfg(unix)]
    #[test]
    fn generated_source_and_compiler_changes_invalidate() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let owner = RetainedTestGeneration::default();
        fixture.retain(&owner);
        std::fs::write(fixture.output.join("src/main.rs"), "broken output")?;
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        fixture.retain(&owner);
        std::fs::write(fixture.output.join("src/added.rs"), "fn added() {}")?;
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        fixture.retain(&owner);
        let replacement = fixture.root.join("replacement");
        std::fs::write(&replacement, "compiler")?;
        std::fs::rename(replacement, &fixture.rustc)?;
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        Ok(())
    }

    /// A source edit during analysis cannot be installed as an unchanged preparation on the next request.
    #[cfg(unix)]
    #[test]
    fn source_mutation_during_generation_is_not_retained() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let owner = RetainedTestGeneration::default();
        let request = fixture.request();
        let observed = request.observe();
        std::fs::write(&fixture.authored[0].0, "changed source")?;
        owner.remember(&fixture.output, &request, observed, StdlibAstCache::default());
        assert!(!owner.reuses(&fixture.output, &request));
        Ok(())
    }

    /// Symlinks keep canonical preparation instead of granting retention under a partial physical observation.
    #[cfg(unix)]
    #[test]
    fn linked_generated_source_is_not_retained() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        std::os::unix::fs::symlink(&fixture.authored[0].0, fixture.output.join("src/linked.rs"))?;
        let owner = RetainedTestGeneration::default();
        fixture.retain(&owner);
        assert!(!owner.reuses(&fixture.output, &fixture.request()));
        Ok(())
    }
}
