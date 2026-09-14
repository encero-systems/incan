//! The imports a project's tests add to its lock: discovered through the same session the runner uses, so the
//! lock and `incan test` agree on which modules a test pulls in.

use std::fs;
use std::path::{Path, PathBuf};

use crate::driver::error::{CliError, CliResult};
use crate::driver::lock::TestLockInputs;
use crate::driver::modules::collect_rust_dependency_uses;
use crate::driver::session::CompilationSession;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::{ParsedModule, diagnostics, lexer, parser};
use crate::provider::ProviderPlan;
use crate::workspace::WorkspaceGraph;

/// Collect inline Rust crate imports and stdlib/provider requirements from test files owned by this project.
pub(crate) fn collect_test_lock_inputs(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&parser::ImportedLibraryDslSurfaces>,
    library_manifest_index: Option<&LibraryManifestIndex>,
    provider_plan: &ProviderPlan,
    session: &CompilationSession,
) -> CliResult<TestLockInputs> {
    let mut inline_imports = Vec::new();
    let mut project_requirement_modules = Vec::new();
    let test_files = discover_project_test_files(project_root, workspace, session);
    let source_root = project_root.join("src");

    for file_path in test_files {
        let source = fs::read_to_string(&file_path)
            .map_err(|e| CliError::failure(format!("Failed to read test file '{}': {}", file_path.display(), e)))?;
        let tokens = lexer::lex(&source).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;
        let path_display = file_path.to_string_lossy();
        let ast = parser::parse_with_context_and_surfaces(
            &tokens,
            Some(path_display.as_ref()),
            library_imported_vocab,
            library_imported_dsl_surfaces,
        )
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;

        let test_module = ParsedModule {
            name: file_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("test")
                .to_string(),
            path_segments: vec!["test".to_string()],
            file_path: file_path.clone(),
            source: source.clone(),
            ast: ast.clone(),
        };
        inline_imports.extend(collect_rust_dependency_uses(&test_module, true));

        let source_modules = crate::driver::testing::module_graph::collect_source_modules_for_test(
            &ast,
            &source_root,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            library_manifest_index,
            provider_plan,
        )
        .map_err(CliError::failure)?;
        for module in &source_modules {
            inline_imports.extend(collect_rust_dependency_uses(module, false));
        }
        project_requirement_modules.push(test_module);
        project_requirement_modules.extend(source_modules);
    }

    Ok(TestLockInputs {
        inline_imports,
        project_requirement_modules,
    })
}

/// Discover test files owned by one project, excluding descendant workspace members in rooted workspaces.
fn discover_project_test_files(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    session: &CompilationSession,
) -> Vec<PathBuf> {
    crate::driver::testing::discover_test_files_with_compilation_session(project_root, session)
        .into_iter()
        .filter(|path| {
            workspace.is_none_or(|graph| {
                graph
                    .member_containing_path(path)
                    .is_some_and(|owner| owner.root() == project_root)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::FeatureSelection;

    #[test]
    fn lock_collects_test_imported_source_modules_as_normal_deps() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        fs::create_dir_all(project_root.join("src"))?;
        fs::create_dir_all(project_root.join("tests"))?;
        fs::write(
            project_root.join("src").join("internal.incn"),
            "from rust::datafusion @ \"53\" import SessionContext\n",
        )?;
        fs::write(
            project_root.join("tests").join("test_internal.incn"),
            "from internal import SessionContext\nfrom rust::tokio @ \"1\" import spawn\n",
        )?;

        let session = CompilationSession::discover_for_oven(
            &project_root.join("tests/test_internal.incn"),
            &FeatureSelection::default(),
            None,
        )?;
        let inputs =
            collect_test_lock_inputs(project_root, None, None, None, None, &ProviderPlan::default(), &session)?;
        let imports = inputs.inline_imports;
        let tokio = imports
            .iter()
            .find(|import| import.crate_name == "tokio")
            .ok_or("expected direct test tokio import")?;
        let datafusion = imports
            .iter()
            .find(|import| import.crate_name == "datafusion")
            .ok_or("expected test-imported source module datafusion import")?;

        assert!(tokio.is_test_context);
        assert!(!datafusion.is_test_context);
        Ok(())
    }

    #[test]
    fn project_test_discovery_excludes_descendant_workspace_members() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path();
        let consumer_root = root.join("packages/consumer");
        fs::create_dir_all(root.join("tests/nested"))?;
        fs::create_dir_all(consumer_root.join("tests"))?;
        fs::write(
            root.join("loaf.toml"),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/consumer"]
"#,
        )?;
        fs::write(
            consumer_root.join("loaf.toml"),
            r#"
[project]
name = "consumer"
"#,
        )?;
        let root_test = root.join("tests/test_root.incn");
        let nested_root_test = root.join("tests/nested/test_nested.incn");
        let consumer_test = consumer_root.join("tests/test_consumer.incn");
        fs::write(&root_test, "def test_root() -> None:\n    pass\n")?;
        fs::write(&nested_root_test, "def test_nested() -> None:\n    pass\n")?;
        fs::write(&consumer_test, "def test_consumer() -> None:\n    pass\n")?;

        let workspace = WorkspaceGraph::load_from_root(root)?;
        let root_session = CompilationSession::discover_for_oven(&root_test, &FeatureSelection::default(), None)?;
        let root_files = discover_project_test_files(workspace.root(), Some(&workspace), &root_session);
        let consumer = workspace
            .members()
            .find(|member| member.name() == "consumer")
            .ok_or("consumer workspace member should exist")?;
        let consumer_session =
            CompilationSession::discover_for_oven(&consumer_test, &FeatureSelection::default(), None)?;
        let consumer_files = discover_project_test_files(consumer.root(), Some(&workspace), &consumer_session);
        let mut expected_root_files = vec![fs::canonicalize(root_test)?, fs::canonicalize(nested_root_test)?];
        expected_root_files.sort();

        assert_eq!(root_files, expected_root_files);
        assert_eq!(consumer_files, [fs::canonicalize(consumer_test)?]);
        Ok(())
    }
}
