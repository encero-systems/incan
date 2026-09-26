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
//! | `c_abi`       | Checked C ABI diagnostics (RFC 116)                      |
//! | `capability_requirements` | Types lacking a derive or task capability a program needs of them |

mod c_abi;
mod capability_requirements;
mod const_eval;
mod modules;
mod rust_module;
mod syntax;
mod types;

pub use c_abi::*;
pub use capability_requirements::*;
pub use const_eval::*;
pub use modules::*;
pub use rust_module::*;
pub use syntax::*;
pub use types::*;
