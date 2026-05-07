//! Language “surface” vocabulary.
//!
//! This module is for **user-facing** names that are part of the language experience but are not
//! pure syntax tokens:
//!
//! - prelude/runtime functions like `spawn(...)`, `timeout(...)`
//! - runtime/interop types like `Mutex[T]`, `Sender[T]`, `Vec[T]`
//! - builtin methods like `str.split(...)`
//!
//! The goal is the same as other `incan_core::lang` registries: avoid stringly-typed checks
//! scattered through the compiler/tooling by providing stable IDs + metadata.

pub mod collection_helpers;
pub mod constructors;
pub mod functions;
pub mod methods;
pub mod types;

// Re-export method registries for backwards-compatible paths:
// `crate::lang::surface::string_methods`, `crate::lang::surface::list_methods`, ...
pub use methods::{
    dict_methods, float_methods, frozen_bytes_methods, frozen_dict_methods, frozen_list_methods, frozen_set_methods,
    list_methods, option_methods, set_methods, string_methods,
};
