//! Incan-native view of Rust items extracted from a Cargo workspace (RFC 041).
//!
//! These types are intentionally free of rust-analyzer or compiler-internal IDs so the typechecker and lowering stages
//! can consume stable, snapshot-friendly metadata.

/// Whether an item is visible across crate boundaries for ordinary `pub` Rust APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RustVisibility {
    /// `pub` — visible outside the defining crate (subject to future path-specific rules).
    Public,
    /// Anything else (`pub(crate)`, `pub(super)`, private, etc.).
    Restricted,
}

/// Top-level classification for a resolved Rust path.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustItemMetadata {
    /// Canonical path as Incan already models it, e.g. `std::collections::HashMap`.
    pub canonical_path: String,
    pub visibility: RustVisibility,
    pub kind: RustItemKind,
}

/// A single parameter in a Rust function signature (display strings only for Phase 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustParam {
    /// Parameter name when rust-analyzer can recover it from the HIR body.
    pub name: Option<String>,
    /// Pretty-printed type suitable for diagnostics and future coercion work.
    pub type_display: String,
}

/// Callable signature extracted from rust-analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustFunctionSig {
    pub params: Vec<RustParam>,
    pub return_type: String,
    pub is_async: bool,
    pub is_unsafe: bool,
}

/// An inherent or trait method surfaced on a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustMethodSig {
    pub name: String,
    pub signature: RustFunctionSig,
}

/// Structured Rust type information used by Incan interop consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustVariantInfo {
    /// Variant name as it appears in Rust.
    pub name: String,
    /// Positional payload field shapes in declaration order.
    pub fields: Vec<RustTypeShape>,
}

/// Method, field, and variant surface for a Rust ADT or builtin type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTypeInfo {
    /// Public inherent methods and associated functions.
    pub methods: Vec<RustMethodSig>,
    /// Public fields for struct/union-like types.
    pub fields: Vec<RustFieldInfo>,
    /// Enum variants when the type is an enum; empty for non-enums.
    pub variants: Vec<RustVariantInfo>,
}

/// One exported name inside a module (lightweight summary for namespace resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustModuleChild {
    pub name: String,
    pub kind_hint: RustModuleChildKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RustModuleChildKind {
    Module,
    Type,
    Function,
    Constant,
    Trait,
    Other,
}

/// Children visible in a module scope (public items when resolved from outside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustModuleInfo {
    pub children: Vec<RustModuleChild>,
}

/// Associated items declared on a trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustTraitAssoc {
    Function { name: String, signature: RustFunctionSig },
    TypeAlias { name: String },
    Constant { name: String, type_display: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTraitInfo {
    pub items: Vec<RustTraitAssoc>,
}
