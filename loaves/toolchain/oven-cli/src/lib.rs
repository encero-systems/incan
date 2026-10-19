//! The `oven` command surface as it exists today, packaged so the `oven` binary and the `incan` binary mount one
//! implementation: the explicit Oven receipt, bounded-store and native direct-rustc workflow (`oven`), lock
//! generation (`lock`), and the toolchain inspection helpers (`tools`).
//!
//! This is the workspace layout's `toolchain/oven-cli` package with the command handlers moved in as they were.
//! RFC 118 authors the canonical `oven` against the Oven API in v0.7; until then the handlers keep their reaches into
//! the driver (`oven bake` and `oven interop` of an Incan project, the store defaults, the lock resolution) and the
//! frontend (`tools`), which is why this crate depends on both. `incan` keeps mounting every command here under its
//! own spellings, so scripts, CI and tests that say `incan oven bake --project .` are unchanged.

pub mod binding_inspect;
pub mod cli;
pub mod commands;
pub mod invocation_trace;
pub mod project_owner;
pub mod test_command;
pub mod test_runner;
pub mod workspace_scope;

pub use cli::{
    LockArgs, OvenCommand, OvenGateCommand, OvenInteropAdapterArgument, OvenInteropCommand, OvenLoafEnvelopeArgument,
    OvenOutputFormat, OvenPlanCommand, OvenStoreCliFlags, OvenStoreCommand, PackageFeatureCliFlags, SdkProfileCliFlags,
    ToolsCommand, ToolsMetadataCommand,
};
pub use incan_driver::error::{CliError, CliResult, ExitCode};

