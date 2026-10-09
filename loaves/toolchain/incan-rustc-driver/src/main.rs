//! Pinned native driver and scalar-plan conformance executable.
#![feature(rustc_private)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_feature;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate thin_vec;

mod adapter;
mod bodies;
mod callees;
mod captured_generators;
mod closures;
mod declarations;
mod error;
mod frontend;
mod identity;
mod plan;
mod spans;
mod terminators;
mod types;
mod unit;
mod validation;
mod values;

use incan_mir_lowering::caller::incan::scalar_example;
use std::path::{Path, PathBuf};

/// Take the hand-filled fixture plan from Incan and compile it through the native boundary.
///
/// Arguments are source, crate name, output, sysroot, runtime rlib, optional mode, and admitted dependency directories.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--source") {
        return frontend::compile(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "--rust-unit") {
        let separator = args
            .iter()
            .position(|arg| arg == "--")
            .ok_or("Rust unit invocation requires -- before rustc arguments")?;
        let sysroot = args.get(1).ok_or("Rust unit invocation requires a sysroot")?;
        unit::compile(&args[separator + 1..], Path::new(sysroot), &args[2..separator])?;
        return Ok(());
    }
    if args.len() < 5 {
        return Err("usage: incan-rustc-driver SOURCE CRATE OUTPUT SYSROOT RUNTIME_RLIB [normal|overflow|dangling] [DEPENDENCY_DIR ...]".into());
    }
    let mode = args.get(5).map(String::as_str).unwrap_or("normal");
    if !matches!(mode, "normal" | "overflow" | "dangling") {
        return Err("unknown fixture mode".into());
    }
    identity::verify(Path::new(&args[3]))?;
    let plan = scalar_example(
        args[0].clone(),
        "native_output::print_int".into(),
        mode == "overflow",
        mode == "dangling",
    );
    adapter::compile(
        plan,
        &args[1],
        Path::new(&args[2]),
        Path::new(&args[3]),
        &[("native_output".into(), PathBuf::from(&args[4]))],
        &args.iter().skip(6).map(PathBuf::from).collect::<Vec<_>>(),
    )?;
    Ok(())
}
