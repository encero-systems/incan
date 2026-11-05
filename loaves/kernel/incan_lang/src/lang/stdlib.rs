//! Canonical stdlib module names.
use super::{
    derives::{self, DeriveId},
    keywords,
    traits::{self, TraitId},
};

mod tables;

pub use tables::*;

/// Root stdlib namespace (e.g. `import std::...`).
pub const STDLIB_ROOT: &str = "std";

/// The file stem of a namespace's own module (`std.async` is `async/prelude.incn`).
pub const STDLIB_PRELUDE_SEGMENT: &str = "prelude";

/// RFC 023: Rust module namespace for compiled `std.*` modules.
///
/// Compiled stdlib `.incn` files are emitted as submodules under `crate::__incan_std::*` to avoid shadowing Rust's
/// `std` crate. For example, `std.testing` compiles to `crate::__incan_std::testing`.
pub const INCAN_STD_NAMESPACE: &str = "__incan_std";

/// `std.web` module name.
pub const STDLIB_WEB: &str = "web";

/// `std.web.routing.route`, the decorator that registers a function as an HTTP route handler.
///
/// The checker recognizes the decorator by this name together with its backing crate
/// ([`STDLIB_WEB_MACROS_CRATE`]) rather than by import spelling, so `@route`, `@web.route` and
/// `@std.web.routing.route` are one decorator once the import resolves.
pub const STDLIB_WEB_ROUTE_DECORATOR: &str = "route";

/// The proc-macro crate that backs `std.web.routing.route`.
///
/// A `@rust.extern` stdlib function bound to this crate is emitted as a passthrough Rust attribute; the checker uses
/// the same binding to know which decorated functions are route handlers and to check their signatures.
pub const STDLIB_WEB_MACROS_CRATE: &str = "incan_web_macros";

/// `std.web.macros.IntoResponse`, the derive that makes a wrapper type usable as a route handler's return type.
pub const STDLIB_WEB_INTO_RESPONSE_TRAIT: &str = "IntoResponse";

/// `std.web.macros.FromRequestParts`, the derive that makes a wrapper type usable as a route handler parameter.
pub const STDLIB_WEB_FROM_REQUEST_PARTS_TRAIT: &str = "FromRequestParts";

/// `std.reflection` module name.
pub const STDLIB_REFLECTION: &str = "reflection";

/// `std::this` module name (`import this`).
pub const STDLIB_THIS: &str = "this";

/// `std.async` module name.
pub const STDLIB_ASYNC: &str = "async";
/// `std.interop` module name for import-activated foreign binding vocabularies.
pub const STDLIB_INTEROP: &str = "interop";
/// `std.graph` module name.
pub const STDLIB_GRAPH: &str = "graph";
/// `std.rust` module name for capability bounds (RFC 041).
pub const STDLIB_RUST: &str = "rust";
/// Runtime error helper module under the compiled stdlib Rust namespace.
pub const INCAN_STD_ERRORS_MODULE: &str = "errors";
/// Rust stdlib error helpers known to diverge.
pub const DIVERGING_RUST_ERROR_HELPERS: &[&str] = &[
    "raise_value_error",
    "raise_type_error",
    "raise_index_error",
    "raise_key_error",
    "raise_json_serialization_error",
    "raise_json_decode_error",
];
/// `std.builtins` module name for explicit builtin-function escape calls (RFC 045).
pub const STDLIB_BUILTINS: &str = "builtins";
/// `std.json` module name.
pub const STDLIB_JSON: &str = "json";
/// `std.result` module name.
pub const STDLIB_RESULT: &str = "result";
/// `std.serde` module name.
pub const STDLIB_SERDE: &str = "serde";
/// The standard-library namespace declaring `Registry` and `RegistryEntry` (RFC 113).
pub const STDLIB_REGISTRY: &str = "registry";
/// Dynamic JSON value type exported by `std.json`.
pub const JSON_VALUE_TYPE_NAME: &str = "JsonValue";
/// Runtime Rust path carried by `std.json.JsonValue`.
pub const JSON_VALUE_RUST_PATH: &str = "incan_std_data::json::JsonValue";

