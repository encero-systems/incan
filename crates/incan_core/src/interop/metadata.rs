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
    /// Preserve string literals as `&str` and adapt owned Incan strings with `.as_str()`.
    StringAsStr,
}

/// Receiver class used by metadata-free external method compatibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFreeReceiverClass {
    IoValue,
    EncodingInstance,
    TokenizerInstance,
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

/// One parameter in a metadata-free Rust free-function signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFreeFunctionParamRule {
    pub name: Option<&'static str>,
    pub type_display: &'static str,
}

/// Complete callable signature for one metadata-free Rust free-function surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFreeFunctionSignatureRule {
    pub path: &'static str,
    pub params: &'static [MetadataFreeFunctionParamRule],
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
        methods: &["encode"],
        receiver: MetadataFreeReceiverClass::TokenizerInstance,
        arg: MetadataFreeArgClass::StringBuffer,
        policy: MetadataFreeMethodArgBorrowPolicy::StringAsStr,
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

/// Metadata-free Rust free-function signatures used when rust-inspect cannot inspect sysroot crates.
///
/// rust-inspect intentionally indexes Cargo workspace crates and path/registry dependencies; sysroot crates such as
/// `std` are not always available as ordinary metadata roots. Keep this table to stable std APIs whose signatures are
/// part of Rust's public contract and whose result shapes matter for Incan source typing.
pub const METADATA_FREE_FUNCTION_SIGNATURE_RULES: &[MetadataFreeFunctionSignatureRule] = &[
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::metadata",
        params: &[MetadataFreeFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::Metadata>",
        is_async: false,
        is_unsafe: false,
    },
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::symlink_metadata",
        params: &[MetadataFreeFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::Metadata>",
        is_async: false,
        is_unsafe: false,
    },
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::read",
        params: &[MetadataFreeFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<Vec<u8>>",
        is_async: false,
        is_unsafe: false,
    },
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::read_dir",
        params: &[MetadataFreeFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<std::fs::ReadDir>",
        is_async: false,
        is_unsafe: false,
    },
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::read_to_string",
        params: &[MetadataFreeFunctionParamRule {
            name: Some("path"),
            type_display: "impl AsRef<std::path::Path>",
        }],
        return_type: "std::io::Result<String>",
        is_async: false,
        is_unsafe: false,
    },
    MetadataFreeFunctionSignatureRule {
        path: "std::fs::write",
        params: &[
            MetadataFreeFunctionParamRule {
                name: Some("path"),
                type_display: "impl AsRef<std::path::Path>",
            },
            MetadataFreeFunctionParamRule {
                name: Some("contents"),
                type_display: "impl AsRef<[u8]>",
            },
        ],
        return_type: "std::io::Result<()>",
        is_async: false,
        is_unsafe: false,
    },
];

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

/// Return conservative callable metadata for Rust free functions whose signatures are stable but unavailable through
/// rust-inspect's Cargo-workspace view.
#[must_use]
pub fn metadata_free_function_signature(rust_path: &str) -> Option<RustFunctionSig> {
    let rule = METADATA_FREE_FUNCTION_SIGNATURE_RULES
        .iter()
        .find(|rule| rule.path == rust_path)?;
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

/// Fallback for a parsed Rust type name after structured primitives, wrappers, and path resolution have failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustTypeShapePathFallback {
    /// Leave unresolved names as unknown. Use this when an extractor has an authoritative name resolver.
    Unknown,
    /// Preserve unresolved names as Rust paths. Use this for generated Rust surfaces that already canonicalized text.
    RustPath,
}

