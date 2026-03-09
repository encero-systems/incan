//! Canonical stdlib module names.
use super::keywords;

/// Root stdlib namespace (e.g. `import std::...`).
pub const STDLIB_ROOT: &str = "std";

/// RFC 023: Rust module namespace for compiled `std.*` modules.
///
/// Compiled stdlib `.incn` files are emitted as submodules under `crate::__incan_std::*` to avoid shadowing Rust's
/// `std` crate. For example, `std.testing` compiles to `crate::__incan_std::testing`.
pub const INCAN_STD_NAMESPACE: &str = "__incan_std";

/// `std.web` module name.
pub const STDLIB_WEB: &str = "web";

/// `std.reflection` module name.
pub const STDLIB_REFLECTION: &str = "reflection";

/// `std::this` module name (`import this`).
pub const STDLIB_THIS: &str = "this";

/// `std.async` module name.
pub const STDLIB_ASYNC: &str = "async";

/// How a `std.*` namespace is implemented at runtime/emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibImplMode {
    /// Backed by the Rust runtime facade (`incan_stdlib::<ns>::...`).
    RuntimeFacade,
    /// Backed by emitted Incan-source modules (`crate::__incan_std::<ns>::...`).
    IncanSource,
}

/// Check if a module path starts with `std.<module>`.
pub fn is_stdlib_module(path: &[String], module: &str) -> bool {
    path.len() >= 2 && path[0] == STDLIB_ROOT && path[1] == module
}

/// Check if a module path is any `std.*` module.
///
/// Any path starting with `std.` is an Incan stdlib import. Rust standard library paths must use the `rust::` prefix
/// (e.g. `from rust::std::f64::consts import PI`), which routes through `ImportKind::RustFrom` and gets
/// `IrImportQualifier::None` — bypassing this check entirely.
pub fn is_any_stdlib_path(path: &[String]) -> bool {
    path.len() >= 2 && path[0] == STDLIB_ROOT
}

/// A top-level stdlib namespace with optional metadata.
///
/// Only top-level namespaces (`std.<name>`) are registered explicitly. Submodule stub paths are derived by convention
/// (`stdlib/{ns}/{sub}.incn`) relative to the active stdlib source root (resolved by loader), so adding a new
/// submodule requires zero changes here — just drop an `.incn` file in the right directory.
#[derive(Debug, Clone, Copy)]
pub struct StdlibNamespace {
    /// Top-level namespace name (e.g., `"web"`, `"testing"`, `"async"`).
    pub name: &'static str,
    /// How this namespace is materialized in emitted Rust.
    pub impl_mode: StdlibImplMode,
    /// Optional Cargo feature gate required for this namespace.
    pub feature: Option<&'static str>,
    /// Extra crate dependencies required by generated projects when this namespace is enabled.
    pub extra_crate_deps: &'static [StdlibExtraCrateDep],
    /// Known submodules for validation and LSP completion. Empty for leaf modules.
    pub submodules: &'static [&'static str],
}

/// Additional crate dependency needed by a stdlib namespace.
#[derive(Debug, Clone, Copy)]
pub struct StdlibExtraCrateDep {
    /// Cargo dependency key.
    pub crate_name: &'static str,
    /// Dependency source and version/path metadata.
    pub source: StdlibExtraCrateSource,
}

