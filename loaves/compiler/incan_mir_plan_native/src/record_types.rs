//! The compiler record types a plan names: a checked carrier type and a canonical source-trait identity.
//!
//! These mirror `internal_models.checked_types` and `internal_models.compiler_records` at the reference commit
//! `f0fa3b405d`, alternative for alternative, until the Rust record models (branch `rust/records`) publish their
//! types; this module then re-exports theirs, so a plan never owns a second copy of a record type.

use serde::{Deserialize, Serialize};

/// Every numeric registry identity carried by a checked primitive type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedNumericType {
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    ISize,
    USize,
    Bool,
}

/// Primitive and primitive-like checked types with compiler-owned semantics.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedPrimitiveType {
    Int,
    Float,
    Numeric(CheckedNumericType),
    Bool,
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    Unit,
}

/// Checked callable input access, separate from binding mutability and native ownership.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedInputAccess {
    Inferred,
    Shared,
    Exclusive,
    Owned,
    InferredShared,
}

/// The source-level position shape of one semantic callable parameter.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedCallableParamKind {
    Normal,
    RestPositional,
    RestKeyword,
}

/// Dependency-owned builtin runtime storage admitted by checked identity proof.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedRuntimeBuiltin {
    ValidationError,
}

/// Every callable-parameter fact retained in a semantic function type.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckedCallableParam {
    pub name: Option<String>,
    pub ty: CheckedType,
    pub kind: CheckedCallableParamKind,
    pub has_default: bool,
    pub is_partial_preset: bool,
    pub is_mut: bool,
    pub native_owned: bool,
    pub input_access: CheckedInputAccess,
}

/// A checked type; recursive sequence arity is part of the record grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckedType {
    Never,
    Primitive(CheckedPrimitiveType),
    Named(String),
    Generic(String, Vec<CheckedType>),
    Decimal(u8, u8),
    Function(Vec<CheckedCallableParam>, Vec<CheckedType>),
    TypeToken(Vec<CheckedType>),
    Tuple(Vec<CheckedType>),
    TypeVar(String),
    SelfType,
    Ref(Vec<CheckedType>),
    RefMut(Vec<CheckedType>),
    RustInteropPath(String),
    NativeBuiltin(CheckedRuntimeBuiltin),
    Unsupported(String),
    Infer,
    Unknown,
    NativeArray(Vec<CheckedType>, u64),
    NativeTraitObject(String, bool, bool),
    NativePointer(bool, Vec<CheckedType>),
    NativeCChar,
    Trait(CanonicalSymbolId),
}

/// How a checked binding is resolved; equal spellings in different namespaces remain different identities.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolNamespace {
    OrdinaryLexical,
    Member,
    ModulePath,
}

/// The selected package name and immutable version owning one published declaration.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageIdentity {
    pub name: String,
    pub version: String,
}

/// The semantic owner of a declaration, independent of the source spelling used to reach it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolOrigin {
    Module(Vec<String>),
    Package(PackageIdentity, Vec<String>),
    RustCrate(Vec<String>),
    Builtin,
}

/// Every declaration category carried by the native `SemanticSourceTargetKind` source enum.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeclarationKind {
    Function,
    Model,
    Class,
    Newtype,
    Rusttype,
    Enum,
    TypeAlias,
    Partial,
    Variant,
    Trait,
    Capability,
    Field,
    Method,
    Property,
    Const,
    Static,
    Local,
    Parameter,
    Receiver,
    GenericBinder,
    Module,
    RustItem,
    Builtin,
    Other(String),
}

/// An owned source file, inclusive-exclusive byte range and optional 32-byte expansion identity.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HirSourceSpan {
    pub file: String,
    pub start: i64,
    pub end: i64,
    pub expansion: Option<Vec<i64>>,
}

/// One compilation's checked declaration identity, retaining its exact source and expansion origin.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanonicalSymbolId {
    pub namespace: SymbolNamespace,
    pub origin: SymbolOrigin,
    pub declaration_name: String,
    pub kind: DeclarationKind,
    pub scope_discriminant: Option<i64>,
    pub declaration_span: HirSourceSpan,
}

crate::structural_debug!(
    CheckedCallableParam,
    "CheckedCallableParam",
    name,
    ty,
    kind,
    has_default,
    is_partial_preset,
    is_mut,
    native_owned,
    input_access,
);
crate::structural_debug!(PackageIdentity, "PackageIdentity", name, version);
crate::structural_debug!(HirSourceSpan, "HirSourceSpan", file, start, end, expansion);
crate::structural_debug!(
    CanonicalSymbolId,
    "CanonicalSymbolId",
    namespace,
    origin,
    declaration_name,
    kind,
    scope_discriminant,
    declaration_span,
);
