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
mod comparison_bridges;
mod coroutines;
mod declarations;
mod error;
mod fallible_bridges;
mod field_metadata;
mod formatting;
mod frontend;
mod frozen_values;
mod futures;
mod generic_layouts;
mod identity;
mod library_exports;
mod native_attributes;
mod native_crates;
mod plan;
mod program_entries;
mod rust_codegraph;
mod rust_facts;
mod rust_graph_bodies;
mod rust_graph_records;
mod shared_callables;
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

use incan_cli::commands::DiagnosticOutputFormat;
use incan_driver::unit::UnitDescription;

const USAGE: &str = "usage: incan-rustc-driver --unit UNIT CRATE OUTPUT SYSROOT [OPTIONS] | --check UNIT SYSROOT [--format \
                     text|json] [OPTIONS] | --fmt UNIT PATH [--check] | --codegraph UNIT SYSROOT OUTPUT | --rust-codegraph OUTPUT ROOT RUSTC_ARGS... | --rust-abi SYSROOT OUTPUT CRATE... [--extern CRATE=RLIB] [--search DIR] | --identity SYSROOT\n\
                     OPTIONS: --rustc-arg ARG, --library (native artifacts and ABI caches come from UNIT)";

/// Compile an Incan unit (`--unit`) or check one (`--check`), each from the unit description Oven wrote, or write the
/// ABI cache of Rust crates (`--rust-abi`), format source with the unit's vocabulary (`--fmt`), or export checked
/// graphs (`--codegraph` and `--rust-codegraph`).
///
/// The shared opt-in invocation sink is initialized before dispatch; disabled tracing uses the ordinary logging filter.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    incan_driver::invocation_trace::initialize()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(unix)]
        Some("--resident") => worker::serve(&args[1..]),
        Some("--unit") => frontend::compile(&args[1..]),
        Some("--check") => check(&args[1..]),
        Some("--fmt") => format(&args[1..]),
        Some("--codegraph") => codegraph::incan(&args[1..]),
        Some("--read-codegraph") => codegraph::read(&args[1..]),
        Some("--rust-codegraph") => rust_codegraph::export(&args[1..]),
        Some("--rust-compile-codegraph") => rust_codegraph::compile(&args[1..]),
        Some("--rust-abi") => rust_abi(&args[1..]),
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

/// Format a source through the compiler's command function using the unit vocabulary supplied by Oven.
fn format(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit_path, source, options @ ..] = arguments else {
        return Err("usage: incan-rustc-driver --fmt UNIT PATH [--check]".into());
    };
    let check = match options {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: incan-rustc-driver --fmt UNIT PATH [--check]".into()),
    };
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    if let Err(error) = incan_cli::commands::format_files(source, &unit, check, false) {
        if !error.message.is_empty() {
            eprintln!("{}", error.message);
        }
        std::process::exit(error.exit_code.0);
    }
    Ok(())
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

/// Check one unit and exit with the check's status; the report goes to stdout, a refusal's message to stderr. The
/// unit's crates and their ABI caches supply its Rust facts, as for `--unit`.
fn check(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path, sysroot, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let (format, options) = match rest {
        [flag, format, options @ ..] if flag == "--format" => match format.as_str() {
            "text" => (DiagnosticOutputFormat::Text, options),
            "json" => (DiagnosticOutputFormat::Json, options),
            _ => return Err(USAGE.into()),
        },
        options => (DiagnosticOutputFormat::Text, options),
    };
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    let rust = frontend::dependencies(&unit, options)?.rust_facts(&unit, Path::new(sysroot));
    if let Err(error) = incan_cli::commands::check(&unit, &rust, format) {
        if !error.message.is_empty() {
            eprintln!("{}", error.message);
        }
        std::process::exit(error.exit_code.0);
    }
    Ok(())
}
