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

/// Borrow shape for a metadata-free external method compatibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFreeMethodArgBorrowPolicy {
    Shared,
    Mutable,
}

/// Receiver class used by metadata-free external method compatibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFreeReceiverClass {
    IoValue,
    EncodingInstance,
    ExternalAssociated,
}

/// Argument class used by metadata-free external method compatibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFreeArgClass {
    StringBuffer,
    ByteBuffer,
    Any,
}

/// Borrow compatibility rule for one metadata-free Rust method surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFreeMethodBorrowRule {
    pub methods: &'static [&'static str],
    pub receiver: MetadataFreeReceiverClass,
    pub arg: MetadataFreeArgClass,
    pub policy: MetadataFreeMethodArgBorrowPolicy,
}

/// One parameter in a metadata-free Rust method signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFreeMethodParamRule {
    pub name: Option<&'static str>,
    pub type_display: &'static str,
}

/// Complete callable signature for one metadata-free Rust method surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFreeMethodSignatureRule {
    pub receiver_path: &'static str,
    pub method: &'static str,
    pub params: &'static [MetadataFreeMethodParamRule],
    pub return_type: &'static str,
    pub is_async: bool,
    pub is_unsafe: bool,
}

/// Metadata-free external method borrow policies used when rust-inspect metadata is unavailable.
pub const METADATA_FREE_METHOD_BORROW_RULES: &[MetadataFreeMethodBorrowRule] = &[
    MetadataFreeMethodBorrowRule {
        methods: &["read_to_string"],
        receiver: MetadataFreeReceiverClass::IoValue,
        arg: MetadataFreeArgClass::StringBuffer,
        policy: MetadataFreeMethodArgBorrowPolicy::Mutable,
    },
    MetadataFreeMethodBorrowRule {
        methods: &["read", "read_to_end", "read_exact", "read_buf", "read_buf_exact"],
        receiver: MetadataFreeReceiverClass::IoValue,
        arg: MetadataFreeArgClass::ByteBuffer,
        policy: MetadataFreeMethodArgBorrowPolicy::Mutable,
    },
    MetadataFreeMethodBorrowRule {
        methods: &["write"],
        receiver: MetadataFreeReceiverClass::IoValue,
        arg: MetadataFreeArgClass::ByteBuffer,
        policy: MetadataFreeMethodArgBorrowPolicy::Shared,
    },
    MetadataFreeMethodBorrowRule {
        methods: &["write_all"],
        receiver: MetadataFreeReceiverClass::IoValue,
        arg: MetadataFreeArgClass::Any,
        policy: MetadataFreeMethodArgBorrowPolicy::Shared,
    },
    MetadataFreeMethodBorrowRule {
        methods: &["for_label", "encode", "decode"],
        receiver: MetadataFreeReceiverClass::EncodingInstance,
        arg: MetadataFreeArgClass::Any,
        policy: MetadataFreeMethodArgBorrowPolicy::Shared,
    },
    MetadataFreeMethodBorrowRule {
        methods: &["decode"],
        receiver: MetadataFreeReceiverClass::ExternalAssociated,
        arg: MetadataFreeArgClass::ByteBuffer,
        policy: MetadataFreeMethodArgBorrowPolicy::Shared,
    },
];

/// Metadata-free external method signatures used when rust-inspect metadata is unavailable.
pub const METADATA_FREE_METHOD_SIGNATURE_RULES: &[MetadataFreeMethodSignatureRule] =
    &[MetadataFreeMethodSignatureRule {
        receiver_path: "encoding_rs::Encoding",
        method: "for_label",
        params: &[MetadataFreeMethodParamRule {
            name: Some("label"),
            type_display: "&[u8]",
        }],
        return_type: "Option<&'static encoding_rs::Encoding>",
        is_async: false,
        is_unsafe: false,
    }];

