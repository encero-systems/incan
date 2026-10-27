//! Compiler-pinned namespace ownership of ordinary standard packages, independent of SDK catalogs/profiles.
//!
//! These immutable facts describe policy only. A matching name or public DTO does not select trusted source or issue
//! an installed namespace grant. Publication must retain genuine executable-relative source selection; installed use
//! must additionally authenticate original checked package/generation associations.

/// One compiler-owned package and the reserved top-level namespaces it may publish.
#[derive(Debug, PartialEq, Eq)]
pub struct StandardPackageNamespacePolicy {
    /// Canonical ordinary package name.
    pub package_name: &'static str,
    /// Reserved roots this package owns below `std`; compiler-only namespaces have no package issuer here.
    pub namespace_roots: &'static [&'static str],
}

/// Compiler-pinned ordinary ownership registry; changing ownership requires a compiler source change.
pub const STANDARD_PACKAGE_NAMESPACE_POLICIES: &[StandardPackageNamespacePolicy] = &[
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_vocab",
        namespace_roots: &["vocab"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_core",
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
        namespace_roots: &["interop"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_system",
        namespace_roots: &["environ", "io", "tempfile", "fs"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_codecs",
        namespace_roots: &["checksum", "encoding"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_compression",
        namespace_roots: &["compression"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_data",
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
        namespace_roots: &["async"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_observability",
        namespace_roots: &["logging", "telemetry"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_web",
        namespace_roots: &["web"],
    },
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_testing",
        namespace_roots: &["testing"],
    },
];

/// Look up immutable compiler policy by canonical package name; this pure query confers no source/namespace grant.
pub fn standard_package_namespace_policy(name: &str) -> Option<&'static StandardPackageNamespacePolicy> {
    STANDARD_PACKAGE_NAMESPACE_POLICIES
        .iter()
        .find(|policy| policy.package_name == name)
}
