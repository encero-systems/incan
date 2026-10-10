//! Compiler-pinned namespace ownership of ordinary standard packages, independent of SDK catalogs/profiles.
//!
//! These immutable facts describe policy only. A matching name, version, declaration or public DTO does not select
//! trusted source or issue an installed namespace grant. Publication must retain genuine executable-relative source
//! selection; installed use must additionally authenticate original checked package/generation associations.

/// One compiler-owned package declaration and the reserved top-level namespaces it may publish.
#[derive(Debug)]
pub struct StandardPackageNamespacePolicy {
    /// Canonical ordinary package name, checked against the embedded authored declaration by the provider.
    pub package_name: &'static str,
    /// Exact authored package version; standard packages need not share one distribution version.
    pub version: &'static str,
    /// Fixed package directory below the compiler-owned standard source root.
    pub source_directory: &'static str,
    /// Complete authored Loaf declaration bytes embedded into this compiler.
    pub declaration: &'static str,
    /// Reserved roots this package owns below `std`; compiler-only namespaces have no package issuer here.
    pub namespace_roots: &'static [&'static str],
}

/// Compiler-pinned ordinary ownership registry; changing ownership requires a compiler source change.
pub const STANDARD_PACKAGE_NAMESPACE_POLICIES: &[StandardPackageNamespacePolicy] = &[
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_core",
        version: "0.6.0-dev.6",
        source_directory: "core",
        declaration: include_str!("../../../../stdlib/core/loaf.toml"),
        namespace_roots: &[
            "features",
            "prelude",
            "registry",
            "result",
            "reflection",
            "this",
            "derives",
            "traits",
            "runtime",
        ],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_interop",
        version: "0.5.0",
        source_directory: "interop",
        declaration: include_str!("../../../../stdlib/interop/loaf.toml"),
        namespace_roots: &["interop"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_system",
        version: "0.5.0",
        source_directory: "system",
        declaration: include_str!("../../../../stdlib/system/loaf.toml"),
        namespace_roots: &["environ", "io", "tempfile", "fs"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_codecs",
        version: "0.5.0",
        source_directory: "codecs",
        declaration: include_str!("../../../../stdlib/codecs/loaf.toml"),
        namespace_roots: &["checksum", "encoding"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_compression",
        version: "0.5.0",
        source_directory: "compression",
        declaration: include_str!("../../../../stdlib/compression/loaf.toml"),
        namespace_roots: &["compression"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_data",
        version: "0.5.0",
        source_directory: "data",
        declaration: include_str!("../../../../stdlib/data/loaf.toml"),
        namespace_roots: &[
            "collections",
            "graph",
            "hash",
            "json",
            "toml",
            "math",
            "uuid",
            "datetime",
            "regex",
            "serde",
        ],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_async",
        version: "0.5.0",
        source_directory: "async",
        declaration: include_str!("../../../../stdlib/async/loaf.toml"),
        namespace_roots: &["async"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_observability",
        version: "0.5.0",
        source_directory: "observability",
        declaration: include_str!("../../../../stdlib/observability/loaf.toml"),
        namespace_roots: &["logging", "telemetry"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_web",
        version: "0.5.0",
        source_directory: "web",
        declaration: include_str!("../../../../stdlib/web/loaf.toml"),
        namespace_roots: &["web"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_testing",
        version: "0.5.0",
        source_directory: "testing",
        declaration: include_str!("../../../../stdlib/testing/loaf.toml"),
        namespace_roots: &["testing"],
    },
];

/// Look up immutable compiler policy by canonical package name; this pure query confers no source/namespace grant.
pub fn standard_package_namespace_policy(name: &str) -> Option<&'static StandardPackageNamespacePolicy> {
    STANDARD_PACKAGE_NAMESPACE_POLICIES
        .iter()
        .find(|policy| policy.package_name == name)
}