/// Source descriptor for a namespace-provided extra crate dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibExtraCrateSource {
    /// Path dependency rooted at compiler workspace root.
    Path(&'static str),
    /// Registry version dependency.
    Version(&'static str),
}

/// Registry of top-level stdlib namespaces.
///
/// Submodule stub paths are derived by convention, so this list stays compact even as the stdlib grows. Adding a new
/// submodule (e.g. `std.async.broadcast`) only requires adding the `.incn` file and appending the name to the parent
/// namespace's `submodules` array.
pub const STDLIB_NAMESPACES: &[StdlibNamespace] = &[
    StdlibNamespace {
        name: "web",
        impl_mode: StdlibImplMode::IncanSource,
        feature: Some("web"),
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "incan_web_macros",
                source: StdlibExtraCrateSource::Path("crates/incan_web_macros"),
            },
            StdlibExtraCrateDep {
                crate_name: "inventory",
                source: StdlibExtraCrateSource::Version("0.3"),
            },
            StdlibExtraCrateDep {
                crate_name: "axum",
                source: StdlibExtraCrateSource::Version("0.8"),
            },
        ],
        submodules: &["app", "routing", "request", "response", "macros", "prelude"],
    },
    StdlibNamespace {
        name: "testing",
        impl_mode: StdlibImplMode::IncanSource,
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
    },
    StdlibNamespace {
        name: "async",
        impl_mode: StdlibImplMode::IncanSource,
        feature: Some("async"),
        extra_crate_deps: &[],
        submodules: &["time", "task", "channel", "select", "sync", "prelude"],
    },
    StdlibNamespace {
        name: "serde",
        impl_mode: StdlibImplMode::IncanSource,
        feature: Some("json"),
        extra_crate_deps: &[],
        submodules: &["json"],
    },
    StdlibNamespace {
        name: "reflection",
        impl_mode: StdlibImplMode::IncanSource,
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
    },
    StdlibNamespace {
        name: "derives",
        impl_mode: StdlibImplMode::IncanSource,
        feature: None,
        extra_crate_deps: &[],
        submodules: &["string", "comparison", "copying", "collection"],
    },
    StdlibNamespace {
        name: "traits",
        impl_mode: StdlibImplMode::IncanSource,
        feature: None,
        extra_crate_deps: &[],
        submodules: &["convert", "ops", "error", "indexing", "callable", "prelude"],
    },
    StdlibNamespace {
        name: "math",
        impl_mode: StdlibImplMode::IncanSource,
        feature: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "libm",
            source: StdlibExtraCrateSource::Version("0.2"),
        }],
        submodules: &[],
    },
];

/// Look up a top-level stdlib namespace by name.
pub fn find_namespace(name: &str) -> Option<&'static StdlibNamespace> {
    STDLIB_NAMESPACES.iter().find(|ns| ns.name == name)
}

/// Resolve implementation mode for a stdlib module path (e.g. `["std", "testing"]`).
pub fn stdlib_impl_mode_for(path: &[String]) -> Option<StdlibImplMode> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    find_namespace(&path[1]).map(|ns| ns.impl_mode)
}

/// Resolve soft keywords activated by a stdlib import path.
///
/// `path` is expected in canonical segmented form (e.g. `["std", "async", "time"]`).
/// Returns an empty vector for non-stdlib paths or namespaces without soft keywords.
pub fn soft_keywords_for_import(path: &[String]) -> Vec<keywords::KeywordId> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return Vec::new();
    }
    keywords::soft_keywords_for_namespace(&path[1])
}

/// Check if a module path matches a known Incan stdlib module.
///
/// Unlike [`is_any_stdlib_path`] which accepts anything starting with `"std"`, this validates that the second segment
/// is a registered namespace and (for depth-3 paths) the third segment is a known submodule.
///
/// Use this to reject unknown `std.*` paths with a helpful diagnostic.
pub fn is_known_stdlib_module(path: &[String]) -> bool {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return false;
    }
    let Some(ns) = find_namespace(&path[1]) else {
        return false;
    };
    if path.len() == 2 {
        return true;
    }
    // Leaf modules (no submodules) don't have children.
    if ns.submodules.is_empty() {
        return false;
    }
    ns.submodules.contains(&path[2].as_str())
}

/// Human-friendly list of known stdlib modules for diagnostics.
///
/// Includes top-level namespaces and registered direct submodules.
pub fn known_stdlib_modules_for_hint() -> Vec<String> {
    let mut known = Vec::new();
    for ns in STDLIB_NAMESPACES {
        known.push(format!("std.{}", ns.name));
        for sub in ns.submodules {
            known.push(format!("std.{}.{}", ns.name, sub));
        }
    }
    known.sort();
    known.dedup();
    known
}

