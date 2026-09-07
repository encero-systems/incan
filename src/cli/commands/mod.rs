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

pub mod binding_inspect;
pub mod build;
pub mod build_report;
pub mod cache;
pub mod codegraph;
pub mod common;
pub mod debug;
pub mod diagnostics;
pub mod format;
pub mod init;
pub mod interop_plan;
pub mod lifecycle;
pub mod lock;
pub mod oven;
pub mod provider_inspect;
pub(crate) mod shadow_support;
pub mod stdlib_loader;
pub mod tools;
pub(crate) mod vocab_extraction;
pub mod workspace;

// Re-export public API so callers can use `commands::build_file()` etc.
pub use binding_inspect::{BindingInspectionFormat, inspect_bindings};
pub use build::{build_file, build_library, inspect_rust, run_file, run_inline_source};
pub use cache::{inspect_generated_cache, prune_generated_cache};
pub use codegraph::{CodegraphInspectionFormat, inspect_codegraph};
pub(crate) use common::sdk_provider_store_identity_for_compiler_root;
pub use common::{collect_modules, read_source};
pub use debug::{check_file, emit_rust, lex_file, parse_file};
pub use diagnostics::{
    DiagnosticOutputFormat, check_path, check_path_with_features, check_path_with_selections, explain_diagnostic,
};
pub use format::format_files;
pub use init::init_project;
pub use interop_plan::{InteropPlanInspectionFormat, inspect_interop_plan};
pub use lifecycle::{env_list, env_run, env_show, version_project};
pub use lock::lock_project;
pub use oven::{
    OvenCompilerLibtestsRunCommandOptions, OvenImportCommandOptions, OvenInteropBakeCommandOptions,
    OvenInteropStageCommandOptions, OvenLegacyCargoPrepareCommandOptions, OvenLoafBakeCommandOptions,
    OvenPlanPublishCommandOptions, OvenReceiptInspectCommandOptions, OvenRunCommandOptions, OvenStoreCommandOptions,
    OvenTestCommandOptions, inspect_oven_receipt, inspect_oven_store, oven_bake_project, oven_import,
    oven_interop_bake, oven_interop_stage, oven_legacy_cargo_bake_loafs, oven_legacy_cargo_prepare,
    oven_publish_direct_rustc_plan, oven_run, oven_run_compiler_libtests, oven_test, prune_oven_store,
};
pub use provider_inspect::{ProviderInspectionFormat, inspect_features, inspect_providers};
pub use shadow_support::compare_source_observable;
pub use tools::{
    ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat, tools_doctor, tools_metadata_api,
    tools_metadata_model,
};
pub use workspace::{WorkspaceInspectFormat, workspace_inspect};
