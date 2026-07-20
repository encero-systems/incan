//! CLI command implementations
//!
//! All command functions return `CliResult<ExitCode>` instead of calling `process::exit`.
//! Error handling and exits happen in the top-level `run()`.
//!
//! ## Submodules
//!
//! - `build` — Build and run pipelines
//! - `common` — Shared utilities (module collection, dependency helpers, etc.)
//! - `debug` — Debug commands (lex, parse, check, emit)
//! - `format` — Source formatting
//! - `init` — Project scaffolding
//! - `lifecycle` — Project lifecycle commands (`version` and `env`)
//! - `lock` — Lock file generation and resolution
//! - `stdlib_loader` — RFC 023: Stdlib module loading for compilation
//! - `tools` — Local toolchain inspection and metadata helpers

pub mod build;
pub mod build_report;
pub mod cache;
pub mod codegraph;
pub mod common;
pub mod debug;
pub mod diagnostics;
pub mod format;
pub mod init;
pub mod lifecycle;
pub mod lock;
pub mod provider_inspect;
pub mod stdlib_loader;
pub mod tools;
pub(crate) mod vocab_extraction;
pub mod workspace;

// Re-export public API so callers can use `commands::build_file()` etc.
pub use build::{build_file, build_library, inspect_rust, run_file, run_inline_source};
pub use cache::{inspect_generated_cache, prune_generated_cache};
pub use codegraph::{CodegraphInspectionFormat, inspect_codegraph};
pub use common::{collect_modules, read_source};
pub use debug::{check_file, emit_rust, lex_file, parse_file};
pub use diagnostics::{
    DiagnosticOutputFormat, check_path, check_path_with_features, check_path_with_selections, explain_diagnostic,
};
pub use format::format_files;
pub use init::init_project;
pub use lifecycle::{env_list, env_run, env_show, version_project};
pub use lock::lock_project;
pub use provider_inspect::{ProviderInspectionFormat, inspect_features, inspect_providers};
pub use tools::{
    ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat, tools_doctor, tools_metadata_api,
    tools_metadata_model,
};
pub use workspace::{WorkspaceInspectFormat, workspace_inspect};

// Crate-internal API (used by test_runner and other CLI modules)
pub(crate) use lock::{LockResolutionRequest, resolve_lock_context};