/// Parse a Rust type display into the shared structural shape used by Rust interop consumers.
///
/// Callers own path resolution because HIR extraction can resolve source-relative names, while generated Rust fallback
/// surfaces have already normalized module paths textually. Keeping the parser here prevents those two routes from
/// reimplementing primitive, wrapper, tuple, reference, and byte-buffer classification independently.
#[must_use]
pub fn parse_rust_type_shape_text<F>(
    text: &str,
    mut resolve_path: F,
    fallback: RustTypeShapePathFallback,
) -> RustTypeShape
where
    F: FnMut(&str) -> Option<String>,
{
    parse_rust_type_shape_text_inner(text, &mut resolve_path, fallback)
}

/// Parse normalized Rust type-display text recursively while preserving the caller's path-resolution policy.
fn parse_rust_type_shape_text_inner<F>(
    text: &str,
    resolve_path: &mut F,
    fallback: RustTypeShapePathFallback,
) -> RustTypeShape
where
    F: FnMut(&str) -> Option<String>,
{
    let text = strip_rust_borrow_lifetimes(text).trim().replace(' ', "");
    if text.is_empty() {
        return RustTypeShape::Unknown;
    }
    match text.as_str() {
        "bool" => return RustTypeShape::Bool,
        "f32" | "f64" => return RustTypeShape::Float,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => {
            return RustTypeShape::Int;
        }
        "str" | "String" | "std::string::String" | "alloc::string::String" => return RustTypeShape::Str,
        "()" => return RustTypeShape::Unit,
        "[u8]" => return RustTypeShape::Bytes,
        _ => {}
    }

    if let Some(inner) = text.strip_prefix('&') {
        let inner = inner.strip_prefix("mut").unwrap_or(inner).trim();
        return RustTypeShape::Ref(Box::new(parse_rust_type_shape_text_inner(
            inner,
            resolve_path,
            fallback,
        )));
    }

    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        if inner.is_empty() {
            return RustTypeShape::Unit;
        }
        return RustTypeShape::Tuple(
            split_top_level_rust_args(inner)
                .into_iter()
                .map(|arg| parse_rust_type_shape_text_inner(arg, resolve_path, fallback))
                .collect(),
        );
    }

    if let Some(start) = text.find('<')
        && text.ends_with('>')
    {
        let raw_base = &text[..start];
        let base = resolve_path(raw_base).unwrap_or_else(|| raw_base.to_string());
        let inner = &text[start + 1..text.len() - 1];
        let args: Vec<RustTypeShape> = split_top_level_rust_args(inner)
            .into_iter()
            .map(|arg| parse_rust_type_shape_text_inner(arg, resolve_path, fallback))
            .collect();
        match base.as_str() {
            "Option" | "std::option::Option" | "core::option::Option" => {
                return RustTypeShape::Option(Box::new(args.into_iter().next().unwrap_or(RustTypeShape::Unknown)));
            }
            "Result" | "std::result::Result" | "core::result::Result" => {
                let mut it = args.into_iter();
                return RustTypeShape::Result(
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                    Box::new(it.next().unwrap_or(RustTypeShape::Unknown)),
                );
            }
            "Vec" | "std::vec::Vec" | "alloc::vec::Vec"
                if matches!(args.first(), Some(RustTypeShape::Int)) && text.ends_with("<u8>") =>
            {
                return RustTypeShape::Bytes;
            }
            _ => {}
        }
        return RustTypeShape::RustPath { path: base, args };
    }

    if let Some(path) = resolve_path(text.as_str()) {
        return RustTypeShape::RustPath { path, args: Vec::new() };
    }

    if rust_type_shape_text_looks_like_type_param(text.as_str()) {
        return RustTypeShape::TypeParam(text);
    }

    match fallback {
        RustTypeShapePathFallback::Unknown => RustTypeShape::Unknown,
        RustTypeShapePathFallback::RustPath => RustTypeShape::RustPath {
            path: text,
            args: Vec::new(),
        },
    }
}

