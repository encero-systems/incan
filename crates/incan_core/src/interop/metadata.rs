//! Incan-native view of Rust items extracted from a Cargo workspace (RFC 041).
//!
//! These types are intentionally free of rust-analyzer or compiler-internal IDs so the typechecker and lowering stages
//! can consume stable, snapshot-friendly metadata.

use serde::{Deserialize, Serialize};

use crate::lang::types::collections::{self, CollectionTypeId};

/// Whether an item is visible across crate boundaries for ordinary `pub` Rust APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustVisibility {
    /// `pub` — visible outside the defining crate (subject to future path-specific rules).
    Public,
    /// Anything else (`pub(crate)`, `pub(super)`, private, etc.).
    Restricted,
}

/// Top-level classification for a resolved Rust path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustItemKind {
    /// A Rust module (namespace of nested items).
    Module(RustModuleInfo),
    /// A struct, enum, union, or builtin type surface (methods + associates).
    Type(RustTypeInfo),
    /// A free function, associated function, or method item viewed as callable.
    Function(RustFunctionSig),
    /// A `const` item.
    Constant {
        /// Pretty-printed Rust type string from the analyzer.
        type_display: String,
    },
    /// A `trait` definition and its associated items.
    Trait(RustTraitInfo),
    /// Placeholder for statics, macros, type aliases, etc. until RFC 041 narrows support.
    Unsupported { description: String },
}

/// Metadata for one resolved Rust item (type, fn, module, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustItemMetadata {
    /// Canonical path as Incan already models it, e.g. `std::collections::HashMap`.
    pub canonical_path: String,
    /// Underlying Rust definition path after resolving re-exports, when rust-analyzer can provide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_path: Option<String>,
    pub visibility: RustVisibility,
    pub kind: RustItemKind,
}

/// Rust std/alloc collection families whose lookup methods rely on Rust borrow semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustCollectionFamily {
    /// Hash-map families keyed by borrowed lookup probes (`get`, `contains_key`).
    HashMap,
    /// Ordered map families keyed by borrowed lookup probes (`get`, `contains_key`).
    BTreeMap,
    /// Hash-set families queried by borrowed element probes (`contains`).
    HashSet,
    /// Ordered set families queried by borrowed element probes (`contains`).
    BTreeSet,
}

impl RustCollectionFamily {
    /// Classify a canonical Rust path into a supported collection family.
    #[must_use]
    pub fn for_canonical_path(path: &str) -> Option<Self> {
        let path = path.split('<').next().unwrap_or(path);
        match path {
            "std::collections::HashMap"
            | "std::collections::hash_map::HashMap"
            | "hashbrown::HashMap"
            | "hashbrown::map::HashMap" => Some(Self::HashMap),
            "std::collections::BTreeMap" | "alloc::collections::btree_map::BTreeMap" => Some(Self::BTreeMap),
            "std::collections::HashSet"
            | "std::collections::hash_set::HashSet"
            | "hashbrown::HashSet"
            | "hashbrown::set::HashSet" => Some(Self::HashSet),
            "std::collections::BTreeSet" | "alloc::collections::btree_set::BTreeSet" => Some(Self::BTreeSet),
            _ => None,
        }
    }

    /// Classify an Incan or imported collection type name into a supported collection family.
    #[must_use]
    pub fn for_type_name(name: &str) -> Option<Self> {
        match collections::from_str(name) {
            Some(CollectionTypeId::Dict) => return Some(Self::HashMap),
            Some(CollectionTypeId::Set) => return Some(Self::HashSet),
            _ => {}
        }
        match name {
            "BTreeMap" => Some(Self::BTreeMap),
            "BTreeSet" => Some(Self::BTreeSet),
            _ => None,
        }
    }

    /// Whether `method` is a borrow-sensitive lookup on this collection family.
    #[must_use]
    pub fn preserves_lookup_arg_shape(self, method: &str) -> bool {
        match self {
            Self::HashMap | Self::BTreeMap => matches!(method, "get" | "contains_key"),
            Self::HashSet | Self::BTreeSet => method == "contains",
        }
    }
}

impl RustItemMetadata {
    /// Classify this metadata item as a supported std/alloc collection family when applicable.
    #[must_use]
    pub fn collection_family(&self) -> Option<RustCollectionFamily> {
        RustCollectionFamily::for_canonical_path(&self.canonical_path)
    }
}

/// A single parameter in a Rust function signature (display strings only for Phase 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustParam {
    /// Parameter name when rust-analyzer can recover it from the HIR body.
    pub name: Option<String>,
    /// Pretty-printed type suitable for diagnostics and future coercion work.
    pub type_display: String,
}

/// Callable signature extracted from rust-analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustFunctionSig {
    pub params: Vec<RustParam>,
    pub return_type: String,
    pub is_async: bool,
    pub is_unsafe: bool,
}

/// An inherent or trait method surfaced on a type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustMethodSig {
    pub name: String,
    pub signature: RustFunctionSig,
}

/// One trait implementation rust-inspect can associate with a concrete Rust type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustImplementedTrait {
    /// Canonical Rust trait path, for example `std::fmt::Display`.
    pub path: String,
}

