//! IR type definitions
//!
//! These types represent the resolved type information for IR nodes.

use std::fmt;

use super::decl::IrTraitBound;
use incan_core::lang::types::numerics::{self, NumericTypeId};

/// Canonical IR generic name used for anonymous union types.
pub const IR_UNION_TYPE_NAME: &str = "Union";

/// Ownership semantics for a value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ownership {
    /// Owned value (moved or copied)
    #[default]
    Owned,
    /// Immutable borrow (&T)
    Borrowed,
    /// Mutable borrow (&mut T)
    BorrowedMut,
}

/// Mutability of a binding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mutability {
    #[default]
    Immutable,
    Mutable,
}

/// IR type representation
///
/// This is a resolved type that maps directly to Rust types.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IrType {
    // Primitives
    Unit,
    Bool,
    Int,
    Float,
    /// Exact-width numeric type introduced by RFC 009.
    Numeric(NumericTypeId),
    /// Fixed-precision decimal value. Precision/scale are checked by the frontend; Rust emission uses the
    /// toolchain-owned Decimal128 runtime representation.
    Decimal {
        precision: u8,
        scale: u8,
    },
    String,
    Bytes,
    /// &'static str (for compile-time string constants)
    StaticStr,
    /// &'static [u8] (for compile-time byte string constants)
    StaticBytes,
    /// FrozenStr wrapper (deeply immutable `'static` string)
    FrozenStr,
    /// FrozenBytes wrapper (deeply immutable `'static` byte slice)
    FrozenBytes,
    /// &str (borrowed string slice)
    StrRef,

    // Collections
    List(Box<IrType>),
    Dict(Box<IrType>, Box<IrType>),
    Set(Box<IrType>),
    Tuple(Vec<IrType>),

    // Option and Result
    Option(Box<IrType>),
    Result(Box<IrType>, Box<IrType>),

    // User-defined types
    Struct(String),
    Enum(String),
    Trait(String),

    /// Represent a named generic instantiation (e.g. `FrozenList<i64>` or `Box<T>`).
    ///
    /// ## Notes
    /// - This is used for generic types that are not encoded as dedicated IR variants.
    /// - Codegen emits this as `Name<Arg0, Arg1, ...>`.
    NamedGeneric(String, Vec<IrType>),

    /// Zero-sized value-level marker for an Incan source type, emitted as `incan_stdlib::reflection::TypeToken<T>`.
    TypeToken(Box<IrType>),

    /// Exact Rust type display carried from interop metadata.
    ///
    /// This is reserved for Rust boundary shapes that the Incan type model cannot faithfully spell yet, such as
    /// borrowed slices (`&[T]`) in closure parameters.
    RustDisplay(String),

    /// Anonymous union wrapper owned by a public dependency crate.
    ///
    /// The inner type keeps the semantic union members available to assignment, argument, collection, and option
    /// conversion planning while emission uses `library::__IncanUnion...` instead of re-owning the wrapper locally.
    ExternalUnion {
        library: String,
        union: Box<IrType>,
    },

    /// Opaque trait return type emitted as Rust `impl Trait`, RFC 042.
    ImplTrait(IrTraitBound),

    // Function type
    Function {
        params: Vec<IrType>,
        ret: Box<IrType>,
    },

    // Generic type parameter (for generic functions)
    Generic(String),

    // Self type (in trait/impl contexts)
    SelfType,

    // Reference types (explicit borrows)
    Ref(Box<IrType>),
    RefMut(Box<IrType>),

    // Unknown (for error recovery)
    #[default]
    Unknown,
}

