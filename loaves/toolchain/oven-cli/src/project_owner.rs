//! Shared ownership of ordinary executable builds.
//!
//! The command sends resolved intent to one compatible owner. The driver remains the sole build implementation;
//! this module owns transport, readiness and request isolation. It introduces no artifact or source cache.

use incan_driver::build::BuildCommandOptions;
use incan_driver::error::{CliError, CliResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

mod source_format;

#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod unix;

/// Binding inspection uses the same compatible owner and ordinary dependency session as build/test.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingInspectionIntent {
    binding_inspection: crate::binding_inspect::BindingInspectionRequest,
}

/// Submit fresh checked binding analysis through the existing owner; clients retain stdout presentation.
pub(crate) fn inspect_bindings(request: crate::binding_inspect::BindingInspectionRequest) -> CliResult<String> {
    #[cfg(unix)]
    {
        let result = unix::submit(
            &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
            serde_json::to_value(BindingInspectionIntent {
                binding_inspection: request,
            })
            .map_err(|error| CliError::failure(error.to_string()))?,
            |_| {},
        )?;
        serde_json::from_value(result).map_err(|error| CliError::failure(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        crate::binding_inspect::execute_request(
            &request,
            &mut incan_driver::build::executable::ExecutableBuildOwner::default(),
        )
    }
}

/// One metadata-only check addressed to the same compatible project owner as build and test.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckIntent {
    check: incan_driver::build::check::CheckRequest,
}

/// Submit checking to the existing owner and return canonical presentation data to the invoking CLI.
/// Non-Unix clients use the same driver operation in process; transport failure never starts another compiler route.
pub fn check(
    path: &Path,
    features: &incan_provider::FeatureSelection,
    sdk_profile: Option<&str>,
    interop_target: Option<&str>,
) -> CliResult<incan_driver::build::check::CheckOutput> {
    let request = incan_driver::build::check::CheckRequest {
        path: std::path::absolute(path).map_err(|error| CliError::failure(error.to_string()))?,
        features: features.clone(),
        sdk_profile: sdk_profile.map(str::to_owned),
        interop_target: interop_target.map(str::to_owned),
    };
    #[cfg(unix)]
    {
        let result = unix::submit(
            &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
            serde_json::to_value(CheckIntent { check: request })
                .map_err(|error| CliError::failure(error.to_string()))?,
            |_| {},
        )?;
        serde_json::from_value(result).map_err(|error| CliError::failure(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        incan_driver::build::executable::ExecutableBuildOwner::default().check(&request)
    }
}

/// Source formatting batch addressed to the same compatible owner as build, check and tests.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFormatIntent {
    source_format: Vec<source_format::SourceFormatRequest>,
}

/// Format fresh file contents through bounded batches in the existing owner, preserving input/result order.
/// Each batch reconciles each project once and returns its ordered results in one completion message. The protocol
/// fragments large replies at its existing wire bound. Callers retain file writes and presentation; no result is
/// cached.
pub fn format_sources(files: &[(PathBuf, String)]) -> CliResult<Vec<CliResult<String>>> {
    let mut results = Vec::with_capacity(files.len());
    let mut batch = Vec::new();
    let mut batch_bytes = 0usize;
    for (path, source) in files {
        let request = source_format::SourceFormatRequest {
            path: std::path::absolute(path).map_err(|error| CliError::failure(error.to_string()))?,
            display_path: path.clone(),
            source: source.clone(),
        };
        let bytes = serde_json::to_vec(&request)
            .map_err(|error| CliError::failure(error.to_string()))?
            .len();
        if !batch.is_empty() && batch_bytes.saturating_add(bytes) > 8 * 1024 * 1024 {
            results.extend(submit_format_batch(std::mem::take(&mut batch))?);
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes.saturating_add(bytes);
        batch.push(request);
    }
    if !batch.is_empty() {
        results.extend(submit_format_batch(batch)?);
    }
    Ok(results
        .into_iter()
        .map(|result| result.map_err(CliError::failure))
        .collect())
}

/// Submit one bounded set of formatting inputs and verify that every input has exactly one ordered result.
fn submit_format_batch(requests: Vec<source_format::SourceFormatRequest>) -> CliResult<Vec<Result<String, String>>> {
    #[cfg(unix)]
    {
        let expected = requests.len();
        let result = unix::submit(
            &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
            serde_json::to_value(SourceFormatIntent {
                source_format: requests,
            })
            .map_err(|error| CliError::failure(error.to_string()))?,
            |_| {},
        )?;
        let results: Vec<Result<String, String>> =
            serde_json::from_value(result).map_err(|error| CliError::failure(error.to_string()))?;
        if results.len() != expected {
            return Err(CliError::failure("owner formatting result count differs from request"));
        }
        Ok(results)
    }
    #[cfg(not(unix))]
    {
        Ok(source_format::execute_batch(
            &requests,
            &mut incan_driver::build::executable::ExecutableBuildOwner::default(),
        ))
    }
}

/// A build intent with explicit paths and the existing driver option model.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildIntent {
    file_path: String,
    output_dir: Option<String>,
    options: BuildCommandOptions,
}

/// Internal native executable envelope; the same driver owner retains its completed native preparation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeBuildIntent {
    native_build: incan_driver::build::executable::NativeExecutableBuildRequest,
}

/// Submit an explicit receipt-bound native executable to the selected compiler's existing project owner.
///
/// Compiler helpers must name an executable that hosts their linked owner protocol; the ordinary CLI and the compiler
/// helper both implement that same internal host mode. This is a Rust interop transport for compiler tooling, not a new
/// user command. The owner validates all paths and runs the same direct executor; serialized reports carry no execution
/// leases or authority.
#[allow(clippy::too_many_arguments)]
pub fn build_native_executable(
    owner_executable: &Path,
    store: &oven_store::store::OvenStore,
    source_receipt: &oven_store::OvenReceipt,
    declaration: &Path,
    rustc: &Path,
    graph: &Path,
    index: &Path,
    blobs: &Path,
    preparation_output: &Path,
    source: &Path,
    output: &Path,
    crate_name: &str,
    edition: &str,
    source_key: &str,
) -> CliResult<String> {
    let intent = NativeBuildIntent {
        native_build: incan_driver::build::executable::NativeExecutableBuildRequest {
            store: std::path::absolute(store.root()).map_err(|error| CliError::failure(error.to_string()))?,
            limits: *store.limits(),
            source_receipt: source_receipt.clone(),
            declaration: std::path::absolute(declaration).map_err(|error| CliError::failure(error.to_string()))?,
            rustc: std::path::absolute(rustc).map_err(|error| CliError::failure(error.to_string()))?,
            graph: std::path::absolute(graph).map_err(|error| CliError::failure(error.to_string()))?,
            index: std::path::absolute(index).map_err(|error| CliError::failure(error.to_string()))?,
            blobs: std::path::absolute(blobs).map_err(|error| CliError::failure(error.to_string()))?,
            preparation_output: std::path::absolute(preparation_output)
                .map_err(|error| CliError::failure(error.to_string()))?,
            source: std::path::absolute(source).map_err(|error| CliError::failure(error.to_string()))?,
            output: std::path::absolute(output).map_err(|error| CliError::failure(error.to_string()))?,
            crate_name: crate_name.to_string(),
            edition: edition.to_string(),
            source_key: source_key.to_string(),
        },
    };
    intent.native_build.validate_paths()?;
    #[cfg(unix)]
    let result = unix::submit(
        owner_executable,
        serde_json::to_value(intent).map_err(|error| CliError::failure(error.to_string()))?,
        |_| {},
    )?;
    #[cfg(not(unix))]
    let result = {
        let _host = owner_executable;
        incan_driver::build::executable::ExecutableBuildOwner::default().build_native(&intent.native_build)?
    };
    serde_json::to_string(&result).map_err(|error| CliError::failure(error.to_string()))
}

/// One parser-only collection request, using the same bounded protocol and serial project owner as builds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestListIntent {
    test_list: crate::test_runner::TestCollectionOptions,
}

