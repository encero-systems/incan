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
    OVEN_COMPILER_SUITE_STORE_ENV, OVEN_TOOLCHAIN_BUILD_OUTPUT_RELATIVE_PATH, OvenCompilerLibtestsRunCommandOptions,
    OvenHarvestCommandOptions, OvenImportCommandOptions, OvenInteropBakeCommandOptions, OvenInteropStageCommandOptions,
    OvenLegacyCargoPrepareCommandOptions, OvenLoafBakeCommandOptions, OvenPlanPublishCommandOptions,
    OvenReceiptInspectCommandOptions, OvenRunCommandOptions, OvenStoreCommandOptions, OvenTestCommandOptions,
    OvenToolchainBinaryReport, OvenToolchainBuildCommandOptions, inspect_oven_receipt, inspect_oven_store,
    oven_bake_project, oven_build_toolchain_binaries, oven_harvest, oven_import, oven_interop_bake, oven_interop_stage,
    oven_legacy_cargo_bake_loafs, oven_legacy_cargo_prepare, oven_publish_direct_rustc_plan, oven_run,
    oven_run_compiler_libtests, oven_test, prune_oven_store,
};
pub use tools::{
    ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat, tools_doctor, tools_metadata_api,
    tools_metadata_model,
};
