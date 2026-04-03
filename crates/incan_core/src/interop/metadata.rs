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

/// A public field surfaced on a Rust struct/union-like type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustFieldInfo {
    pub name: String,
    pub type_display: String,
}

/// Method and associated-fn surface for a Rust ADT or builtin type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTypeInfo {
    pub methods: Vec<RustMethodSig>,
    pub fields: Vec<RustFieldInfo>,
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
