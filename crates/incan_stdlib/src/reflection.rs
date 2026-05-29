//! Reflection support for Incan models and classes.
//!
//! The `HasFieldInfo` trait provides introspection capabilities for structured types,
//! allowing generated code to query field names and types at runtime.

use crate::frozen::{FrozenDict, FrozenList, FrozenStr};

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

/// Provides type-level class/model names for generated models and classes.
///
/// The compiler uses this trait for generic schema helpers that reflect on an explicit type argument, for example
/// `T.__class_name__()`, without requiring a dummy runtime value.
pub trait HasTypeClassName {
    /// Returns this type's Incan class/model name.
    fn __class_name__() -> &'static str;
}

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