/// The standard library's compiled native artifacts, named by the standard Loaves' published runtime contracts.
/// The mandatory `core` artifact serves reflection, frozen constants, numerics, strings and collection helpers;
/// other artifacts serve their owning namespaces. A unit's selected artifacts are explicit compiler inputs.
pub mod facets {
    /// The mandatory facet every generated program links.
    pub const CORE: &str = "incan_std_core";
    /// `std.json`, `std.serde` and the ordinal-map helpers behind `std.collections`.
    pub const DATA: &str = "incan_std_data";
    /// `std.async`.
    pub const ASYNC: &str = "incan_std_async";
    /// `std.web`.
    pub const WEB: &str = "incan_std_web";
    /// `std.testing`.
    pub const TESTING: &str = "incan_std_testing";
    /// Every facet, the mandatory one first.
    pub const ALL: [&str; 5] = [CORE, DATA, ASYNC, WEB, TESTING];

    /// The facet a namespace's generated code links: the namespace's own facet, or the mandatory core facet.
    pub fn for_namespace(namespace: &str) -> &'static str {
        super::find_namespace(namespace)
            .and_then(|entry| entry.facet)
            .unwrap_or(CORE)
    }

    /// Whether a namespace's runtime module is the crate root of its facet rather than a module inside it.
    ///
    /// The core and data facets keep one module per namespace (`incan_std_core::strings`, `incan_std_data::json`);
    /// the async, web and testing facets serve a single namespace and are that module themselves, so a path into
    /// them starts at the crate (`incan_std_async::task`, never `incan_std_async::r#async::task`).
    pub fn namespace_is_facet_root(namespace: &str) -> bool {
        matches!(for_namespace(namespace), ASYNC | WEB | TESTING)
    }

    /// Whether a `::`-qualified Rust path starts with one of the facets, as `incan_std_core::strings::str_len` does.
    pub fn path_names_a_facet(path: &str) -> bool {
        path.split("::").next().is_some_and(is_facet)
    }

    /// Whether a crate name is one of the standard library's facets.
    ///
    /// Import sinks use this where they used to compare against the single runtime crate: a `from rust::` import of
    /// a facet is toolchain-supplied, never a package dependency the project has to declare.
    pub fn is_facet(crate_name: &str) -> bool {
        ALL.contains(&crate_name)
    }

    /// The facet of the catalog component `stdlib-<name>`, or `None` when that component has no Rust facet.
    pub fn for_component(component_id: &str) -> Option<&'static str> {
        let name = component_id.strip_prefix("stdlib-")?;
        ALL.iter()
            .copied()
            .find(|facet| facet.strip_prefix("incan_std_") == Some(name))
    }
}

/// Stable ids for compiler-known stdlib JSON protocol traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StdlibJsonTraitId {
    Serialize,
    Deserialize,
}

const STDLIB_JSON_SERIALIZE_TRAIT_METHODS: &[&str] = &["to_json"];
const STDLIB_JSON_DESERIALIZE_TRAIT_METHODS: &[&str] = &["from_json"];

const STDLIB_JSON_SERIALIZE_TRAIT_NAMES: &[&str] = &["Serialize", "json.Serialize", "std.serde.json.Serialize"];

const STDLIB_JSON_CANONICAL_TRAIT_NAMES: &[&str] = &["std.serde.json.Serialize", "std.serde.json.Deserialize"];

const STDLIB_JSON_DESERIALIZE_TRAIT_NAMES: &[&str] = &["Deserialize", "json.Deserialize", "std.serde.json.Deserialize"];

/// Return whether `name` is the canonical dynamic JSON value type.
#[must_use]
pub fn is_json_value_type_name(name: &str) -> bool {
    name == JSON_VALUE_TYPE_NAME
}

/// Return whether `name` is a Rust stdlib error helper that always diverges.
#[must_use]
pub fn is_diverging_rust_error_helper_name(name: &str) -> bool {
    DIVERGING_RUST_ERROR_HELPERS.contains(&name)
}

/// Return the stdlib JSON trait id for a source or qualified trait spelling.
///
/// The spellings here are the ones a program can write without an import binding of its own: the declaration name
/// and the module-qualified forms. An import alias (`Serialize as JsonSerialize`) is a binding the frontend proves,
/// so it is resolved by identity through [`stdlib_json_trait_id_for_identity`], never by its spelling (#1712).
#[must_use]
pub fn stdlib_json_trait_id(name: &str) -> Option<StdlibJsonTraitId> {
    if STDLIB_JSON_SERIALIZE_TRAIT_NAMES.contains(&name) {
        Some(StdlibJsonTraitId::Serialize)
    } else if STDLIB_JSON_DESERIALIZE_TRAIT_NAMES.contains(&name) {
        Some(StdlibJsonTraitId::Deserialize)
    } else {
        None
    }
}

