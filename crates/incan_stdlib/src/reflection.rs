//! Reflection support for Incan models and classes.
//!
//! The `HasFieldInfo` trait provides introspection capabilities for structured types,
//! allowing generated code to query field names and types at runtime.

use crate::frozen::{FrozenDict, FrozenStr};

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