/// Run one `oven` command family member.
pub fn run_oven_command(command: OvenCommand) -> CliResult<ExitCode> {
    match command {
        OvenCommand::Tests {
            arguments,
            incan_only,
            rust_only,
        } => {
            use std::io::IsTerminal;
            let languages = if incan_only {
                test_runner::TestLanguages::Incan
            } else if rust_only {
                test_runner::TestLanguages::Rust
            } else {
                test_runner::TestLanguages::All
            };
            test_command::run(arguments, std::io::stdout().is_terminal(), languages)
        }
        OvenCommand::CompileClosure(arguments) => commands::oven::compile_closure::run(arguments),
        OvenCommand::PrepareStandardPackages { native_graph_output } => {
            commands::oven::oven_prepare_standard_packages(&native_graph_output)
        }
        OvenCommand::RetainSuiteOutput { clock, arguments } => commands::oven_retain_suite_output(clock, &arguments),
        OvenCommand::ReconcilePartitions { reports, summary } => {
            commands::oven_reconcile_partitions(&reports, summary.as_deref())
        }
        OvenCommand::Bake {
            project,
            target,
            package_features,
            format,
        } => commands::oven_bake_project(project, target, package_features.into(), format),
        OvenCommand::Import {
            project,
            target,
            toolchain,
            profile,
            features,
            source_inputs,
            output,
            format,
        } => commands::oven_import(commands::OvenImportCommandOptions {
            project,
            target,
            toolchain,
            profile,
            features,
            source_inputs,
            output,
            format,
        }),
        OvenCommand::Gate { command } => match command {
            OvenGateCommand::RegistryPin { pin, checkout } => commands::oven_gate_registry_pin(&pin, &checkout),
        },
        OvenCommand::Interop { command } => match command {
            OvenInteropCommand::Bake {
                project,
                target,
                base_receipt,
                c_compiler,
                cxx_compiler,
                archiver,
                toolchain_version,
                sdk_root,
                sdk_version,
                sdk_identity_file,
                store,
                format,
            } => commands::oven_interop_bake(commands::OvenInteropBakeCommandOptions {
                project,
                target,
                base_receipt,
                c_compiler,
                cxx_compiler,
                archiver,
                toolchain_version,
                sdk_root,
                sdk_version,
                sdk_identity_file,
                store: store.into(),
                format,
            }),
            OvenInteropCommand::Stage {
                project,
                target,
                base_receipt,
                adapter,
                output,
                store,
                format,
            } => commands::oven_interop_stage(commands::OvenInteropStageCommandOptions {
                project,
                target,
                base_receipt,
                adapter,
                output,
                store: store.into(),
                format,
            }),
        },
        OvenCommand::CompilerLibtests {
            compiler_root,
            rustc,
            features,
            targets,
            exact_names,
            partition_index,
            partition_count,
            fixture_cargo,
            explicit_bake_workspace,
            output,
            store,
            format,
        } => commands::oven_run_compiler_libtests(commands::OvenCompilerLibtestsRunCommandOptions {
            compiler_root,
            rustc,
            features,
            targets,
            exact_names,
            partition_index,
            partition_count,
            fixture_cargo,
            explicit_bake_workspace,
            output,
            store: store.into(),
            format,
        }),
        OvenCommand::CompilerNativeTests {
            compiler_root,
            target,
            declaration,
            native_graph,
            native_index,
            native_blobs,
            source_inputs,
            exact_names,
            output,
            store,
            explicit_bake_workspace,
            rustc,
        } => commands::oven::native_compiler_tests::run(
            compiler_root,
            target,
            declaration,
            native_graph,
            native_index,
            native_blobs,
            source_inputs,
            exact_names,
            output,
            store.into(),
            explicit_bake_workspace,
            rustc,
        ),
        OvenCommand::Plan { command } => match command {
            OvenPlanCommand::Publish {
                receipt,
                manifest,
                artifact_root,
                domain,
                store,
                format,
            } => commands::oven_publish_direct_rustc_plan(commands::OvenPlanPublishCommandOptions {
                receipt,
                manifest,
                artifact_root,
                domain,
                store: store.into(),
                format,
            }),
        },
        OvenCommand::Store { command } => match command {
            OvenStoreCommand::Inspect { store, format } => commands::inspect_oven_store(store.into(), format),
            OvenStoreCommand::Prune { store, dry_run, format } => {
                commands::prune_oven_store(store.into(), dry_run, format)
            }
        },
        OvenCommand::Test {
            receipt,
            plan_identity,
            rustc,
            source,
            output,
            crate_name,
            edition,
            source_evidence_key,
            exact_names,
            store,
            format,
        } => commands::oven_test(commands::OvenTestCommandOptions {
            receipt,
            plan_identity,
            rustc,
            source,
            output,
            crate_name,
            edition,
            source_evidence_key,
            exact_names,
            store: store.into(),
            format,
        }),
        OvenCommand::Run {
            receipt,
            plan_identity,
            rustc,
            source,
            output,
            crate_name,
            edition,
            source_evidence_key,
            arguments,
            store,
            format,
        } => commands::oven_run(commands::OvenRunCommandOptions {
            receipt,
            plan_identity,
            rustc,
            source,
            output,
            crate_name,
            edition,
            source_evidence_key,
            arguments,
            store: store.into(),
            format,
        }),
    }
}

/// Run one `tools` command.
pub fn run_tools_command(command: ToolsCommand) -> CliResult<ExitCode> {
    match command {
        ToolsCommand::Doctor { format } => commands::tools_doctor(format),
        ToolsCommand::Metadata { command } => match command {
            ToolsMetadataCommand::Api { path, format } => commands::tools_metadata_api(&path, format),
            ToolsMetadataCommand::Model { path, model, format } => {
                commands::tools_metadata_model(&path, &model, format)
            }
        },
    }
}

/// Generate or update `oven.lock` for a project from the parsed `lock` arguments.
pub fn run_lock_command(args: LockArgs) -> CliResult<ExitCode> {
    let LockArgs {
        file,
        package_features,
        sdk_profile,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    } = args;
    commands::lock_project(
        file.as_ref(),
        &package_features.into(),
        sdk_profile.profile(),
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )
}
