//! The decisions Oven actually needs from the forked grammar.
//!
//! These are not upstream's tests. They pin the behaviour Oven depends on when it projects a locked Rust dependency
//! graph onto one target: a `cfg` expression evaluated against the cfg set the selected compiler reports, and a
//! named-triple selector matched exactly.

use std::str::FromStr;

use incan_cargo_platform::{Cfg, Platform};

/// Build the cfg set the way Oven will: from `rustc --print cfg` output, not a table maintained by hand.
fn cfgs(print_cfg_output: &str) -> Vec<Cfg> {
    print_cfg_output
        .lines()
        .filter_map(|line| Cfg::from_str(line.trim()).ok())
        .collect()
}

/// Decide one manifest selector against a target, the way a dependency edge is decided.
fn edge_applies(selector: &str, target_triple: &str, cfg_set: &[Cfg]) -> bool {
    match Platform::from_str(selector) {
        Ok(Platform::Name(name)) => name == target_triple,
        Ok(Platform::Cfg(expression)) => expression.matches(cfg_set),
        Err(_) => false,
    }
}

const MACOS_ARM: &str = "unix\ntarget_family=\"unix\"\ntarget_os=\"macos\"\ntarget_arch=\"aarch64\"\ntarget_pointer_width=\"64\"\ntarget_endian=\"little\"\n";
const LINUX_X64: &str = "unix\ntarget_family=\"unix\"\ntarget_os=\"linux\"\ntarget_arch=\"x86_64\"\ntarget_env=\"gnu\"\ntarget_pointer_width=\"64\"\n";

/// The exact shape that prunes 83 packages from this repository's own graph on macOS.
#[test]
fn a_windows_only_edge_is_dropped_on_a_unix_target() {
    let macos = cfgs(MACOS_ARM);
    assert!(!edge_applies("cfg(windows)", "aarch64-apple-darwin", &macos));
    assert!(edge_applies("cfg(unix)", "aarch64-apple-darwin", &macos));
}

/// `any(...)` is the form real manifests use, e.g. mio's libc dependency.
#[test]
fn an_any_expression_matches_when_one_arm_holds() {
    let macos = cfgs(MACOS_ARM);
    let selector = "cfg(any(unix, target_os = \"hermit\", target_os = \"wasi\"))";
    assert!(edge_applies(selector, "aarch64-apple-darwin", &macos));
}

/// `not(...)` and `all(...)` must compose, or a nested selector silently flips.
#[test]
fn not_and_all_compose_correctly() {
    let macos = cfgs(MACOS_ARM);
    assert!(edge_applies("cfg(not(windows))", "aarch64-apple-darwin", &macos));
    assert!(edge_applies(
        "cfg(all(unix, target_arch = \"aarch64\"))",
        "aarch64-apple-darwin",
        &macos
    ));
    assert!(!edge_applies(
        "cfg(all(unix, target_arch = \"x86_64\"))",
        "aarch64-apple-darwin",
        &macos
    ));
}

/// The same selector must decide differently per target, which is the whole point of projecting a graph.
#[test]
fn one_selector_decides_differently_for_two_targets() {
    let selector = "cfg(target_env = \"gnu\")";
    assert!(!edge_applies(selector, "aarch64-apple-darwin", &cfgs(MACOS_ARM)));
    assert!(edge_applies(selector, "x86_64-unknown-linux-gnu", &cfgs(LINUX_X64)));
}

/// A bare triple selector is matched literally, never as a cfg.
#[test]
fn a_named_triple_selector_matches_only_itself() {
    let macos = cfgs(MACOS_ARM);
    assert!(edge_applies("aarch64-apple-darwin", "aarch64-apple-darwin", &macos));
    assert!(!edge_applies("x86_64-pc-windows-msvc", "aarch64-apple-darwin", &macos));
}

/// An unparseable selector must refuse rather than silently admit the edge.
#[test]
fn an_unparseable_selector_does_not_admit_the_edge() {
    let macos = cfgs(MACOS_ARM);
    assert!(!edge_applies("cfg(", "aarch64-apple-darwin", &macos));
}
