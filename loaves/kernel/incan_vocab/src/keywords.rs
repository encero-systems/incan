//! Compiler-side transport for foundational vocabulary descriptors.
//!
//! Incan authors use the canonical contracts from `std.primitives.vocab`, normally through the identity-preserving
//! `std.vocab` facade. This crate decodes their published representation for parser and compiler tooling; it does not
//! own a second source-language declaration identity. Companion crates should usually describe their grammar through
//! [`DslSurface`], [`DeclarationSurface`], and [`ClauseSurface`] inside the `library_vocab()` entrypoint.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

mod declaration_surfaces;
mod embedded_fragments;
mod registrations;
mod scoped_surfaces;
mod scoped_symbols;

pub use declaration_surfaces::*;
pub use embedded_fragments::*;
pub use registrations::*;
pub use scoped_surfaces::*;
pub use scoped_symbols::*;

/// Whether a declaration lowers into an expression or a statement list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum DesugarTarget {
    /// Lower into compilation-unit declarations.
    Declarations,
    /// Lower into host statements.
    #[default]
    Statements,
    /// Lower into one host expression.
    Expression,
}

/// One activated DSL surface contributed by a library.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DslSurface {
    /// Activation rule shared by declarations in this surface.
    pub activation: KeywordActivation,
    /// Declarations contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub declarations: Vec<DeclarationSurface>,
    /// Scoped surface forms contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub scoped_surfaces: Vec<ScopedSurfaceDescriptor>,
    /// Scoped identifier symbols contributed by this activated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub scoped_symbols: Vec<ScopedSymbolDescriptor>,
    /// Descriptor-gated embedded-fragment submode claims contributed by this activated surface (RFC 081).
    #[cfg_attr(feature = "serde", serde(default))]
    pub embedded_fragments: Vec<EmbeddedFragmentDescriptor>,
}

impl DslSurface {
    /// Create an empty activated surface.
    #[must_use]
    pub fn new(activation: KeywordActivation) -> Self {
        Self {
            activation,
            declarations: Vec::new(),
            scoped_surfaces: Vec::new(),
            scoped_symbols: Vec::new(),
            embedded_fragments: Vec::new(),
        }
    }

    /// Create an import-activated DSL surface.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::new(KeywordActivation::on_import(namespace))
    }

    /// Create an always-active DSL surface.
    #[must_use]
    pub fn always_on() -> Self {
        Self::new(KeywordActivation::Always)
    }

    /// Add one declaration to this surface.
    #[must_use]
    pub fn with_declaration(mut self, declaration: DeclarationSurface) -> Self {
        self.declarations.push(declaration);
        self
    }

    /// Add multiple declarations to this surface.
    #[must_use]
    pub fn with_declarations<I>(mut self, declarations: I) -> Self
    where
        I: IntoIterator<Item = DeclarationSurface>,
    {
        self.declarations.extend(declarations);
        self
    }

    /// Add one scoped surface descriptor to this activated surface.
    #[must_use]
    pub fn with_scoped_surface(mut self, scoped_surface: ScopedSurfaceDescriptor) -> Self {
        self.scoped_surfaces.push(scoped_surface);
        self
    }

    /// Add multiple scoped surface descriptors to this activated surface.
    #[must_use]
    pub fn with_scoped_surfaces<I>(mut self, scoped_surfaces: I) -> Self
    where
        I: IntoIterator<Item = ScopedSurfaceDescriptor>,
    {
        self.scoped_surfaces.extend(scoped_surfaces);
        self
    }

    /// Add one scoped symbol descriptor to this activated surface.
    #[must_use]
    pub fn with_scoped_symbol(mut self, scoped_symbol: ScopedSymbolDescriptor) -> Self {
        self.scoped_symbols.push(scoped_symbol);
        self
    }

    /// Add multiple scoped symbol descriptors to this activated surface.
    #[must_use]
    pub fn with_scoped_symbols<I>(mut self, scoped_symbols: I) -> Self
    where
        I: IntoIterator<Item = ScopedSymbolDescriptor>,
    {
        self.scoped_symbols.extend(scoped_symbols);
        self
    }

    /// Add one embedded-fragment descriptor to this activated surface (RFC 081).
    #[must_use]
    pub fn with_embedded_fragment(mut self, embedded_fragment: EmbeddedFragmentDescriptor) -> Self {
        self.embedded_fragments.push(embedded_fragment);
        self
    }

    /// Add multiple embedded-fragment descriptors to this activated surface (RFC 081).
    #[must_use]
    pub fn with_embedded_fragments<I>(mut self, embedded_fragments: I) -> Self
    where
        I: IntoIterator<Item = EmbeddedFragmentDescriptor>,
    {
        self.embedded_fragments.extend(embedded_fragments);
        self
    }
}

fn split_spelling(spelling: &str) -> (String, Vec<String>) {
    let mut parts = spelling.split_whitespace();
    let keyword = parts.next().unwrap_or_default().to_string();
    let compound_tokens = parts.map(str::to_string).collect();
    (keyword, compound_tokens)
}

/// Parser surface shape for a keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordSurfaceKind {
    /// A prefix expression such as `await value`.
    PrefixExpression,
    /// `def name(...):`
    #[default]
    FunctionDecl,
    /// `class/model/trait/enum Name:`
    TypeDecl,
    /// `if ...:`
    ConditionalChain,
    /// `for ... in ...:`
    ForLoop,
    /// `while ...:`
    WhileLoop,
    /// `match ...:`
    MatchBlock,
    /// `try: ...`
    TryBlock,
    /// `import ...` / `from ... import ...`
    ImportStatement,
    /// `return`, `break`, `continue`, `pass`, `raise`, `yield`.
    ControlFlow,
    /// `let ... = ...`
    BindingDecl,
    /// Library/DSL block declaration (for example `routes:`).
    BlockDeclaration,
    /// Keyword only valid inside a parent block (for example `GET` in `routes`).
    BlockContextKeyword,
    /// Sub-block in a DSL block.
    SubBlock,
}
