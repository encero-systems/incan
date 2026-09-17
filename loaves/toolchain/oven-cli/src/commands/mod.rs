//! The command handlers behind `oven`, `lock` and `tools`.
//!
//! - `oven` — the explicit Oven Alpha receipt, bounded-store, and native direct-rustc workflow
//! - `lock` — lock file generation and resolution
//! - `tools` — local toolchain inspection and metadata helpers

pub mod lock;
pub mod oven;
pub mod tools;

pub use lock::lock_project;
pub use oven::{
    OvenCompilerLibtestsRunCommandOptions, OvenImportCommandOptions, OvenInteropBakeCommandOptions,
    OvenInteropStageCommandOptions, OvenLegacyCargoPrepareCommandOptions, OvenLoafBakeCommandOptions,
    OvenPlanPublishCommandOptions, OvenReceiptInspectCommandOptions, OvenRunCommandOptions, OvenStoreCommandOptions,
    OvenTestCommandOptions, inspect_oven_receipt, inspect_oven_store, oven_bake_project, oven_import,
    oven_interop_bake, oven_interop_stage, oven_legacy_cargo_bake_loafs, oven_legacy_cargo_prepare,
    oven_publish_direct_rustc_plan, oven_run, oven_run_compiler_libtests, oven_test, prune_oven_store,
};
pub use tools::{
    ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat, tools_doctor, tools_metadata_api,
    tools_metadata_model,
};
