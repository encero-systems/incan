//! CLI command implementations
//!
//! All command functions return `CliResult<ExitCode>` instead of calling `process::exit`. Error handling and exits
//! happen in the top-level `run()`.
//!
//! - `build`: `incan build` and `incan run` through one compiled unit
//! - `debug`: lex, parse and check debug commands
//! - `format`: source formatting
//! - `init`: project scaffolding
//! - `lifecycle`: `version` and `env`

pub mod binding_inspect;
pub mod build;
pub mod codegraph;
pub mod debug;
pub mod diagnostics;
pub mod feature_inspect;
pub mod format;
pub mod init;
pub mod lifecycle;
pub mod representation_inspect;

pub use binding_inspect::{BindingInspectionFormat, inspect_bindings};
pub use codegraph::{CodegraphInspectionFormat, inspect_codegraph};
pub use debug::{check_file, lex_file, parse_file};
pub use diagnostics::{DiagnosticOutputFormat, check_path, check_path_with_features, explain_diagnostic};
pub use feature_inspect::{FeatureInspectionFormat, inspect_features};
pub use format::format_files;
pub use init::init_project;
pub use lifecycle::{env_list, env_run, env_show, version_project};
pub use representation_inspect::{RepresentationInspectionFormat, inspect_representation};