impl IrType {
    /// Remove one owning provider's qualification from nominal paths while preserving the complete type shape.
    ///
    /// Public-library expressions retain qualified names in consumer IR. Provider-owned signature and anonymous-union
    /// metadata use names local to the compiled provider, so only the matching provider prefix may be removed when
    /// comparing or hashing those boundary types.
    pub(crate) fn provider_localized(&self, library: &str) -> Self {
        let prefix = format!("{library}::");
        let local_name = |name: &str| name.strip_prefix(&prefix).unwrap_or(name).to_string();
        match self {
            Self::Struct(name) => Self::Struct(local_name(name)),
            Self::Enum(name) => Self::Enum(local_name(name)),
            Self::Trait(name) => Self::Trait(local_name(name)),
            Self::NamedGeneric(name, args) => Self::NamedGeneric(
                local_name(name),
                args.iter().map(|arg| arg.provider_localized(library)).collect(),
            ),
            Self::List(inner) => Self::List(Box::new(inner.provider_localized(library))),
            Self::Dict(key, value) => Self::Dict(
                Box::new(key.provider_localized(library)),
                Box::new(value.provider_localized(library)),
            ),
            Self::Set(inner) => Self::Set(Box::new(inner.provider_localized(library))),
            Self::Tuple(items) => Self::Tuple(items.iter().map(|item| item.provider_localized(library)).collect()),
            Self::Option(inner) => Self::Option(Box::new(inner.provider_localized(library))),
            Self::Result(ok, err) => Self::Result(
                Box::new(ok.provider_localized(library)),
                Box::new(err.provider_localized(library)),
            ),
            Self::Function { params, ret } => Self::Function {
                params: params.iter().map(|param| param.provider_localized(library)).collect(),
                ret: Box::new(ret.provider_localized(library)),
            },
            Self::Ref(inner) => Self::Ref(Box::new(inner.provider_localized(library))),
            Self::RefMut(inner) => Self::RefMut(Box::new(inner.provider_localized(library))),
            Self::TypeToken(inner) => Self::TypeToken(Box::new(inner.provider_localized(library))),
            Self::ExternalUnion { library: owner, union } if owner == library => Self::ExternalUnion {
                library: owner.clone(),
                union: Box::new(union.provider_localized(library)),
            },
            Self::ExternalUnion { .. } => self.clone(),
            other => other.clone(),
        }
    }

