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
mod shared_callables;
mod spans;
mod statics;
mod terminators;
mod types;
mod union_debug;
mod unit;
mod validation;
mod value_enum_serde;
mod values;

use std::path::Path;
use std::process::Command;

/// Obtain the existing ordinary platform policy; source-native compilation refuses an unpinned target.
fn native_link(identity: &identity::DriverIdentity) -> Result<(Command, String), Box<dyn std::error::Error>> {
    let rustc = identity.sysroot.join("bin/rustc");
    let mut command = Command::new(&rustc);
    let identity = oven_rustc::rustc::configure_pinned_native_link(&mut command, &rustc, &identity.target)?
        .ok_or("native driver target has no pinned link policy")?;
    Ok((command, identity))
}

/// Relaunch a cache miss under the declared link environment before rustc can create compiler threads.
///
/// Environment changes are confined to the child process rather than mutating global state around embedded rustc.
/// On unchanged output reuse this function is never entered. Reapplying the same policy in the child terminates the
/// relaunch because its explicit environment already matches; no inherited flag can bypass policy verification.
fn relaunch_with_native_environment(native_link: &Command) -> Result<(), Box<dyn std::error::Error>> {
    if native_link
        .get_envs()
        .all(|(key, value)| std::env::var_os(key).as_deref() == value)
    {
        return Ok(());
    }
    let mut child = Command::new(std::env::current_exe()?);
    child.args(std::env::args_os().skip(1));
    for (key, value) in native_link.get_envs() {
        if let Some(value) = value {
            child.env(key, value);
        } else {
            child.env_remove(key);
        }
    }
    std::process::exit(child.status()?.code().unwrap_or(1));
}

/// Compile an Incan unit (`--source`) or a Rust unit (`--rust-unit`) through the pinned rustc.
///
/// The shared opt-in invocation sink is initialized before dispatch; disabled tracing uses the ordinary logging filter.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    oven_cli::invocation_trace::initialize()?;
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
    Err("usage: incan-rustc-driver --source SOURCE CRATE OUTPUT SYSROOT [OPTIONS] | --rust-unit SYSROOT ... -- RUSTC_ARGS".into())
}
