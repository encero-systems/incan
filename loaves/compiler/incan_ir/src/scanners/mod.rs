//! Feature scanners and collectors for IR codegen.
//!
//! This module centralizes feature detection logic. The functions here are pure analyzers over parsed AST or lowered
//! IR and do not mutate global state.

mod binding_usage;
mod rust_crates;
mod this;

pub use binding_usage::{binding_use_scan, expr_uses_binding_name};
pub use incan_frontend::serde_usage::{detect_serde_non_import_usage, detect_serde_usage};
pub use rust_crates::collect_rust_crates;
pub use this::check_for_this_import;
