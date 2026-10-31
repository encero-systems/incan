//! The pinned native driver: compiles Incan units and Rust units through the pinned rustc.
#![feature(rustc_private)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_feature;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_infer;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_trait_selection;
extern crate thin_vec;

mod adapter;
mod ast_build;
mod bodies;
mod callable_bridges;
mod callees;
mod captured_generators;
mod checked_json;
mod cloneable_closures;
mod closures;
mod codegraph;
mod command_diagnostics;
mod command_format;
mod comparison_bridges;
mod coroutines;
mod declarations;
mod derived_display;
mod error;
mod fallible_bridges;
mod field_metadata;
mod foreign;
mod formatting;
mod frontend;
mod frozen_values;
mod futures;
mod generic_layouts;
mod generic_loans;
mod identity;
mod library_exports;
mod native_attributes;
mod native_crates;
mod native_loans;
mod native_search;
mod native_shared;
mod native_timing;
mod native_trait_bridges;
mod panic_hooks;
mod plan;
mod program_entries;
mod rust_codegraph;
mod rust_facts;
mod rust_graph_bodies;
mod rust_graph_records;
mod rust_imports;
mod shared_callables;
mod signature_lifetimes;
mod spans;
mod statics;
mod terminators;
mod test_unit;
mod types;
mod union_debug;
mod validation;
mod value_enum_serde;
mod values;
#[cfg(unix)]
mod worker;
mod zip_carriers;

use std::path::Path;

use command_diagnostics::DiagnosticOutputFormat;
use incan_driver::unit::UnitDescription;

/// The receipt `--check --certify PATH` writes when unchanged inputs reproduce the check's exact report.
const CHECK_CERTIFICATE: &str = "incan.compiler-check/1\n";

const USAGE: &str = "usage: incan-rustc-driver --unit UNIT CRATE OUTPUT SYSROOT [--certify PATH] [OPTIONS] | \
                     --source-interface UNIT SYSROOT DIRECTORY | \
                     --check UNIT SYSROOT [--format text|json] [--certify PATH] [OPTIONS] | --explain CODE | --fmt UNIT PATH [--check] | --source-files PATH | --rust-imports UNIT | --codegraph UNIT SYSROOT OUTPUT [INTERFACE] | --read-codegraph GRAPH ROOT GROUP [FIELDS_JSON] | --rust-codegraph OUTPUT ROOT RUSTC_ARGS... | --rust-compile-codegraph OUTPUT ROOT RUSTC_ARGS... | --rust-abi SYSROOT OUTPUT CRATE... [--extern CRATE=RLIB] [--search DIR] | --resident SOCKET GENERATION READY | --identity SYSROOT\n\
                     OPTIONS: --rustc-arg ARG, --library, --shared-native DIR (native artifacts and ABI caches come from UNIT)";

/// Compile an Incan unit (`--unit`, or serially from a `--resident` worker) or check one (`--check`), each from the
/// unit description Oven wrote, publish a checking-only source interface (`--source-interface`), or write the ABI cache
/// of Rust crates (`--rust-abi`), format source with the unit's vocabulary (`--fmt`), discover authored files
/// (`--source-files`), explain a stable diagnostic (`--explain`), or export and read checked graphs (`--codegraph`,
/// `--read-codegraph`, `--rust-codegraph` and `--rust-compile-codegraph`).
///
/// The shared opt-in invocation sink is initialized before dispatch; disabled tracing uses the ordinary logging filter.
/// A command that fails prints its error as [`report`] does and exits with that status.
fn main() -> std::process::ExitCode {
    match command() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => std::process::ExitCode::from(report(error.as_ref())),
    }
}

/// Dispatch the command the first argument names.
fn command() -> Result<(), Box<dyn std::error::Error>> {
    incan_driver::invocation_trace::initialize()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(unix)]
        Some("--resident") => worker::serve(&args[1..]),
        Some("--unit") => frontend::compile(&args[1..]),
        Some("--source-interface") => frontend::source_interface(&args[1..]),
        Some("--check") => exit_with(check(&args[1..])?),
        Some("--explain") => exit_with(command_diagnostics::explain(&args[1..])?),
        Some("--fmt") => exit_with(format(&args[1..])?),
        Some("--source-files") => exit_with(source_files(&args[1..])?),
        Some("--codegraph") => codegraph::incan(&args[1..]),
        Some("--read-codegraph") => codegraph::read(&args[1..]),
        Some("--rust-codegraph") => rust_codegraph::export(&args[1..]),
        Some("--rust-compile-codegraph") => rust_codegraph::compile(&args[1..]),
        Some("--rust-abi") => rust_abi(&args[1..]),
        Some("--rust-imports") => rust_imports::export(&args[1..]),
        Some("--identity") => verify_sysroot(&args[1..]),
        _ => Err(USAGE.into()),
    }
}

/// Verify the sysroot's compiler identity (`--identity`) and print its host target: the proof, without compiling,
/// that this driver runs against that sysroot. Installers use it to check a relocated toolchain.
fn verify_sysroot(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [sysroot] = arguments else {
        return Err("usage: incan-rustc-driver --identity SYSROOT".into());
    };
    let verified = identity::verify(Path::new(sysroot))?;
    println!("{}", verified.target);
    Ok(())
}

