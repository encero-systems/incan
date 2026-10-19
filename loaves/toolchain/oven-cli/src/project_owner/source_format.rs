//! Fresh source formatting through the existing owner's parser-only project context.
//!
//! The caller owns disk reads, writes and presentation. Only dependency preparation is retained; each supplied
//! source is parsed and formatted again, so edits never reuse an old syntax tree or formatted result.

use crate::{CliError, CliResult};
use incan_driver::build::executable::ExecutableBuildOwner;
use incan_driver::project::resolve_project_root;
use incan_format::{FormatConfig, format_parsed_source_with_config};
use incan_frontend::diagnostics;
use incan_driver::features::FeatureSelection;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One file's explicit contents and location; these carry no cached compiler authority.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceFormatRequest {
    /// Absolute source location used for ordinary project and vocabulary discovery.
    pub path: PathBuf,
    /// Original caller spelling retained only for diagnostics.
    pub display_path: PathBuf,
    /// Freshly read source text; never retained after this request.
    pub source: String,
}

/// Format one bounded command batch, borrowing each project's existing session once for this request.
/// Every file still discovers its own manifest and parses fresh text. These borrowed sessions are request-local;
/// the existing owner remains the only retained authority, and the next batch reconciles inputs again.
pub(super) fn execute_batch(
    requests: &[SourceFormatRequest],
    owner: &mut ExecutableBuildOwner,
) -> Vec<Result<String, String>> {
    let mut sessions = BTreeMap::new();
    requests
        .iter()
        .map(|request| {
            validate_request(request)
                .and_then(|()| format_with_sessions(owner, request, &mut sessions))
                .map_err(|error| error.to_string())
        })
        .collect()
}

/// Reject paths whose diagnostic spelling would discover a different project than the explicit source location.
fn validate_request(request: &SourceFormatRequest) -> CliResult<()> {
    if !request.path.is_absolute() {
        return Err(CliError::failure("owner requires an absolute formatting path"));
    }
    let display_location =
        std::path::absolute(&request.display_path).map_err(|error| CliError::failure(error.to_string()))?;
    if display_location != request.path {
        return Err(CliError::failure(
            "formatting display path does not match source location",
        ));
    }
    Ok(())
}

/// Discover nested/overridden manifests per file, then borrow one reconciled parser context per project in the batch.
/// Standalone files retain canonical discovery because their entry paths can affect context selection.
fn format_with_sessions(
    owner: &mut ExecutableBuildOwner,
    request: &SourceFormatRequest,
    sessions: &mut BTreeMap<PathBuf, Arc<incan_driver::session::CompilationSession>>,
) -> CliResult<String> {
    let inferred_root = resolve_project_root(&request.display_path);
    let manifest = ProjectManifest::discover(&inferred_root).map_err(|error| CliError::failure(error.to_string()))?;
    let root = manifest
        .as_ref()
        .map_or(inferred_root.as_path(), ProjectManifest::project_root);
    let root = fs::canonicalize(root).map_err(|error| CliError::failure(error.to_string()))?;
    let session = if manifest.is_some() {
        match sessions.entry(root.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let session =
                    owner.test_collection_session(&root, &request.path, &FeatureSelection::default(), None)?;
                entry.insert(Arc::clone(&session));
                session
            }
        }
    } else {
        owner.test_collection_session(&root, &request.path, &FeatureSelection::default(), None)?
    };
    let ast = session
        .parse_source_for_collection(&request.path, &request.source)
        .map_err(|errs| CliError::failure(format_parse_errors(&request.display_path, &request.source, &errs)))?;
    format_parsed_source_with_config(&request.source, &ast, FormatConfig::default())
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Exercise single-file requests through the same batch implementation used by the client.
#[cfg(test)]
fn format_file_source(
    owner: &mut ExecutableBuildOwner,
    file_path: &Path,
    source: &str,
    display_path: &Path,
) -> CliResult<String> {
    let request = SourceFormatRequest {
        path: file_path.to_path_buf(),
        display_path: display_path.to_path_buf(),
        source: source.to_string(),
    };
    execute_batch(&[request], owner)
        .into_iter()
        .next()
        .ok_or_else(|| CliError::failure("missing formatting result"))?
        .map_err(CliError::failure)
}