/// Structured Rust type information used by Incan interop consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustTypeShape {
    /// Any Rust `bool`.
    Bool,
    /// Any floating-point scalar. Width is intentionally erased at this layer.
    Float,
    /// Any signed or unsigned integer scalar. Width is intentionally erased at this layer.
    Int,
    /// UTF-8 string data such as `str` or `String`.
    Str,
    /// Byte buffers such as `Vec<u8>` or `&[u8]`.
    Bytes,
    /// The unit type `()`.
    Unit,
    /// An `Option<T>`-like wrapper.
    Option(Box<RustTypeShape>),
    /// A `Result<T, E>`-like wrapper.
    Result(Box<RustTypeShape>, Box<RustTypeShape>),
    /// A tuple shape with one entry per element.
    Tuple(Vec<RustTypeShape>),
    /// A shared or mutable reference.
    Ref(Box<RustTypeShape>),
    /// A concrete Rust path plus any generic arguments preserved by the extractor.
    RustPath { path: String, args: Vec<RustTypeShape> },
    /// A generic type parameter such as `T`.
    TypeParam(String),
    /// Metadata recovery could not determine a stable semantic shape.
    Unknown,
}

/// A public field surfaced on a Rust struct/union-like type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustFieldInfo {
    /// Field name as it appears in Rust.
    pub name: String,
    /// Pretty-printed type for diagnostics and debug output.
    pub type_display: String,
    /// Semantic type shape used by the typechecker for field access and pattern payload binding.
    pub type_shape: RustTypeShape,
}

/// One enum variant and its payload field types.
///
/// Payload shapes are normalized for matching. For example, prost-style `Box<T>` payloads are recorded as `T` because
/// that is what Incan binds in constructor patterns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustVariantInfo {
    /// Variant name as it appears in Rust.
    pub name: String,
    /// Positional payload field shapes in declaration order.
    pub fields: Vec<RustTypeShape>,
}

/// Method, field, and variant surface for a Rust ADT or builtin type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustTypeInfo {
    /// Public inherent methods and associated functions.
    pub methods: Vec<RustMethodSig>,
    /// Trait implementations rust-inspect can prove for this Rust type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implemented_traits: Vec<RustImplementedTrait>,
    /// Public fields for struct/union-like types.
    pub fields: Vec<RustFieldInfo>,
    /// Enum variants when the type is an enum; empty for non-enums.
    pub variants: Vec<RustVariantInfo>,
}

/// One exported name inside a module (lightweight summary for namespace resolution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustModuleChild {
    pub name: String,
    pub kind_hint: RustModuleChildKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustModuleChildKind {
    Module,
    Type,
    Function,
    Constant,
    Trait,
    Other,
}

/// Children visible in a module scope (public items when resolved from outside).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustModuleInfo {
    pub children: Vec<RustModuleChild>,
}

/// Associated items declared on a trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustTraitAssoc {
    Function { name: String, signature: RustFunctionSig },
    TypeAlias { name: String },
    Constant { name: String, type_display: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustTraitInfo {
    pub items: Vec<RustTraitAssoc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_type_metadata(path: &str) -> RustItemMetadata {
        RustItemMetadata {
            canonical_path: path.to_string(),
            definition_path: Some(path.to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                methods: Vec::new(),
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        }
    }

    #[test]
    fn collection_family_matches_supported_map_and_set_paths() {
        for (path, expected) in [
            ("std::collections::HashMap", RustCollectionFamily::HashMap),
            ("hashbrown::HashMap", RustCollectionFamily::HashMap),
            ("hashbrown::map::HashMap<K, V>", RustCollectionFamily::HashMap),
            ("std::collections::BTreeMap", RustCollectionFamily::BTreeMap),
            ("std::collections::HashSet", RustCollectionFamily::HashSet),
            ("hashbrown::set::HashSet<T>", RustCollectionFamily::HashSet),
            ("std::collections::BTreeSet", RustCollectionFamily::BTreeSet),
            ("std::collections::HashMap<String, i64>", RustCollectionFamily::HashMap),
        ] {
            let meta = dummy_type_metadata(path);
            assert_eq!(meta.collection_family(), Some(expected), "path `{path}`");
        }
    }

    #[test]
    fn collection_family_matches_incan_and_imported_type_names() {
        for (name, expected) in [
            ("Dict", RustCollectionFamily::HashMap),
            ("HashMap", RustCollectionFamily::HashMap),
            ("Set", RustCollectionFamily::HashSet),
            ("BTreeMap", RustCollectionFamily::BTreeMap),
            ("BTreeSet", RustCollectionFamily::BTreeSet),
        ] {
            assert_eq!(
                RustCollectionFamily::for_type_name(name),
                Some(expected),
                "name `{name}`"
            );
        }
    }

    #[test]
    fn collection_family_reports_lookup_methods_that_preserve_arg_shape() {
        assert!(RustCollectionFamily::HashMap.preserves_lookup_arg_shape("get"));
        assert!(RustCollectionFamily::HashMap.preserves_lookup_arg_shape("contains_key"));
        assert!(!RustCollectionFamily::HashMap.preserves_lookup_arg_shape("insert"));
        assert!(RustCollectionFamily::HashSet.preserves_lookup_arg_shape("contains"));
        assert!(!RustCollectionFamily::HashSet.preserves_lookup_arg_shape("insert"));
    }
}