/// End the process with a command's nonzero status; status zero returns, so `main` completes normally.
fn exit_with(code: i32) -> Result<(), Box<dyn std::error::Error>> {
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Print a failed command's message to stderr and return its status; a command that succeeded has status zero.
fn status_of<T>(result: incan_driver::error::CliResult<T>) -> i32 {
    match result {
        Ok(_) => 0,
        Err(error) => {
            print_failure(&error.message);
            error.exit_code.0
        }
    }
}

/// Print an error a command returned and give the status it exits with. A refusal reads exactly as `--check` prints
/// it, never as a quoted, escaped debug string; the short-lived command and the resident worker share this.
pub(crate) fn report(error: &dyn std::error::Error) -> u8 {
    print_failure(&error.to_string());
    1
}

/// Print a failure's message to stderr as it reads, with a final newline; an empty message prints nothing.
fn print_failure(message: &str) {
    if !message.is_empty() {
        eprintln!("{message}");
    }
}

/// Print the authored Incan sources under one path as a JSON list, the files a formatting run selects, and return
/// status zero. A short-lived process and the resident worker share this exact path.
pub(crate) fn source_files(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let [path] = arguments else {
        return Err(USAGE.into());
    };
    let path = std::path::absolute(path)?;
    let files = command_format::source_files(&path).map_err(|error| error.message)?;
    println!("{}", serde_json::to_string(&files)?);
    Ok(0)
}

/// Format a source through the compiler's command function using the unit vocabulary supplied by Oven, and return
/// the command's status. A short-lived process and the resident worker share this exact path.
pub(crate) fn format(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let [unit_path, source, options @ ..] = arguments else {
        return Err("usage: incan-rustc-driver --fmt UNIT PATH [--check]".into());
    };
    let check = match options {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: incan-rustc-driver --fmt UNIT PATH [--check]".into()),
    };
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let formatted = command_format::format_files(source, &unit, check);
    Ok(status_of(formatted))
}

/// Write the ABI cache of the named Rust crates: every public item, described from rustc's crate metadata.
fn rust_abi(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [sysroot, output, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let mut crates = std::collections::BTreeSet::new();
    let mut externs = Vec::new();
    let mut search = Vec::new();
    let mut remaining = rest.iter();
    while let Some(argument) = remaining.next() {
        match argument.as_str() {
            "--extern" => {
                let value = remaining.next().ok_or(USAGE)?;
                let (name, path) = value.split_once('=').ok_or("--extern takes CRATE=RLIB")?;
                externs.push((name.to_owned(), std::path::PathBuf::from(path)));
            }
            "--search" => search.push(std::path::PathBuf::from(remaining.next().ok_or(USAGE)?)),
            name => {
                crates.insert(name.to_owned());
            }
        }
    }
    let facts = rust_facts::extract(rust_facts::RustFactsRequest {
        sysroot: Path::new(sysroot),
        externs: &externs,
        search: &search,
        scope: rust_facts::RustFactsScope::Crates(crates),
    })?;
    std::fs::write(output, incan_lang::interop::encode_rust_abi_cache(&facts)?)?;
    Ok(())
}

/// Check one unit and return the check's status; the report goes to stdout, a refusal's message to stderr. The
/// unit's crates and their ABI caches supply its Rust facts, as for `--unit`. A short-lived process and the resident
/// worker share this exact path.
///
/// With `--certify PATH`, a report that unchanged inputs reproduce exactly (a pass, or a refusal by source
/// diagnostics) also writes the check receipt there, which lets Oven replay it; any other outcome writes none.
pub(crate) fn check(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let [path, sysroot, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let (format, rest) = match rest {
        [flag, format, rest @ ..] if flag == "--format" => match format.as_str() {
            "text" => (DiagnosticOutputFormat::Text, rest),
            "json" => (DiagnosticOutputFormat::Json, rest),
            _ => return Err(USAGE.into()),
        },
        rest => (DiagnosticOutputFormat::Text, rest),
    };
    let (certificate, options) = certified_options(rest);
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    let rust = frontend::dependencies(&unit, options)?.rust_facts(&unit, Path::new(sysroot));
    let checked = incan_driver::check::check_unit(&unit, &rust);
    let reproducible = checked.as_ref().is_ok_and(|report| report.reproducible);
    let status = status_of(checked.and_then(|report| command_diagnostics::render_check_report(report, format)));
    if reproducible && let Some(certificate) = certificate {
        // A receipt that cannot be written only means the next unchanged check runs again.
        let _ = std::fs::write(certificate, CHECK_CERTIFICATE);
    }
    Ok(status)
}

/// Split a leading `--certify PATH` from a command's native options. The path is where `--check` and `--unit` write
/// their receipt when unchanged inputs reproduce the outcome exactly; the remaining options are returned as they are.
pub(crate) fn certified_options(options: &[String]) -> (Option<&Path>, &[String]) {
    match options {
        [flag, certificate, options @ ..] if flag == "--certify" => (Some(Path::new(certificate)), options),
        options => (None, options),
    }
}
