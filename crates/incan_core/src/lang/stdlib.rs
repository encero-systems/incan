//! Canonical stdlib module names.
use super::{
    derives::{self, DeriveId},
    keywords,
    traits::{self, TraitId},
};

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
/// `std.rust` module name for capability bounds (RFC 041).
pub const STDLIB_RUST: &str = "rust";

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
    /// Optional Cargo feature gate required for this namespace.
    pub feature: Option<&'static str>,
    /// Extra crate dependencies required by generated projects when this namespace is enabled.
    pub extra_crate_deps: &'static [StdlibExtraCrateDep],
    /// Known submodules for validation and LSP completion. Empty for leaf modules.
    pub submodules: &'static [&'static str],
    /// When `true`, this namespace is handled entirely by the typechecker with no corresponding `.incn` stub file or
    /// emitted Rust module. Items are resolved symbolically at import time.
    ///
    /// `std.rust` is the canonical example: `Send`, `Sync`, etc. map to native Rust traits that are already in scope
    /// and must not be re-declared or imported in generated code.
    pub typechecker_only: bool,
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

/// Builtin trait identity for stdlib method-stub lookup.
///
/// Most source-defined trait contracts are canonical builtin traits. `Copy` is represented as a derive vocabulary item
/// today because it has no separate [`TraitId`] entry, but still needs the same empty-stub fallback as trait contracts
/// such as `Clone` and `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibTraitMethodOwner {
    /// Builtin trait vocabulary item.
    Trait(TraitId),
    /// Builtin derive vocabulary item that also names a source-defined stdlib trait.
    Derive(DeriveId),
}

impl StdlibTraitMethodOwner {
    /// Return the canonical source spelling used to match this owner against a typechecker trait name.
    fn canonical_name(self) -> &'static str {
        match self {
            Self::Trait(id) => traits::as_str(id),
            Self::Derive(id) => derives::as_str(id),
        }
    }
}

/// Registry entry for stdlib modules whose trait stubs provide fallback method signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibTraitMethodModule {
    /// Builtin trait or derive vocabulary item resolved by canonical spelling.
    pub owner: StdlibTraitMethodOwner,
    /// Canonical segmented stdlib module path, e.g. `["std", "derives", "copying"]`.
    pub module_segments: &'static [&'static str],
}

const STDLIB_DERIVES_COPYING_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "copying"];
const STDLIB_DERIVES_STRING_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "string"];
const STDLIB_DERIVES_COMPARISON_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "comparison"];

/// Builtin stdlib trait-method fallback registry.
///
/// This intentionally covers only the derive-tree source-defined traits that previously needed fallback from empty
/// symbol-table stubs. Other stdlib trait families, such as `std.traits.convert`, do not get implicit fallback unless
/// they are added here deliberately.
pub const STDLIB_TRAIT_METHOD_MODULES: &[StdlibTraitMethodModule] = &[
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Clone),
        module_segments: STDLIB_DERIVES_COPYING_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Derive(DeriveId::Copy),
        module_segments: STDLIB_DERIVES_COPYING_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Default),
        module_segments: STDLIB_DERIVES_COPYING_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Debug),
        module_segments: STDLIB_DERIVES_STRING_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Display),
        module_segments: STDLIB_DERIVES_STRING_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Eq),
        module_segments: STDLIB_DERIVES_COMPARISON_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::PartialEq),
        module_segments: STDLIB_DERIVES_COMPARISON_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Ord),
        module_segments: STDLIB_DERIVES_COMPARISON_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::PartialOrd),
        module_segments: STDLIB_DERIVES_COMPARISON_SEGMENTS,
    },
    StdlibTraitMethodModule {
        owner: StdlibTraitMethodOwner::Trait(TraitId::Hash),
        module_segments: STDLIB_DERIVES_COMPARISON_SEGMENTS,
    },
];

