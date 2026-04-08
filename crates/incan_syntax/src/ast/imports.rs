//! Import-related AST types: import declarations, paths, and items.

use std::fmt;

use super::{Ident, Spanned, Type, Visibility};

// ============================================================================
// Const bindings (module-level)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub ty: Option<Spanned<Type>>,
    pub value: Spanned<super::Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticDecl {
    pub visibility: Visibility,
    pub name: Ident,
    pub ty: Spanned<Type>,
    pub value: Spanned<super::Expr>,
}

// ============================================================================
// Imports
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub visibility: Visibility,
    pub kind: ImportKind,
    pub alias: Option<Ident>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportKind {
    /// `import foo::bar::baz` or `import crate::config` - Rust-style module path
    Module(ImportPath),
    /// `from module import item1, item2` or `from ..utils import x` - Python-style multi-import
    From { module: ImportPath, items: Vec<ImportItem> },
    /// `import pub::mylib` - Incan library namespace import
    PubLibrary {
        /// Library dependency key from `incan.toml [dependencies]`
        library: Ident,
    },
    /// `from pub::mylib import Widget, helper` - Incan library symbols from manifest exports
    PubFrom {
        /// Library dependency key from `incan.toml [dependencies]`
        library: Ident,
        items: Vec<ImportItem>,
    },
    /// `import python "module"` - Python interop  FIXME: this doesn't actually work yet
    Python(String),
    /// `import rust::serde_json` - Rust crate import (direct crate usage)
    RustCrate {
        crate_name: String,
        /// Optional path within crate: `import rust::serde_json::Value`
        path: Vec<Ident>,
        /// Optional version requirement string (Cargo semver syntax).
        version: Option<String>,
        /// Optional feature list (only valid when `version` is provided).
        features: Vec<String>,
    },
    /// `from rust::time import Instant, Duration` - Rust crate with specific items
    RustFrom {
        crate_name: String,
        /// Optional path within crate before items: `from rust::std::collections import HashMap`
        path: Vec<Ident>,
        /// Optional version requirement string (Cargo semver syntax).
        version: Option<String>,
        /// Optional feature list (only valid when `version` is provided).
        features: Vec<String>,
        items: Vec<ImportItem>,
    },
}

/// A path in an import statement, supporting:
/// - Simple paths: `models`, `utils::helpers`
/// - Relative paths: `..common`, `super::utils`
/// - Absolute paths: `crate::config`
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPath {
    /// How many parent levels to go up (0 = current/absolute, 1 = parent, 2 = grandparent, etc.)
    pub parent_levels: usize,
    /// Whether this is an absolute path from project root (crate::...)
    pub is_absolute: bool,
    /// The path segments (module names)
    pub segments: Vec<Ident>,
}

impl ImportPath {
    pub fn simple(segments: Vec<Ident>) -> Self {
        Self {
            parent_levels: 0,
            is_absolute: false,
            segments,
        }
    }

    pub fn relative(parent_levels: usize, segments: Vec<Ident>) -> Self {
        Self {
            parent_levels,
            is_absolute: false,
            segments,
        }
    }

    pub fn absolute(segments: Vec<Ident>) -> Self {
        Self {
            parent_levels: 0,
            is_absolute: true,
            segments,
        }
    }

    /// Convert to Rust-style path string (using ::)
    pub fn to_rust_path(&self) -> String {
        let mut parts = Vec::new();

        if self.is_absolute {
            parts.push("crate".to_string());
        } else {
            for _ in 0..self.parent_levels {
                parts.push("super".to_string());
            }
        }

        parts.extend(self.segments.clone());
        parts.join("::")
    }
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_rust_path())
    }
}

/// An item in a `from ... import` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub name: Ident,
    pub alias: Option<Ident>,
}
