//! Prelude module for common runtime imports.
//!
//! Import this in generated code to get access to all runtime functionality:
//!
//! ```ignore
//! use incan_stdlib::prelude::*;
//! ```

// Re-export runtime traits and helpers
pub use crate::reflection::{FieldInfo, HasClassName, HasFieldInfo, HasFieldMetadata};
// frozen runtime types for consts (RFC 008)
pub use crate::frozen::{FrozenBytes, FrozenDict, FrozenList, FrozenSet, FrozenStr};
// Python-like numeric operations (generic entrypoints + compatibility helpers)
pub use crate::num::{py_div, py_floor_div, py_floor_div_f64, py_floor_div_i64, py_mod, py_mod_f64, py_mod_i64};

#[cfg(feature = "json")]
pub use crate::json::{FromJson, ToJson};

// Re-export derive macros from incan_derive
// Note: These are proc macros and must be re-exported with `pub use`
pub use incan_derive::{FieldInfo as DeriveFieldInfo, IncanClass, IncanJson, IncanReflect};
