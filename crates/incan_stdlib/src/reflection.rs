//! Reflection support for Incan models and classes.
//!
//! The `HasFieldInfo` trait provides introspection capabilities for structured types,
//! allowing generated code to query field names and types at runtime.

use crate::frozen::{FrozenDict, FrozenList, FrozenStr};
use std::marker::PhantomData;

/// Provides reflection information about a type's fields.
///
/// This trait is typically derived using `#[derive(FieldInfo)]` on models and classes.
///
/// # Examples
///
/// ```ignore
/// #[derive(FieldInfo)]
/// struct Person {
///     name: String,
///     age: i64,
/// }
///
/// // Generated implementation provides:
/// use incan_stdlib::reflection::HasFieldInfo;
/// assert_eq!(<Person as HasFieldInfo>::field_names(), vec!["name", "age"]);
/// assert_eq!(<Person as HasFieldInfo>::field_types(), vec!["String", "i64"]);
/// ```
pub trait HasFieldInfo {
    /// Returns the names of all fields in this type.
    fn field_names() -> Vec<&'static str>;

    /// Returns the type names of all fields in this type.
    fn field_types() -> Vec<&'static str>;
}

/// Provides the rich field metadata returned by Incan's value-level `__fields__()` helper.
///
/// The compiler implements this trait for generated models and classes so generic Incan code can use
/// `value.__fields__()` through an inferred Rust capability bound without changing the concrete reflection result.
pub trait HasFieldMetadata {
    /// Returns field metadata for this value's type.
    fn __fields__(&self) -> FrozenList<FieldInfo>;
}

/// Provides value-level field reflection for generic Incan row helpers.
///
/// Concrete model and class values keep their compiler-generated typed `__field_value__()` and `__field_items__()`
/// inherent methods. This trait is the erased generic capability used when source calls those helpers through a type
/// parameter such as `T`, following the same generated field-overlay surface.
pub trait HasFieldValueReflection {
    /// Return a reflected field value by canonical field name or model alias.
    fn __field_value__(&self, name: &str) -> Option<String>;

    /// Return reflected field name/value pairs in declaration order.
    fn __field_items__(&self) -> Vec<(String, String)>;
}

/// Provides type-level field metadata for generated models and classes.
///
/// The compiler uses this trait for generic schema helpers that reflect on an explicit type argument, for example
/// `T.__fields__()`, without requiring a dummy runtime value.
pub trait HasTypeFieldMetadata {
    /// Returns field metadata for this type.
    fn __fields__() -> FrozenList<FieldInfo>;
}

/// Provides the value-level `__class_name__()` reflection helper for generated models and classes.
pub trait HasClassName {
    /// Returns this value's Incan class/model name.
    fn __class_name__(&self) -> &'static str;
}

/// Provides type-level source names for generated nominal and primitive type parameters.
///
/// The compiler uses this trait for generic helpers that reflect on an explicit type argument, for example
/// `T.__class_name__()`, without requiring a dummy runtime value. Nominal types return their declaration name, while
/// primitive Incan type arguments return canonical source spellings such as `"int"` or `"str"`.
pub trait HasTypeClassName {
    /// Returns this type's Incan source name.
    fn __class_name__() -> &'static str;
}

/// Zero-sized runtime marker for an Incan type used as a value-level token.
///
/// The compiler emits this for source expressions such as `int` when they are used as values. It exists so APIs can
/// accept `Type[int]`, `Type[float]`, and similar parameters without forcing users to pass strings or dummy values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TypeToken<T>(PhantomData<T>);

impl<T> TypeToken<T> {
    /// Construct a type token for `T`.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

macro_rules! impl_primitive_type_class_name {
    ($ty:ty => $name:literal) => {
        impl HasTypeClassName for $ty {
            fn __class_name__() -> &'static str {
                $name
            }
        }
    };
}

impl_primitive_type_class_name!(i64 => "int");
impl_primitive_type_class_name!(f64 => "float");
impl_primitive_type_class_name!(String => "str");
impl_primitive_type_class_name!(bool => "bool");
impl_primitive_type_class_name!(i8 => "i8");
impl_primitive_type_class_name!(i16 => "i16");
impl_primitive_type_class_name!(i32 => "i32");
impl_primitive_type_class_name!(i128 => "i128");
impl_primitive_type_class_name!(isize => "isize");
impl_primitive_type_class_name!(u8 => "u8");
impl_primitive_type_class_name!(u16 => "u16");
impl_primitive_type_class_name!(u32 => "u32");
impl_primitive_type_class_name!(u64 => "u64");
impl_primitive_type_class_name!(u128 => "u128");
impl_primitive_type_class_name!(usize => "usize");
impl_primitive_type_class_name!(f32 => "f32");

/// Runtime value type for field reflection (RFC 021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: FrozenStr,
    pub alias: Option<FrozenStr>,
    pub description: Option<FrozenStr>,
    pub wire_name: FrozenStr,
    pub type_name: FrozenStr,
    pub has_default: bool,
    pub extra: FrozenDict<FrozenStr, FrozenStr>,
}