/// Return conservative callable metadata for Rust surfaces the stdlib must compile against even when rust-inspect
/// cannot recover full crate metadata in generated smoke projects.
#[must_use]
pub fn metadata_free_method_signature(rust_path: &str, method: &str) -> Option<RustFunctionSig> {
    let rule = METADATA_FREE_METHOD_SIGNATURE_RULES
        .iter()
        .find(|rule| rule.receiver_path == rust_path && rule.method == method)?;
    Some(RustFunctionSig {
        params: rule
            .params
            .iter()
            .map(|param| RustParam {
                name: param.name.map(str::to_string),
                type_display: param.type_display.to_string(),
            })
            .collect(),
        return_type: rule.return_type.to_string(),
        is_async: rule.is_async,
        is_unsafe: rule.is_unsafe,
    })
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

/// Render `path` with generic arguments as `path<A, B, ...>` for stable Rust-like display.
#[must_use]
pub fn render_rust_type_shape_path(path: &str, args: &[RustTypeShape]) -> String {
    if args.is_empty() {
        return path.to_string();
    }
    let rendered_args: Vec<String> = args.iter().map(render_rust_type_shape).collect();
    format!("{path}<{}>", rendered_args.join(", "))
}

/// Pretty-print a [`RustTypeShape`] as a stable Rust-like type string.
#[must_use]
pub fn render_rust_type_shape(shape: &RustTypeShape) -> String {
    match shape {
        RustTypeShape::Bool => "bool".to_string(),
        RustTypeShape::Float => "f64".to_string(),
        RustTypeShape::Int => "i64".to_string(),
        RustTypeShape::Str => "String".to_string(),
        RustTypeShape::Bytes => "Vec<u8>".to_string(),
        RustTypeShape::Unit => "()".to_string(),
        RustTypeShape::Option(inner) => format!("Option<{}>", render_rust_type_shape(inner)),
        RustTypeShape::Result(ok, err) => {
            format!(
                "Result<{}, {}>",
                render_rust_type_shape(ok),
                render_rust_type_shape(err)
            )
        }
        RustTypeShape::Tuple(items) => {
            let rendered: Vec<String> = items.iter().map(render_rust_type_shape).collect();
            format!("({})", rendered.join(", "))
        }
        RustTypeShape::Ref(inner) => format!("&{}", render_rust_type_shape(inner)),
        RustTypeShape::RustPath { path, args } => render_rust_type_shape_path(path, args),
        RustTypeShape::TypeParam(name) => name.clone(),
        RustTypeShape::Unknown => "?".to_string(),
    }
}

/// Remove Rust lifetime labels that decorate borrowed display types.
#[must_use]
pub fn strip_rust_borrow_lifetimes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        out.push(ch);
        if ch != '&' {
            continue;
        }
        while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        }
        if !matches!(chars.peek(), Some('\'')) {
            continue;
        }
        chars.next();
        while matches!(chars.peek(), Some(next) if next.is_ascii_alphanumeric() || *next == '_') {
            chars.next();
        }
        while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            chars.next();
        }
    }
    out
}

/// Split a comma-separated Rust generic/tuple argument list without splitting inside nested generic, tuple, or slice
/// delimiters.
#[must_use]
pub fn split_top_level_rust_args(text: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            ',' if angle == 0 && paren == 0 && bracket == 0 => {
                args.push(text[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        args.push(tail);
    }
    args
}

/// A public field surfaced on a Rust struct/union-like type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustFieldInfo {
    /// Source-facing Rust field name accepted by Incan, with raw identifier prefixes removed.
    ///
    /// A Rust field declared as `r#type` is surfaced as `type`; an ordinary Rust field declared as `type_` remains
    /// `type_`. Codegen rawifies keyword names when emitting Rust.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RustTypeInfo {
    /// Pretty-printed target type when this item is a Rust `type` alias.
    ///
    /// Ordinary structs, enums, traits, and builtins leave this empty. Alias targets are metadata, not a substitute
    /// type identity: callers should use them only when the alias itself is the expected surface and the target shape
    /// is needed for contextual typing or boundary planning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_target: Option<String>,
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
                alias_target: None,
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
