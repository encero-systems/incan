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

const USAGE: &str =
    "usage: incan-rustc-driver --unit UNIT CRATE OUTPUT SYSROOT [OPTIONS] | --check UNIT [--format text|json]";

/// Compile an Incan unit (`--unit`) or check one (`--check`), each from the unit description Oven wrote.
///
/// The shared opt-in invocation sink is initialized before dispatch; disabled tracing uses the ordinary logging filter.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    incan_driver::invocation_trace::initialize()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--unit") => frontend::compile(&args[1..]),
        Some("--check") => check(&args[1..]),
        _ => Err(USAGE.into()),
    }
}

/// Check one unit and exit with the check's status; the report goes to stdout, a refusal's message to stderr.
fn check(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (path, format) = match arguments {
        [path] => (path, DiagnosticOutputFormat::Text),
        [path, flag, format] if flag == "--format" && format == "text" => (path, DiagnosticOutputFormat::Text),
        [path, flag, format] if flag == "--format" && format == "json" => (path, DiagnosticOutputFormat::Json),
        _ => return Err(USAGE.into()),
    };
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    if let Err(error) = incan_cli::commands::check(&unit, format) {
        if !error.message.is_empty() {
            eprintln!("{}", error.message);
        }
        std::process::exit(error.exit_code.0);
    }
    Ok(())
}
