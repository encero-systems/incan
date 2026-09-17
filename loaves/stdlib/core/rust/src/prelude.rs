//! Convenience re-exports for hand-written Rust that works with Incan-generated types.
//!
//! Generated code names every runtime item by its full path and never imports this module; it exists for host code
//! and for this crate's own doctests, which want the reflection traits, the frozen constant types, the numeric
//! helpers and the derive macros in scope with one glob:
//!
//! ```ignore
//! use incan_std_core::prelude::*;
//! ```

// Re-export runtime traits and helpers
pub use crate::reflection::{
    FieldInfo, HasClassName, HasFieldInfo, HasFieldMetadata, HasFieldValueReflection, HasTypeClassName,
    HasTypeFieldMetadata,
};
// frozen runtime types for consts (RFC 008)
pub use crate::frozen::{FrozenBytes, FrozenDict, FrozenList, FrozenSet, FrozenStr};
// Python-like numeric operations (generic entrypoints + compatibility helpers)
pub use crate::num::{py_div, py_floor_div, py_floor_div_f64, py_floor_div_i64, py_mod, py_mod_f64, py_mod_i64};

// Re-export derive macros from incan_derive
// Note: These are proc macros and must be re-exported with `pub use`
pub use incan_derive::{FieldInfo as DeriveFieldInfo, IncanClass, IncanJson, IncanReflect};
