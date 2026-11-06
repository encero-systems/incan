//! Namespace ownership derived from the compiler's single standard-library catalog.
//!
//! These immutable facts describe publication policy only. A matching package name does not select trusted source or
//! issue an installed namespace grant. Publication retains genuine executable-relative source selection; installed use
//! authenticates original checked package/generation associations.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::stdlib::STDLIB_NAMESPACES;

/// One ordinary standard package and the reserved top-level namespaces it may publish.
#[derive(Debug, PartialEq, Eq)]
pub struct StandardPackageNamespacePolicy {
    /// Canonical ordinary package name.
    pub package_name: &'static str,
    /// Reserved roots this package owns below `std`; compiler-only namespaces have no package issuer here.
    pub namespace_roots: Vec<&'static str>,
}

/// Compiler-pinned publication policies, projected from the module catalog without a second ownership list.
pub static STANDARD_PACKAGE_NAMESPACE_POLICIES: LazyLock<Vec<StandardPackageNamespacePolicy>> = LazyLock::new(|| {
    let mut packages = BTreeMap::<_, Vec<_>>::new();
    for namespace in STDLIB_NAMESPACES {
        if let Some(package) = namespace.package {
            packages.entry(package).or_default().push(namespace.name);
        }
    }
    packages
        .into_iter()
        .map(|(package_name, namespace_roots)| StandardPackageNamespacePolicy {
            package_name,
            namespace_roots,
        })
        .collect()
});

/// Look up immutable compiler policy by canonical package name; this pure query confers no source/namespace grant.
pub fn standard_package_namespace_policy(name: &str) -> Option<&'static StandardPackageNamespacePolicy> {
    STANDARD_PACKAGE_NAMESPACE_POLICIES
        .iter()
        .find(|policy| policy.package_name == name)
}

static PRIMITIVES_SOURCE_BRIDGE_POLICY: LazyLock<StandardPackageNamespacePolicy> = LazyLock::new(|| {
    let mut namespace_roots = Vec::new();
    if let Some(policy) = standard_package_namespace_policy("incan_stdlib_primitives") {
        namespace_roots.extend_from_slice(&policy.namespace_roots);
    }
    if let Some(policy) = standard_package_namespace_policy("incan_stdlib_core") {
        namespace_roots.extend_from_slice(&policy.namespace_roots);
    }
    StandardPackageNamespacePolicy {
        package_name: "incan_stdlib_primitives",
        namespace_roots,
    }
});

/// Look up the transition policy for a package's source publication and compiled Loaf namespace.
///
/// The bridge compiler admits the destination primitive package for the roots whose source moves in the next step.
/// Compiled consumers select this wider destination policy only after their native closure stops linking core, so the
/// destination Loaf can answer the moved roots without changing old-floor units.
pub fn standard_package_source_policy(name: &str) -> Option<&'static StandardPackageNamespacePolicy> {
    if name == "incan_stdlib_primitives" {
        return Some(&PRIMITIVES_SOURCE_BRIDGE_POLICY);
    }
    standard_package_namespace_policy(name)
}