/// Registry of top-level stdlib namespaces.
///
/// Submodule stub paths are derived by convention, so this list stays compact even as the stdlib grows. Adding a new
/// submodule (e.g. `std.async.broadcast`) only requires adding the `.incn` file and appending the name to the parent
/// namespace's `submodules` array.
pub const STDLIB_NAMESPACES: &[StdlibNamespace] = &[
    StdlibNamespace {
        name: "web",
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
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "testing",
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "async",
        feature: Some("async"),
        extra_crate_deps: &[],
        submodules: &["time", "task", "channel", "select", "sync", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "serde",
        feature: Some("json"),
        extra_crate_deps: &[],
        submodules: &["json"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "reflection",
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "derives",
        feature: None,
        extra_crate_deps: &[],
        submodules: &["string", "comparison", "copying", "collection"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "traits",
        feature: None,
        extra_crate_deps: &[],
        submodules: &["convert", "ops", "error", "indexing", "callable", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "math",
        feature: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "libm",
            source: StdlibExtraCrateSource::Version("0.2"),
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "fs",
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "rust",
        feature: None,
        extra_crate_deps: &[],
        submodules: &[],
        // Capability bounds (Send, Sync, Static, Fn, FnMut, FnOnce) are native Rust traits already in scope in all
        // generated code. They have no .incn stub and no emitted Rust module.
        typechecker_only: true,
    },
];

/// Look up a top-level stdlib namespace by name.
pub fn find_namespace(name: &str) -> Option<&'static StdlibNamespace> {
    STDLIB_NAMESPACES.iter().find(|ns| ns.name == name)
}

/// Return the stdlib module path that owns fallback method signatures for a builtin trait name.
///
/// The returned segments can be passed to the typechecker's stdlib cache to load the full `.incn` trait declaration
/// when an imported symbol-table stub has no methods. Unknown traits and builtin traits outside the registered
/// fallback surface return `None`.
#[must_use]
pub fn trait_method_module_segments(trait_name: &str) -> Option<Vec<String>> {
    STDLIB_TRAIT_METHOD_MODULES
        .iter()
        .find(|entry| entry.owner.canonical_name() == trait_name)
        .map(|entry| {
            entry
                .module_segments
                .iter()
                .map(|segment| (*segment).to_string())
                .collect()
        })
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

/// Returns `true` when a stdlib module path refers to a typechecker-only namespace.
///
/// Typechecker-only namespaces (`std.rust`) are handled entirely by the frontend: their items are resolved
/// symbolically at import time and have no corresponding `.incn` stub file or emitted Rust module. The dependency
/// resolver and emission layer must skip them.
#[must_use]
pub fn is_typechecker_only_stdlib(path: &[String]) -> bool {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return false;
    }
    find_namespace(&path[1]).is_some_and(|ns| ns.typechecker_only)
}

/// Resolve the relative stub `.incn` file path for a stdlib module path.
///
/// Uses convention-based resolution:
/// - `std.X` (leaf, no submodules) → `stdlib/X.incn`
/// - `std.X` (namespace with submodules) → `stdlib/X/prelude.incn`
/// - `std.X.Y` (and deeper) → `stdlib/X/Y.incn`
///
/// Returns `None` for typechecker-only namespaces (e.g. `std.rust`) which have no stub file.
pub fn stdlib_stub_path(path: &[String]) -> Option<String> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    let ns = find_namespace(&path[1])?;
    if ns.typechecker_only {
        return None;
    }
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
        assert!(is_known_stdlib_module(&segs(&["std", "fs"])));
        assert!(is_known_stdlib_module(&segs(&["std", "rust"])));
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
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "fs"])),
            Some("stdlib/fs.incn".to_string())
        );
    }

    #[test]
    fn typechecker_only_namespace_std_rust_has_no_stub_path() {
        assert_eq!(stdlib_stub_path(&segs(&["std", "rust"])), None);
        assert!(is_typechecker_only_stdlib(&segs(&["std", "rust"])));
        assert!(!is_typechecker_only_stdlib(&segs(&["std", "testing"])));
        assert!(!is_typechecker_only_stdlib(&segs(&["std", "async"])));
        assert!(!is_typechecker_only_stdlib(&segs(&["not", "stdlib"]))); // non-stdlib path
    }

    #[test]
    fn known_modules_hint_is_registry_driven_and_sorted() {
        let hint = known_stdlib_modules_for_hint();
        assert!(hint.windows(2).all(|w| w[0] <= w[1]));
        assert!(hint.contains(&"std.derives".to_string()));
        assert!(hint.contains(&"std.fs".to_string()));
        assert!(hint.contains(&"std.web.app".to_string()));
        assert!(hint.contains(&"std.async.prelude".to_string()));
        assert!(hint.contains(&"std.rust".to_string()));
    }

    #[test]
    fn trait_method_module_lookup_preserves_derive_fallback_surface() {
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Clone)),
            Some(segs(&["std", "derives", "copying"]))
        );
        assert_eq!(
            trait_method_module_segments(derives::as_str(DeriveId::Copy)),
            Some(segs(&["std", "derives", "copying"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Default)),
            Some(segs(&["std", "derives", "copying"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Debug)),
            Some(segs(&["std", "derives", "string"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Display)),
            Some(segs(&["std", "derives", "string"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Eq)),
            Some(segs(&["std", "derives", "comparison"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::PartialEq)),
            Some(segs(&["std", "derives", "comparison"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Ord)),
            Some(segs(&["std", "derives", "comparison"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::PartialOrd)),
            Some(segs(&["std", "derives", "comparison"]))
        );
        assert_eq!(
            trait_method_module_segments(traits::as_str(TraitId::Hash)),
            Some(segs(&["std", "derives", "comparison"]))
        );
    }

    #[test]
    fn trait_method_module_lookup_leaves_other_builtin_traits_unmapped() {
        assert_eq!(trait_method_module_segments(traits::as_str(TraitId::From)), None);
        assert_eq!(trait_method_module_segments(traits::as_str(TraitId::Into)), None);
        assert_eq!(trait_method_module_segments(derives::as_str(DeriveId::Serialize)), None);
    }

    #[test]
    fn stdlib_registry_keeps_phase_023_metadata() {
        let async_ns = find_namespace("async");
        let reflection_ns = find_namespace("reflection");
        let fs_ns = find_namespace("fs");
        let traits_ns = find_namespace("traits");
        let math_ns = find_namespace("math");

        assert_eq!(async_ns.and_then(|ns| ns.feature), Some("async"));
        assert_eq!(reflection_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(fs_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(fs_ns.and_then(|ns| ns.feature), None);
        assert_eq!(traits_ns.map(|ns| ns.submodules.contains(&"prelude")), Some(true));
        assert_eq!(
            math_ns
                .and_then(|ns| ns.extra_crate_deps.first())
                .map(|dep| dep.crate_name),
            Some("libm")
        );
    }
}
