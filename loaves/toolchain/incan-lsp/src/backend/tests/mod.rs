//! Tests for the LSP backend, one module per editor surface. Each module imports what it needs from `super`, which
//! exposes the backend's private items through `use super::*;`; the modules keep explicit imports because the backend's
//! own `Result` and `SymbolKind` imports would shadow the prelude and frontend names the tests use.

use super::*;

mod completions;
mod contract_model_command;
mod hover_previews;
mod identity_navigation;
mod parse_context_and_receivers;
#[cfg(feature = "rust_inspect")]
mod rust_inspect_workspace;
mod signature_help;
mod vocab_surfaces;
