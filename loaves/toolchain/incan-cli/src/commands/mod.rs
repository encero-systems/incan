//! CLI command implementations
//!
//! All command functions return `CliResult<ExitCode>` instead of calling `process::exit`. Error handling and exits
//! happen in the top-level `run()`.
//!
//! ## Submodules
//!
//! - `build` — Build and run pipelines
//! - `debug` — Debug commands (lex, parse, check, emit)
//! - `format` — Source formatting
//! - `init` — Project scaffolding
//! - `lifecycle` — Project lifecycle commands (`version` and `env`)
//! - `stdlib_loader` — RFC 023: Stdlib module loading for compilation

pub mod binding_inspect;
pub mod build;
pub mod build_report;
pub mod cache;
pub mod codegraph;
pub mod debug;
pub mod diagnostics;
pub mod format;
pub mod init;
pub mod interop_plan;
pub mod lifecycle;
pub mod provider_inspect;
pub mod representation_inspect;
pub mod stdlib_loader;
pub mod workspace;

#[cfg(test)]
mod tools_boundary_tests;

// Re-export public API so callers can use `commands::build_file()` etc.
// The `oven`, `lock` and `tools` families live in the `oven-cli` package; `incan` mounts them under its own
// spellings, so their handlers and option types keep resolving through this module.
pub use oven_cli::commands::{OvenReceiptInspectCommandOptions, inspect_oven_receipt, lock, oven, tools};

pub use binding_inspect::{BindingInspectionFormat, inspect_bindings};
pub use build::{build_file, build_library, inspect_rust, run_file, run_inline_source};
pub use cache::{inspect_generated_cache, prune_generated_cache};
pub use codegraph::{CodegraphInspectionFormat, inspect_codegraph};
pub use debug::{check_file, emit_rust, lex_file, parse_file};
pub use diagnostics::{
    DiagnosticOutputFormat, check_path, check_path_with_features, check_path_with_selections, explain_diagnostic,
};
pub use format::format_files;
pub use init::init_project;
pub use interop_plan::{InteropPlanInspectionFormat, inspect_interop_plan};
pub use lifecycle::{env_list, env_run, env_show, version_project};
pub use provider_inspect::{ProviderInspectionFormat, inspect_features, inspect_providers};
pub use representation_inspect::{RepresentationInspectionFormat, inspect_representation};
pub use workspace::{WorkspaceInspectFormat, workspace_inspect};