/// Submit list-only collection and render this request's notices and identities in the CLI.
///
/// Execution remains in the existing runner. Non-Unix clients invoke the same collection operation in process;
/// transport errors never select a second implementation.
pub(crate) fn list_tests(config: &crate::test_runner::TestRunConfig<'_>) -> CliResult<crate::ExitCode> {
    let mut options = crate::test_runner::TestCollectionOptions::from_config(config);
    options.path = absolute_path(&options.path)?;
    #[cfg(unix)]
    let report: crate::test_runner::TestListReport = serde_json::from_value(unix::submit(
        &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
        serde_json::to_value(TestListIntent { test_list: options })
            .map_err(|error| CliError::failure(error.to_string()))?,
        |message| print!("{message}"),
    )?)
    .map_err(|error| CliError::failure(error.to_string()))?;
    #[cfg(not(unix))]
    let report = crate::test_runner::list_tests_with_owner(
        &options,
        &mut incan_driver::build::executable::ExecutableBuildOwner::default(),
        |message| print!("{message}"),
    )?;
    if report.ids.is_empty() {
        eprintln!("No tests collected");
        return if config.fail_on_empty {
            Err(CliError::new("", crate::ExitCode::FAILURE))
        } else {
            Ok(crate::ExitCode::SUCCESS)
        };
    }
    for id in report.ids {
        println!("{id}");
    }
    Ok(crate::ExitCode::SUCCESS)
}