/// Return the canonical method set for one compiler-known stdlib JSON protocol.
#[must_use]
pub const fn stdlib_json_trait_method_names(id: StdlibJsonTraitId) -> &'static [&'static str] {
    match id {
        StdlibJsonTraitId::Serialize => STDLIB_JSON_SERIALIZE_TRAIT_METHODS,
        StdlibJsonTraitId::Deserialize => STDLIB_JSON_DESERIALIZE_TRAIT_METHODS,
    }
}

/// Return whether `segments` names the `std.serde.json` trait module.
#[must_use]
pub fn is_stdlib_json_trait_module_path(segments: &[String]) -> bool {
    matches!(
        segments,
        [std, serde, json]
            if std == STDLIB_ROOT && serde == STDLIB_SERDE && json == STDLIB_JSON
    )
}

/// Canonical source declaration names of the `std.serde.json` protocol traits, indexed by their id.
const STDLIB_JSON_TRAIT_SOURCE_NAMES: &[(StdlibJsonTraitId, &str)] = &[
    (StdlibJsonTraitId::Serialize, "Serialize"),
    (StdlibJsonTraitId::Deserialize, "Deserialize"),
];

/// Return the stdlib JSON trait id for a canonical trait identity: the declaring module path plus the source
/// declaration name.
///
/// This is the identity-keyed form of [`stdlib_json_trait_id`]. A trait that merely shares the spelling `Serialize`
/// or `Deserialize` but is declared anywhere other than `std.serde.json` carries no JSON protocol, so lowering and
/// emission must consult this form whenever the trait's declaring module is known.
#[must_use]
pub fn stdlib_json_trait_id_for_identity(module_path: &[String], source_name: &str) -> Option<StdlibJsonTraitId> {
    if !is_stdlib_json_trait_module_path(module_path) {
        return None;
    }
    STDLIB_JSON_TRAIT_SOURCE_NAMES
        .iter()
        .find(|(_, name)| *name == source_name)
        .map(|(id, _)| *id)
}

/// Return the stdlib JSON trait id for a resolved source import path.
#[must_use]
pub fn stdlib_json_trait_id_from_path(segments: &[String]) -> Option<StdlibJsonTraitId> {
    if is_stdlib_json_trait_module_path(segments) {
        return None;
    }
    stdlib_json_trait_id(&segments.join("."))
}

/// Return the stdlib JSON trait id when generated Rust must import the trait module for method resolution.
#[must_use]
pub fn stdlib_json_trait_scope_import_id(name: &str) -> Option<StdlibJsonTraitId> {
    match name {
        "json.Serialize" | "std.serde.json.Serialize" => Some(StdlibJsonTraitId::Serialize),
        "json.Deserialize" | "std.serde.json.Deserialize" => Some(StdlibJsonTraitId::Deserialize),
        _ => None,
    }
}

/// Return whether `name` is a fully qualified source path for a stdlib JSON protocol trait.
#[must_use]
pub fn is_canonical_stdlib_json_trait_name(name: &str) -> bool {
    STDLIB_JSON_CANONICAL_TRAIT_NAMES.contains(&name)
}

const STDLIB_GRAPH_CONSTRUCTOR_TYPES: &[&str] = &["DiGraph", "Dag", "MultiDiGraph"];

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

/// Return whether a compiler-generated support call crosses a compiled SDK-provider boundary.
///
/// These names remain underscored in Incan because they are not general user API. They are nevertheless public Rust
/// symbols: generated trait defaults in a consumer must be able to call them when their source contract is owned by
/// the compiled artifact.
#[must_use]
pub fn is_compiled_sdk_provider_support_function(path: &[String], name: &str) -> bool {
    name == "_ordinal_hash"
        && (path.iter().map(String::as_str).eq([STDLIB_ROOT, "collections"])
            || path.iter().map(String::as_str).eq([INCAN_STD_NAMESPACE, "collections"]))
}

/// Return whether `name` is an RFC 047 graph type with direct constructor syntax.
#[must_use]
pub fn is_graph_constructor_type(name: &str) -> bool {
    STDLIB_GRAPH_CONSTRUCTOR_TYPES.contains(&name)
}

/// A top-level stdlib namespace with optional metadata.
///
/// Top-level namespaces (`std.<name>`) are registered explicitly, and their public child paths are listed as
/// submodules. Nested child paths use dotted names such as `"civil.naive"`, while stub file paths still derive from
/// segmented import paths under the active stdlib source root.
#[derive(Debug, Clone, Copy)]
pub struct StdlibNamespace {
    /// Top-level namespace name (e.g., `"web"`, `"testing"`, `"async"`).
    pub name: &'static str,
    /// Ordinary standard Loaf permitted to publish this root; compiler-only namespaces have no issuer.
    pub package: Option<&'static str>,
    /// The Rust facet this namespace's generated code links beyond the mandatory [`facets::CORE`], or `None` when
    /// the core facet serves it.
    pub facet: Option<&'static str>,
    /// Rust extern ownership names used by this namespace; component Loaf manifests own registry requirements and
    /// features.
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
    /// Namespace ownership of a declared registry Loaf or a local toolchain support path.
    pub source: StdlibExtraCrateSource,
}

