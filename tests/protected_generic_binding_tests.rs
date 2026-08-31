//! Frontend regressions for protected builtin spellings used as generic type parameters.

use incan::frontend::diagnostics::CompileError;
use incan::frontend::{lexer, parser, typechecker};

/// Type-check `source`, retaining compiler diagnostics rather than process-level rendering.
fn check_source(source: &str) -> Result<Vec<CompileError>, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    match checker.check_program(&program) {
        Ok(()) => Ok(Vec::new()),
        Err(errors) => Ok(errors),
    }
}

/// Assert that a protected generic parameter is rejected at its enclosing source declaration span.
///
/// `TypeParam` retains no name span, so the declaration/method span is the nearest source-backed diagnostic location.
fn assert_protected_generic_binding(source: &str, spelling: &str, declaration: &str) -> Result<(), String> {
    let errors = check_source(source)?;
    let error = errors
        .iter()
        .find(|error| error.message.contains("protected builtin binding"))
        .ok_or_else(|| format!("expected protected-binding diagnostic for `{spelling}`, got {errors:?}"))?;
    let declaration_start = source
        .find(declaration)
        .ok_or_else(|| format!("missing declaration `{declaration}` in test source"))?;
    if error.span.start > declaration_start || error.span.end < declaration_start + declaration.len() {
        return Err(format!(
            "expected diagnostic for `{spelling}` to cover declaration `{declaration}` at {declaration_start}..{}, got {:?}",
            declaration_start + declaration.len(),
            error.span
        ));
    }
    Ok(())
}

/// Every source-owned generic parameter list rejects both protected builtin spellings.
#[test]
fn generic_type_parameters_cannot_replace_protected_builtin_call_roots_issue1249() -> Result<(), String> {
    for spelling in ["print", "println"] {
        let function_name = format!("generic_{spelling}");
        let method_owner = format!("Methods_{spelling}");
        let model_name = format!("Model_{spelling}");
        let class_name = format!("Class_{spelling}");
        let trait_name = format!("Trait_{spelling}");
        let enum_name = format!("Enum_{spelling}");
        let newtype_name = format!("Token_{spelling}");
        let alias_name = format!("Values_{spelling}");

        let cases = [
            (
                format!("def {function_name}"),
                format!("def {function_name}[{spelling}](value: {spelling}) -> None:\n  {spelling}(\"hello\")\n"),
            ),
            (
                "def transform".to_string(),
                format!(
                    "class {method_owner}:\n  def transform[{spelling}](self, value: {spelling}) -> {spelling}:\n    return value\n"
                ),
            ),
            (
                format!("model {model_name}"),
                format!("model {model_name}[{spelling}]:\n  value: {spelling}\n"),
            ),
            (
                format!("class {class_name}"),
                format!("class {class_name}[{spelling}]:\n  value: {spelling}\n"),
            ),
            (
                format!("trait {trait_name}"),
                format!("trait {trait_name}[{spelling}]:\n  def value(self) -> {spelling}: ...\n"),
            ),
            (
                format!("enum {enum_name}"),
                format!("enum {enum_name}[{spelling}]:\n  Value({spelling})\n"),
            ),
            (
                format!("type {newtype_name}"),
                format!("type {newtype_name}[{spelling}] = newtype {spelling}\n"),
            ),
            (
                format!("type {alias_name}"),
                format!("type {alias_name}[{spelling}] = list[{spelling}]\n"),
            ),
        ];

        for (declaration, source) in cases {
            assert_protected_generic_binding(&source, spelling, &declaration)?;
        }
    }
    Ok(())
}

/// Generic parameters that do not replace protected builtin roots remain legal.
#[test]
fn ordinary_generic_type_parameters_remain_valid_issue1249() -> Result<(), String> {
    let errors = check_source("def identity[T](value: T) -> T:\n  return value\n\nclass Box[U]:\n  value: U\n")?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("ordinary generic parameters must remain valid, got {errors:?}"))
    }
}