/// Return whether unresolved type text has the shape of a Rust generic type parameter.
fn rust_type_shape_text_looks_like_type_param(text: &str) -> bool {
    !text.is_empty()
        && !text.contains("::")
        && !text.contains(['<', '>', '(', ')', '[', ']', '&', ' '])
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text.chars().next().is_some_and(|ch| ch.is_ascii_uppercase())
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

/// Return whether `display` is a Rust callable bound display such as `impl FnMut(&mut T)`.
#[must_use]
pub fn rust_display_is_callable_bound(display: &str) -> bool {
    let mut display = display.trim();
    if let Some(rest) = display.strip_prefix("impl")
        && rust_source_keyword_boundary(display, "impl".len())
    {
        display = rest.trim_start();
    } else if let Some(rest) = display.strip_prefix("dyn")
        && rust_source_keyword_boundary(display, "dyn".len())
    {
        display = rest.trim_start();
    }
    split_top_level_rust_bounds(display)
        .into_iter()
        .any(rust_callable_bound_has_signature)
}

/// Return a source function's callable `Fn*` bound for a generic parameter, if one is declared.
#[must_use]
pub fn rust_source_callable_bound_for_type_param<F>(
    function_source: &str,
    type_param: &str,
    mut normalize_type: F,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let type_param = rust_simple_type_param_name(type_param)?;
    let header = rust_source_function_header(function_source)?;
    if let Some(generic_params) = header.generic_params {
        for generic in split_top_level_rust_args(generic_params) {
            let Some((name, bounds)) = generic.split_once(':') else {
                continue;
            };
            if name.trim() == type_param {
                for bound in split_top_level_rust_bounds(bounds) {
                    if let Some(display) = rust_source_callable_bound_display(bound, &mut normalize_type) {
                        return Some(display);
                    }
                }
            }
        }
    }

    let where_idx = find_top_level_rust_keyword(header.tail, "where")?;
    for predicate in split_top_level_rust_args(&header.tail[where_idx + "where".len()..]) {
        let Some((name, bounds)) = predicate.split_once(':') else {
            continue;
        };
        if name.trim() == type_param {
            for bound in split_top_level_rust_bounds(bounds) {
                if let Some(display) = rust_source_callable_bound_display(bound, &mut normalize_type) {
                    return Some(display);
                }
            }
        }
    }
    None
}

struct RustSourceFunctionHeader<'a> {
    generic_params: Option<&'a str>,
    tail: &'a str,
}

/// Recover the generic parameter list and post-parameter header tail from a Rust function item source string.
fn rust_source_function_header(function_source: &str) -> Option<RustSourceFunctionHeader<'_>> {
    for (fn_idx, _) in function_source.match_indices("fn") {
        if !rust_source_keyword_at(function_source, fn_idx, "fn") {
            continue;
        }
        let mut idx = skip_rust_source_whitespace(function_source, fn_idx + "fn".len());
        if let Some(rest) = function_source.get(idx..).and_then(|rest| rest.strip_prefix("r#")) {
            idx = function_source.len() - rest.len();
        }
        let Some(after_name) = skip_rust_source_ident(function_source, idx) else {
            continue;
        };
        idx = after_name;
        idx = skip_rust_source_whitespace(function_source, idx);
        let Some(rest) = function_source.get(idx..) else {
            continue;
        };
        let generic_params = if rest.starts_with('<') {
            let Some(generic_end) = matching_rust_angle_end(function_source, idx) else {
                continue;
            };
            let params = &function_source[idx + 1..generic_end];
            idx = skip_rust_source_whitespace(function_source, generic_end + 1);
            Some(params)
        } else {
            None
        };
        let Some(rest) = function_source.get(idx..) else {
            continue;
        };
        if !rest.starts_with('(') {
            continue;
        }
        let Some(params_end) = matching_rust_paren_end(function_source, idx) else {
            continue;
        };
        let tail_start = params_end + 1;
        let Some(tail) = function_source.get(tail_start..) else {
            continue;
        };
        let tail_end = find_top_level_rust_header_end(tail).unwrap_or(tail.len());
        return Some(RustSourceFunctionHeader {
            generic_params,
            tail: &tail[..tail_end],
        });
    }
    None
}

