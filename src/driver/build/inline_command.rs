//! `incan run -c <source>`: the throwaway project an inline command compiles in.

use std::env;
use std::path::Path;

use sha2::Sha256;

use crate::driver::build::{INLINE_COMMAND_OUTPUT_PARENT, INLINE_COMMAND_PROJECT_PREFIX, InlineCommandProject};
use crate::driver::error::{CliError, CliResult};
use sha2::Digest as _;

/// Return the stable cache key used for one wrapped inline command source from one working directory.
fn inline_command_cache_key(cwd: &Path, wrapped_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cwd.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(wrapped_source.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

/// Return the stable generated project identity used for one `incan run -c` source.
fn inline_command_project_for_cwd(cwd: &Path, wrapped_source: &str) -> InlineCommandProject {
    let digest = inline_command_cache_key(cwd, wrapped_source);
    let project_name = format!("{INLINE_COMMAND_PROJECT_PREFIX}_{digest}");
    let source_path = env::temp_dir().join(&project_name).join("main.incn");
    let output_dir = format!("{INLINE_COMMAND_OUTPUT_PARENT}/{project_name}");
    InlineCommandProject {
        source_path,
        project_name,
        output_dir,
    }
}

/// Resolve the current invocation's stable inline-command generated project identity.
pub(crate) fn inline_command_project(wrapped_source: &str) -> CliResult<InlineCommandProject> {
    let cwd = env::current_dir().map_err(|err| {
        CliError::failure(format!(
            "failed to determine current directory for inline command cache: {err}"
        ))
    })?;
    Ok(inline_command_project_for_cwd(&cwd, wrapped_source))
}

/// Preserve the legacy `run -c` behavior by adding a no-op `main` only when the snippet did not define one.
pub(crate) fn wrap_inline_command_source(source: &str) -> String {
    if source.contains("def main") {
        source.to_string()
    } else {
        format!("{source}\n\ndef main() -> Unit:\n  pass\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn inline_command_project_is_stable_for_same_source_and_working_directory() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(cwd, &source);
        let second = inline_command_project_for_cwd(cwd, &source);

        assert_eq!(first, second);
        assert_eq!(
            first.source_path.file_name().and_then(|name| name.to_str()),
            Some("main.incn")
        );
        let rendered = first.source_path.to_string_lossy();
        assert!(
            rendered.contains("incan_inline_command_"),
            "inline command temp source should use the stable inline-command prefix: {rendered}"
        );
        assert!(
            !rendered.contains("incan_cmd_"),
            "inline command temp source must not use timestamped incan_cmd names: {rendered}"
        );
        assert!(first.project_name.starts_with("incan_inline_command_"));
        assert!(
            first
                .output_dir
                .starts_with("target/incan/inline/incan_inline_command_")
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_working_directory() {
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/one"), &source);
        let second = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/two"), &source);

        assert_ne!(
            first, second,
            "different working directories should not race on one inline command temp source"
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_source_content() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let first = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"one\")"));
        let second = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"two\")"));

        assert_ne!(
            first, second,
            "different inline snippets in the same working directory must not race on one generated cargo target"
        );
    }

    #[test]
    fn inline_command_source_wrapper_preserves_existing_main() {
        let source = "def main() -> None:\n    println(\"ok\")\n";

        assert_eq!(wrap_inline_command_source(source), source);
    }

    #[test]
    fn inline_command_source_wrapper_adds_stub_main_for_expression_snippets() {
        let wrapped = wrap_inline_command_source("println(\"ok\")");

        assert!(
            wrapped.contains("def main() -> Unit:\n  pass"),
            "inline snippets without a main should preserve existing run -c stub behavior: {wrapped}"
        );
    }
}
