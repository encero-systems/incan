//! Core AST types: spans, spanned nodes, identifiers, programs, and top-level declarations.

/// Source location span (byte offsets)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// A node with source location
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
    /// Extra blank lines to emit before this node when formatting (`0` or `1`).
    ///
    /// Only meaningful on `Spanned<Statement>` nodes from indented statement blocks (function bodies,
    /// `if` / `while` / `for` bodies, match blocks, vocab blocks, etc.): a single newline between statements yields
    /// `0`; two or more consecutive newlines collapse to `1`. All other `Spanned<T>` uses keep the default `0` from
    /// [`Spanned::new`].
    pub leading_blank_lines: u8,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self {
            node,
            span,
            leading_blank_lines: 0,
        }
    }
}

/// Identifier (interned string index in practice, String for simplicity here)
pub type Ident = String;

/// Visibility modifier for module-level items.
///
/// This is intentionally minimal for now; only `pub` is supported for top-level declarations that allow visibility
/// control (for example `const` and `static`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

/// A program is a sequence of declarations, optionally with a `rust.module()` directive (RFC 023).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub declarations: Vec<Spanned<Declaration>>,
    /// The `rust.module("path::to::module")` directive, if present.
    ///
    /// Declares that `@rust.extern` items in this module are backed by Rust functions at the given
    /// Rust module path. See RFC 023 for the full semantic design.
    pub rust_module_path: Option<Spanned<String>>,
    /// Non-fatal warnings emitted during parsing.
    ///
    /// These do not prevent the program from being type-checked or compiled. They are surfaced in CLI output and
    /// forwarded to the LSP as `DiagnosticSeverity::WARNING` squiggles.
    pub warnings: Vec<crate::diagnostics::CompileError>,
}

/// Top-level declarations
#[derive(Debug, Clone, PartialEq)]
pub enum Declaration {
    Import(super::ImportDecl),
    Const(super::ConstDecl),
    Static(super::StaticDecl),
    Model(super::ModelDecl),
    Class(super::ClassDecl),
    Trait(super::TraitDecl),
    TypeAlias(super::TypeAliasDecl),
    Newtype(super::NewtypeDecl),
    Enum(super::EnumDecl),
    Function(super::FunctionDecl),
    TestModule(super::TestModuleDecl),
    Docstring(String), // Module-level docstring
}