/// Return a single uppercase generic type-parameter identifier when `text` has no path or compound type syntax.
fn rust_simple_type_param_name(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    (!trimmed.is_empty()
        && !trimmed.contains("::")
        && !trimmed.contains(['<', '>', '(', ')', '[', ']', '&', ' '])
        && trimmed.chars().all(rust_source_ident_char)
        && trimmed.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()))
    .then_some(trimmed)
}

/// Return whether one top-level bound starts with a callable trait and a parenthesized argument list.
fn rust_callable_bound_has_signature(bound: &str) -> bool {
    let Some((_, after_name)) = rust_callable_bound_name_and_tail(bound) else {
        return false;
    };
    after_name.starts_with('(') && matching_rust_paren_end(after_name, 0).is_some()
}

/// Normalize one source `Fn*` bound into the stable `impl Fn*(...)` display form used by Rust metadata.
fn rust_source_callable_bound_display<F>(bound: &str, mut normalize_type: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let (name, after_name) = rust_callable_bound_name_and_tail(bound)?;
    if !after_name.starts_with('(') {
        return None;
    }
    let args_end = matching_rust_paren_end(after_name, 0)?;
    let args_inner = &after_name[1..args_end];
    let mut args = Vec::new();
    for arg in split_top_level_rust_args(args_inner) {
        args.push(normalize_type(arg)?);
    }
    let after_args = after_name[args_end + 1..].trim_start();
    let ret = if let Some(ret) = after_args.strip_prefix("->") {
        Some(normalize_type(
            split_top_level_rust_bounds(ret).first().copied().unwrap_or(ret),
        )?)
    } else {
        None
    };
    Some(match ret {
        Some(ret) => format!("impl {name}({}) -> {ret}", args.join(", ")),
        None => format!("impl {name}({})", args.join(", ")),
    })
}

/// Split a Rust callable bound into its unqualified callable trait name and the remaining signature text.
fn rust_callable_bound_name_and_tail(bound: &str) -> Option<(&'static str, &str)> {
    let bound = bound.trim();
    for name in ["FnOnce", "FnMut", "Fn"] {
        if let Some(rest) = bound.strip_prefix(name) {
            return Some((name, rest.trim_start()));
        }
        for prefix in ["std::ops::", "core::ops::"] {
            if let Some(rest) = bound.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(name)) {
                return Some((name, rest.trim_start()));
            }
        }
    }
    None
}

