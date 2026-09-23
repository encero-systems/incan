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

/// `std.web.routing.route`, the decorator that registers a function as an HTTP route handler.
///
/// The checker recognises the decorator by this name together with its backing crate
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
/// Dynamic JSON value type exported by `std.json`.
pub const JSON_VALUE_TYPE_NAME: &str = "JsonValue";
/// Runtime Rust path carried by `std.json.JsonValue`.
pub const JSON_VALUE_RUST_PATH: &str = "incan_std_data::json::JsonValue";

/// The standard library's Rust facets: the runtime crates generated code links.
///
/// Each facet is the `rust/` crate of one component in `sdk-components.toml`, named `incan_std_<component>` after the
/// component's `stdlib-<component>` id. The mandatory `core` facet serves the language itself — reflection, frozen
/// constants, numerics, strings, collection helpers, the version check — and every generated program links it; the
/// others serve the namespaces their component owns, and a generated program links one only when it reaches such a
/// namespace. Cargo features on a single runtime crate used to play this role; a facet is a crate, so the compiler
/// records crates, not flags.
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

    /// The facet that serves a runtime requirement named by a vocab manifest's `required_stdlib_features`.
    ///
    /// The vocab contract predates the facets and spells requirements as the runtime crate's old feature names;
    /// those names stay the contract, and this is where they resolve. A facet name is accepted as its own
    /// requirement so a manifest written after the split needs no translation.
    pub fn for_requirement(requirement: &str) -> Option<&'static str> {
        match requirement {
            "json" | "ordinal" => Some(DATA),
            "async" => Some(ASYNC),
            "web" => Some(WEB),
            "testing" => Some(TESTING),
            facet => ALL.iter().copied().find(|known| *known == facet),
        }
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
    /// The Rust facet this namespace's generated code links beyond the mandatory [`facets::CORE`], or `None` when
    /// the core facet serves it.
    pub facet: Option<&'static str>,
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
    /// Cargo features enabled for this stdlib-managed dependency.
    pub features: &'static [&'static str],
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
/// submodule (e.g. `std.async.broadcast`) requires adding the `.incn` file and appending the name to the parent
/// namespace's `submodules` array.
pub const STDLIB_NAMESPACES: &[StdlibNamespace] = &[
    StdlibNamespace {
        name: "web",
        facet: Some(facets::WEB),
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: STDLIB_WEB_MACROS_CRATE,
                source: StdlibExtraCrateSource::Path("crates/incan_web_macros"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "inventory",
                source: StdlibExtraCrateSource::Version("0.3"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "axum",
                source: StdlibExtraCrateSource::Version("0.8"),
                features: &[],
            },
        ],
        submodules: &["app", "routing", "request", "response", "macros", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "testing",
        facet: Some(facets::TESTING),
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "logging",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "registry",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "telemetry",
        facet: None,
        extra_crate_deps: &[],
        submodules: &["core"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "environ",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "async",
        facet: Some(facets::ASYNC),
        extra_crate_deps: &[],
        submodules: &["time", "task", "channel", "race", "sync", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: STDLIB_INTEROP,
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "serde",
        facet: Some(facets::DATA),
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "serde",
            source: StdlibExtraCrateSource::Version("1.0"),
            features: &["derive"],
        }],
        submodules: &["json"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: STDLIB_JSON,
        facet: Some(facets::DATA),
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "serde",
            source: StdlibExtraCrateSource::Version("1.0"),
            features: &["derive"],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "toml",
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "toml_edit",
                source: StdlibExtraCrateSource::Version("0.25"),
                features: &["parse", "serde"],
            },
            StdlibExtraCrateDep {
                crate_name: "serde_path_to_error",
                source: StdlibExtraCrateSource::Version("0.1"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "toml",
                source: StdlibExtraCrateSource::Version("0.9"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "serde",
                source: StdlibExtraCrateSource::Version("1.0"),
                features: &["derive"],
            },
        ],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "reflection",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "result",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "derives",
        facet: None,
        extra_crate_deps: &[],
        submodules: &["string", "comparison", "copying", "collection"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "traits",
        facet: None,
        extra_crate_deps: &[],
        submodules: &["convert", "ops", "error", "indexing", "callable", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "math",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "libm",
            source: StdlibExtraCrateSource::Version("0.2"),
            features: &[],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "fs",
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "encoding_rs",
                source: StdlibExtraCrateSource::Version("0.8"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "rustix",
                source: StdlibExtraCrateSource::Version("1.1"),
                features: &["fs"],
            },
        ],
        submodules: &["path", "file", "locking", "metadata", "glob", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "datetime",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[
            "runtime",
            "civil",
            "civil.intervals",
            "civil.naive",
            "civil.offset",
            "error",
            "prelude",
        ],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "runtime",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[
            "prelude",
            "host",
            "host.clock",
            "host.env",
            "host.fs",
            "host.http",
            "host.model",
            "host.process",
            "host.tool",
        ],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "graph",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "uuid",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "rand",
            source: StdlibExtraCrateSource::Version("0.8"),
            features: &[],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "regex",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "regex",
            source: StdlibExtraCrateSource::Version("1.0"),
            features: &[],
        }],
        submodules: &["_core", "_replacement", "types", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "collections",
        // The source-level ordinal map implementation calls the xxh3 key helpers in
        // `incan_std_data::collections::__private`, so every generated program that imports `std.collections` links
        // the data facet; without it, a direct-rustc Oven unit would compile a source closure whose helpers no
        // crate supplies.
        facet: Some(facets::DATA),
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "io",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "byteorder",
            source: StdlibExtraCrateSource::Version("1"),
            features: &[],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "encoding",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[
            "_shared", "prelude", "hex", "base32", "base64", "base85", "base58", "bech32",
        ],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "checksum",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "crc32fast",
            source: StdlibExtraCrateSource::Version("1"),
            features: &[],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "hash",
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "blake2",
                source: StdlibExtraCrateSource::Version("0.10"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "blake3",
                source: StdlibExtraCrateSource::Version("1"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "hmac",
                source: StdlibExtraCrateSource::Version("0.12"),
                features: &["reset"],
            },
            StdlibExtraCrateDep {
                crate_name: "md5",
                source: StdlibExtraCrateSource::Version("0.10"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "sha1",
                source: StdlibExtraCrateSource::Version("0.10"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "sha2",
                source: StdlibExtraCrateSource::Version("0.10"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "sha3",
                source: StdlibExtraCrateSource::Version("0.10"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "xxhash_rust",
                source: StdlibExtraCrateSource::Version("0.8"),
                features: &["xxh3", "xxh32", "xxh64"],
            },
        ],
        submodules: &["_core", "_hmac", "_streaming", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "compression",
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "flate2",
                source: StdlibExtraCrateSource::Version("1"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "zstd",
                source: StdlibExtraCrateSource::Version("0.13"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "bzip2",
                source: StdlibExtraCrateSource::Version("0.6"),
                features: &[],
            },
            StdlibExtraCrateDep {
                crate_name: "xz2",
                source: StdlibExtraCrateSource::Version("0.1"),
                features: &["static"],
            },
            StdlibExtraCrateDep {
                crate_name: "snap",
                source: StdlibExtraCrateSource::Version("1"),
                features: &[],
            },
        ],
        submodules: &[
            "_core",
            "_auto",
            "gzip",
            "zlib",
            "deflate",
            "zstd",
            "bz2",
            "lzma",
            "snappy",
            "snappy.raw",
        ],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "tempfile",
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "tempfile",
            source: StdlibExtraCrateSource::Version("3"),
            features: &[],
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "rust",
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        // Capability bounds (Send, Sync, Static, Fn, FnMut, FnOnce) are native Rust traits already in scope in all
        // generated code. They have no .incn stub and no emitted Rust module.
        typechecker_only: true,
    },
    StdlibNamespace {
        name: STDLIB_BUILTINS,
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        // `std.builtins.<name>(...)` is an explicit call escape to the compiler's builtin-function registry. Builtin
        // types deliberately stay at the root surface and there is no source stub or emitted Rust module.
        typechecker_only: true,
    },
];

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
    keywords::soft_keywords_for_namespace(&path[1])
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
        assert!(is_known_stdlib_module(&segs(&["std", "json"])));
        assert!(is_known_stdlib_module(&segs(&["std", "serde", "json"])));
        assert!(is_known_stdlib_module(&segs(&["std", "reflection"])));
        assert!(is_known_stdlib_module(&segs(&["std", "result"])));
        assert!(is_known_stdlib_module(&segs(&["std", "environ"])));
        assert!(is_known_stdlib_module(&segs(&["std", "fs"])));
        assert!(is_known_stdlib_module(&segs(&["std", "datetime"])));
        assert!(is_known_stdlib_module(&segs(&["std", "datetime", "runtime"])));
        assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil"])));
        assert!(is_known_stdlib_module(&segs(&[
            "std",
            "datetime",
            "civil",
            "intervals"
        ])));
        assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil", "naive"])));
        assert!(is_known_stdlib_module(&segs(&["std", "datetime", "civil", "offset"])));
        assert!(is_known_stdlib_module(&segs(&["std", "graph"])));
        assert!(is_known_stdlib_module(&segs(&["std", "uuid"])));
        assert!(is_known_stdlib_module(&segs(&["std", "regex"])));
        assert!(is_known_stdlib_module(&segs(&["std", "regex", "_core"])));
        assert!(is_known_stdlib_module(&segs(&["std", "regex", "_replacement"])));
        assert!(is_known_stdlib_module(&segs(&["std", "regex", "types"])));
        assert!(is_known_stdlib_module(&segs(&["std", "io"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "_shared"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "hex"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base32"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base64"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base85"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "base58"])));
        assert!(is_known_stdlib_module(&segs(&["std", "encoding", "bech32"])));
        assert!(is_known_stdlib_module(&segs(&["std", "checksum"])));
        assert!(is_known_stdlib_module(&segs(&["std", "hash"])));
        assert!(is_known_stdlib_module(&segs(&["std", "compression"])));
        assert!(is_known_stdlib_module(&segs(&["std", "compression", "_core"])));
        assert!(is_known_stdlib_module(&segs(&["std", "compression", "_auto"])));
        assert!(is_known_stdlib_module(&segs(&["std", "compression", "gzip"])));
        assert!(is_known_stdlib_module(&segs(&["std", "compression", "snappy", "raw"])));
        assert!(is_known_stdlib_module(&segs(&["std", "tempfile"])));
        assert!(is_known_stdlib_module(&segs(&["std", "collections"])));
        assert!(is_known_stdlib_module(&segs(&["std", "rust"])));
        assert!(is_known_stdlib_module(&segs(&["std", "builtins"])));
    }

    #[test]
    fn graph_constructor_types_are_registry_owned() {
        assert!(is_graph_constructor_type("DiGraph"));
        assert!(is_graph_constructor_type("Dag"));
        assert!(is_graph_constructor_type("MultiDiGraph"));
        assert!(!is_graph_constructor_type("NodeId"));
        assert!(!is_graph_constructor_type("EdgeId"));
    }

    #[test]
    fn unknown_stdlib_modules_are_rejected() {
        assert!(!is_known_stdlib_module(&segs(&["std", "f64", "consts"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "web", "missing"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "math", "extra"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "collections", "deque"])));
        assert!(!is_known_stdlib_module(&segs(&["std", "datetime", "civil", "missing"])));
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
            Some("stdlib/fs/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "fs", "path"])),
            Some("stdlib/fs/path.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "graph"])),
            Some("stdlib/graph.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "uuid"])),
            Some("stdlib/uuid.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "regex"])),
            Some("stdlib/regex/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "regex", "_core"])),
            Some("stdlib/regex/_core.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "datetime"])),
            Some("stdlib/datetime/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "datetime", "runtime"])),
            Some("stdlib/datetime/runtime.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "datetime", "civil", "naive"])),
            Some("stdlib/datetime/civil/naive.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "io"])),
            Some("stdlib/io.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "environ"])),
            Some("stdlib/environ.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "encoding"])),
            Some("stdlib/encoding/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "encoding", "base64"])),
            Some("stdlib/encoding/base64.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "hash"])),
            Some("stdlib/hash/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "checksum"])),
            Some("stdlib/checksum.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "compression"])),
            Some("stdlib/compression/prelude.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "compression", "_core"])),
            Some("stdlib/compression/_core.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "compression", "_auto"])),
            Some("stdlib/compression/_auto.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "compression", "gzip"])),
            Some("stdlib/compression/gzip.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "compression", "snappy", "raw"])),
            Some("stdlib/compression/snappy/raw.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "tempfile"])),
            Some("stdlib/tempfile.incn".to_string())
        );
        assert_eq!(
            stdlib_stub_path(&segs(&["std", "collections"])),
            Some("stdlib/collections.incn".to_string())
        );
    }

    #[test]
    fn typechecker_only_namespace_std_rust_has_no_stub_path() {
        assert_eq!(stdlib_stub_path(&segs(&["std", "rust"])), None);
        assert_eq!(stdlib_stub_path(&segs(&["std", "builtins"])), None);
        assert!(is_typechecker_only_stdlib(&segs(&["std", "rust"])));
        assert!(is_typechecker_only_stdlib(&segs(&["std", "builtins"])));
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
        assert!(hint.contains(&"std.graph".to_string()));
        assert!(hint.contains(&"std.io".to_string()));
        assert!(hint.contains(&"std.uuid".to_string()));
        assert!(hint.contains(&"std.hash".to_string()));
        assert!(hint.contains(&"std.tempfile".to_string()));
        assert!(hint.contains(&"std.rust".to_string()));
        assert!(hint.contains(&"std.web.app".to_string()));
        assert!(hint.contains(&"std.async.prelude".to_string()));
        assert!(hint.contains(&"std.datetime.civil.naive".to_string()));
        assert!(hint.contains(&"std.builtins".to_string()));
        assert!(hint.contains(&"std.collections".to_string()));
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
        assert_eq!(trait_method_module_segments("Serialize"), None);
    }

    #[test]
    fn stdlib_json_trait_lookup_covers_source_and_qualified_names_only() {
        for name in ["Serialize", "json.Serialize", "std.serde.json.Serialize"] {
            assert_eq!(stdlib_json_trait_id(name), Some(StdlibJsonTraitId::Serialize));
        }

        for name in ["Deserialize", "json.Deserialize", "std.serde.json.Deserialize"] {
            assert_eq!(stdlib_json_trait_id(name), Some(StdlibJsonTraitId::Deserialize));
        }

        // An import alias is a binding, resolved by identity rather than by spelling (#1712).
        assert_eq!(stdlib_json_trait_id("JsonSerialize"), None);
        assert_eq!(stdlib_json_trait_id("JsonDeserialize"), None);
        let json_module = ["std", "serde", "json"].map(String::from);
        assert_eq!(
            stdlib_json_trait_id_for_identity(&json_module, "Serialize"),
            Some(StdlibJsonTraitId::Serialize)
        );
        assert_eq!(stdlib_json_trait_id_for_identity(&json_module, "JsonSerialize"), None);
        assert_eq!(stdlib_json_trait_id("yaml.Serialize"), None);
        assert_eq!(stdlib_json_trait_scope_import_id("Serialize"), None);
        assert_eq!(stdlib_json_trait_scope_import_id("JsonSerialize"), None);
        assert_eq!(
            stdlib_json_trait_scope_import_id("json.Serialize"),
            Some(StdlibJsonTraitId::Serialize)
        );
        let json_trait_module = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
        assert!(is_stdlib_json_trait_module_path(&json_trait_module));
        assert_eq!(
            stdlib_json_trait_id_for_identity(&json_trait_module, "Serialize"),
            Some(StdlibJsonTraitId::Serialize)
        );
        assert_eq!(
            stdlib_json_trait_id_for_identity(&json_trait_module, "Deserialize"),
            Some(StdlibJsonTraitId::Deserialize)
        );
        assert_eq!(
            stdlib_json_trait_id_for_identity(&json_trait_module, "JsonSerialize"),
            None
        );
        assert_eq!(stdlib_json_trait_id_for_identity(&segs(&["main"]), "Serialize"), None);
        assert_eq!(
            stdlib_json_trait_id_for_identity(&segs(&["std", "toml"]), "Serialize"),
            None
        );
        let serialize_path = vec![
            "std".to_string(),
            "serde".to_string(),
            "json".to_string(),
            "Serialize".to_string(),
        ];
        assert_eq!(
            stdlib_json_trait_id_from_path(&serialize_path),
            Some(StdlibJsonTraitId::Serialize)
        );
    }

    #[test]
    fn extra_crate_dependency_lookup_is_registry_driven() {
        let axum = find_extra_crate_dep("axum");
        assert_eq!(axum.map(|dep| dep.crate_name), Some("axum"));
        assert_eq!(axum.map(|dep| dep.source), Some(StdlibExtraCrateSource::Version("0.8")));

        let macros = find_extra_crate_dep("incan_web_macros");
        assert_eq!(
            macros.map(|dep| dep.source),
            Some(StdlibExtraCrateSource::Path("crates/incan_web_macros"))
        );
        assert!(is_path_extra_crate_dep("incan_web_macros"));
        assert!(!is_path_extra_crate_dep("axum"));

        assert!(find_extra_crate_dep("not_a_stdlib_dependency").is_none());
    }

    #[test]
    fn stdlib_registry_keeps_phase_023_metadata() {
        let async_ns = find_namespace("async");
        let reflection_ns = find_namespace("reflection");
        let fs_ns = find_namespace("fs");
        let tempfile_ns = find_namespace("tempfile");
        let traits_ns = find_namespace("traits");
        let math_ns = find_namespace("math");
        let graph_ns = find_namespace("graph");
        let uuid_ns = find_namespace("uuid");
        let serde_ns = find_namespace("serde");
        let json_ns = find_namespace(STDLIB_JSON);
        let hash_ns = find_namespace("hash");
        let checksum_ns = find_namespace("checksum");
        let datetime_ns = find_namespace("datetime");
        let collections_ns = find_namespace("collections");
        let compression_ns = find_namespace("compression");

        assert_eq!(async_ns.and_then(|ns| ns.facet), Some(facets::ASYNC));
        // The facet's layout decides the module path: async is its own crate root, json is a module of data,
        // strings a module of core.
        assert!(facets::namespace_is_facet_root("async"));
        assert!(facets::namespace_is_facet_root("testing"));
        assert!(!facets::namespace_is_facet_root("json"));
        assert!(!facets::namespace_is_facet_root("collections"));
        assert_eq!(facets::for_requirement("ordinal"), Some(facets::DATA));
        assert_eq!(facets::for_component("stdlib-web"), Some(facets::WEB));
        assert_eq!(facets::for_component("stdlib-system"), None);
        assert!(facets::path_names_a_facet("incan_std_core::strings::str_len"));
        assert!(!facets::path_names_a_facet("incan_stdlib_core::anything"));
        assert_eq!(reflection_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(fs_ns.map(|ns| ns.submodules.contains(&"path")), Some(true));
        assert_eq!(fs_ns.and_then(|ns| ns.facet), None);
        assert_eq!(
            fs_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["encoding_rs", "rustix"])
        );
        assert_eq!(tempfile_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(tempfile_ns.and_then(|ns| ns.facet), None);
        assert_eq!(
            tempfile_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["tempfile"])
        );
        assert_eq!(traits_ns.map(|ns| ns.submodules.contains(&"prelude")), Some(true));
        assert_eq!(
            math_ns
                .and_then(|ns| ns.extra_crate_deps.first())
                .map(|dep| dep.crate_name),
            Some("libm")
        );
        assert_eq!(graph_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(graph_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(uuid_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(
            uuid_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["rand"])
        );
        assert_eq!(uuid_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(uuid_ns.map(|ns| ns.typechecker_only), Some(false));
        assert_eq!(
            serde_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["serde"])
        );
        assert_eq!(
            serde_ns
                .and_then(|ns| ns.extra_crate_deps.first())
                .map(|dep| dep.features),
            Some(&["derive"][..])
        );
        assert_eq!(
            json_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["serde"])
        );
        assert_eq!(collections_ns.and_then(|ns| ns.facet), Some(facets::DATA));
        assert_eq!(collections_ns.map(|ns| ns.extra_crate_deps.is_empty()), Some(true));
        assert_eq!(collections_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(collections_ns.map(|ns| ns.typechecker_only), Some(false));
        assert_eq!(
            find_namespace("io")
                .and_then(|ns| ns.extra_crate_deps.first())
                .map(|dep| dep.crate_name),
            Some("byteorder")
        );
        assert_eq!(hash_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(
            hash_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec![
                "blake2",
                "blake3",
                "hmac",
                "md5",
                "sha1",
                "sha2",
                "sha3",
                "xxhash_rust",
            ])
        );
        assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"prelude")), Some(true));
        assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"_core")), Some(true));
        assert_eq!(hash_ns.map(|ns| ns.submodules.contains(&"_streaming")), Some(true));
        assert_eq!(hash_ns.map(|ns| ns.typechecker_only), Some(false));
        assert_eq!(checksum_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(
            checksum_ns.map(|ns| ns.extra_crate_deps.iter().map(|dep| dep.crate_name).collect::<Vec<_>>()),
            Some(vec!["crc32fast"])
        );
        assert_eq!(checksum_ns.map(|ns| ns.submodules.is_empty()), Some(true));
        assert_eq!(checksum_ns.map(|ns| ns.typechecker_only), Some(false));
        assert_eq!(compression_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"_core")), Some(true));
        assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"_auto")), Some(true));
        assert_eq!(compression_ns.map(|ns| ns.submodules.contains(&"gzip")), Some(true));
        assert_eq!(
            compression_ns.map(|ns| ns.submodules.contains(&"snappy.raw")),
            Some(true)
        );
        assert_eq!(
            compression_ns
                .and_then(|ns| ns.extra_crate_deps.first())
                .map(|dep| dep.crate_name),
            Some("flate2")
        );
        assert_eq!(datetime_ns.map(|ns| ns.facet), Some(None));
        assert_eq!(datetime_ns.map(|ns| ns.extra_crate_deps.is_empty()), Some(true));
        assert_eq!(datetime_ns.map(|ns| ns.submodules.contains(&"civil.naive")), Some(true));
    }
}
