//! CLI command implementations
//!
//! All command functions return `CliResult<ExitCode>` instead of calling `process::exit`.
//! Error handling and exits happen in the top-level `run()`.
//!
//! ## Submodules
//!
//! - `build` — Build and run pipelines
//! - `common` — Shared utilities (module collection, dependency helpers, etc.)
//! - `debug` — Debug commands (lex, parse, check, emit)
//! - `format` — Source formatting
//! - `init` — Project scaffolding
//! - `lifecycle` — Project lifecycle commands (`version` and `env`)
//! - `lock` — Lock file generation and resolution
//! - `stdlib_loader` — RFC 023: Stdlib module loading for compilation

pub mod build;
pub mod common;
pub mod debug;
pub mod format;
pub mod init;
pub mod lifecycle;
pub mod lock;
pub mod stdlib_loader;
pub(crate) mod vocab_extraction;

// Re-export public API so callers can use `commands::build_file()` etc.
pub use build::{build_file, build_library, run_file};
pub use common::{collect_modules, read_source};
pub use debug::{check_file, emit_rust, lex_file, parse_file};
pub use format::format_files;
pub use init::init_project;
pub use lifecycle::{env_list, env_run, env_show, version_project};
pub use lock::lock_project;

// Crate-internal API (used by test_runner and other CLI modules)
pub(crate) use lock::{LockResolutionRequest, resolve_lock_payload};