/// Look up the Cargo feature gate for a stdlib module path.
pub fn stdlib_feature_for(path: &[String]) -> Option<&'static str> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    find_namespace(&path[1]).and_then(|ns| ns.feature)
}

/// Resolve the relative stub `.incn` file path for a stdlib module path.
///
/// Uses convention-based resolution:
/// - `std.X` (leaf, no submodules) → `stdlib/X.incn`
/// - `std.X` (namespace with submodules) → `stdlib/X/prelude.incn`
/// - `std.X.Y` (and deeper) → `stdlib/X/Y.incn`
pub fn stdlib_stub_path(path: &[String]) -> Option<String> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    let ns = find_namespace(&path[1])?;
    if path.len() == 2 {
        if ns.submodules.is_empty() {
            Some(format!("stdlib/{}.incn", ns.name))
        } else {
            Some(format!("stdlib/{}/prelude.incn", ns.name))
        }
    } else {
        Some(format!("stdlib/{}.incn", path[1..].join("/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn known_stdlib_modules_cover_existing_surface_paths() {
        assert!(is_known_stdlib_module(&segs(&["std", "testing"])));
        assert!(is_known_stdlib_module(&segs(&["std", "web"])));
        assert!(is_known_stdlib_module(&segs(&["std", "web", "app"])));
        assert!(is_known_stdlib_module(&segs(&["std", "web", "routing"])));
        assert!(is_known_stdlib_module(&segs(&["std", "web", "request"])));
        assert!(is_known_stdlib_module(&segs(&["std", "web", "response"])));
        assert!(is_known_stdlib_module(&segs(&["std", "async"])));
        assert!(is_known_stdlib_module(&segs(&["std", "async", "prelude"])));
        assert!(is_known_stdlib_module(&segs(&["std", "async", "time"])));
        assert!(is_known_stdlib_module(&segs(&["std", "serde", "json"])));
        assert!(is_known_stdlib_module(&segs(&["std", "reflection"])));
        assert!(is_known_stdlib_module(&segs(&["std", "traits", "prelude"])));
        assert!(is_known_stdlib_module(&segs(&["std", "math"])));
    }

    #[test]
    fn unknown_stdlib_modules_are_rejected() {
        assert!(!is_known_stdlib_module(&segs(&["std", "f64", "consts"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "web", "missing"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "math", "extra"])));
    }

    #[test]
    fn stub_paths_follow_namespace_conventions() {
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "testing"])),
            Some("stdlib/testing.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "web"])),
            Some("stdlib/web/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "web", "app"])),
            Some("stdlib/web/app.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "async", "prelude"])),
            Some("stdlib/async/prelude.incn".to_string())
        );
    }

    #[test]
    fn known_modules_hint_is_registry_driven_and_sorted() {
        let hint = known_stdlib_modules_for_hint();
        assert!(hint.windows(2).all(|w| w[0] <= w[1]));
        assert!(hint.contains(&"std.derives".to_string()));
        assert!(hint.contains(&"std.web.app".to_string()));
        assert!(hint.contains(&"std.async.prelude".to_string()));
    }

    #[test]
    fn stdlib_impl_modes_are_registry_driven() {
        assert_eq!(
            stdlib_impl_mode_for(&segs(&["std", "testing"])),
            Some(StdlibImplMode::IncanSource)
        );
        assert_eq!(
            stdlib_impl_mode_for(&segs(&["std", "serde"])),
            Some(StdlibImplMode::IncanSource)
        );
        assert_eq!(
            stdlib_impl_mode_for(&segs(&["std", "web"])),
            Some(StdlibImplMode::IncanSource)
        );
        assert_eq!(stdlib_impl_mode_for(&segs(&["not_std", "testing"])), None);
    }
}
