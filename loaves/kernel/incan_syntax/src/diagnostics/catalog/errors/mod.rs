//! Named constructors for every hard error the Incan compiler can emit.
//!
//! Each function returns a fully-formed [`crate::diagnostics::CompileError`] with an appropriate severity,
//! human-readable message, and — where helpful — contextual notes and actionable hints.
//!
//! # Submodules
//!
//! | Module        | Scope                                                    |
//! |---------------|----------------------------------------------------------|
//! | `types`       | Type-system and semantic errors (traits, derives…)       |
//! | `syntax`      | Parser and lexer diagnostics                             |
//! | `modules`     | Module/import resolution errors                          |
//! | `const_eval`  | Const-expression evaluation & builtin calls              |
//! | `rust_module` | `rust.module()` / `@rust.extern` diagnostics (RFC 023)   |
//! | `patterns_and_bounds` | Match-pattern literals, list-method forms, bounds owed to bounded nominals |

mod c_abi;
mod const_eval;
mod modules;
mod patterns_and_bounds;
mod rust_module;
mod syntax;
mod types;

pub use c_abi::*;
pub use const_eval::*;
pub use modules::*;
pub use patterns_and_bounds::*;
pub use rust_module::*;
pub use syntax::*;
pub use types::*;