/// Split Rust trait bounds joined with top-level `+` without splitting inside generic or function syntax.
fn split_top_level_rust_bounds(text: &str) -> Vec<&str> {
    let mut bounds = Vec::new();
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
            '+' if angle == 0 && paren == 0 && bracket == 0 => {
                let bound = text[start..idx].trim();
                if !bound.is_empty() {
                    bounds.push(bound);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        bounds.push(tail);
    }
    bounds
}

/// Return the matching `>` for a generic parameter list, ignoring arrows inside callable return types.
fn matching_rust_angle_end(text: &str, open_idx: usize) -> Option<usize> {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text[open_idx..].char_indices() {
        match ch {
            '<' if paren == 0 && bracket == 0 => angle += 1,
            '>' if paren == 0 && bracket == 0 && previous_rust_source_char(text, open_idx + idx) != Some('-') => {
                angle = angle.checked_sub(1)?;
                if angle == 0 {
                    return Some(open_idx + idx);
                }
            }
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Return the matching `)` for a parenthesized Rust source region.
fn matching_rust_paren_end(text: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text[open_idx..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open_idx + idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the top-level `{` or `;` that ends a Rust function header.
fn find_top_level_rust_header_end(text: &str) -> Option<usize> {
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
            '{' | ';' if angle == 0 && paren == 0 && bracket == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Find a keyword token outside nested generic, function, and slice delimiters.
fn find_top_level_rust_keyword(text: &str, keyword: &str) -> Option<usize> {
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
            _ if angle == 0
                && paren == 0
                && bracket == 0
                && text[idx..].starts_with(keyword)
                && rust_source_keyword_at(text, idx, keyword) =>
            {
                return Some(idx);
            }
            _ => {}
        }
    }
    None
}

/// Skip Rust source whitespace starting at `idx`.
fn skip_rust_source_whitespace(text: &str, mut idx: usize) -> usize {
    while let Some(ch) = text.get(idx..).and_then(|rest| rest.chars().next()) {
        if !ch.is_whitespace() {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

/// Skip a simple Rust identifier starting at `idx`.
fn skip_rust_source_ident(text: &str, mut idx: usize) -> Option<usize> {
    let mut saw_ident = false;
    while let Some(ch) = text.get(idx..).and_then(|rest| rest.chars().next()) {
        if !rust_source_ident_char(ch) {
            break;
        }
        saw_ident = true;
        idx += ch.len_utf8();
    }
    saw_ident.then_some(idx)
}

/// Return whether `keyword` starts at `idx` with identifier-token boundaries on both sides.
fn rust_source_keyword_at(text: &str, idx: usize, keyword: &str) -> bool {
    text.get(idx..).is_some_and(|rest| rest.starts_with(keyword))
        && previous_rust_source_char(text, idx).is_none_or(|ch| !rust_source_ident_char(ch))
        && rust_source_keyword_boundary(text, idx + keyword.len())
}

/// Return whether `idx` is at an identifier boundary in Rust source text.
fn rust_source_keyword_boundary(text: &str, idx: usize) -> bool {
    text.get(idx..)
        .and_then(|rest| rest.chars().next())
        .is_none_or(|ch| !rust_source_ident_char(ch))
}

/// Return whether `ch` belongs to the simple ASCII identifier vocabulary this source scanner accepts.
fn rust_source_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// Return the source character immediately before `idx`.
fn previous_rust_source_char(text: &str, idx: usize) -> Option<char> {
    text.get(..idx).and_then(|prefix| prefix.chars().next_back())
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
    /// Completeness of the recorded type surface.
    #[serde(default, skip_serializing_if = "RustTypeMetadataCompleteness::is_complete")]
    pub metadata_completeness: RustTypeMetadataCompleteness,
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

/// Whether Rust type metadata came from a full HIR extraction or from a partial syntax fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RustTypeMetadataCompleteness {
    /// Full rust-inspect HIR metadata, including methods and proven trait impls when available.
    #[default]
    Complete,
    /// Syntax-only metadata from generated source, limited to public fields and enum variants.
    FieldsAndVariantsOnly,
}

impl RustTypeMetadataCompleteness {
    /// Return whether this metadata can be treated as a full rust-inspect type surface.
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Return whether this metadata can prove inherent methods.
    pub const fn has_methods(self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Return whether this metadata can prove trait implementations.
    pub const fn has_trait_impls(self) -> bool {
        matches!(self, Self::Complete)
    }
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
                metadata_completeness: Default::default(),
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

    #[test]
    fn parse_rust_type_shape_text_classifies_shared_structural_shapes() {
        let shape = parse_rust_type_shape_text(
            "&'static Result<Vec<u8>, demo::Error>",
            |path| (path == "demo::Error").then(|| "demo::Error".to_string()),
            RustTypeShapePathFallback::Unknown,
        );

        assert_eq!(
            shape,
            RustTypeShape::Ref(Box::new(RustTypeShape::Result(
                Box::new(RustTypeShape::Bytes),
                Box::new(RustTypeShape::RustPath {
                    path: "demo::Error".to_string(),
                    args: Vec::new(),
                }),
            )))
        );
    }

    #[test]
    fn parse_rust_type_shape_text_keeps_source_and_generated_fallbacks_distinct() {
        assert_eq!(
            parse_rust_type_shape_text("missing_type", |_| None, RustTypeShapePathFallback::Unknown),
            RustTypeShape::Unknown,
        );
        assert_eq!(
            parse_rust_type_shape_text("missing_type", |_| None, RustTypeShapePathFallback::RustPath),
            RustTypeShape::RustPath {
                path: "missing_type".to_string(),
                args: Vec::new(),
            },
        );
        assert_eq!(
            parse_rust_type_shape_text("T", |_| None, RustTypeShapePathFallback::RustPath),
            RustTypeShape::TypeParam("T".to_string()),
        );
    }

    fn normalize_probe_type(text: &str) -> Option<String> {
        let normalized = text.trim().replace(' ', "");
        if let Some(inner) = normalized.strip_prefix("&mut") {
            return normalize_probe_type(inner).map(|inner| format!("&mut {inner}"));
        }
        if let Some(inner) = normalized.strip_prefix('&') {
            return normalize_probe_type(inner).map(|inner| format!("&{inner}"));
        }
        Some(match normalized.as_str() {
            "Data" => "source_dep::audio::Data".to_string(),
            "OutputCallbackInfo" => "source_dep::audio::OutputCallbackInfo".to_string(),
            other => other.to_string(),
        })
    }

    #[test]
    fn rust_source_callable_bound_for_type_param_reads_inline_generic_bounds() {
        let source = r#"
pub fn run_inline<D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static>(callback: D) {
    let _ = callback;
}
"#;

        assert_eq!(
            rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
            Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
        );
    }

    #[test]
    fn rust_source_callable_bound_for_type_param_reads_where_bounds() {
        let source = r#"
pub fn run_where<D, E>(callback: D, error: E)
where
    D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
    E: FnMut(String),
{
    let _ = callback;
    let _ = error;
}
"#;

        assert_eq!(
            rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
            Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
        );
        assert_eq!(
            rust_source_callable_bound_for_type_param(source, "E", normalize_probe_type).as_deref(),
            Some("impl FnMut(String)")
        );
    }

    #[test]
    fn rust_source_callable_bound_for_type_param_accepts_qualified_fn_traits() {
        let source = r#"
pub fn run_qualified<D: std::ops::FnOnce(Data) -> OutputCallbackInfo>(callback: D);
"#;

        assert_eq!(
            rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
            Some("impl FnOnce(source_dep::audio::Data) -> source_dep::audio::OutputCallbackInfo")
        );
    }

    #[test]
    fn rust_source_callable_bound_for_type_param_skips_incidental_fn_tokens() {
        let source = r#"
const SAMPLE: &str = "fn ";

pub fn run_inline<D: FnMut(&mut Data, &OutputCallbackInfo)>(callback: D) {
    let _ = callback;
}
"#;

        assert_eq!(
            rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
            Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
        );
    }

    #[test]
    fn rust_display_is_callable_bound_detects_only_callable_bound_displays() {
        assert!(rust_display_is_callable_bound(
            "impl FnMut(&mut demo::Data, &demo::Info) + Send"
        ));
        assert!(rust_display_is_callable_bound("dyn std::ops::Fn(String)"));
        assert!(!rust_display_is_callable_bound("impl Buf"));
        assert!(!rust_display_is_callable_bound("implBuf"));
    }

    #[test]
    fn metadata_free_function_signature_describes_stable_std_result_surfaces() {
        let signature = metadata_free_function_signature("std::fs::metadata")
            .expect("std::fs::metadata should have a metadata-free signature");

        assert_eq!(signature.return_type, "std::io::Result<std::fs::Metadata>");
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].name.as_deref(), Some("path"));
        assert_eq!(signature.params[0].type_display, "impl AsRef<std::path::Path>");
        assert!(!signature.is_async);
        assert!(!signature.is_unsafe);
        assert!(metadata_free_function_signature("std::fs::remove_file").is_none());
    }
}