    /// Return whether this type mentions an unbound generic type parameter.
    pub fn contains_generic_parameter(&self) -> bool {
        match self {
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => inner.contains_generic_parameter(),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                key.contains_generic_parameter() || value.contains_generic_parameter()
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                items.iter().any(IrType::contains_generic_parameter)
            }
            IrType::TypeToken(inner) => inner.contains_generic_parameter(),
            IrType::Function { params, ret } => {
                params.iter().any(IrType::contains_generic_parameter) || ret.contains_generic_parameter()
            }
            IrType::ExternalUnion { union, .. } => union.contains_generic_parameter(),
            IrType::Generic(_) => true,
            IrType::RustDisplay(_) => false,
            _ => false,
        }
    }

    /// Check if this type is Copy in Rust
    ///
    /// Returns true for primitive types (unit, bool, int, float) and string references
    /// (`&str`, `&'static str`) since references are Copy.
    pub fn is_copy(&self) -> bool {
        match self {
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Numeric(_)
            | IrType::Decimal { .. }
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
            | IrType::Ref(_)
            | IrType::RefMut(_) => true,
            IrType::Tuple(items) => items.iter().all(IrType::is_copy),
            IrType::TypeToken(_) => true,
            IrType::Option(inner) => inner.is_copy(),
            IrType::Result(ok, err) => ok.is_copy() && err.is_copy(),
            IrType::ExternalUnion { .. } => false,
            _ => false,
        }
    }

    /// Check if this type is a reference
    pub fn is_ref(&self) -> bool {
        matches!(self, IrType::Ref(_) | IrType::RefMut(_))
    }

    /// Return the nominal type constructor name for user-defined or imported nominal types.
    ///
    /// This treats `Foo` and `Foo[T]` as the same nominal family while preserving generic
    /// arguments elsewhere in the IR.
    pub fn nominal_type_name(&self) -> Option<&str> {
        match self {
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) | IrType::NamedGeneric(name, _) => {
                Some(name.as_str())
            }
            _ => None,
        }
    }

    /// Get the Incan-style type name (for reflection/display to users).
    ///
    /// RFC 021: This is used for `FieldInfo.type_name` to show the Incan type, not the Rust representation.
    pub fn incan_name(&self) -> String {
        match self {
            IrType::Unit => incan_core::lang::surface::constructors::as_str(
                incan_core::lang::surface::constructors::ConstructorId::None,
            )
            .to_string(),
            IrType::Bool => "bool".to_string(),
            IrType::Int => "int".to_string(),
            IrType::Float => "float".to_string(),
            IrType::Numeric(id) => numerics::as_str(*id).to_string(),
            IrType::Decimal { precision, scale } => format!("decimal[{precision}, {scale}]"),
            IrType::String => "str".to_string(),
            IrType::Bytes => "bytes".to_string(),
            IrType::StaticStr | IrType::StrRef | IrType::FrozenStr => "str".to_string(),
            IrType::StaticBytes | IrType::FrozenBytes => "bytes".to_string(),
            IrType::List(elem) => format!("list[{}]", elem.incan_name()),
            IrType::Dict(k, v) => format!("dict[{}, {}]", k.incan_name(), v.incan_name()),
            IrType::Set(elem) => format!("set[{}]", elem.incan_name()),
            IrType::Tuple(elems) => {
                let inner: Vec<_> = elems.iter().map(|e| e.incan_name()).collect();
                format!("({})", inner.join(", "))
            }
            IrType::Option(inner) => format!("Option[{}]", inner.incan_name()),
            IrType::Result(ok, err) => format!("Result[{}, {}]", ok.incan_name(), err.incan_name()),
            IrType::Struct(name) => name.clone(),
            IrType::Enum(name) => name.clone(),
            IrType::Trait(name) => name.clone(),
            IrType::RustDisplay(display) => display.clone(),
            IrType::ExternalUnion { union, .. } => union.incan_name(),
            IrType::NamedGeneric(name, args) => {
                let inner: Vec<_> = args.iter().map(|a| a.incan_name()).collect();
                format!("{}[{}]", name, inner.join(", "))
            }
            IrType::TypeToken(inner) => format!("Type[{}]", inner.incan_name()),
            IrType::ImplTrait(bound) => {
                if bound.type_args.is_empty() {
                    bound.trait_path.clone()
                } else {
                    let inner: Vec<_> = bound.type_args.iter().map(|a| a.incan_name()).collect();
                    format!("{}[{}]", bound.trait_path, inner.join(", "))
                }
            }
            IrType::Function { params, ret } => {
                let params: Vec<_> = params.iter().map(|p| p.incan_name()).collect();
                format!("({}) -> {}", params.join(", "), ret.incan_name())
            }
            IrType::Generic(name) => name.clone(),
            IrType::SelfType => "Self".to_string(),
            IrType::Ref(inner) | IrType::RefMut(inner) => inner.incan_name(),
            IrType::Unknown => "_".to_string(),
        }
    }

    /// Get the Rust type name
    pub fn rust_name(&self) -> String {
        match self {
            IrType::Unit => "()".to_string(),
            IrType::Bool => "bool".to_string(),
            IrType::Int => "i64".to_string(),
            IrType::Float => "f64".to_string(),
            IrType::Numeric(id) => numerics::rust_name(*id).to_string(),
            IrType::Decimal { .. } => "incan_stdlib::num::Decimal128".to_string(),
            IrType::String => "String".to_string(),
            IrType::Bytes => "Vec<u8>".to_string(),
            IrType::StaticStr => "&'static str".to_string(),
            IrType::StaticBytes => "&'static [u8]".to_string(),
            IrType::FrozenStr => "FrozenStr".to_string(),
            IrType::FrozenBytes => "FrozenBytes".to_string(),
            IrType::StrRef => "&str".to_string(),
            IrType::List(elem) => format!("Vec<{}>", elem.rust_name()),
            IrType::Dict(k, v) => format!("std::collections::HashMap<{}, {}>", k.rust_name(), v.rust_name()),
            IrType::Set(elem) => format!("std::collections::HashSet<{}>", elem.rust_name()),
            IrType::Tuple(elems) => {
                let inner: Vec<_> = elems.iter().map(|e| e.rust_name()).collect();
                format!("({})", inner.join(", "))
            }
            IrType::Option(inner) => format!("Option<{}>", inner.rust_name()),
            IrType::Result(ok, err) => format!("Result<{}, {}>", ok.rust_name(), err.rust_name()),
            IrType::Struct(name) | IrType::Enum(name) => name.clone(),
            IrType::Trait(name) => format!("dyn {}", name),
            IrType::RustDisplay(display) => display.clone(),
            IrType::ExternalUnion { library, union } => union
                .union_type_name()
                .map(|name| format!("{library}::{name}"))
                .unwrap_or_else(|| union.rust_name()),
            IrType::NamedGeneric(name, _) if name == IR_UNION_TYPE_NAME => {
                self.union_type_name().unwrap_or_else(|| IR_UNION_TYPE_NAME.to_string())
            }
            IrType::NamedGeneric(name, args) => {
                let inner: Vec<_> = args.iter().map(|a| a.rust_name()).collect();
                format!("{}<{}>", name, inner.join(", "))
            }
            IrType::TypeToken(inner) => format!("incan_stdlib::reflection::TypeToken<{}>", inner.rust_name()),
            IrType::ImplTrait(bound) => {
                let args = if bound.type_args.is_empty() {
                    String::new()
                } else {
                    let inner: Vec<_> = bound.type_args.iter().map(|a| a.rust_name()).collect();
                    format!("<{}>", inner.join(", "))
                };
                format!("impl {}{}", bound.trait_path, args)
            }
            IrType::Function { params, ret } => {
                let params: Vec<_> = params.iter().map(|p| p.rust_name()).collect();
                format!("fn({}) -> {}", params.join(", "), ret.rust_name())
            }
            IrType::Generic(name) => name.clone(),
            IrType::SelfType => "Self".to_string(),
            IrType::Ref(inner) => format!("&{}", inner.rust_name()),
            IrType::RefMut(inner) => format!("&mut {}", inner.rust_name()),
            IrType::Unknown => "_".to_string(),
        }
    }
}

