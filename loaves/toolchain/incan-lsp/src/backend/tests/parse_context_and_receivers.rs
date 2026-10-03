//! The LSP parse context and receiver detection: tuple-unpack `for` bindings parse under the LSP context, and
//! `@classmethod` bodies surface the `cls` receiver while `@staticmethod` bodies do not.

use std::collections::HashMap;

use super::{classmethod_cls_detail, classmethod_context_at_offset, identifier_at_offset};
use incan_frontend::{lexer, parser};

#[test]
fn lsp_parse_context_accepts_for_tuple_unpack_binding() {
    let source = r#"
def bind(input_columns: list[str]) -> list[str]:
    mut bindings: list[str] = []
    for idx, name in enumerate(input_columns):
        bindings.append(name)
    return bindings
"#;

    let tokens = match lexer::lex(source) {
        Ok(tokens) => tokens,
        Err(errors) => panic!("lexer failed for LSP tuple-for regression: {errors:?}"),
    };
    let parsed = parser::parse_with_context(
        &tokens,
        Some("/workspace/src/substrait/plan.incn"),
        Some(&std::collections::HashMap::new()),
    );

    assert!(
        parsed.is_ok(),
        "LSP parser context should accept tuple-unpack for bindings, got {parsed:?}"
    );
}

fn parse_source(source: &str) -> Result<incan_frontend::ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    parser::parse_with_context(&tokens, Some("src/main.incn"), Some(&HashMap::new()))
        .map_err(|errors| format!("parser failed: {errors:?}"))
}

#[test]
fn classmethod_context_surfaces_cls_receiver_for_lsp() -> Result<(), String> {
    let source = r#"
class Box[T with Clone]:
    value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)
"#;
    let ast = parse_source(source)?;
    let offset = source
        .find("cls(value")
        .ok_or_else(|| "expected cls call".to_string())?;
    let aliases = HashMap::new();

    let context = classmethod_context_at_offset(&ast, offset, &aliases)
        .ok_or_else(|| "expected classmethod context".to_string())?;
    assert_eq!(context.owner_type, "Box[T]");
    assert_eq!(classmethod_cls_detail(&context), "cls: type[Box[T]]");

    let (ident, span) =
        identifier_at_offset(source, offset).ok_or_else(|| "expected identifier at cls call".to_string())?;
    assert_eq!(ident, "cls");
    assert_eq!(&source[span.start..span.end], "cls");
    Ok(())
}

#[test]
fn staticmethod_body_does_not_surface_cls_receiver_for_lsp() -> Result<(), String> {
    let source = r#"
class Box[T with Clone]:
    value: T

    @staticmethod
    def make(value: T) -> Self:
        return Box(value=value)
"#;
    let ast = parse_source(source)?;
    let offset = source
        .find("return Box")
        .ok_or_else(|| "expected static factory body".to_string())?;
    let aliases = HashMap::new();

    assert!(classmethod_context_at_offset(&ast, offset, &aliases).is_none());
    Ok(())
}
