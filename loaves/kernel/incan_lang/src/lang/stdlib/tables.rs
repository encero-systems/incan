//! The standard trait method modules and the standard library namespace table.

use super::*;

pub(super) const STDLIB_DERIVES_COPYING_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "copying"];

pub(super) const STDLIB_DERIVES_STRING_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "string"];

pub(super) const STDLIB_DERIVES_COMPARISON_SEGMENTS: &[&str] = &[STDLIB_ROOT, "derives", "comparison"];

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
/// namespace's `submodules` array. Package ownership is declared here once and projected by `standard_packages`;
/// manifests declare dependency intent, not authority to publish reserved namespaces.
pub const STDLIB_NAMESPACES: &[StdlibNamespace] = &[
    StdlibNamespace {
        name: "web",
        package: Some("incan_stdlib_web"),
        facet: Some(facets::WEB),
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: STDLIB_WEB_MACROS_CRATE,
                source: StdlibExtraCrateSource::Path("loaves/stdlib/derive/incan_web_macros"),
            },
            StdlibExtraCrateDep {
                crate_name: "inventory",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "axum",
                source: StdlibExtraCrateSource::Declared,
            },
        ],
        submodules: &["app", "routing", "request", "response", "macros", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "testing",
        package: Some("incan_stdlib_testing"),
        facet: Some(facets::TESTING),
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "logging",
        package: Some("incan_stdlib_observability"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "registry",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "telemetry",
        package: Some("incan_stdlib_observability"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &["core"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "environ",
        package: Some("incan_stdlib_system"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "async",
        package: Some("incan_stdlib_async"),
        facet: Some(facets::ASYNC),
        extra_crate_deps: &[],
        submodules: &["time", "task", "channel", "race", "sync", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: STDLIB_INTEROP,
        package: Some("incan_stdlib_interop"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "serde",
        package: Some("incan_stdlib_data"),
        facet: Some(facets::DATA),
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "serde",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &["json"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: STDLIB_JSON,
        package: Some("incan_stdlib_data"),
        facet: Some(facets::DATA),
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "serde",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "toml",
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "toml_edit",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "serde_path_to_error",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "toml",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "serde",
                source: StdlibExtraCrateSource::Declared,
            },
        ],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "reflection",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "result",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "derives",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &["string", "comparison", "copying", "collection"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "traits",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &["convert", "ops", "error", "indexing", "callable", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "math",
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "libm",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "fs",
        package: Some("incan_stdlib_system"),
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "encoding_rs",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "rustix",
                source: StdlibExtraCrateSource::Declared,
            },
        ],
        submodules: &["path", "file", "locking", "metadata", "glob", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "datetime",
        package: Some("incan_stdlib_data"),
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
        package: Some("incan_stdlib_core"),
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
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "uuid",
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "rand",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "regex",
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "regex",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &["_core", "_replacement", "types", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "collections",
        package: Some("incan_stdlib_data"),
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
        package: Some("incan_stdlib_system"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "byteorder",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "encoding",
        package: Some("incan_stdlib_codecs"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[
            "_shared", "prelude", "hex", "base32", "base64", "base85", "base58", "bech32",
        ],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "checksum",
        package: Some("incan_stdlib_codecs"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "crc32fast",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "hash",
        package: Some("incan_stdlib_data"),
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "blake2",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "blake3",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "hmac",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "md5",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "sha1",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "sha2",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "sha3",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "xxhash_rust",
                source: StdlibExtraCrateSource::Declared,
            },
        ],
        submodules: &["_core", "_hmac", "_streaming", "prelude"],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "compression",
        package: Some("incan_stdlib_compression"),
        facet: None,
        extra_crate_deps: &[
            StdlibExtraCrateDep {
                crate_name: "flate2",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "zstd",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "bzip2",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "xz2",
                source: StdlibExtraCrateSource::Declared,
            },
            StdlibExtraCrateDep {
                crate_name: "snap",
                source: StdlibExtraCrateSource::Declared,
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
        package: Some("incan_stdlib_system"),
        facet: None,
        extra_crate_deps: &[StdlibExtraCrateDep {
            crate_name: "tempfile",
            source: StdlibExtraCrateSource::Declared,
        }],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "rust",
        package: None,
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        // Capability bounds (Send, Sync, Static, Fn, FnMut, FnOnce) are native Rust traits already in scope in all
        // generated code. They have no .incn stub and no emitted Rust module.
        typechecker_only: true,
    },
    StdlibNamespace {
        name: STDLIB_BUILTINS,
        package: None,
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        // `std.builtins.<name>(...)` is an explicit call escape to the compiler's builtin-function registry. Builtin
        // types deliberately stay at the root surface and there is no source stub or emitted Rust module.
        typechecker_only: true,
    },
    StdlibNamespace {
        name: "features",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "prelude",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "this",
        package: Some("incan_stdlib_core"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "vocab",
        package: Some("incan_stdlib_vocab"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &[],
        typechecker_only: false,
    },
    StdlibNamespace {
        name: "primitives",
        package: Some("incan_stdlib_primitives"),
        facet: None,
        extra_crate_deps: &[],
        submodules: &["vocab"],
        typechecker_only: false,
    },
];
