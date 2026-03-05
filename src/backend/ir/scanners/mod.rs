//! Feature scanners and collectors for IR codegen.
//!
//! This module centralizes feature detection logic. The functions here are pure analyzers over the parsed AST and do
//! not mutate global state.

mod decorators;
mod list_helpers;
mod rust_crates;
mod serde;
mod this;
mod web;

pub use list_helpers::detect_list_helpers_usage;
pub use rust_crates::collect_rust_crates;
pub use serde::detect_serde_usage;
pub use this::check_for_this_import;
pub use web::detect_web_usage;