impl IrType {
    /// Return the normalized members of an anonymous union type.
    pub fn union_members(&self) -> Option<&[IrType]> {
        match self {
            IrType::NamedGeneric(name, members) if name == IR_UNION_TYPE_NAME => Some(members.as_slice()),
            IrType::ExternalUnion { union, .. } => union.union_members(),
            _ => None,
        }
    }

    /// Return whether this type is an anonymous union type.
    pub fn is_union(&self) -> bool {
        self.union_members().is_some()
    }

    /// Return the deterministic generated Rust type name for an anonymous union shape.
    pub fn union_type_name(&self) -> Option<String> {
        let members = self.union_members()?;
        let key = members.iter().map(IrType::rust_name).collect::<Vec<_>>().join("|");
        Some(format!("__IncanUnion{:016x}", stable_union_hash(key.as_bytes())))
    }

    /// Return the fully qualified Rust pattern path for one anonymous-union variant.
    pub fn union_variant_path(&self, index: usize) -> Option<String> {
        let union_name = self.union_type_name()?;
        let variant = Self::union_variant_name(index);
        Some(match self {
            IrType::ExternalUnion { library, .. } => format!("{library}::{union_name}::{variant}"),
            _ => format!("{union_name}::{variant}"),
        })
    }

    /// Return the variant name for a normalized union member index.
    pub fn union_variant_name(index: usize) -> String {
        format!("V{index}")
    }

    /// Find the union variant index that can hold `member_ty`.
    pub fn union_variant_index_for_member(&self, member_ty: &IrType) -> Option<usize> {
        let members = self.union_members()?;
        let member_ty = match self {
            Self::ExternalUnion { library, .. } => member_ty.provider_localized(library),
            _ => member_ty.clone(),
        };
        members
            .iter()
            .position(|member| union_member_type_matches(member, &member_ty))
    }
}

