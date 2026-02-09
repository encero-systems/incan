//! Named constructors for every hard error the Incan compiler can emit.
//!
//! Each function returns a fully-formed [`CompileError`] with an appropriate
//! severity, human-readable message, and — where helpful — contextual notes
//! and actionable hints.
//!
//! # Submodules
//!
//! | Module       | Scope                                              |
//! |--------------|----------------------------------------------------|
//! | `types`      | Type-system and semantic errors (traits, derives…) |
//! | `syntax`     | Parser and lexer diagnostics                       |
//! | `modules`    | Module/import resolution errors                    |
//! | `const_eval` | Const-expression evaluation & builtin calls        |

mod const_eval;
mod modules;
mod syntax;
mod types;

pub use const_eval::*;
pub use modules::*;
pub use syntax::*;
pub use types::*;
