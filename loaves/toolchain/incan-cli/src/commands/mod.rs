//! The compiler's commands, each over a unit the Oven describes (DD-0008).
//!
//! Every function returns `CliResult<ExitCode>`; the Oven's command line renders errors and exits.
//!
//! - `diagnostics`: `check` and `explain`
//! - `debug`: lex and parse
//! - `format`: source formatting
//! - `codegraph`, `binding_inspect`, `representation_inspect`: compiler-backed projections

pub mod binding_inspect;
pub mod codegraph;
pub mod debug;
pub mod diagnostics;
pub mod format;
pub mod representation_inspect;

pub use binding_inspect::{BindingInspectionFormat, inspect_bindings};
pub use codegraph::{CodegraphInspectionFormat, inspect_codegraph};
pub use debug::{lex_file, parse_file};
pub use diagnostics::{DiagnosticOutputFormat, check, explain_diagnostic};
pub use format::format_files;
pub use representation_inspect::{RepresentationInspectionFormat, inspect_representation};