/// Format parser diagnostics with the real file path instead of the context-free formatter placeholder.
fn format_parse_errors(file_path: &Path, source: &str, errors: &[diagnostics::CompileError]) -> String {
    let file_name = file_path.to_string_lossy();
    errors
        .iter()
        .map(|error| diagnostics::format_error(file_name.as_ref(), source, error))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::{ExecutableBuildOwner, format_file_source};
    use std::fs;

    /// A client cannot attach a different project context by changing its diagnostic-only path.
    #[test]
    fn mismatched_format_display_location_refuses() {
        let request = super::SourceFormatRequest {
            path: std::path::PathBuf::from("/format-source.incn"),
            display_path: std::path::PathBuf::from("/different-project/source.incn"),
            source: String::new(),
        };
        assert!(super::validate_request(&request).is_err());
    }

    /// Reusing dependency preparation must never reuse a file's parsed contents or hide syntax errors.
    #[test]
    fn retained_format_context_parses_each_source_revision() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        fs::write(
            directory.path().join("loaf.toml"),
            "[project]\nname = \"format_revisions\"\nversion = \"0.1.0\"\n",
        )?;
        let file = directory.path().join("main.incn");
        let mut owner = ExecutableBuildOwner::default();
        for value in [1, 2] {
            let source = format!("def main() -> None:\n    println({value})\n");
            fs::write(&file, &source)?;
            assert_eq!(format_file_source(&mut owner, &file, &source, &file)?, source);
        }
        let invalid = "def main(\n";
        fs::write(&file, invalid)?;
        assert!(format_file_source(&mut owner, &file, invalid, &file).is_err());
        Ok(())
    }

    /// Parser-only retention replays canonical warnings without emitting internal model reconciliation duplicates.
    #[test]
    fn retained_format_context_replays_manifest_warning() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        fs::write(
            directory.path().join("loaf.toml"),
            "[project]\nname = \"format_warnings\"\nversion = \"0.1.0\"\n[rust-dependencies]\n",
        )?;
        let file = directory.path().join("main.incn");
        let source = "def main() -> None:\n    println(42)\n";
        fs::write(&file, source)?;
        let mut owner = ExecutableBuildOwner::default();
        let (first, first_warnings) =
            incan_driver::diagnostics::capture_warning_output(|| format_file_source(&mut owner, &file, source, &file));
        assert_eq!(first?, source);
        let (repeat, repeat_warnings) =
            incan_driver::diagnostics::capture_warning_output(|| format_file_source(&mut owner, &file, source, &file));
        assert_eq!(repeat?, source);
        assert_eq!(first_warnings.len(), 1);
        assert!(first_warnings[0].contains("[rust-dependencies] is deprecated"));
        assert_eq!(first_warnings, repeat_warnings);
        Ok(())
    }

    /// A nested manifest must own its files even when an ancestor project already has retained preparation.
    #[test]
    fn retained_format_context_preserves_nested_manifest_errors() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let manifest = "[project]\nname = \"format_parent\"\nversion = \"0.1.0\"\n";
        fs::write(directory.path().join("loaf.toml"), manifest)?;
        let file = directory.path().join("main.incn");
        let source = "def main() -> None:\n    println(42)\n";
        fs::write(&file, source)?;
        let mut owner = ExecutableBuildOwner::default();
        assert_eq!(format_file_source(&mut owner, &file, source, &file)?, source);
        let child = directory.path().join("child");
        fs::create_dir(&child)?;
        fs::write(child.join("loaf.toml"), "[project\n")?;
        let child_file = child.join("main.incn");
        fs::write(&child_file, source)?;
        assert!(format_file_source(&mut owner, &child_file, source, &child_file).is_err());
        fs::write(
            child.join("loaf.toml"),
            manifest.replace("format_parent", "format_child"),
        )?;
        assert_eq!(
            format_file_source(&mut owner, &child_file, source, &child_file)?,
            source
        );
        Ok(())
    }
    /// One malformed source or nested manifest must not poison sibling files or share their project context.
    #[test]
    fn format_batch_keeps_file_errors_and_nested_projects_independent() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"batch_parent\"\nversion = \"0.1.0\"\n",
        )?;
        let child = root.join("child");
        fs::create_dir(&child)?;
        fs::write(child.join("loaf.toml"), "[project\n")?;
        let source = "def main() -> None:\n    println(42)\n";
        let files = [
            (root.join("one.incn"), source),
            (root.join("bad.incn"), "def main(\n"),
            (child.join("main.incn"), source),
            (root.join("two.incn"), source),
        ];
        let mut requests = Vec::new();
        for (path, text) in files {
            fs::write(&path, text)?;
            requests.push(super::SourceFormatRequest {
                display_path: path.clone(),
                path,
                source: text.to_string(),
            });
        }
        let mut owner = ExecutableBuildOwner::default();
        let results = super::execute_batch(&requests, &mut owner);
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].as_ref().map(String::as_str), Ok(source));
        assert!(results[1].is_err());
        assert!(results[2].is_err());
        assert_eq!(results[3].as_ref().map(String::as_str), Ok(source));
        fs::write(
            child.join("loaf.toml"),
            "[project]\nname = \"batch_child\"\nversion = \"0.1.0\"\n",
        )?;
        let restored = super::execute_batch(&requests, &mut owner);
        assert_eq!(restored[2].as_ref().map(String::as_str), Ok(source));
        Ok(())
    }
}
