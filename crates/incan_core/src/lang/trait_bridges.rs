//! Trait bridge registry for generic newtype trait delegation.
//!
//! This module defines a data-driven registry mapping dunder methods to external Rust traits,
//! enabling automatic trait delegation for newtype wrappers without hardcoding trait-specific logic.
//!
//! ## Design
//!
//! When a user writes:
//! ```incan
//! pub type Json[T] = newtype AxumJson[T]
//! ```
//!
//! The compiler automatically generates trait implementations by delegating to the wrapped type:
//! ```rust,ignore
//! impl<T: Serialize> IntoResponse for Json<T> {
//!     fn into_response(self) -> Response {
//!         self.0.into_response()
//!     }
//! }
//! ```
//!
//! Users can override with a dunder method:
//! ```incan
//! pub type Json[T] = newtype AxumJson[T]:
//!     def __into_response__(self) -> Response:
//!         # Custom implementation
//!         return self.0.into_response()
//! ```
//!
//! ## Current Limitations
//!
//! The trait bridge system currently supports only:
//! - Synchronous methods taking `self` by value
//! - Single-method traits
//! - Simple return types
//!
//! **Not yet supported** (but planned):
//! - Async methods (e.g., `async fn from_request(...)` for extractors)
//! - Static/associated functions (e.g., `fn from_request(req, state)` not `self.from_request()`)
//! - Associated types (e.g., `type Rejection = ...`)
//! - Multiple parameters
//!
//! ## Adding New Traits
//!
//! To support a new trait, add an entry to [`TRAIT_BRIDGES`]:
//! ```rust,ignore
//! TraitBridge {
//!     dunder_method: "__new_trait__",
//!     trait_path: "crate::NewTrait",
//!     trait_method: "new_method",
//!     return_type: "ReturnType",
//!     applies_to_type_path: "crate::",
//! }
//! ```

/// Metadata for a trait that can be auto-delegated through newtype wrappers.
///
/// Each bridge defines:
/// - **dunder method**: The Incan method name that maps to this trait
/// - **trait path**: The Rust trait to implement
/// - **trait method**: The method within the trait
/// - **applies_to_type_path**: A prefix that the wrapped type's path must match for auto-delegation
///
/// The `applies_to_type_path` enables smart auto-delegation: only emit the trait impl when we know the wrapped type
/// actually implements it.
///
/// # Examples
///
/// - `applies_to_type_path = "axum::response::"` → only apply IntoResponse to types from that module
/// - `applies_to_type_path = ""` → apply to all types (use with caution, e.g., for Clone/Debug on primitives)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitBridge {
    /// The Incan dunder method name (e.g., `"__into_response__"`).
    pub dunder_method: &'static str,

    /// The Rust trait path (e.g., `"axum::response::IntoResponse"`).
    pub trait_path: &'static str,

    /// The trait method name (e.g., `"into_response"`).
    pub trait_method: &'static str,

    /// The return type of the trait method (e.g., `"axum::response::Response"`).
    pub return_type: &'static str,

    /// Type path prefix that indicates the wrapped type implements this trait.
    /// For example, `"axum::response::"` means any type from that module implements `IntoResponse`.
    pub applies_to_type_path: &'static str,

    /// Whether this trait method is async.
    pub is_async: bool,

    /// Whether this is a static/associated function (no `self` parameter).
    pub is_static: bool,

    /// Additional parameters beyond `self` (for static functions, these are ALL parameters).
    /// Format: &[("param_name", "param_type")]
    /// Example: &[("req", "Request"), ("state", "&S")]
    pub parameters: &'static [(&'static str, &'static str)],

    /// Associated types that must be declared in the impl block.
    /// Format: &[("type_name", "type_value")]
    /// Example: &[("Rejection", "axum::response::Response")]
    pub associated_types: &'static [(&'static str, &'static str)],

    /// Custom delegation pattern for complex cases. Placeholders:
    /// - `{wrapped_type}` → the wrapped type (e.g., `AxumQuery<T>`)
    /// - `{method}` → the trait method name
    /// - `{params}` → comma-separated parameter names
    ///
    ///   Empty string means use default simple delegation: `self.0.method()`
    pub delegation_pattern: &'static str,

    /// Generic parameters to add to the trait itself.
    /// Example: `"<S>"` for `FromRequest<S>`
    /// Empty string means no generics.
    pub trait_generics: &'static str,

    /// Extra type parameters for the impl block beyond the newtype's own generics.
    /// Example: `&["S"]` for `impl<T, S>` when newtype has `<T>`
    pub extra_impl_generics: &'static [&'static str],

    /// Optional where clause for trait bound constraints.
    /// Example: `"where T: Deserialize"`
    pub where_clause: &'static str,

    /// Required imports for the impl to compile.
    /// Example: `&["std::future::Future"]` for async functions
    pub required_imports: &'static [&'static str],
}

