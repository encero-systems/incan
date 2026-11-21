//! Checked type facts and the shape validation used at the record boundary.

use crate::compiler_records::{CanonicalSymbolId, valid_canonical_symbol_id};
use serde::{Deserialize, Serialize};

/// Every numeric registry identity carried by a checked primitive type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckedInputAccess {
    Inferred,
    Shared,
    Exclusive,
    Owned,
    InferredShared,
}

/// The source-level position shape of one semantic callable parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckedCallableParamKind {
    Normal,
    RestPositional,
    RestKeyword,
}

/// Dependency-owned builtin runtime storage admitted by checked identity proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckedRuntimeBuiltin {
    ValidationError,
}

/// Every callable-parameter fact retained in a semantic function type.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
crate::incan_debug::incan_debug!(CheckedCallableParam {
    name,
    ty,
    kind,
    has_default,
    is_partial_preset,
    is_mut,
    native_owned,
    input_access
});

/// Recursive sequence arity is part of the record grammar. Malformed arities remain representable for validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Validate retained type identities, nested type lists and every exactly-one projection.
pub fn valid_checked_type(ty: &CheckedType) -> bool {
    match ty {
        CheckedType::Generic(base, args) => !base.is_empty() && args.iter().all(valid_checked_type),
        CheckedType::Function(params, result) => {
            params.iter().all(|p| valid_checked_type(&p.ty)) && valid_single_type(result)
        }
        CheckedType::TypeToken(types)
        | CheckedType::Ref(types)
        | CheckedType::RefMut(types)
        | CheckedType::NativeArray(types, _)
        | CheckedType::NativePointer(_, types) => valid_single_type(types),
        CheckedType::Tuple(types) => types.iter().all(valid_checked_type),
        CheckedType::Named(name)
        | CheckedType::TypeVar(name)
        | CheckedType::RustInteropPath(name)
        | CheckedType::Unsupported(name)
        | CheckedType::NativeTraitObject(name, _, _) => !name.is_empty(),
        CheckedType::Trait(id) => valid_canonical_symbol_id(id),
        CheckedType::Never
        | CheckedType::Primitive(_)
        | CheckedType::Decimal(_, _)
        | CheckedType::SelfType
        | CheckedType::NativeBuiltin(_)
        | CheckedType::Infer
        | CheckedType::Unknown
        | CheckedType::NativeCChar => true,
    }
}

/// A boxed native type projects to one child in the existing wire schema.
fn valid_single_type(types: &[CheckedType]) -> bool {
    matches!(types, [child] if valid_checked_type(child))
}
