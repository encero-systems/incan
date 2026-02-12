//! Canonical stdlib module names.

/// Root stdlib namespace (e.g. `import std::...`).
pub const STDLIB_ROOT: &str = "std";

/// RFC 023: Rust module namespace for compiled `std.*` modules.
///
/// Compiled stdlib `.incn` files are emitted as submodules under `crate::__incan_std::*` to avoid shadowing Rust's
/// `std` crate. For example, `std.testing` compiles to `crate::__incan_std::testing`.
pub const INCAN_STD_NAMESPACE: &str = "__incan_std";

/// `std.web` module name.
pub const STDLIB_WEB: &str = "web";

/// `std.testing` module name.
pub const STDLIB_TESTING: &str = "testing";

/// `std.reflection` module name.
pub const STDLIB_REFLECTION: &str = "reflection";

/// `std::this` module name (`import this`).
pub const STDLIB_THIS: &str = "this";

/// Check if a module path starts with `std.<module>`.
pub fn is_stdlib_module(path: &[String], module: &str) -> bool {
    path.len() >= 2 && path[0] == STDLIB_ROOT && path[1] == module
}

/// Metadata for a stdlib surface module.
#[derive(Debug, Clone, Copy)]
pub struct StdlibModuleInfo {
    pub path: &'static [&'static str],
    pub feature: Option<&'static str>,
    pub stub_path: &'static str,
}

pub const STDLIB_MODULES: &[StdlibModuleInfo] = &[
    StdlibModuleInfo {
        path: &["std", "web"],
        feature: Some("web"),
        stub_path: "stdlib/web/prelude.incn",
    },
    StdlibModuleInfo {
        path: &["std", "testing"],
        feature: None,
        stub_path: "stdlib/testing.incn",
    },
    StdlibModuleInfo {
        path: &["std", "async"],
        feature: None,
        stub_path: "stdlib/async/prelude.incn",
    },
    StdlibModuleInfo {
        path: &["std", "serde", "json"],
        feature: Some("json"),
        stub_path: "stdlib/serde/json.incn",
    },
    StdlibModuleInfo {
        path: &["std", "reflection"],
        feature: None,
        stub_path: "stdlib/reflection.incn",
    },
];

pub fn stdlib_module_info(path: &[String]) -> Option<&'static StdlibModuleInfo> {
    STDLIB_MODULES
        .iter()
        .find(|info| info.path.len() == path.len() && info.path.iter().zip(path.iter()).all(|(a, b)| a == b))
}

pub fn stdlib_feature_for(path: &[String]) -> Option<&'static str> {
    STDLIB_MODULES
        .iter()
        .find(|info| path.len() >= info.path.len() && info.path.iter().zip(path.iter()).all(|(a, b)| a == b))
        .and_then(|info| info.feature)
}

pub fn stdlib_stub_path(path: &[String]) -> Option<String> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    if let Some(info) = stdlib_module_info(path) {
        return Some(info.stub_path.to_string());
    }
    if path.len() >= 3 {
        return Some(format!("stdlib/{}.incn", path[1..].join("/")));
    }
    None
}
