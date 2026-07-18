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
    /// Positive package features required for this compilation-unit declaration to participate.
    ///
    /// The parser flattens `when feature(...):` blocks into ordinary declarations and attaches the normalized
    /// conjunction here. Statement and expression nodes leave this empty.
    pub required_features: Vec<String>,
}

impl<T> Spanned<T> {
    /// Wrap one syntax node with its source span and empty formatter and feature-projection metadata.
    pub fn new(node: T, span: Span) -> Self {
        Self {
            node,
            span,
            leading_blank_lines: 0,
            required_features: Vec::new(),
        }
    }

    /// Attach a normalized positive package-feature conjunction to this node.
    pub fn with_required_features(mut self, required_features: Vec<String>) -> Self {
        self.required_features = required_features;
        self
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
    /// Source path supplied by the host parser, when available.
    ///
    /// This is contextual metadata used by later compiler phases for source-aware stdlib behavior. It is not authored
    /// syntax and may be absent for inline `-c` programs or tests that parse bare snippets.
    pub source_path: Option<String>,
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

impl Program {
    /// Build the active compilation projection without discarding inactive syntax from the parsed source tree.
    pub fn projected_for_features(&self, active_features: &std::collections::BTreeSet<String>) -> Self {
        let mut projected = self.clone();
        projected.declarations = self
            .declarations
            .iter()
            .filter(|declaration| {
                declaration
                    .required_features
                    .iter()
                    .all(|feature| active_features.contains(feature))
            })
            .cloned()
            .map(|mut declaration| {
                if let Declaration::TestModule(module) = &mut declaration.node {
                    module.body.retain(|nested| {
                        nested
                            .required_features
                            .iter()
                            .all(|feature| active_features.contains(feature))
                    });
                }
                declaration
            })
            .collect();
        projected
    }
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
    Alias(super::AliasDecl),
    Partial(super::PartialDecl),
    TypeAlias(super::TypeAliasDecl),
    Newtype(super::NewtypeDecl),
    Enum(super::EnumDecl),
    Function(super::FunctionDecl),
    TestModule(super::TestModuleDecl),
    Docstring(String), // Module-level docstring
}
