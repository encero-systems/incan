//! Tests for source module import resolution and exported module symbols.

use super::*;

use crate::ast::{
    ClassDecl, ConstDecl, Declaration, EnumDecl, Expr, FunctionDecl, ImportDecl, ImportItem, ImportKind, ImportPath,
    IntLiteral, Literal, ModelDecl, NewtypeDecl, Program, Span, Spanned, StaticDecl, TraitDecl, Type, VariantDecl,
    Visibility,
};
use crate::{lexer, parser};

mod exported_symbols;
mod import_resolution;

/// Public type-like declarations whose block docstrings the export summary reads.
const BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE: &str = r##"pub model CliModelProbe:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    pub id: int

pub class CliClassProbe:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    pub x: int

pub enum CliEnumProbe:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    V

pub trait CliTraitProbe:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """
    def meth(self) -> None: ...

pub type CliNewtypeProbe = newtype int:
    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """

    def zero(self) -> int:
        return 0
"##;

fn make_spanned<T>(node: T) -> Spanned<T> {
    Spanned {
        node,
        span: Span::default(),
        leading_blank_lines: 0,
        required_features: Vec::new(),
    }
}

fn relative_from_import(module: &str) -> ImportDecl {
    ImportDecl {
        visibility: Visibility::Private,
        kind: ImportKind::From {
            module: ImportPath {
                segments: vec![module.to_string()],
                is_absolute: false,
                parent_levels: 0,
            },
            items: vec![],
        },
        alias: None,
    }
}

fn relative_module_import(segments: &[&str]) -> ImportDecl {
    ImportDecl {
        visibility: Visibility::Private,
        kind: ImportKind::Module(ImportPath {
            segments: segments.iter().map(|segment| (*segment).to_string()).collect(),
            is_absolute: false,
            parent_levels: 0,
        }),
        alias: None,
    }
}

/// Resolve every import of `source`, parsed as a program at `src/main.incn`, to the stdlib module paths it depends
/// on.
fn stdlib_dependencies_of(source: &str) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("fixture should lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("fixture should parse: {errors:?}"))?;
    Ok(resolve_program_source_imports(&program, Path::new("src"), None)
        .into_iter()
        .filter_map(|resolved| match resolved.resolution {
            SourceModuleImportResolution::Stdlib { module_path } => Some(module_path),
            _ => None,
        })
        .collect())
}

// ========================================
// exported_type_like_docs tests
// ========================================

fn assert_docstring_has_marker_lines(doc: Option<&str>, ctx: &str) -> Result<(), Vec<String>> {
    let Some(doc) = doc else {
        return Err(vec![format!("{ctx}: expected body docstring")]);
    };
    let trimmed = doc.trim();
    if !trimmed.contains("Line A documents the class API.") {
        return Err(vec![format!("{ctx}: missing marker A in {trimmed:?}")]);
    }
    if !trimmed.contains("Line B keeps interior newlines after trim().") {
        return Err(vec![format!("{ctx}: missing marker B in {trimmed:?}")]);
    }
    Ok(())
}