/// One owned execution request addressed to the same project owner as build and collection.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestExecutionIntent {
    test_execute: crate::test_runner::TestExecutionRequest,
}

/// Submit actual test execution and render only this request's framed output in its invoking CLI.
/// The existing runner remains the only implementation; non-Unix clients call that operation in process.
pub(crate) fn execute_tests(config: &crate::test_runner::TestRunConfig<'_>) -> CliResult<crate::ExitCode> {
    let mut request = crate::test_runner::TestExecutionRequest::from_config(config);
    request.collection.path = absolute_path(&request.collection.path)?;
    request.junit_path = request
        .junit_path
        .as_ref()
        .map(std::path::absolute)
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    request.validate_paths()?;
    #[cfg(unix)]
    {
        let result = unix::submit(
            &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
            serde_json::to_value(TestExecutionIntent { test_execute: request })
                .map_err(|error| CliError::failure(error.to_string()))?,
            |message| print!("{message}"),
        )?;
        let exit: i32 = serde_json::from_value(result).map_err(|error| CliError::failure(error.to_string()))?;
        Ok(crate::ExitCode(exit))
    }
    #[cfg(not(unix))]
    {
        crate::test_runner::execute_tests_with_owner(
            &request,
            &mut incan_driver::build::executable::ExecutableBuildOwner::default(),
            crate::test_runner::output::TestOutputSink::default(),
        )
    }
}

/// Resolve paths once at the client boundary without changing the owner's working directory.
fn absolute_path(path: &str) -> CliResult<String> {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::failure(error.to_string()))?
            .join(path)
    };
    path.into_os_string()
        .into_string()
        .map_err(|_| CliError::failure("build path is not valid UTF-8"))
}

/// Submit an actual build to the compatible owner, streaming progress to this request's presentation sink.
///
/// Unix clients start the owner automatically. Other platforms keep the same extracted driver operation in process
/// until their service transport is accepted; this is a platform boundary, not a fallback after a failed request.
pub fn build(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    progress: impl FnMut(String),
) -> CliResult<serde_json::Value> {
    let intent = BuildIntent {
        file_path: absolute_path(file_path)?,
        output_dir: output_dir.map(|path| absolute_path(path)).transpose()?,
        options,
    };
    #[cfg(unix)]
    {
        unix::build(intent, progress)
    }
    #[cfg(not(unix))]
    {
        incan_driver::build::executable::build_executable(
            &intent.file_path,
            intent.output_dir.as_ref(),
            intent.options,
            progress,
        )
    }
}

/// Recognize only the internal owner invocation before ordinary CLI parsing.
///
/// Returns false for ordinary commands. The service host uses the normal compiler stack and tracing setup; startup
/// never creates an alternative compiler route or suppresses a driver failure.
pub fn run_internal() -> CliResult<bool> {
    #[cfg(unix)]
    {
        unix::run_internal()
    }
    #[cfg(not(unix))]
    {
        Ok(false)
    }
}
