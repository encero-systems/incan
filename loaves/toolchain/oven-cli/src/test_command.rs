//! Common test command option conversion and workspace dispatch.
use crate::workspace_scope::{resolve_workspace_command_scope, workspace_member_relative_path};
use crate::{CliError, CliResult, ExitCode, test_runner};
use incan_driver::lock::policy::{LockPolicy, LockPolicyCliFlags};
use incan_provider::FeatureSelection;
use std::path::{Path, PathBuf};

/// Apply shared test options through canonical workspace selection and the retained owner.
pub fn run(
    arguments: crate::cli::TestArgs,
    use_color: bool,
    languages: test_runner::TestLanguages,
) -> CliResult<ExitCode> {
    let crate::cli::TestArgs {
        path,
        package_features,
        sdk_profile,
        verbose,
        stop_on_fail,
        slow,
        filter,
        marker_expr,
        strict_markers,
        jobs,
        isolate,
        test_features,
        timeout,
        no_capture,
        fail_on_empty,
        list_only,
        report_format,
        junit_path,
        durations,
        shuffle,
        seed,
        run_xfail,
        locked,
        offline,
        no_offline,
        frozen,
        no_frozen,
        no_locked,
        workspace,
        members,
    } = arguments;
    execute_tests(
        TestCommandOptions {
            languages,
            path,
            verbose,
            stop_on_fail,
            include_slow: slow,
            filter,
            marker_expr,
            strict_markers,
            jobs,
            isolate,
            test_features,
            timeout,
            no_capture,
            use_color,
            fail_on_empty,
            list_only,
            report_format,
            junit_path,
            durations,
            shuffle,
            seed,
            run_xfail,
            package_features: package_features.into(),
            sdk_profile: sdk_profile.sdk_profile,
            lock_policy: LockPolicy::from_cli_and_env(LockPolicyCliFlags {
                offline,
                no_offline,
                locked,
                no_locked,
                frozen,
                no_frozen,
            }),
        },
        workspace,
        members,
    )
}

/// Owned test command inputs that can be replayed once for each validated workspace member.
struct TestCommandOptions {
    languages: test_runner::TestLanguages,
    path: PathBuf,
    verbose: bool,
    stop_on_fail: bool,
    include_slow: bool,
    filter: Option<String>,
    marker_expr: Option<String>,
    strict_markers: bool,
    jobs: usize,
    isolate: bool,
    test_features: Vec<String>,
    timeout: Option<String>,
    no_capture: bool,
    use_color: bool,
    fail_on_empty: bool,
    list_only: bool,
    report_format: test_runner::TestOutputFormat,
    junit_path: Option<PathBuf>,
    durations: Option<usize>,
    shuffle: bool,
    seed: Option<u64>,
    run_xfail: bool,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    lock_policy: LockPolicy,
}

impl TestCommandOptions {
    /// Run one member-local test batch without asking the runner to rediscover workspace topology.
    fn run_for(
        &self,
        path: &Path,
        workspace_context: Option<test_runner::WorkspaceTestContext>,
    ) -> CliResult<ExitCode> {
        let path = path.to_string_lossy();
        test_runner::run_tests(test_runner::TestRunConfig {
            languages: self.languages,
            path: &path,
            verbose: self.verbose,
            stop_on_fail: self.stop_on_fail,
            include_slow: self.include_slow,
            filter: self.filter.as_deref(),
            marker_expr: self.marker_expr.as_deref(),
            strict_markers: self.strict_markers,
            jobs: self.jobs,
            isolate: self.isolate,
            test_features: self.test_features.clone(),
            package_features: self.package_features.clone(),
            sdk_profile: self.sdk_profile.clone(),
            timeout: self.timeout.as_deref(),
            no_capture: self.no_capture,
            use_color: self.use_color,
            fail_on_empty: self.fail_on_empty,
            list_only: self.list_only,
            report_format: self.report_format,
            junit_path: self.junit_path.clone(),
            durations: self.durations,
            shuffle: self.shuffle,
            seed: self.seed,
            run_xfail: self.run_xfail,
            lock_policy: self.lock_policy.clone(),
            workspace_context,
        })
    }
}

/// Fan out test batches only after compiler-owned workspace scope selection has completed.
fn execute_tests(
    options: TestCommandOptions,
    select_workspace: bool,
    member_selectors: Vec<String>,
) -> CliResult<ExitCode> {
    let Some(scope) = resolve_workspace_command_scope(select_workspace, &member_selectors)? else {
        return options.run_for(&options.path, None);
    };
    if !scope.is_single_member() && options.junit_path.is_some() {
        return Err(CliError::failure(
            "--junit requires exactly one workspace member; select one with --member <name-or-path>",
        ));
    }

    if options.report_format == test_runner::TestOutputFormat::Json {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": "incan.test.v1",
                "event": "workspace_scope",
                "workspace": {
                    "root": scope.workspace_root().display().to_string(),
                    "selected_scope": {
                        "origin": scope.origin().as_str(),
                        "members": scope.members().map(|member| serde_json::json!({
                            "name": member.name(),
                            "root": member.root().display().to_string(),
                        })).collect::<Vec<_>>(),
                    },
                },
            })
        );
    }

    let mut failures = Vec::new();
    for member in scope.members() {
        let target = match workspace_member_relative_path(&options.path, member.root()) {
            Ok(target) => target,
            Err(error) => {
                failures.push(format!("{}: {}", member.name(), error.message));
                continue;
            }
        };
        if options.report_format == test_runner::TestOutputFormat::Console {
            println!("workspace member {}: {}", member.name(), target.display());
        }
        let workspace_context = test_runner::WorkspaceTestContext {
            workspace_root: scope.workspace_root().to_path_buf(),
            scope_origin: scope.origin().as_str().to_string(),
            member_name: member.name().to_string(),
            member_root: member.root().to_path_buf(),
        };
        if let Err(error) = options.run_for(&target, Some(workspace_context)) {
            if options.report_format == test_runner::TestOutputFormat::Json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema_version": "incan.test.v1",
                        "event": "workspace_member_error",
                        "workspace": {
                            "root": scope.workspace_root().display().to_string(),
                            "scope_origin": scope.origin().as_str(),
                            "member": {
                                "name": member.name(),
                                "root": member.root().display().to_string(),
                            },
                        },
                        "error": error.message,
                    })
                );
            } else if !error.message.is_empty() {
                eprintln!("{}", error.message);
            }
            failures.push(member.name().to_string());
        }
    }

    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::new("", ExitCode::FAILURE))
    }
}
