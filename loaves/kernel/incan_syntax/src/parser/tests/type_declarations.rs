//! Nominal type declarations: models, classes, traits, enums, newtypes and `rusttype` bindings. Fields and RFC 021
//! metadata, `pub` field visibility (#884), leading docstrings, method receivers and trailing commas, computed
//! properties, bodyless trait methods, supertraits and trait adoption, interop edges, enum diagnostics (#113) and value
//! enums.

use super::*;

mod methods_models_and_properties;
mod traits_newtypes_and_enums;

/// Test helper: expected `Type::Simple` for generic-argument position assertions.
fn require_simple_type(ty: &Spanned<Type>) -> Result<&String, Vec<CompileError>> {
    match &ty.node {
        Type::Simple(name) => Ok(name),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected simple type".to_string(),
            ty.span,
        )]),
    }
}

fn require_enum_decl(decl: &Spanned<Declaration>) -> Result<&EnumDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Enum(e) => Ok(e),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected enum declaration".to_string(),
            decl.span,
        )]),
    }
}