/// Return whether a concrete value type can inhabit a normalized union member type.
fn union_member_type_matches(member: &IrType, value_ty: &IrType) -> bool {
    member == value_ty
        || matches!(
            (member, value_ty),
            (IrType::String, IrType::StaticStr | IrType::StrRef | IrType::FrozenStr)
        )
}

/// Hash a union member-key into a deterministic generated Rust type suffix.
fn stable_union_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

impl fmt::Display for IrType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rust_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // CATEGORY 1: Simple Types
    // ============================================================================

    #[test]
    fn test_simple_int_rust_name() {
        assert_eq!(IrType::Int.rust_name(), "i64");
    }

    #[test]
    fn test_simple_float_rust_name() {
        assert_eq!(IrType::Float.rust_name(), "f64");
    }

    #[test]
    fn test_simple_string_rust_name() {
        assert_eq!(IrType::String.rust_name(), "String");
    }

    #[test]
    fn test_simple_bool_rust_name() {
        assert_eq!(IrType::Bool.rust_name(), "bool");
    }

    #[test]
    fn test_exact_width_numeric_names() {
        let i32_type = IrType::Numeric(NumericTypeId::I32);
        assert_eq!(i32_type.incan_name(), "i32");
        assert_eq!(i32_type.rust_name(), "i32");

        let byte_type = IrType::Numeric(NumericTypeId::U8);
        assert_eq!(byte_type.incan_name(), "u8");
        assert_eq!(byte_type.rust_name(), "u8");
    }

    #[test]
    fn test_simple_unit_rust_name() {
        assert_eq!(IrType::Unit.rust_name(), "()");
    }

    #[test]
    fn test_simple_static_str_rust_name() {
        assert_eq!(IrType::StaticStr.rust_name(), "&'static str");
    }

    #[test]
    fn test_simple_static_bytes_rust_name() {
        assert_eq!(IrType::StaticBytes.rust_name(), "&'static [u8]");
    }

    // ============================================================================
    // CATEGORY 2: Generic Types
    // ============================================================================

    #[test]
    fn test_generic_list_int() {
        assert_eq!(IrType::List(Box::new(IrType::Int)).rust_name(), "Vec<i64>");
    }

    #[test]
    fn test_generic_dict_string_int() {
        assert_eq!(
            IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)).rust_name(),
            "std::collections::HashMap<String, i64>"
        );
    }

    #[test]
    fn test_generic_set_int() {
        assert_eq!(
            IrType::Set(Box::new(IrType::Int)).rust_name(),
            "std::collections::HashSet<i64>"
        );
    }

    #[test]
    fn test_generic_option_int() {
        assert_eq!(IrType::Option(Box::new(IrType::Int)).rust_name(), "Option<i64>");
    }

    #[test]
    fn test_generic_result_int_string() {
        assert_eq!(
            IrType::Result(Box::new(IrType::Int), Box::new(IrType::String)).rust_name(),
            "Result<i64, String>"
        );
    }

    #[test]
    fn test_generic_nested_list_list_int() {
        let inner = IrType::List(Box::new(IrType::Int));
        let outer = IrType::List(Box::new(inner));
        assert_eq!(outer.rust_name(), "Vec<Vec<i64>>");
    }

    #[test]
    fn test_generic_list_option_string() {
        let opt = IrType::Option(Box::new(IrType::String));
        let list = IrType::List(Box::new(opt));
        assert_eq!(list.rust_name(), "Vec<Option<String>>");
    }

    #[test]
    fn test_generic_dict_string_list_int() {
        let list = IrType::List(Box::new(IrType::Int));
        let dict = IrType::Dict(Box::new(IrType::String), Box::new(list));
        assert_eq!(dict.rust_name(), "std::collections::HashMap<String, Vec<i64>>");
    }

    // ============================================================================
    // CATEGORY 3: Tuple Types
    // ============================================================================

    #[test]
    fn test_tuple_empty() {
        assert_eq!(IrType::Tuple(vec![]).rust_name(), "()");
    }

    #[test]
    fn test_tuple_single_int() {
        assert_eq!(IrType::Tuple(vec![IrType::Int]).rust_name(), "(i64)");
    }

    #[test]
    fn test_tuple_multiple_int_string_bool() {
        assert_eq!(
            IrType::Tuple(vec![IrType::Int, IrType::String, IrType::Bool]).rust_name(),
            "(i64, String, bool)"
        );
    }

    #[test]
    fn test_tuple_nested_option() {
        let opt = IrType::Option(Box::new(IrType::Int));
        assert_eq!(
            IrType::Tuple(vec![opt, IrType::String]).rust_name(),
            "(Option<i64>, String)"
        );
    }

    #[test]
    fn test_tuple_complex_nested() {
        let list = IrType::List(Box::new(IrType::Int));
        let opt = IrType::Option(Box::new(IrType::String));
        assert_eq!(
            IrType::Tuple(vec![list, opt, IrType::Bool]).rust_name(),
            "(Vec<i64>, Option<String>, bool)"
        );
    }

    // ============================================================================
    // CATEGORY 4: Type Helper Methods
    // ============================================================================

    #[test]
    fn test_is_copy_int_true() {
        assert!(IrType::Int.is_copy());
    }

    #[test]
    fn test_is_copy_bool_true() {
        assert!(IrType::Bool.is_copy());
    }

    #[test]
    fn test_is_copy_float_true() {
        assert!(IrType::Float.is_copy());
    }

    #[test]
    fn test_is_copy_unit_true() {
        assert!(IrType::Unit.is_copy());
    }

    #[test]
    fn test_is_copy_string_false() {
        assert!(!IrType::String.is_copy());
    }

    #[test]
    fn test_is_copy_list_false() {
        assert!(!IrType::List(Box::new(IrType::Int)).is_copy());
    }

    #[test]
    fn test_is_copy_dict_false() {
        assert!(!IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)).is_copy());
    }

    #[test]
    fn test_is_copy_option_tracks_inner_type() {
        assert!(IrType::Option(Box::new(IrType::Int)).is_copy());
        assert!(!IrType::Option(Box::new(IrType::String)).is_copy());
    }

    #[test]
    fn test_is_copy_result_tracks_inner_types() {
        assert!(IrType::Result(Box::new(IrType::Int), Box::new(IrType::Bool)).is_copy());
        assert!(!IrType::Result(Box::new(IrType::Int), Box::new(IrType::String)).is_copy());
    }

    #[test]
    fn test_is_copy_tuple_tracks_inner_types() {
        assert!(IrType::Tuple(vec![IrType::Int, IrType::Bool]).is_copy());
        assert!(!IrType::Tuple(vec![IrType::Int, IrType::String]).is_copy());
    }

    #[test]
    fn test_is_ref_ref_true() {
        assert!(IrType::Ref(Box::new(IrType::Int)).is_ref());
    }

    #[test]
    fn test_is_ref_refmut_true() {
        assert!(IrType::RefMut(Box::new(IrType::Int)).is_ref());
    }

    // ============================================================================
    // CATEGORY 5: Reference Types
    // ============================================================================

    #[test]
    fn test_ref_int() {
        assert_eq!(IrType::Ref(Box::new(IrType::Int)).rust_name(), "&i64");
    }

    #[test]
    fn test_refmut_int() {
        assert_eq!(IrType::RefMut(Box::new(IrType::Int)).rust_name(), "&mut i64");
    }

    #[test]
    fn test_ref_complex_type() {
        let list = IrType::List(Box::new(IrType::String));
        assert_eq!(IrType::Ref(Box::new(list)).rust_name(), "&Vec<String>");
    }

    // ============================================================================
    // CATEGORY 6: Edge Cases - Complex Nested Types
    // ============================================================================

    #[test]
    fn test_nested_list_of_list() {
        let inner = IrType::List(Box::new(IrType::Int));
        let outer = IrType::List(Box::new(inner));
        assert_eq!(outer.rust_name(), "Vec<Vec<i64>>");
    }

    #[test]
    fn test_nested_list_of_dict() {
        let dict = IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int));
        let list = IrType::List(Box::new(dict));
        assert_eq!(list.rust_name(), "Vec<std::collections::HashMap<String, i64>>");
    }

    #[test]
    fn test_dict_with_complex_value() {
        let value = IrType::List(Box::new(IrType::Option(Box::new(IrType::String))));
        let dict = IrType::Dict(Box::new(IrType::String), Box::new(value));
        assert_eq!(
            dict.rust_name(),
            "std::collections::HashMap<String, Vec<Option<String>>>"
        );
    }

    #[test]
    fn test_option_of_result() {
        let result = IrType::Result(Box::new(IrType::Int), Box::new(IrType::String));
        let option = IrType::Option(Box::new(result));
        assert_eq!(option.rust_name(), "Option<Result<i64, String>>");
    }

    #[test]
    fn test_result_of_option() {
        let option = IrType::Option(Box::new(IrType::String));
        let result = IrType::Result(Box::new(option), Box::new(IrType::String));
        assert_eq!(result.rust_name(), "Result<Option<String>, String>");
    }

    // ============================================================================
    // CATEGORY 7: Incan Type Names
    // ============================================================================

    #[test]
    fn test_incan_name_list_int() {
        assert_eq!(IrType::List(Box::new(IrType::Int)).incan_name(), "list[int]");
    }

    #[test]
    fn test_incan_name_dict_string_int() {
        assert_eq!(
            IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)).incan_name(),
            "dict[str, int]"
        );
    }

    #[test]
    fn test_incan_name_option_list() {
        let inner = IrType::List(Box::new(IrType::String));
        let opt = IrType::Option(Box::new(inner));
        assert_eq!(opt.incan_name(), "Option[list[str]]");
    }

    #[test]
    fn test_incan_name_tuple() {
        let tuple = IrType::Tuple(vec![IrType::Int, IrType::String, IrType::Bool]);
        assert_eq!(tuple.incan_name(), "(int, str, bool)");
    }

    #[test]
    fn test_incan_name_function() {
        let func = IrType::Function {
            params: vec![IrType::Int, IrType::String],
            ret: Box::new(IrType::Bool),
        };
        assert_eq!(func.incan_name(), "(int, str) -> bool");
    }

    #[test]
    fn test_incan_name_named_generic() {
        let ty = IrType::NamedGeneric("Json".to_string(), vec![IrType::Struct("User".to_string())]);
        assert_eq!(ty.incan_name(), "Json[User]");
    }

    /// Regression for #755/#892: external unions accept only members qualified by their own provider.
    #[test]
    fn external_union_member_matching_is_provider_aware_issue892() {
        let union = IrType::ExternalUnion {
            library: "widgets".to_string(),
            union: Box::new(IrType::NamedGeneric(
                IR_UNION_TYPE_NAME.to_string(),
                vec![IrType::Struct("Widget".to_string())],
            )),
        };

        assert_eq!(
            union.union_variant_index_for_member(&IrType::Struct("widgets::Widget".to_string())),
            Some(0)
        );
        assert_eq!(
            union.union_variant_index_for_member(&IrType::Struct("other::Widget".to_string())),
            None
        );
    }

    /// Regression for #892: localizing one provider must not rewrite a union owned by another provider.
    #[test]
    fn provider_localization_preserves_foreign_external_union_issue892() {
        let foreign_union = IrType::ExternalUnion {
            library: "other".to_string(),
            union: Box::new(IrType::NamedGeneric(
                IR_UNION_TYPE_NAME.to_string(),
                vec![IrType::Struct("widgets::Widget".to_string())],
            )),
        };

        assert_eq!(foreign_union.provider_localized("widgets"), foreign_union);
    }
}
