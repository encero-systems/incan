//! Canonical module resolution for multi-file Incan projects.
//!
//! This module owns source-module import classification, on-disk path resolution, and logical module identity
//! derivation. CLI, LSP, and test-runner orchestration may parse sources differently, but they should all ask this
//! module which imports point at local source modules and what logical module name those files carry.

use std::path::{Path, PathBuf};

use super::ast::{Declaration, ImportDecl, ImportKind, ImportPath, Program, Span, Visibility};
use incan_lang::lang::stdlib;

mod exports;
mod source_imports;

pub use exports::*;
pub use source_imports::*;

/// A resolved local source module import.
///
/// `file_path` is the concrete source file to read. `path_segments` is the canonical logical module identity derived
/// from the source root and normalized so directory entrypoints such as `mod.incn` and `__init__.incn` do not become
/// semantic module names. `module_name` is the underscore-joined key used by existing typechecker/codegen adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModuleRef {
    pub file_path: PathBuf,
    pub module_name: String,
    pub path_segments: Vec<String>,
}

impl SourceModuleRef {
    /// Build a resolved source-module reference from an on-disk file path and logical path segments.
    fn new(file_path: PathBuf, path_segments: Vec<String>) -> Self {
        let module_name = path_segments.join("_");
        Self {
            file_path,
            module_name,
            path_segments,
        }
    }
}

/// Classification for an import that may affect source-module graph traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceModuleImportResolution {
    /// Import resolved to a local Incan source file.
    Local(SourceModuleRef),
    /// Import resolved to the importing source file itself.
    ///
    /// Source-graph consumers must reject this before typechecking or lowering. A bare import is
    /// same-directory-relative, so a nested collision that intends the project root must spell `crate::...`.
    SelfImport {
        module_ref: SourceModuleRef,
        import_path: Vec<String>,
        can_use_root_import: bool,
    },
    /// Import points at an Incan stdlib source module. Callers that materialize stdlib source decide how to load it.
    ///
    /// `module_path` is the module the import names, a namespace's `prelude` path resolved to the namespace.
    Stdlib { module_path: Vec<String> },
    /// Import is not a source-backed Incan module, or no matching local source file exists.
    External,
}

/// One resolved import declaration inside a parsed program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProgramSourceImport {
    pub span: Span,
    pub resolution: SourceModuleImportResolution,
}

/// Kind of declaration surfaced by [`exported_type_like_docs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportedTypeLikeKind {
    Model,
    Class,
    Enum,
    Trait,
    Newtype,
}

/// Body docstring attached to a **public** type-like declaration, for documentation tooling and IDE features.
///
/// This reads the AST fields populated by the parser (`ModelDecl::docstring`, `ClassDecl::docstring`, etc.). It does
/// **not** include freestanding [`Declaration::Docstring`] items (module-level narrative) or `const` / `static` docs,
/// which use the preceding docstring declaration pattern instead.
///
/// Visibility matches [`exported_symbols`]: only `pub` declarations are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedTypeLikeDoc {
    pub name: String,
    pub kind: ExportedTypeLikeKind,
    /// Raw docstring payload from the lexer/parser (same string the formatter round-trips).
    pub docstring: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum ExportedSymbol {
    Type(String),
    Trait(String),
    /// A `pub capability` declaration (RFC 104).
    ///
    /// A capability is exported like any other public declaration because the stdlib is only the *first* capability
    /// publisher: a library author declares domain capabilities in their own module and consumers reference them by
    /// import. Withholding them from the export set would make `pub` meaningless on the one declaration form whose
    /// entire purpose is to be referenced from elsewhere.
    Capability(String),
    Function(String),
    Const(String),
    Static(String),
    Reexported(String),
    Variant {
        enum_name: String,
        variant_name: String,
    },
}

#[cfg(test)]
mod tests;
