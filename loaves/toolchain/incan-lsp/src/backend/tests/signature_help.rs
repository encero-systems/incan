//! Signature help and function signature display: source defaults in the signature, parameter labels reused by
//! signature help, and signature help inside `unsafe` blocks.

use super::{format_function_signature, local_signature_help_at_offset};
use incan_frontend::ast::Declaration;
use incan_frontend::{lexer, parser};
use tower_lsp::lsp_types::ParameterLabel;

fn parse_source(source: &str) -> Result<incan_frontend::ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))
}

#[test]
fn local_function_signature_displays_source_defaults() -> Result<(), String> {
    let source = r#"
def greet(name: str = "Ada", count: int = 2 + 3) -> str:
    return name
"#;
    let ast = parse_source(source)?;
    let function = ast
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or_else(|| "expected function declaration".to_string())?;

    assert_eq!(
        format_function_signature(function, source),
        r#"def greet(name: str = "Ada", count: int = 2 + 3) -> str"#
    );
    Ok(())
}

#[test]
fn local_signature_help_reuses_default_parameter_display() -> Result<(), String> {
    let source = r#"
def greet(name: str = "Ada", count: int = 2 + 3) -> str:
    return name

def main() -> str:
    return greet("Grace", 4)
"#;
    let ast = parse_source(source)?;
    let call_prefix = r#"greet("Grace", "#;
    let call_offset = source
        .rfind(call_prefix)
        .map(|start| start + call_prefix.len())
        .ok_or_else(|| "expected call expression in fixture".to_string())?;
    let help = local_signature_help_at_offset(&ast, source, call_offset)
        .ok_or_else(|| "expected local function signature help".to_string())?;
    let signature = help
        .signatures
        .first()
        .ok_or_else(|| "expected one signature".to_string())?;
    let parameters = signature
        .parameters
        .as_ref()
        .ok_or_else(|| "expected parameter labels".to_string())?;

    assert_eq!(
        signature.label,
        r#"def greet(name: str = "Ada", count: int = 2 + 3) -> str"#
    );
    assert_eq!(help.active_parameter, Some(1));
    assert_eq!(
        parameters.first().map(|param| &param.label),
        Some(&ParameterLabel::Simple(r#"name: str = "Ada""#.to_string()))
    );
    assert_eq!(
        parameters.get(1).map(|param| &param.label),
        Some(&ParameterLabel::Simple("count: int = 2 + 3".to_string()))
    );
    Ok(())
}

#[test]
fn local_signature_help_descends_into_unsafe_blocks() -> Result<(), String> {
    let source = r#"
def greet(name: str) -> str:
    return name

def main() -> str:
    unsafe:
        return greet("Grace")
"#;
    let ast = parse_source(source)?;
    let call_offset = source
        .rfind("greet(\"Grace\")")
        .map(|start| start + "greet(\"".len())
        .ok_or_else(|| "expected call expression in fixture".to_string())?;

    let help = local_signature_help_at_offset(&ast, source, call_offset)
        .ok_or_else(|| "expected signature help inside unsafe block".to_string())?;
    let signature = help
        .signatures
        .first()
        .ok_or_else(|| "expected one signature".to_string())?;
    assert_eq!(signature.label, "def greet(name: str) -> str");
    Ok(())
}
