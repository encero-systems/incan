//! The `oven` command surface as it exists today, packaged so the `oven` binary and the `incan` binary mount one
//! implementation: the explicit Oven receipt, bounded-store and native direct-rustc workflow (`oven`), lock
//! generation (`lock`), and the toolchain inspection helpers (`tools`).
//!
//! This is the workspace layout's `toolchain/oven-cli` package with the command handlers moved in as they were.
//! RFC 118 authors the canonical `oven` against the Oven API in v0.7; until then the handlers keep their reaches into
//! the driver (`oven bake` and `oven interop` of an Incan project, the store defaults, the lock resolution) and the
//! frontend (`tools`), which is why this crate depends on both. `incan` keeps mounting every command here under its
//! own spellings, so scripts, CI and tests that say `incan oven bake --project .` are unchanged.

pub mod cli;
pub mod commands;

pub use cli::{
    LockArgs, OvenCommand, OvenHarvestProfileArgument, OvenInteropAdapterArgument, OvenInteropCommand,
    OvenLegacyCargoCommand, OvenLoafEnvelopeArgument, OvenOutputFormat, OvenPlanCommand, OvenStoreCliFlags,
    OvenStoreCommand, PackageFeatureCliFlags, SdkProfileCliFlags, ToolsCommand, ToolsMetadataCommand,
};
pub use incan_driver::error::{CliError, CliResult, ExitCode};

/// Run one `oven` command family member.
pub fn run_oven_command(command: OvenCommand) -> CliResult<ExitCode> {
    match command {
        OvenCommand::Bake {
            project,
            target,
            package_features,
            format,
        } => commands::oven_bake_project(project, target, package_features.into(), format),
        OvenCommand::SdkProviderStoreIdentity { compiler_root } => {
            let identity = incan_provider::sdk_store::sdk_provider_store_identity_for_compiler_root(&compiler_root)?;
            println!("{identity}");
            Ok(ExitCode::SUCCESS)
        }
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
        OvenCommand::Harvest {
            project,
            target,
            profile,
            cargo,
            rustc,
            cargo_lock,
            output,
            format,
        } => commands::oven_harvest(commands::OvenHarvestCommandOptions {
            project,
            target,
            profile: profile.as_str().to_string(),
            cargo,
            rustc,
            cargo_lock,
            output,
            format,
        }),
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
        OvenCommand::LegacyCargo { command } => match command {
            OvenLegacyCargoCommand::Prepare {
                receipt,
                generated_project,
                cargo,
                rustc,
                domain,
                store,
                format,
            } => commands::oven_legacy_cargo_prepare(commands::OvenLegacyCargoPrepareCommandOptions {
                receipt,
                generated_project,
                cargo,
                rustc,
                domain,
                store: store.into(),
                format,
            }),
            OvenLegacyCargoCommand::BakeLoafs {
                compiler_root,
                output,
                suite_store,
                policy_engine_store,
                policy_engine_identity,
                policy_engine_target,
                envelope,
                sdk_inventory,
                cargo,
                rustc,
                max_physical_bytes,
                max_domain_physical_bytes,
                max_domain_logical_bytes,
                format,
                loaf_registry,
                loaf_registry_commit,
                harvest_dir,
            } => commands::oven_legacy_cargo_bake_loafs(commands::OvenLoafBakeCommandOptions {
                compiler_root,
                output,
                suite_store,
                policy_engine_store,
                policy_engine_identity,
                policy_engine_target,
                envelope,
                sdk_inventory,
                cargo,
                rustc,
                max_physical_bytes,
                max_domain_physical_bytes,
                max_domain_logical_bytes,
                format,
                loaf_registry,
                loaf_registry_commit,
                harvest_dir,
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
            output,
            store: store.into(),
            format,
        }),
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
