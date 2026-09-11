//! One parsed module and the provenance a later stage needs to talk about it.

use std::path::PathBuf;

use crate::frontend::ast::Program;

/// A parsed module together with the source and location it came from.
///
/// This is the product of parsing, so it belongs to the frontend rather than to any one consumer. It was defined in
/// `cli::prelude` until the codegraph producer moved behind the compiler boundary, at which point a compiler-side
/// module would have had to import from `crate::cli` to name its own input — the "backend/oven/lsp -> cli" knot
/// that `loaves/LAYOUT.md` lists as one of the four to cut before the crate split, pointing the wrong way.
///
/// `cli::prelude` re-exports it, so callers that already name it there are unaffected.
#[derive(Clone)]
pub struct ParsedModule {
    /// The module's own name.
    pub name: String,
    /// Path segments for nested modules, such as `["db", "models"]` for `db::models`.
    pub path_segments: Vec<String>,
    /// Absolute path to the module file, retained for diagnostics.
    pub file_path: PathBuf,
    /// The source text, retained so a diagnostic can quote the span it points at.
    pub source: String,
    /// The parsed program.
    pub ast: Program,
}