/// Registry of known trait bridges for auto-delegation.
///
/// This registry is intentionally **data-driven** to avoid hardcoding trait-specific logic in the compiler.
/// Adding support for a new trait only requires adding an entry here.
///
/// **Type applicability patterns:**
/// - Specific module prefixes (e.g., `"axum::response::"`) → only apply to types from that module
/// - Empty string `""` → currently disabled for safety (would apply to all types)
pub const TRAIT_BRIDGES: &[TraitBridge] = &[
    TraitBridge {
        dunder_method: "__into_response__",
        trait_path: "axum::response::IntoResponse",
        trait_method: "into_response",
        return_type: "axum::response::Response",
        applies_to_type_path: "axum::response::",
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    // FromRequestParts: for extractors that don't consume the request body (like Query, Header, etc.)
    // The blanket impl in axum_core automatically provides FromRequest<S, ViaParts> for types implementing this
    TraitBridge {
        dunder_method: "__from_request_parts__",
        trait_path: "axum::extract::FromRequestParts",
        trait_method: "from_request_parts",
        return_type: "impl Future<Output = Result<Self, Self::Rejection>> + Send",
        applies_to_type_path: "axum::extract::",
        is_async: false, // NOT async fn - it's a sync function returning impl Future
        is_static: true,
        parameters: &[("parts", "&mut axum::http::request::Parts"), ("state", "&S")],
        associated_types: &[(
            "Rejection",
            "<{wrapped_type} as axum::extract::FromRequestParts<S>>::Rejection",
        )],
        delegation_pattern: "async { {wrapped_type}::from_request_parts(parts, state).await.map(Self) }",
        trait_generics: "<S>",
        extra_impl_generics: &["S"],
        where_clause: "where T: DeserializeOwned, S: Send + Sync, {wrapped_type}: axum::extract::FromRequestParts<S>",
        required_imports: &["std::future::Future", "serde::de::DeserializeOwned"],
    },
    // Note: FromRequest (consuming body extractors like Json, Bytes) would need different handling
    // For now, focusing on FromRequestParts which covers Query, Path, Header, etc.,
    // Note: Serialize/Deserialize require type inspection (T: Serialize) - Phase 3
    TraitBridge {
        dunder_method: "__serialize__",
        trait_path: "serde::Serialize",
        trait_method: "serialize",
        return_type: "()",
        applies_to_type_path: "", // Disabled: needs trait bound inference
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    TraitBridge {
        dunder_method: "__deserialize__",
        trait_path: "serde::Deserialize",
        trait_method: "deserialize",
        return_type: "Result<Self, D::Error>",
        applies_to_type_path: "", // Disabled: needs trait bound inference
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    TraitBridge {
        dunder_method: "__into_iter__",
        trait_path: "IntoIterator",
        trait_method: "into_iter",
        return_type: "Self::IntoIter",
        applies_to_type_path: "", // Disabled: would apply to too many types unsafely
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    TraitBridge {
        dunder_method: "__from_str__",
        trait_path: "std::str::FromStr",
        trait_method: "from_str",
        return_type: "Result<Self, Self::Err>",
        applies_to_type_path: "", // Disabled: needs trait bound inference
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    TraitBridge {
        dunder_method: "__clone__",
        trait_path: "Clone",
        trait_method: "clone",
        return_type: "Self",
        applies_to_type_path: "", // Disabled: needs type system inspection (Copy types)
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
    TraitBridge {
        dunder_method: "__debug__",
        trait_path: "std::fmt::Debug",
        trait_method: "fmt",
        return_type: "std::fmt::Result",
        applies_to_type_path: "", // Disabled: would apply to all types
        is_async: false,
        is_static: false,
        parameters: &[],
        associated_types: &[],
        delegation_pattern: "",
        trait_generics: "",
        extra_impl_generics: &[],
        where_clause: "",
        required_imports: &[],
    },
];

/// Find a trait bridge by dunder method name.
///
/// Returns `None` if the method is not a registered trait bridge.
///
/// # Examples
///
/// ```
/// use incan_core::lang::trait_bridges;
///
/// let bridge = trait_bridges::find_by_dunder("__into_response__");
/// assert!(bridge.is_some());
/// assert_eq!(bridge.unwrap().trait_path, "axum::response::IntoResponse");
///
/// let unknown = trait_bridges::find_by_dunder("__unknown__");
/// assert!(unknown.is_none());
/// ```
pub fn find_by_dunder(dunder_method: &str) -> Option<&'static TraitBridge> {
    TRAIT_BRIDGES
        .iter()
        .find(|bridge| bridge.dunder_method == dunder_method)
}

/// Check if a method name is a registered trait bridge dunder.
///
/// This is used to quickly determine if a method should trigger trait delegation without performing a full lookup.
///
/// # Examples
///
/// ```
/// use incan_core::lang::trait_bridges;
///
/// assert!(trait_bridges::is_trait_bridge("__into_response__"));
/// assert!(!trait_bridges::is_trait_bridge("__eq__")); // Magic method, not trait bridge
/// assert!(!trait_bridges::is_trait_bridge("regular_method"));
/// ```
pub fn is_trait_bridge(method_name: &str) -> bool {
    TRAIT_BRIDGES.iter().any(|bridge| bridge.dunder_method == method_name)
}

/// Check if a trait bridge applies to a given type path.
///
/// This determines whether auto-delegation should occur for a newtype wrapping a particular type.
/// A bridge applies if the type path starts with the bridge's `applies_to_type_path` pattern.
///
/// An empty pattern (`""`) means the bridge is disabled and won't auto-apply to any type.
///
/// # Examples
///
/// ```
/// use incan_core::lang::trait_bridges;
///
/// let bridge = trait_bridges::find_by_dunder("__into_response__").unwrap();
/// assert!(trait_bridges::bridge_applies_to_type(
///     bridge,
///     "axum::response::Response"
/// ));
/// assert!(trait_bridges::bridge_applies_to_type(
///     bridge,
///     "axum::response::Html<String>"
/// ));
/// assert!(!trait_bridges::bridge_applies_to_type(bridge, "String"));
/// assert!(!trait_bridges::bridge_applies_to_type(bridge, "i64"));
/// ```
pub fn bridge_applies_to_type(bridge: &TraitBridge, type_path: &str) -> bool {
    if bridge.applies_to_type_path.is_empty() {
        return false; // Empty pattern = disabled
    }
    type_path.starts_with(bridge.applies_to_type_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_by_dunder() {
        let bridge = find_by_dunder("__into_response__");
        assert!(bridge.is_some());
        assert_eq!(bridge.unwrap().trait_path, "axum::response::IntoResponse");
        assert_eq!(bridge.unwrap().trait_method, "into_response");
    }

    #[test]
    fn test_find_by_dunder_not_found() {
        let bridge = find_by_dunder("__nonexistent__");
        assert!(bridge.is_none());
    }

    #[test]
    fn test_is_trait_bridge() {
        assert!(is_trait_bridge("__into_response__"));
        assert!(is_trait_bridge("__serialize__"));
        assert!(is_trait_bridge("__clone__"));
        assert!(!is_trait_bridge("__eq__"));
        assert!(!is_trait_bridge("normal_method"));
    }

    #[test]
    fn test_all_bridges_have_consistent_data() {
        for bridge in TRAIT_BRIDGES {
            assert!(bridge.dunder_method.starts_with("__"));
            assert!(bridge.dunder_method.ends_with("__"));
            assert!(!bridge.trait_path.is_empty());
            assert!(!bridge.trait_method.is_empty());
            // applies_to_type_path can be empty (disabled bridges)
        }
    }

    #[test]
    fn test_bridge_applies_to_type() {
        let into_response = find_by_dunder("__into_response__").unwrap();
        assert!(bridge_applies_to_type(into_response, "axum::response::Response"));
        assert!(bridge_applies_to_type(into_response, "axum::response::Html<String>"));
        assert!(!bridge_applies_to_type(into_response, "String"));
        assert!(!bridge_applies_to_type(into_response, "i64"));

        // Disabled bridges (empty pattern) don't apply to anything
        let clone = find_by_dunder("__clone__").unwrap();
        assert!(!bridge_applies_to_type(clone, "String"));
        assert!(!bridge_applies_to_type(clone, "i64"));
    }
}
