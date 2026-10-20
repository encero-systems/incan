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
mod comparison_bridges;
mod coroutines;
mod declarations;
mod error;
mod fallible_bridges;
mod field_metadata;
mod frontend;
mod frozen_values;
mod futures;
mod identity;
mod library_exports;
mod marker_traits;
mod plan;
mod rust_facts;
mod shared_callables;
mod spans;
mod statics;
mod terminators;
mod types;
mod union_debug;
mod validation;
mod value_enum_serde;
mod values;

use std::path::Path;

use incan_cli::commands::DiagnosticOutputFormat;
use incan_driver::unit::UnitDescription;

const USAGE: &str = "usage: incan-rustc-driver --unit UNIT CRATE OUTPUT SYSROOT [OPTIONS] | --check UNIT SYSROOT [--format \
                     text|json] [OPTIONS] | --rust-abi SYSROOT OUTPUT CRATE... [--extern CRATE=RLIB] [--search DIR]\n\
                     OPTIONS: --extern CRATE=RLIB, --search DIR, --rust-abi CACHE, --rustc-arg ARG, --library";

/// Compile an Incan unit (`--unit`) or check one (`--check`), each from the unit description Oven wrote, or write the
/// ABI cache of Rust crates (`--rust-abi`).
///
/// The shared opt-in invocation sink is initialized before dispatch; disabled tracing uses the ordinary logging filter.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    incan_driver::invocation_trace::initialize()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--unit") => frontend::compile(&args[1..]),
        Some("--check") => check(&args[1..]),
        Some("--rust-abi") => rust_abi(&args[1..]),
        _ => Err(USAGE.into()),
    }
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
    let rust = frontend::dependencies(options)?.rust_facts(Path::new(sysroot));
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    if let Err(error) = incan_cli::commands::check(&unit, &rust, format) {
        if !error.message.is_empty() {
            eprintln!("{}", error.message);
        }
        std::process::exit(error.exit_code.0);
    }
    Ok(())
}