/// Source descriptor for a namespace-provided extra crate dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibExtraCrateSource {
    /// Path dependency rooted at compiler workspace root.
    Path(&'static str),
    /// Registry Loaf whose requirement and features belong to its component manifest.
    Declared,
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

/// Look up a top-level stdlib namespace by name.
pub fn find_namespace(name: &str) -> Option<&'static StdlibNamespace> {
    STDLIB_NAMESPACES.iter().find(|ns| ns.name == name)
}

/// Look up an extra Cargo crate dependency declared by any registered stdlib namespace.
///
/// This is the registry boundary for compiler subsystems that need stdlib-managed dependency metadata without
/// duplicating namespace traversal or crate version knowledge.
#[must_use]
pub fn find_extra_crate_dep(crate_name: &str) -> Option<&'static StdlibExtraCrateDep> {
    extra_crate_deps().find(|dep| dep.crate_name == crate_name)
}

/// Return whether a crate is supplied by the workspace as a stdlib-managed path dependency.
#[must_use]
pub fn is_path_extra_crate_dep(crate_name: &str) -> bool {
    find_extra_crate_dep(crate_name).is_some_and(|dep| matches!(dep.source, StdlibExtraCrateSource::Path(_)))
}

/// Return the published Cargo package name when a stdlib-managed Rust crate imports under a different crate key.
#[must_use]
pub fn extra_crate_package_alias(crate_name: &str) -> Option<&'static str> {
    match crate_name {
        "md5" => Some("md-5"),
        "xxhash_rust" => Some("xxhash-rust"),
        _ => None,
    }
}

/// Iterate over every extra Cargo crate dependency declared by registered stdlib namespaces.
///
/// Consumers that need to filter by dependency source can use this iterator while keeping namespace traversal
/// centralized in the stdlib registry.
pub fn extra_crate_deps() -> impl Iterator<Item = &'static StdlibExtraCrateDep> {
    STDLIB_NAMESPACES.iter().flat_map(|ns| ns.extra_crate_deps)
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
/// `path` is expected in canonical segmented form (e.g. `["std", "async", "time"]`). Returns an empty vector for
/// non-stdlib paths or namespaces without soft keywords.
pub fn soft_keywords_for_import(path: &[String]) -> Vec<keywords::KeywordId> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return Vec::new();
    }
    keywords::soft_keywords_for_namespace(&path[1..].join("."))
}

/// Check if a module path matches a known Incan stdlib module.
///
/// Unlike [`is_any_stdlib_path`] which accepts anything starting with `"std"`, this validates that the second segment
/// is a registered namespace and deeper paths match a known dotted submodule entry.
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
    let submodule = path[2..].join(".");
    ns.submodules.contains(&submodule.as_str())
}

/// Return the module a `std.<namespace>...prelude` path names, or `None` for any other path.
///
/// A namespace's `prelude` source file is the namespace's own module, the way a directory's `mod` file is, so
/// `std.async.prelude` and `std.async` name one module: the checker resolves the import through the namespace and a
/// provider claims it once, as `std.async`. The root-level `std.prelude` is a module of its own and is left alone.
pub fn stdlib_prelude_module_namespace(path: &[String]) -> Option<&[String]> {
    match path {
        [root, .., last] if path.len() >= 3 && root == STDLIB_ROOT && last == STDLIB_PRELUDE_SEGMENT => {
            Some(&path[..path.len() - 1])
        }
        _ => None,
    }
}

/// Human-friendly list of known stdlib modules for diagnostics.
///
/// Includes top-level namespaces and registered submodules.
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

/// The Rust facet a stdlib module path links beyond the mandatory core facet, or `None` when core serves it.
pub fn stdlib_facet_for(path: &[String]) -> Option<&'static str> {
    if path.len() < 2 || path[0] != STDLIB_ROOT {
        return None;
    }
    find_namespace(&path[1]).and_then(|ns| ns.facet)
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

/// Resolve a stdlib module's historical monolithic stub-layout path.
///
/// These are logical compatibility paths, not current repository locations. Source packages now live under
/// `loaves/stdlib/`; their manifests and namespace policy select the source files.
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
mod tests;
