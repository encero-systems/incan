#[cfg(test)]
/// Parser unit tests.
///
/// These tests focus on correctness of specific syntactic forms and on the parser’s error recovery behavior
/// (avoiding cascaded errors).
mod tests {
    use super::*;
    use crate::lexer;
    use incan_core::lang::types::collections::{self, CollectionTypeId};

    fn parse_str(source: &str) -> Result<Program, Vec<CompileError>> {
        let tokens = lexer::lex(source).map_err(|_| vec![])?;
        parse(&tokens)
    }

    fn parse_str_err(source: &str, context: &str) -> Vec<CompileError> {
        match parse_str(source) {
            Err(errs) => errs,
            Ok(_) => panic!("{context}"),
        }
    }

    fn parse_str_with_module_path(source: &str, module_path: Option<&str>) -> Result<Program, Vec<CompileError>> {
        let tokens = lexer::lex(source).map_err(|_| vec![])?;
        parse_with_module_path(&tokens, module_path)
    }

    /// Test helper: surface a structured failure instead of panicking when a declaration is not a trait.
    fn require_trait_decl(decl: &Spanned<Declaration>) -> Result<&TraitDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::Trait(t) => Ok(t),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected trait declaration".to_string(),
                decl.span,
            )]),
        }
    }

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

    fn require_newtype_decl(decl: &Spanned<Declaration>) -> Result<&NewtypeDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::Newtype(nt) => Ok(nt),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected newtype/rusttype declaration".to_string(),
                decl.span,
            )]),
        }
    }

    fn require_model_decl(decl: &Spanned<Declaration>) -> Result<&ModelDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::Model(m) => Ok(m),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected model declaration".to_string(),
                decl.span,
            )]),
        }
    }

    fn require_class_decl(decl: &Spanned<Declaration>) -> Result<&ClassDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::Class(c) => Ok(c),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected class declaration".to_string(),
                decl.span,
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

    fn require_function_decl(decl: &Spanned<Declaration>) -> Result<&FunctionDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::Function(f) => Ok(f),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected function declaration".to_string(),
                decl.span,
            )]),
        }
    }

    fn require_test_module_decl(decl: &Spanned<Declaration>) -> Result<&TestModuleDecl, Vec<CompileError>> {
        match &decl.node {
            Declaration::TestModule(t) => Ok(t),
            _ => Err(vec![CompileError::new(
                "parser test internal error: expected test module declaration".to_string(),
                decl.span,
            )]),
        }
    }

    #[test]
    fn test_parse_trait_method_named_from() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait From[T]:
  @classmethod
  def from(cls, value: T) -> Self: ...
"#;
        let program = parse_str(source)?;
        let trait_decl = require_trait_decl(&program.declarations[0])?;
        assert_eq!(trait_decl.name, "From");
        assert_eq!(trait_decl.methods.len(), 1);
        assert_eq!(trait_decl.methods[0].node.name, "from");
        Ok(())
    }

    #[test]
    fn test_assert_keyword_lexes_as_identifier() -> Result<(), Vec<CompileError>> {
        let tokens = lexer::lex("assert value\n").map_err(|_| {
            vec![CompileError::new(
                "parser test internal error: lex failed".to_string(),
                Span::default(),
            )]
        })?;

        assert!(matches!(&tokens[0].kind, TokenKind::Ident(name) if name == "assert"));
        Ok(())
    }

    #[test]
    fn test_parse_assert_statement_without_testing_import() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(value: int) -> None:
  assert value > 0
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };

        assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
        assert!(assert_stmt.message.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_assert_statement_with_message() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(value: int) -> None:
  assert value > 0, "positive required"
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };

        assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
        assert!(matches!(
            assert_stmt.message.as_ref().map(|msg| &msg.node),
            Some(Expr::Literal(Literal::String(msg))) if msg == "positive required"
        ));
        Ok(())
    }

    #[test]
    fn test_parse_assert_raises_statement() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check() -> None:
  assert explode() raises AssertionError, "boom"
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };

        let AssertKind::Raises { call, error_type } = &assert_stmt.kind else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected raises assert".to_string(),
                func.body[0].span,
            )]);
        };
        assert!(matches!(call.node, Expr::Call(_, _, _)));
        assert!(matches!(error_type.node, Type::Simple(ref name) if name == "AssertionError"));
        assert!(assert_stmt.message.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_assert_identity_bool_literals_as_condition() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(ready: bool, done: bool) -> None:
  assert ready is true, "ready should be true"
  assert done is false
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;

        let Statement::Assert(true_assert) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected true assert statement".to_string(),
                func.body[0].span,
            )]);
        };
        assert!(matches!(true_assert.kind, AssertKind::Condition(_)));
        assert!(true_assert.message.is_some());

        let Statement::Assert(false_assert) = &func.body[1].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected false assert statement".to_string(),
                func.body[1].span,
            )]);
        };
        assert!(matches!(false_assert.kind, AssertKind::Condition(_)));
        assert!(false_assert.message.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_assert_is_some_pattern_statement() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(user: Option[str]) -> None:
  assert user is Some(value), "user required"
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };

        let AssertKind::IsPattern { value, pattern } = &assert_stmt.kind else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected pattern assert".to_string(),
                func.body[0].span,
            )]);
        };
        assert!(matches!(value.node, Expr::Ident(ref name) if name == "user"));
        assert!(matches!(
            &pattern.node,
            Pattern::Constructor(name, args)
                if name == "Some"
                    && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(&arg.node, Pattern::Binding(binding) if binding == "value"))
        ));
        assert!(assert_stmt.message.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_assert_is_none_pattern_statement() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(user: Option[str]) -> None:
  assert user is None
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };

        assert!(matches!(
            &assert_stmt.kind,
            AssertKind::IsPattern { pattern, .. }
                if matches!(&pattern.node, Pattern::Constructor(name, args) if name == "None" && args.is_empty())
        ));
        Ok(())
    }

    #[test]
    fn test_parse_assert_is_ok_and_err_pattern_statements() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(result: Result[int, str]) -> None:
  assert result is Ok(value)
  assert result is Err(_), "error required"
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;

        let Statement::Assert(ok_assert) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected Ok assert statement".to_string(),
                func.body[0].span,
            )]);
        };
        assert!(matches!(
            &ok_assert.kind,
            AssertKind::IsPattern { pattern, .. }
                if matches!(
                    &pattern.node,
                    Pattern::Constructor(name, args)
                        if name == "Ok"
                            && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(&arg.node, Pattern::Binding(binding) if binding == "value"))
                )
        ));

        let Statement::Assert(err_assert) = &func.body[1].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected Err assert statement".to_string(),
                func.body[1].span,
            )]);
        };
        assert!(matches!(
            &err_assert.kind,
            AssertKind::IsPattern { pattern, .. }
                if matches!(
                    &pattern.node,
                    Pattern::Constructor(name, args)
                        if name == "Err"
                            && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(arg.node, Pattern::Wildcard))
                )
        ));
        assert!(err_assert.message.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_module_tests_block() -> Result<(), Vec<CompileError>> {
        let source = r#"
def add(a: int, b: int) -> int:
  return a + b

module tests:
  from testing import assert_eq

  def test_add() -> None:
    assert add(1, 2) == 3
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 2);
        let test_module = require_test_module_decl(&program.declarations[1])?;
        assert_eq!(test_module.name, "tests");
        assert_eq!(test_module.body.len(), 2);
        assert!(matches!(test_module.body[0].node, Declaration::Import(_)));
        assert!(matches!(test_module.body[1].node, Declaration::Function(_)));
        Ok(())
    }

    #[test]
    fn test_duplicate_module_tests_block_is_error() {
        let source = r#"
module tests:
  pass

module tests:
  pass
"#;
        let errors = parse_str_err(source, "duplicate module tests block should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Only one `module tests:` block is allowed")),
            "expected duplicate module tests error, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_unexpected_indent_at_toplevel_is_single_clear_error() {
        // We intentionally allow the lexer to emit INDENT/DEDENT tokens at the top-level.
        // The parser should produce a single clear error and avoid cascading failures.
        let source = "  x = 1\n";
        let Err(err) = parse_str(source) else {
            panic!("Top-level indentation should be rejected by the parser");
        };
        assert_eq!(err.len(), 1, "Parser should return exactly one error (no cascade)");
        assert!(
            err[0].message.contains("Expected declaration") && err[0].message.contains("Indent"),
            "Error message should clearly indicate the unexpected INDENT token; got: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_model() -> Result<(), Vec<CompileError>> {
        let source = r#"
model User:
  name: str
  age: int = 0
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        let m = require_model_decl(&program.declarations[0])?;
        assert_eq!(m.name, "User");
        assert_eq!(m.fields.len(), 2);
        assert!(m.traits.is_empty());
        Ok(())
    }

    #[test]
    fn test_parse_generic_method_type_params() -> Result<(), Vec<CompileError>> {
        let source = r#"
class Box:
  def get[T with Clone](self, value: T) -> T:
    return value
"#;
        let program = parse_str(source)?;
        let class = require_class_decl(&program.declarations[0])?;
        assert_eq!(class.methods.len(), 1);
        let method = &class.methods[0].node;
        assert_eq!(method.name, "get");
        assert_eq!(method.type_params.len(), 1);
        assert_eq!(method.type_params[0].name, "T");
        assert_eq!(method.type_params[0].bounds.len(), 1);
        assert_eq!(method.type_params[0].bounds[0].name, "Clone");
        Ok(())
    }

    #[test]
    fn test_parse_trait_bodyless_method_is_abstract() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait Serializer:
  def serialize(self, value: str) -> str
  def deserialize(self, data: str) -> str: ...
"#;
        let program = parse_str(source)?;
        let trait_decl = require_trait_decl(&program.declarations[0])?;
        assert_eq!(trait_decl.methods.len(), 2);
        assert_eq!(trait_decl.methods[0].node.name, "serialize");
        assert!(trait_decl.methods[0].node.body.is_none());
        assert_eq!(trait_decl.methods[1].node.name, "deserialize");
        assert!(trait_decl.methods[1].node.body.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_bodyless_methods_outside_traits_are_rejected() {
        for source in [
            "model User:\n  def name(self) -> str\n",
            "class User:\n  def name(self) -> str\n",
            "type UserId = newtype str:\n  def display(self) -> str\n",
            "enum Token:\n  Word\n  def text(self) -> str\n",
        ] {
            let errs = parse_str_err(source, "bodyless methods outside traits should fail");
            assert!(
                errs.iter()
                    .any(|err| err.message.contains("Expected ':' after method return type")),
                "expected method body diagnostic, got: {errs:?}"
            );
        }
    }

    #[test]
    fn test_parse_bodyless_trait_method_followed_by_docstring_is_rejected() {
        let source = r#"
trait Named:
  def name(self) -> str
    "Return the display name."
"#;
        parse_str_err(source, "bodyless trait method docstring should require a colon body");
    }

    #[test]
    fn test_parse_pub_class_preserves_authored_field_visibility() -> Result<(), Vec<CompileError>> {
        let source = r#"
pub class LazyFrame:
  _cursor: int
  pub schema: str
"#;
        let program = parse_str(source)?;
        let class = require_class_decl(&program.declarations[0])?;
        assert!(matches!(class.visibility, Visibility::Public));
        assert!(matches!(class.fields[0].node.visibility, Visibility::Private));
        assert!(matches!(class.fields[1].node.visibility, Visibility::Public));
        Ok(())
    }

    #[test]
    fn test_parse_method_multiline_receiver_allows_trailing_comma() -> Result<(), Vec<CompileError>> {
        let source = r#"
class Box:
  def get(
    self,
  ) -> int:
    return 1
"#;
        let program = parse_str(source)?;
        let class = require_class_decl(&program.declarations[0])?;
        assert_eq!(class.methods.len(), 1);
        let method = &class.methods[0].node;
        assert_eq!(method.name, "get");
        assert!(matches!(method.receiver, Some(Receiver::Immutable)));
        assert!(method.params.is_empty());
        Ok(())
    }

    #[test]
    fn test_parse_class_docstring() -> Result<(), Vec<CompileError>> {
        let source = r#"
class FieldInfo:
  """
  Compiler-provided field metadata returned by __fields__().
  Instances are immutable and read-only.
  """
  name: str
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        let c = require_class_decl(&program.declarations[0])?;
        assert_eq!(c.name, "FieldInfo");
        assert_eq!(
            c.docstring.as_deref(),
            Some(
                "\n  Compiler-provided field metadata returned by __fields__().\n  Instances are immutable and read-only.\n  "
            )
        );
        assert_eq!(c.fields.len(), 1);
        assert_eq!(c.fields[0].node.name, "name");
        Ok(())
    }

    #[test]
    fn test_parse_model_leading_docstring_stored_on_ast() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Widget:
  """Model-level narrative for tooling."""
  id: int
"#;
        let program = parse_str(source)?;
        let m = require_model_decl(&program.declarations[0])?;
        assert_eq!(m.docstring.as_deref(), Some("Model-level narrative for tooling."));
        assert_eq!(m.fields.len(), 1);
        assert_eq!(m.fields[0].node.name, "id");
        Ok(())
    }

    #[test]
    fn test_parse_enum_leading_docstring_stored_on_ast() -> Result<(), Vec<CompileError>> {
        let source = r#"
enum Color:
  """Semantic colors for UI."""
  Red
  Green
"#;
        let program = parse_str(source)?;
        let en = require_enum_decl(&program.declarations[0])?;
        assert_eq!(en.docstring.as_deref(), Some("Semantic colors for UI."));
        assert_eq!(en.variants.len(), 2);
        assert_eq!(en.variants[0].node.name, "Red");
        assert_eq!(en.variants[1].node.name, "Green");
        Ok(())
    }

    #[test]
    fn test_parse_block_leading_blank_lines_single_empty_line() -> Result<(), Vec<CompileError>> {
        let source = r#"def f() -> int:
    a = 1

    b = 2
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 2, "expected two statements");
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_parse_block_leading_blank_lines_collapses_multiple_empty_lines() -> Result<(), Vec<CompileError>> {
        let source = r#"def f() -> int:
    a = 1



    b = 2
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 2);
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_parse_block_preserves_blank_line_after_nested_suite() -> Result<(), Vec<CompileError>> {
        let source = r#"def f(items: list[int]) -> int:
    for item in items:
        value = item

    result = 1
    return result
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 3);
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 1);
        assert_eq!(func.body[2].leading_blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_parse_block_preserves_single_blank_line_between_sibling_if_statements() -> Result<(), Vec<CompileError>> {
        let source = r#"def f(a: bool, b: bool) -> None:
    if a:
        x = 1

    if b:
        y = 2

    z = 3
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 3);
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 1);
        assert_eq!(func.body[2].leading_blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_parse_block_does_not_invent_blank_line_between_sibling_if_statements() -> Result<(), Vec<CompileError>> {
        let source = r#"def f(a: bool, b: bool) -> None:
    if a:
        x = 1
    if b:
        y = 2
    z = 3
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 3);
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 0);
        assert_eq!(func.body[2].leading_blank_lines, 0);
        Ok(())
    }

    #[test]
    fn test_parse_block_preserves_single_blank_line_between_if_blocks_ending_in_match() -> Result<(), Vec<CompileError>> {
        let source = r#"def f(a: bool, b: bool, result: Result[int, str]) -> None:
    if a:
        match result:
            Ok(_) => return
            Err(err) => return

    if b:
        match result:
            Ok(_) => return
            Err(err) => return

    z = 3
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.body.len(), 3);
        assert_eq!(func.body[0].leading_blank_lines, 0);
        assert_eq!(func.body[1].leading_blank_lines, 1);
        assert_eq!(func.body[2].leading_blank_lines, 1);
        Ok(())
    }

    #[test]
    fn test_parse_model_field_metadata() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Account:
  type_ [alias="type", description="Account tier"]: str
  balance [description="Balance in cents"]: int
"#;
        let program = parse_str(source)?;
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            _ => panic!("Expected model"),
        };
        let type_field = &model.fields[0].node;
        assert_eq!(type_field.metadata.alias.as_deref(), Some("type"));
        assert_eq!(
            type_field.metadata.description.as_deref(),
            Some("Account tier")
        );
        let balance_field = &model.fields[1].node;
        assert_eq!(balance_field.metadata.alias, None);
        assert_eq!(
            balance_field.metadata.description.as_deref(),
            Some("Balance in cents")
        );
        Ok(())
    }

    #[test]
    fn test_parse_model_field_alias_sugar() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Account:
  type_ as "type": str
"#;
        let program = parse_str(source)?;
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            _ => panic!("Expected model"),
        };
        let field = &model.fields[0].node;
        assert_eq!(field.metadata.alias.as_deref(), Some("type"));
        assert_eq!(field.metadata.description, None);
        Ok(())
    }

    #[test]
    fn test_parse_model_field_alias_and_as_error() {
        let source = r#"
model Account:
  type_ [alias="type"] as "type": str
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected alias + as sugar to be rejected");
        };
        assert!(
            err[0]
                .message
                .contains("Cannot combine 'alias=\"...\"' with 'as \"...\"'"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_keyword_named_args_and_member_access() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(a: Foo) -> int:
  let x = Foo(type=1, class=2)
  return a.type
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function"),
        };
        let call_expr = match &func.body[0].node {
            Statement::Assignment(stmt) => match &stmt.value.node {
                Expr::Call(_, _, args) => args,
                _ => panic!("Expected call expression"),
            },
            _ => panic!("Expected assignment statement"),
        };
        assert!(matches!(call_expr[0], CallArg::Named(ref name, _) if name == "type"));
        assert!(matches!(call_expr[1], CallArg::Named(ref name, _) if name == "class"));
        let return_expr = match &func.body[1].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return"),
        };
        assert!(matches!(&return_expr.node, Expr::Field(_, name) if name == "type"));
        Ok(())
    }

    #[test]
    fn test_parse_rest_params_and_call_unpacking() -> Result<(), Vec<CompileError>> {
        let source = r#"
def collect(prefix: str, *items: int, **labels: str) -> int:
  return 0

def use(xs: list[int], kw: dict[str, str]) -> int:
  return collect("x", 1, *xs, name="demo", **kw)
"#;
        let program = parse_str(source)?;
        let collect = require_function_decl(&program.declarations[0])?;
        assert_eq!(collect.params[0].node.kind, ParamKind::Normal);
        assert_eq!(collect.params[1].node.kind, ParamKind::RestPositional);
        assert_eq!(collect.params[1].node.name, "items");
        assert_eq!(collect.params[2].node.kind, ParamKind::RestKeyword);
        assert_eq!(collect.params[2].node.name, "labels");

        let use_fn = require_function_decl(&program.declarations[1])?;
        let call_args = match &use_fn.body[0].node {
            Statement::Return(Some(expr)) => match &expr.node {
                Expr::Call(_, _, args) => args,
                _ => panic!("expected call expression"),
            },
            _ => panic!("expected return statement"),
        };
        assert!(matches!(call_args[0], CallArg::Positional(_)));
        assert!(matches!(call_args[1], CallArg::Positional(_)));
        assert!(matches!(call_args[2], CallArg::PositionalUnpack(_)));
        assert!(matches!(call_args[3], CallArg::Named(ref name, _) if name == "name"));
        assert!(matches!(call_args[4], CallArg::KeywordUnpack(_)));
        Ok(())
    }

    #[test]
    fn test_parse_list_and_dict_literal_spread_entries() -> Result<(), Vec<CompileError>> {
        let source = r#"
def use(xs: list[int], headers: dict[str, str]) -> None:
  values = [1, *xs, 4]
  merged = {"accept": "json", **headers}
"#;
        let program = parse_str(source)?;
        let use_fn = require_function_decl(&program.declarations[0])?;

        let list_entries = match &use_fn.body[0].node {
            Statement::Assignment(stmt) => match &stmt.value.node {
                Expr::List(entries) => entries,
                _ => panic!("expected list literal"),
            },
            _ => panic!("expected assignment statement"),
        };
        assert!(matches!(list_entries[0], ListEntry::Element(_)));
        assert!(matches!(list_entries[1], ListEntry::Spread(_)));
        assert!(matches!(list_entries[2], ListEntry::Element(_)));

        let dict_entries = match &use_fn.body[1].node {
            Statement::Assignment(stmt) => match &stmt.value.node {
                Expr::Dict(entries) => entries,
                _ => panic!("expected dict literal"),
            },
            _ => panic!("expected assignment statement"),
        };
        assert!(matches!(dict_entries[0], DictEntry::Pair(_, _)));
        assert!(matches!(dict_entries[1], DictEntry::Spread(_)));
        Ok(())
    }

    #[test]
    fn test_parse_collection_literal_spread_invalid_markers() {
        let list_errs = parse_str_err(
            "def f(xs: list[int]) -> None:\n  values = [**xs]\n",
            "list literal should reject dictionary spread marker",
        );
        assert!(
            list_errs
                .iter()
                .any(|err| err.message.contains("Invalid list spread marker `**`")),
            "expected invalid list spread marker diagnostic, got: {list_errs:?}"
        );

        let dict_errs = parse_str_err(
            "def f(xs: list[int]) -> None:\n  values = {*xs}\n",
            "dict literal should reject list spread marker",
        );
        assert!(
            dict_errs
                .iter()
                .any(|err| err.message.contains("Invalid dictionary spread marker `*`")),
            "expected invalid dictionary spread marker diagnostic, got: {dict_errs:?}"
        );
    }

    #[test]
    fn test_parse_pattern_named_key_keyword() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(a: Foo) -> int:
  match a:
    Foo(type=x) => return x
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function"),
        };
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        let arm = &arms[0].node;
        match &arm.pattern.node {
            Pattern::Constructor(name, args) => {
                assert_eq!(name, "Foo");
                assert!(matches!(
                    &args[0],
                    PatternArg::Named(field, pat)
                        if field == "type" && matches!(&pat.node, Pattern::Binding(b) if b == "x")
                ));
            }
            _ => panic!("Expected constructor pattern"),
        }
        Ok(())
    }

    /// Qualified unit variant patterns parse as `Type::Variant` in the AST (for Rust lowering); surface syntax uses `.`.
    #[test]
    fn test_parse_qualified_unit_pattern_stores_double_colon_in_ast() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(x: int) -> int:
  match x:
    Kind.Read =>
      return 1
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function"),
        };
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        let arm = &arms[0].node;
        match &arm.pattern.node {
            Pattern::Constructor(name, args) => {
                assert_eq!(name, "Kind::Read");
                assert!(args.is_empty());
            }
            _ => panic!("Expected constructor pattern"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_match_pattern_alternation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(kind: Kind) -> int:
  match kind:
    Kind.Read | Kind.Scan | _ => return 1
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function"),
        };
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        match &arms[0].node.pattern.node {
            Pattern::Or(patterns) => {
                assert_eq!(patterns.len(), 3);
                assert!(matches!(&patterns[0].node, Pattern::Constructor(name, args) if name == "Kind::Read" && args.is_empty()));
                assert!(matches!(&patterns[1].node, Pattern::Constructor(name, args) if name == "Kind::Scan" && args.is_empty()));
                assert!(matches!(&patterns[2].node, Pattern::Wildcard));
            }
            _ => panic!("Expected pattern alternation"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_grouped_pattern_alternation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(kind: Kind) -> int:
  match kind:
    (
      Kind.Read
      | Kind.Scan
      | _
    ) => return 1
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Expr(match_expr) = &func.body[0].node else {
            panic!("Expected match expression statement");
        };
        let Expr::Match(_, arms) = &match_expr.node else {
            panic!("Expected match expression");
        };
        match &arms[0].node.pattern.node {
            Pattern::Group(inner) => {
                assert!(matches!(&inner.node, Pattern::Or(patterns) if patterns.len() == 3));
            }
            _ => panic!("Expected grouped pattern alternation"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_if_let_condition() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(opt: Option[int]) -> int:
  if let Some(value) = opt:
    return value
  return 0
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let stmt = &func.body[0].node;
        let Statement::If(if_stmt) = stmt else {
            panic!("Expected if statement");
        };
        match &if_stmt.condition {
            Condition::Let { pattern, value } => {
                assert!(matches!(&value.node, Expr::Ident(name) if name == "opt"));
                match &pattern.node {
                    Pattern::Constructor(name, args) => {
                        assert_eq!(name, "Some");
                        assert!(matches!(
                            &args[0],
                            PatternArg::Positional(pat)
                                if matches!(&pat.node, Pattern::Binding(binding) if binding == "value")
                        ));
                    }
                    _ => panic!("Expected constructor pattern"),
                }
            }
            Condition::Expr(_) => panic!("Expected let condition"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_if_let_pattern_alternation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(result: Result[int, int]) -> int:
  if let Ok(value) | Err(value) = result:
    return value
  return 0
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::If(if_stmt) = &func.body[0].node else {
            panic!("Expected if statement");
        };
        match &if_stmt.condition {
            Condition::Let { pattern, value } => {
                let Expr::Ident(value_name) = &value.node else {
                    panic!("Expected identifier value");
                };
                assert_eq!(value_name, "result");
                match &pattern.node {
                    Pattern::Or(patterns) => {
                        assert_eq!(patterns.len(), 2);
                        assert!(matches!(
                            &patterns[0].node,
                            Pattern::Constructor(name, args)
                                if name == "Ok"
                                    && matches!(&args[0], PatternArg::Positional(pat) if matches!(&pat.node, Pattern::Binding(binding) if binding == "value"))
                        ));
                        assert!(matches!(
                            &patterns[1].node,
                            Pattern::Constructor(name, args)
                                if name == "Err"
                                    && matches!(&args[0], PatternArg::Positional(pat) if matches!(&pat.node, Pattern::Binding(binding) if binding == "value"))
                        ));
                    }
                    _ => panic!("Expected pattern alternation"),
                }
            }
            Condition::Expr(_) => panic!("Expected let condition"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_while_let_condition() -> Result<(), Vec<CompileError>> {
        let source = r#"
def drain(current: Option[int]) -> int:
  while let Some(value) = current:
    return value
  return 0
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let stmt = &func.body[0].node;
        let Statement::While(while_stmt) = stmt else {
            panic!("Expected while statement");
        };
        match &while_stmt.condition {
            Condition::Let { pattern, value } => {
                assert!(matches!(&value.node, Expr::Ident(name) if name == "current"));
                assert!(matches!(
                    &pattern.node,
                    Pattern::Constructor(name, args)
                        if name == "Some"
                            && matches!(
                                &args[0],
                                PatternArg::Positional(pat)
                                    if matches!(&pat.node, Pattern::Binding(binding) if binding == "value")
                            )
                ));
            }
            Condition::Expr(_) => panic!("Expected let condition"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_while_let_rejects_pattern_alternation() {
        let source = r#"
def drain(current: Result[int, int]) -> int:
  while let Ok(value) | Err(value) = current:
    return value
  return 0
"#;
        let errors = parse_str_err(source, "`while let` pattern alternation should fail");
        assert!(
            errors
                .iter()
                .any(|err| err.message.contains("Pattern alternation is only supported in match arms and if let patterns")),
            "expected `while let` pattern alternation rejection, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_if_let_rejects_else_branch() {
        let source = r#"
def f(opt: Option[int]) -> int:
  if let Some(value) = opt:
    return value
  else:
    return 0
"#;
        let errors = parse_str_err(source, "`if let` with else should fail");
        assert!(
            errors.iter().any(|err| err.message.contains("`if let` does not support `else` branches")),
            "expected `if let` else rejection, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_decorator_paths() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.web as web

@std.web.route("/")
def a() -> None:
  pass

@std::web::route("/b")
def b() -> None:
  pass

@web.route("/c")
def c() -> None:
  pass
"#;
        let program = parse_str(source)?;
        let funcs: Vec<_> = program
            .declarations
            .iter()
            .filter_map(|d| match &d.node {
                Declaration::Function(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(funcs.len(), 3);

        let dec_a = &funcs[0].decorators[0].node;
        assert_eq!(dec_a.path.segments, vec!["std", "web", "route"]);
        assert_eq!(dec_a.name, "route");

        let dec_b = &funcs[1].decorators[0].node;
        assert_eq!(dec_b.path.segments, vec!["std", "web", "route"]);
        assert_eq!(dec_b.name, "route");

        let dec_c = &funcs[2].decorators[0].node;
        assert_eq!(dec_c.path.segments, vec!["web", "route"]);
        assert_eq!(dec_c.name, "route");
        Ok(())
    }

    #[test]
    fn test_parse_unknown_decorators_on_functions_async_defs_and_methods() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.async

@logged
def sync_func() -> None:
  pass

@traced
async def async_func() -> None:
  pass

class Service:
  value: int

  @cached
  def read(self) -> int:
    return self.value
"#;
        let program = parse_str(source)?;
        let funcs: Vec<_> = program
            .declarations
            .iter()
            .filter_map(|d| match &d.node {
                Declaration::Function(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].decorators[0].node.name, "logged");
        assert_eq!(funcs[1].decorators[0].node.name, "traced");

        let class = program
            .declarations
            .iter()
            .find_map(|d| match &d.node {
                Declaration::Class(c) => Some(c),
                _ => None,
            })
            .ok_or_else(|| vec![CompileError::new("expected class declaration".to_string(), Span::default())])?;
        assert_eq!(class.methods[0].node.decorators[0].node.name, "cached");
        Ok(())
    }

    #[test]
    fn test_parse_namespaced_decorator_with_named_args() -> Result<(), Vec<CompileError>> {
        // RFC 022: Namespaced decorators with positional + named arguments
        let source = r#"
from std.web import POST
import std.async

@std.web.route("/things", methods=[POST])
async def create() -> None:
  pass
"#;
        let program = parse_str(source)?;
        let funcs: Vec<_> = program
            .declarations
            .iter()
            .filter_map(|d| match &d.node {
                Declaration::Function(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(funcs.len(), 1);

        let dec = &funcs[0].decorators[0].node;
        assert_eq!(dec.path.segments, vec!["std", "web", "route"]);
        assert_eq!(dec.name, "route");
        assert_eq!(dec.args.len(), 2);
        // Positional: "/"
        assert!(matches!(&dec.args[0], DecoratorArg::Positional(_)));
        // Named: methods=[POST]
        assert!(matches!(&dec.args[1], DecoratorArg::Named(name, _) if name == "methods"));
        Ok(())
    }

    #[test]
    fn test_parse_decorator_with_rust_namespace() -> Result<(), Vec<CompileError>> {
        // RFC 023: @rust.extern decorator must parse correctly (rust is a keyword)
        let source = r#"
@rust.extern
def foo() -> None:
  pass
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        assert_eq!(func.decorators.len(), 1);
        let dec = &func.decorators[0].node;
        assert_eq!(dec.path.segments, vec!["rust", "extern"]);
        assert_eq!(dec.name, "extern");
        Ok(())
    }

    #[test]
    fn test_parse_import_path_with_async_segment() -> Result<(), Vec<CompileError>> {
        let source = r#"
from std.async.time import sleep
"#;
        let program = parse_str(source)?;
        let decl = match &program.declarations[0].node {
            Declaration::Import(import) => import,
            _ => panic!("Expected import declaration"),
        };
        let ImportKind::From { module, .. } = &decl.kind else {
            panic!("Expected from-import");
        };
        assert_eq!(module.segments, vec!["std", "async", "time"]);
        Ok(())
    }

    #[test]
    fn test_parse_async_requires_std_async_import() {
        let source = r#"
async def foo() -> None:
  pass
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected async function without std.async import to fail");
        };
        assert!(
            err[0].message.contains("only available after importing `std.async`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_async_with_std_async_import_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.async

async def foo() -> None:
  pass
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[1].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function declaration"),
        };
        assert!(func.is_async());
        Ok(())
    }

    #[test]
    fn test_parse_async_fixture_with_yield_ok() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
import std.async
from std.testing import fixture

@fixture(scope="function")
async def resource() -> int:
  yield 1
"#;
        let program = parse_str(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        let Declaration::Function(func) = &program.declarations[2].node else {
            return Err(std::io::Error::other("expected function declaration").into());
        };
        assert!(func.is_async());
        assert!(matches!(
            &func.body[0].node,
            Statement::Expr(expr) if matches!(expr.node, Expr::Yield(Some(_)))
        ));
        Ok(())
    }

    #[test]
    fn test_parse_await_with_std_async_import_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
from std.async.time import sleep

async def foo() -> None:
  await sleep(1.0)
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[1].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function declaration"),
        };
        assert!(matches!(
            &func.body[0].node,
            Statement::Expr(expr)
                if matches!(
                    expr.node,
                    Expr::Surface(ref surface)
                        if matches!(
                            surface.payload,
                            SurfaceExprPayload::PrefixUnary(_)
                        )
                )
        ));
        Ok(())
    }

    #[test]
    fn test_parse_async_identifier_without_import_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
def value(async: int) -> int:
  return async
"#;
        parse_str(source)?;
        Ok(())
    }

    #[test]
    fn test_parse_property_identifier_without_member_context_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
def value(property: int) -> int:
  return property
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert_eq!(func.params[0].node.name, "property");
        Ok(())
    }

    #[test]
    fn test_parse_class_computed_property() -> Result<(), Vec<CompileError>> {
        let source = r#"
class Dataset:
  property schema_fields -> list[str]:
    return self._fields
"#;
        let program = parse_str(source)?;
        let class = require_class_decl(&program.declarations[0])?;
        assert_eq!(class.properties.len(), 1);
        let property = &class.properties[0];
        assert_eq!(property.node.name, "schema_fields");
        assert!(matches!(property.node.visibility, Visibility::Private));
        assert!(
            matches!(property.node.return_type.node, Type::Generic(ref name, _) if collections::from_str(name) == Some(CollectionTypeId::List))
        );
        assert!(property.node.body.is_some());
        assert!(
            property.span.start < property.node.return_type.span.start
                && property.node.return_type.span.end <= property.span.end,
            "property span should enclose return type span"
        );
        Ok(())
    }

    #[test]
    fn test_parse_model_pub_computed_property() -> Result<(), Vec<CompileError>> {
        let source = r#"
model User:
  name: str

  pub property display_name -> str:
    return self.name
"#;
        let program = parse_str(source)?;
        let model = require_model_decl(&program.declarations[0])?;
        assert_eq!(model.fields.len(), 1);
        assert_eq!(model.properties.len(), 1);
        let property = &model.properties[0].node;
        assert_eq!(property.name, "display_name");
        assert!(matches!(property.visibility, Visibility::Public));
        assert!(property.body.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_trait_abstract_computed_property() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait HasArea:
  property area -> float: ...
"#;
        let program = parse_str(source)?;
        let trait_decl = require_trait_decl(&program.declarations[0])?;
        assert_eq!(trait_decl.properties.len(), 1);
        let property = &trait_decl.properties[0].node;
        assert_eq!(property.name, "area");
        assert!(matches!(property.return_type.node, Type::Simple(ref name) if name == "float"));
        assert!(property.body.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_trait_abstract_computed_property_without_ellipsis() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait HasArea:
  property area -> float
"#;
        let program = parse_str(source)?;
        let trait_decl = require_trait_decl(&program.declarations[0])?;
        assert_eq!(trait_decl.properties.len(), 1);
        assert!(trait_decl.properties[0].node.body.is_none());
        Ok(())
    }

    #[test]
    fn test_parse_class_abstract_property_is_focused_error() {
        let errs = parse_str_err(
            "class Shape:\n  property area -> float\n",
            "bodyless properties outside traits should fail",
        );
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Expected ':' after property return type")),
            "expected property body diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn test_parse_property_parameter_list_is_focused_error() {
        let errs = parse_str_err(
            "class Shape:\n  property area(self) -> float:\n    return 0.0\n",
            "property declarations with parameter lists should fail",
        );
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Computed properties do not accept parameter lists")),
            "expected property parameter diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn test_parse_property_declaration_modifier_is_focused_error() {
        let errs = parse_str_err(
            "import std.async\n\nclass Worker:\n  pub async property status -> str:\n    return \"ready\"\n",
            "property declarations with async modifiers should fail",
        );
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Declaration modifiers are not supported on properties")),
            "expected property modifier diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn test_parse_assert_without_std_testing_import_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(x: int) -> None:
  assert x > 0
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        assert!(matches!(&func.body[0].node, Statement::Assert(_)));
        Ok(())
    }

    #[test]
    fn test_parse_assert_with_std_testing_import_still_uses_core_assert_ast() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.testing

def f(x: int) -> None:
  assert x > 0, "x must be positive"
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[1])?;
        let Statement::Assert(assert_stmt) = &func.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assert statement".to_string(),
                func.body[0].span,
            )]);
        };
        assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
        assert!(assert_stmt.message.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_async_method_requires_std_async_import() {
        let source = r#"
class Worker:
  async def run(self) -> None:
    pass
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected async method without std.async import to fail");
        };
        assert!(
            err[0].message.contains("only available after importing `std.async`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_async_trait_method_requires_std_async_import() {
        let source = r#"
trait Worker:
  async def run(self) -> None:
    ...
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected async trait method without std.async import to fail");
        };
        assert!(
            err[0].message.contains("only available after importing `std.async`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_trait_with_docstring() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait Debug:
    """Debug representation."""
    def __repr__(self) -> str: ...
"#;
        let program = parse_str(source)?;
        let tr = require_trait_decl(&program.declarations[0])?;
        assert_eq!(tr.name, "Debug");
        assert_eq!(tr.docstring.as_deref(), Some("Debug representation."));
        assert_eq!(tr.methods.len(), 1);
        assert_eq!(tr.methods[0].node.name, "__repr__");
        Ok(())
    }

    #[test]
    fn test_parse_trait_with_supertraits() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait DataSet[T]:
    def len(self) -> int: ...

trait BoundedDataSet[T] with DataSet[T]:
    def sorted(self) -> Self: ...

trait Combo with BoundedDataSet[int], DataSet[str]:
    def go(self) -> None: ...
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 3);

        let ds = require_trait_decl(&program.declarations[0])?;
        assert_eq!(ds.name, "DataSet");
        assert!(ds.traits.is_empty());

        let bounded = require_trait_decl(&program.declarations[1])?;
        assert_eq!(bounded.name, "BoundedDataSet");
        assert_eq!(bounded.traits.len(), 1);
        assert_eq!(bounded.traits[0].node.name, "DataSet");
        assert_eq!(bounded.traits[0].node.type_args.len(), 1);
        assert_eq!(require_simple_type(&bounded.traits[0].node.type_args[0])?, "T");

        let combo = require_trait_decl(&program.declarations[2])?;
        assert_eq!(combo.name, "Combo");
        assert_eq!(combo.traits.len(), 2);
        assert_eq!(combo.traits[0].node.name, "BoundedDataSet");
        assert_eq!(combo.traits[0].node.type_args.len(), 1);
        assert_eq!(combo.traits[1].node.name, "DataSet");
        assert_eq!(combo.traits[1].node.type_args.len(), 1);
        Ok(())
    }

    #[test]
    fn test_parse_newtype_with_docstring() -> Result<(), Vec<CompileError>> {
        let source = r#"
type UserId[T] = newtype int:
    """Opaque identifier wrapper."""

    def raw(self) -> int:
        return 1
"#;
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert_eq!(nt.name, "UserId");
        assert_eq!(nt.type_params.len(), 1);
        assert_eq!(nt.docstring.as_deref(), Some("Opaque identifier wrapper."));
        assert_eq!(nt.methods.len(), 1);
        assert_eq!(nt.methods[0].node.name, "raw");
        assert!(!nt.is_rusttype);
        assert!(nt.rebindings.is_empty());
        assert!(nt.interop_edges.is_empty());
        Ok(())
    }

    #[test]
    fn test_parse_rusttype_with_rebinding_and_interop() -> Result<(), Vec<CompileError>> {
        let source = r#"
type Sender[T] = rusttype RustSender[T]:
    send_now = try_send

    interop:
        from str try Sender.parse
        into bytes via Sender.encode
"#;
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert!(nt.is_rusttype);
        assert_eq!(nt.rebindings.len(), 1);
        assert_eq!(nt.rebindings[0].node.name, "send_now");
        assert_eq!(nt.interop_edges.len(), 2);
        assert!(matches!(
            nt.interop_edges[0].node.direction,
            InteropDirection::From
        ));
        assert!(matches!(
            nt.interop_edges[0].node.adapter_kind,
            InteropAdapterKind::Try
        ));
        assert!(matches!(
            nt.interop_edges[1].node.direction,
            InteropDirection::Into
        ));
        assert!(matches!(
            nt.interop_edges[1].node.adapter_kind,
            InteropAdapterKind::Via
        ));
        Ok(())
    }

    #[test]
    fn test_parse_rusttype_with_trait_adoption_method_target_and_associated_type() -> Result<(), Vec<CompileError>> {
        let source = r#"
type UserId = rusttype i64 with Display, Debug:
    type Output for Add[int] = UserId

    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        pass
"#;
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert!(nt.is_rusttype);
        assert_eq!(nt.traits.len(), 2);
        assert_eq!(nt.traits[0].node.name, "Display");
        assert_eq!(nt.traits[1].node.name, "Debug");

        assert_eq!(nt.associated_types.len(), 1);
        let associated_type = &nt.associated_types[0].node;
        assert_eq!(associated_type.name, "Output");
        assert_eq!(associated_type.trait_target.node.name, "Add");
        assert_eq!(associated_type.trait_target.node.type_args.len(), 1);
        assert_eq!(require_simple_type(&associated_type.trait_target.node.type_args[0])?, "int");
        assert!(matches!(associated_type.ty.node, Type::Simple(ref name) if name == "UserId"));

        assert_eq!(nt.methods.len(), 1);
        let method = &nt.methods[0].node;
        assert_eq!(method.name, "fmt");
        let Some(target) = &method.trait_target else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected method trait target".to_string(),
                nt.methods[0].span,
            )]);
        };
        assert_eq!(target.node.name, "Display");
        assert!(target.node.type_args.is_empty());
        assert!(matches!(
            method.return_type.node,
            Type::Generic(ref name, _) if name == collections::as_str(CollectionTypeId::Result)
        ));
        Ok(())
    }

    #[test]
    fn test_parse_method_trait_target_after_return_type_is_rejected() {
        let source = r#"
type UserId = rusttype i64 with Display:
    def fmt(self, f: Formatter) -> Result[None, FmtError] for Display:
        pass
"#;
        let Err(errors) = parse_str(source) else {
            panic!("method trait target after return type should be rejected");
        };
        assert!(
            errors
                .iter()
                .any(|err| err.message.contains("Method trait target must appear before the return type")),
            "expected focused method trait target placement diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn test_parse_rusttype_minimal() -> Result<(), Vec<CompileError>> {
        let source = r#"
type Email = rusttype RustEmailAddress
"#;
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert!(nt.is_rusttype);
        assert!(nt.traits.is_empty());
        assert!(nt.associated_types.is_empty());
        assert!(nt.methods.is_empty());
        assert!(nt.rebindings.is_empty());
        assert!(nt.interop_edges.is_empty());
        Ok(())
    }

    #[test]
    fn test_parse_rusttype_qualified_underlying() -> Result<(), Vec<CompileError>> {
        let source = "type MyBin = rusttype proto_type::Binary\n";
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert!(nt.is_rusttype);
        assert!(matches!(
            &nt.underlying.node,
            Type::Qualified(segs) if segs == &vec!["proto_type".to_string(), "Binary".to_string()]
        ));
        Ok(())
    }

    #[test]
    fn test_parse_into_try_interop_edge() -> Result<(), Vec<CompileError>> {
        let source = r#"
type Email = rusttype RustEmailAddress:
    def try_into_str(self) -> Result[str, str]:
        ...

    interop:
        into str try Email.try_into_str
"#;
        let program = parse_str(source)?;
        let nt = require_newtype_decl(&program.declarations[0])?;
        assert_eq!(nt.interop_edges.len(), 1);
        assert!(matches!(
            nt.interop_edges[0].node.direction,
            InteropDirection::Into
        ));
        assert!(matches!(
            nt.interop_edges[0].node.adapter_kind,
            InteropAdapterKind::Try
        ));
        Ok(())
    }

    #[test]
    fn test_parse_interop_edge_missing_via_or_try_is_error() {
        let source = r#"
type Email = rusttype RustEmailAddress:
    interop:
        from str Email.parse
"#;
        let result = parse_str(source);
        assert!(
            result.is_err(),
            "expected parser error for missing `via`/`try` in interop edge"
        );
        let errs = match result {
            Err(errs) => errs,
            Ok(_) => Vec::new(),
        };
        assert!(
            errs.iter().any(|e| e.message.contains("Expected `via` or `try` in interop edge")),
            "expected missing-adapter-kind parser error, got: {errs:?}"
        );
    }

    #[test]
    fn test_parse_interop_edge_missing_from_or_into_is_error() {
        let source = r#"
type Email = rusttype RustEmailAddress:
    interop:
        str via Email.parse
"#;
        let result = parse_str(source);
        assert!(
            result.is_err(),
            "expected parser error for missing `from`/`into` in interop edge"
        );
        let errs = match result {
            Err(errs) => errs,
            Ok(_) => Vec::new(),
        };
        assert!(
            errs.iter().any(|e| e.message.contains("Expected `from` or `into` in interop edge")),
            "expected missing-direction parser error, got: {errs:?}"
        );
    }

    #[test]
    fn test_parse_non_identifier_alias() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Weird:
  one_ [alias="1"]: int
"#;
        let program = parse_str(source)?;
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            _ => panic!("Expected model"),
        };
        let field = &model.fields[0].node;
        assert_eq!(field.metadata.alias.as_deref(), Some("1"));
        Ok(())
    }

    #[test]
    fn test_parse_duplicate_metadata_key_error() {
        // RFC 021: Duplicate metadata keys are compile-time errors
        let source = r#"
model Account:
  type_ [alias="a", alias="b"]: str
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected duplicate alias key error");
        };
        assert!(
            err[0].message.contains("Duplicate 'alias'"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_duplicate_description_key_error() {
        // RFC 021: Duplicate metadata keys are compile-time errors
        let source = r#"
model Account:
  type_ [description="a", description="b"]: str
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected duplicate description key error");
        };
        assert!(
            err[0].message.contains("Duplicate 'description'"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_unknown_metadata_key_error() {
        // RFC 021: Any other keys are compile-time errors
        let source = r#"
model Account:
  type_ [unknown="value"]: str
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected unknown metadata key error");
        };
        assert!(
            err[0].message.contains("Unknown field metadata key"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_non_string_metadata_value_error() {
        // RFC 021: Values must be string literals
        let source = r#"
model Account:
  type_ [alias=123]: str
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected non-string metadata value error");
        };
        // Parser should fail because it expects a string literal
        assert!(
            err[0].message.contains("string") || err[0].message.contains("Expected"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_model_with_traits() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait Describable:
  def describe(self) -> str: ...

model User with Describable:
  name: str
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 2);
        match &program.declarations[1].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.traits.len(), 1);
                assert_eq!(m.traits[0].node.name, "Describable");
            }
            _ => panic!("Expected model"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_model_with_multiple_traits() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait A:
  def a(self) -> int: ...

trait B:
  def b(self) -> int: ...

model User with A, B:
  x: int
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 3);
        match &program.declarations[2].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.traits.len(), 2);
                assert_eq!(m.traits[0].node.name, "A");
                assert_eq!(m.traits[1].node.name, "B");
            }
            _ => panic!("Expected model"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_model_with_generic_trait_adoption_named_from() -> Result<(), Vec<CompileError>> {
        let source = r#"
trait From[T]:
  @classmethod
  def from(cls, value: T) -> Self: ...

model UserId with From[int]:
  value: int
"#;
        let program = parse_str(source)?;
        match &program.declarations[1].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "UserId");
                assert_eq!(m.traits.len(), 1);
                assert_eq!(m.traits[0].node.name, "From");
                assert_eq!(m.traits[0].node.type_args.len(), 1);
            }
            _ => panic!("Expected model"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_function() -> Result<(), Vec<CompileError>> {
        let source = r#"
def add(a: int, b: int) -> int:
  return a + b
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("Expected function"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_function_multiline_params_allow_trailing_comma() -> Result<(), Vec<CompileError>> {
        let source = r#"
def identity(
  value: int,
) -> int:
  return value
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.name, "identity");
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].node.name, "value");
            }
            _ => panic!("Expected function"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_import() -> Result<(), Vec<CompileError>> {
        let source = "import polars::prelude as pl";
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Import(i) => {
                match &i.kind {
                    ImportKind::Module(path) => {
                        assert_eq!(path.segments, vec!["polars".to_string(), "prelude".to_string()]);
                        assert_eq!(path.parent_levels, 0);
                        assert!(!path.is_absolute);
                    }
                    _ => panic!("Expected module import"),
                }
                assert_eq!(i.alias, Some("pl".to_string()));
            }
            _ => panic!("Expected import"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_pub_from_in_src_lib_is_public_reexport() -> Result<(), Vec<CompileError>> {
        let source = "pub from widgets import Widget, Layout as UiLayout\n";
        let program = parse_str_with_module_path(source, Some("project/src/lib.incn"))?;
        assert_eq!(program.declarations.len(), 1);

        let Declaration::Import(import) = &program.declarations[0].node else {
            panic!("Expected import declaration");
        };
        assert!(matches!(import.visibility, Visibility::Public));
        let ImportKind::From { module, items } = &import.kind else {
            panic!("Expected from-import");
        };
        assert_eq!(module.segments, vec!["widgets".to_string()]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Widget");
        assert_eq!(items[0].alias, None);
        assert_eq!(items[1].name, "Layout");
        assert_eq!(items[1].alias, Some("UiLayout".to_string()));
        Ok(())
    }

    #[test]
    fn test_parse_pub_from_in_src_main_is_public_reexport() -> Result<(), Vec<CompileError>> {
        let source = "pub from widgets import Widget\n";
        let program = parse_str_with_module_path(source, Some("project/src/main.incn"))?;
        assert_eq!(program.declarations.len(), 1);

        let Declaration::Import(import) = &program.declarations[0].node else {
            panic!("Expected import declaration");
        };
        assert!(matches!(import.visibility, Visibility::Public));
        let ImportKind::From { module, items } = &import.kind else {
            panic!("Expected from-import");
        };
        assert_eq!(module.segments, vec!["widgets".to_string()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Widget");
        assert_eq!(items[0].alias, None);
        Ok(())
    }

    #[test]
    fn test_parse_pub_from_rust_in_src_module_is_public_reexport() -> Result<(), Vec<CompileError>> {
        let source = "pub from rust::time import Instant\n";
        let program = parse_str_with_module_path(source, Some("project/src/session/mod.incn"))?;
        assert_eq!(program.declarations.len(), 1);

        let Declaration::Import(import) = &program.declarations[0].node else {
            panic!("Expected import declaration");
        };
        assert!(matches!(import.visibility, Visibility::Public));
        let ImportKind::RustFrom {
            crate_name,
            path,
            items,
            ..
        } = &import.kind
        else {
            panic!("Expected rust from-import");
        };
        assert_eq!(crate_name, "time");
        assert!(path.is_empty());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Instant");
        assert_eq!(items[0].alias, None);
        Ok(())
    }

    #[test]
    fn test_parse_pub_from_outside_src_is_error() {
        let source = "pub from widgets import Widget\n";
        let result = parse_str_with_module_path(source, Some("project/tests/test_main.incn"));
        assert!(result.is_err(), "Expected parser to reject `pub from` outside src/");
        let err = result.err().unwrap_or_default();
        assert!(
            err[0].message.contains("only valid in modules under `src/`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_pub_import_is_error() {
        let source = "pub import widgets\n";
        let result = parse_str_with_module_path(source, Some("project/src/lib.incn"));
        assert!(result.is_err(), "Expected parser to reject `pub import`");
        let err = result.err().unwrap_or_default();
        assert!(
            err[0].message.contains("only supported on `from ... import ...`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_rust_import_with_version_and_features() -> Result<(), Vec<CompileError>> {
        let source = r#"import rust::tokio @ "1.0" with ["full", "macros"] as rt"#;
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustCrate {
                    crate_name,
                    path,
                    version,
                    features,
                } => {
                    assert_eq!(crate_name, "tokio");
                    assert!(path.is_empty());
                    assert_eq!(version.as_deref(), Some("1.0"));
                    assert_eq!(features, &vec!["full".to_string(), "macros".to_string()]);
                    assert_eq!(i.alias, Some("rt".to_string()));
                }
                _ => panic!("Expected rust crate import"),
            },
            _ => panic!("Expected import"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_rust_from_with_version_and_features() -> Result<(), Vec<CompileError>> {
        let source = r#"from rust::time @ "0.3" with ["formatting"] import Instant"#;
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    version,
                    features,
                    items,
                } => {
                    assert_eq!(crate_name, "time");
                    assert!(path.is_empty());
                    assert_eq!(version.as_deref(), Some("0.3"));
                    assert_eq!(features, &vec!["formatting".to_string()]);
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].name, "Instant");
                }
                _ => panic!("Expected rust from import"),
            },
            _ => panic!("Expected import"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_rust_import_with_features_requires_version() {
        let source = r#"import rust::tokio with ["full"]"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected rust import features to require version");
        };
        assert!(
            err[0].message.contains("features require a version"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_pub_library_import_with_alias() -> Result<(), Vec<CompileError>> {
        let source = "import pub::mylib as lib\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::PubLibrary { library } => {
                    assert_eq!(library, "mylib");
                    assert_eq!(i.alias.as_deref(), Some("lib"));
                }
                _ => panic!("Expected pub library import"),
            },
            _ => panic!("Expected import"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_pub_from_import_parenthesized_items() -> Result<(), Vec<CompileError>> {
        let source = "from pub::mylib import (\n    Widget,\n    make_widget as build_widget,\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::PubFrom { library, items } => {
                    assert_eq!(library, "mylib");
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Widget");
                    assert_eq!(items[0].alias, None);
                    assert_eq!(items[1].name, "make_widget");
                    assert_eq!(items[1].alias.as_deref(), Some("build_widget"));
                }
                _ => panic!("Expected pub from import"),
            },
            _ => panic!("Expected import"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_pub_import_dot_notation_is_error() {
        let source = "from pub.mylib import Widget\n";
        let Err(err) = parse_str(source) else {
            panic!("Expected parser to reject dot-notation `pub` import");
        };
        assert!(
            err[0].message.contains("Expected `::` after `pub`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_pub_import_nested_path_is_error() {
        let source = "from pub::mylib::widgets import Widget\n";
        let Err(err) = parse_str(source) else {
            panic!("Expected parser to reject nested `pub::` path");
        };
        assert!(
            err[0].message.contains("single library name"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    /// RFC 005: `from rust.crate import Item` emits a warning and parses successfully.
    #[test]
    fn test_parse_rust_from_import_dot_notation_is_warning() {
        let source = "from rust.chrono import Utc\n";
        let Ok(program) = parse_str(source) else {
            panic!("`from rust.crate import ...` dot-notation should parse successfully with a warning");
        };
        assert_eq!(program.warnings.len(), 1, "Expected exactly one warning");
        assert!(
            program.warnings[0].message.contains("::"),
            "Expected warning to mention '::' notation; got: {}",
            program.warnings[0].message
        );
    }

    /// RFC 005: `import rust.crate` emits a warning and parses successfully.
    #[test]
    fn test_parse_rust_import_dot_notation_is_warning() {
        let source = "import rust.serde_json\n";
        let Ok(program) = parse_str(source) else {
            panic!("`import rust.crate` dot-notation should parse successfully with a warning");
        };
        assert_eq!(program.warnings.len(), 1, "Expected exactly one warning");
        assert!(
            program.warnings[0].message.contains("::"),
            "Expected warning to mention '::' notation; got: {}",
            program.warnings[0].message
        );
    }

    /// RFC 005: `import rust.std.time` (multi-segment bare import, dot path) recovers fully.
    ///
    /// Mirrors `test_parse_rust_from_import_multi_dot_notation_is_warning` but for the bare  `import rust.X.Y` form,
    /// ensuring `rust_crate_path()` dot-recovery works on both branches.
    #[test]
    fn test_parse_rust_import_multi_dot_notation_is_warning() {
        let source = "import rust.std.time\n";
        let Ok(program) = parse_str(source) else {
            panic!("`import rust.std.time` multi-dot dot-notation should parse successfully with a warning");
        };
        assert_eq!(program.warnings.len(), 1, "Expected exactly one warning for the leading dot");
        assert!(
            program.warnings[0].message.contains("::"),
            "Expected warning to mention '::' notation; got: {}",
            program.warnings[0].message
        );
        // Verify the path was correctly decomposed: crate=std, path=[time]
        if let Some(decl) = program.declarations.first()
            && let crate::ast::Declaration::Import(import) = &decl.node
        {
            assert!(
                matches!(
                    &import.kind,
                    crate::ast::ImportKind::RustCrate { crate_name, path, .. }
                    if crate_name == "std" && path == &["time".to_string()]
                ),
                "Expected RustCrate {{ crate_name: std, path: [time] }}; got: {:?}",
                import.kind
            );
        }
    }

    /// RFC 005: `from rust.std.time import Instant` (multi-segment dot path) recovers fully.
    ///
    /// `rust_crate_path()` accepts both `::` and `.` as separators, so the entire dotted path is consumed and no
    /// cascading parse error occurs.
    #[test]
    fn test_parse_rust_from_import_multi_dot_notation_is_warning() {
        let source = "from rust.std.time import Instant\n";
        let Ok(program) = parse_str(source) else {
            panic!("`from rust.std.time import ...` multi-dot dot-notation should parse successfully with a warning");
        };
        assert_eq!(program.warnings.len(), 1, "Expected exactly one warning for the leading dot");
        assert!(
            program.warnings[0].message.contains("::"),
            "Expected warning to mention '::' notation; got: {}",
            program.warnings[0].message
        );
    }

    // ==============================================
    // Issue #116: parenthesized multi-line imports
    // ==============================================

    /// Single identifier in parentheses: `from db import (CategoryId)`.
    #[test]
    fn test_parse_from_import_parenthesized_single_item() -> Result<(), Vec<CompileError>> {
        let source = "from db import (CategoryId)\n";
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::From { items, .. } => {
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].name, "CategoryId");
                    assert_eq!(items[0].alias, None);
                }
                _ => panic!("Expected From import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Multiple identifiers in parentheses on one line: `from db import (CategoryId, TagId)`.
    #[test]
    fn test_parse_from_import_parenthesized_multi_item_single_line() -> Result<(), Vec<CompileError>> {
        let source = "from db import (CategoryId, TagId)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::From { items, .. } => {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "CategoryId");
                    assert_eq!(items[1].name, "TagId");
                }
                _ => panic!("Expected From import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Multi-line parenthesized import — the lexer drops newlines inside `(...)` so the parser sees the same token
    /// stream as the single-line version.
    #[test]
    fn test_parse_from_import_parenthesized_multi_line() -> Result<(), Vec<CompileError>> {
        let source = "from db import (\n    CategoryId,\n    TagId,\n    OtherId\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::From { items, .. } => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0].name, "CategoryId");
                    assert_eq!(items[1].name, "TagId");
                    assert_eq!(items[2].name, "OtherId");
                }
                _ => panic!("Expected From import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Trailing comma before `)` is allowed: `from db import (CategoryId, TagId,)`.
    #[test]
    fn test_parse_from_import_parenthesized_trailing_comma() -> Result<(), Vec<CompileError>> {
        let source = "from db import (CategoryId, TagId,)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::From { items, .. } => {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "CategoryId");
                    assert_eq!(items[1].name, "TagId");
                }
                _ => panic!("Expected From import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Items with `as` aliases in a parenthesized list.
    #[test]
    fn test_parse_from_import_parenthesized_with_aliases() -> Result<(), Vec<CompileError>> {
        let source = "from db import (\n    CategoryId as CatId,\n    TagId,\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::From { items, .. } => {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "CategoryId");
                    assert_eq!(items[0].alias, Some("CatId".to_string()));
                    assert_eq!(items[1].name, "TagId");
                    assert_eq!(items[1].alias, None);
                }
                _ => panic!("Expected From import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Missing `)` produces a parse error that mentions the closing delimiter.
    #[test]
    fn test_parse_from_import_parenthesized_unclosed_error() {
        let source = "from db import (CategoryId, TagId\n";
        let err = parse_str_err(source, "Unclosed import list should produce a parse error");
        assert!(
            err[0].message.contains(')') || err[0].message.to_lowercase().contains("close"),
            "Expected error to mention ')'; got: {}",
            err[0].message
        );
    }

    /// Empty parenthesized list `from db import ()` is a parse error.
    #[test]
    fn test_parse_from_import_empty_parens_error() {
        let source = "from db import ()\n";
        let err = parse_str_err(source, "Empty import list should produce a parse error");
        assert!(
            err[0].message.to_lowercase().contains("empty") || err[0].message.to_lowercase().contains("cannot"),
            "Expected 'empty' diagnostic; got: {}",
            err[0].message
        );
    }

    /// `from rust::...` also supports parenthesized items.
    #[test]
    fn test_parse_rust_from_import_parenthesized() -> Result<(), Vec<CompileError>> {
        let source = "from rust::serde_json import (\n    Value,\n    Map,\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom { crate_name, items, .. } => {
                    assert_eq!(crate_name, "serde_json");
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Value");
                    assert_eq!(items[1].name, "Map");
                }
                _ => panic!("Expected RustFrom import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Mixed aliased/non-aliased items work in parenthesized `from rust::` imports.
    #[test]
    fn test_parse_rust_from_import_parenthesized_mixed_aliases() -> Result<(), Vec<CompileError>> {
        let source = "from rust::polars import (\n    DataFrame,\n    Series as S,\n    LazyFrame as LF,\n    Expr,\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom { crate_name, items, .. } => {
                    assert_eq!(crate_name, "polars");
                    assert_eq!(items.len(), 4);
                    assert_eq!(items[0].name, "DataFrame");
                    assert_eq!(items[0].alias, None);
                    assert_eq!(items[1].name, "Series");
                    assert_eq!(items[1].alias, Some("S".to_string()));
                    assert_eq!(items[2].name, "LazyFrame");
                    assert_eq!(items[2].alias, Some("LF".to_string()));
                    assert_eq!(items[3].name, "Expr");
                    assert_eq!(items[3].alias, None);
                }
                _ => panic!("Expected RustFrom import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// `from rust::...` with version/feature specifiers also supports parenthesized items.
    #[test]
    fn test_parse_rust_from_import_with_version_and_parens() -> Result<(), Vec<CompileError>> {
        let source = "from rust::serde_json @ \"1.0\" with [\"derive\"] import (\n    Value,\n    Map,\n)\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom { crate_name, version, features, items, .. } => {
                    assert_eq!(crate_name, "serde_json");
                    assert_eq!(version.as_deref(), Some("1.0"));
                    assert_eq!(features, &["derive".to_string()]);
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Value");
                    assert_eq!(items[1].name, "Map");
                }
                _ => panic!("Expected RustFrom import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// Rust item paths may use module names that are Incan keywords (e.g. Substrait `proto::type`).
    #[test]
    fn test_parse_rust_from_import_path_type_keyword_segment() -> Result<(), Vec<CompileError>> {
        let source = "from rust::substrait::proto::type import Binary, Boolean\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom { crate_name, path, items, .. } => {
                    assert_eq!(crate_name, "substrait");
                    assert_eq!(path, &["proto".to_string(), "type".to_string()]);
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Binary");
                    assert_eq!(items[1].name, "Boolean");
                }
                _ => panic!("Expected RustFrom import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_rust_import_crate_path_type_keyword_segment() -> Result<(), Vec<CompileError>> {
        let source = "import rust::substrait::proto::type::Binary\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustCrate {
                    crate_name,
                    path,
                    version,
                    features,
                } => {
                    assert_eq!(crate_name, "substrait");
                    assert_eq!(
                        path,
                        &[
                            "proto".to_string(),
                            "type".to_string(),
                            "Binary".to_string()
                        ]
                    );
                    assert!(version.is_none());
                    assert!(features.is_empty());
                }
                _ => panic!("Expected RustCrate import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    /// `from rust::... import` may name Rust items that are Incan keywords (e.g. `type` module).
    #[test]
    fn test_parse_rust_from_import_keyword_item_with_alias() -> Result<(), Vec<CompileError>> {
        let source = "from rust::substrait::proto import type as proto_type\n";
        let program = parse_str(source)?;
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom { crate_name, path, items, .. } => {
                    assert_eq!(crate_name, "substrait");
                    assert_eq!(path, &["proto".to_string()]);
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].name, "type");
                    assert_eq!(items[0].alias.as_deref(), Some("proto_type"));
                }
                _ => panic!("Expected RustFrom import"),
            },
            _ => panic!("Expected import declaration"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_from_import_rejects_keyword_item_name_for_incan_modules() {
        let source = "from db import type\n";
        let result = parse_str(source);
        assert!(result.is_err(), "expected parse error for keyword import item on Incan from-import");
    }

    #[test]
    fn test_parse_match() -> Result<(), Vec<CompileError>> {
        let source = r#"
def handle(opt: Option[int]) -> int:
  match opt:
    case Some(x):
      return x
    case None:
      return 0
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        Ok(())
    }

    #[test]
    fn test_parse_match_fat_arrow_inline_return() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f() -> int:
  match Ok(1):
    Ok(x) => return x
    Err(_) => return 0
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function declaration"),
        };
        assert_eq!(func.body.len(), 1);
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        assert_eq!(arms.len(), 2);
        for arm in arms {
            match &arm.node.body {
                MatchBody::Block(stmts) => {
                    assert_eq!(stmts.len(), 1);
                    assert!(matches!(stmts[0].node, Statement::Return(_)));
                }
                MatchBody::Expr(_) => panic!("Expected inline return to parse as statement block"),
            }
        }
        Ok(())
    }

    #[test]
    fn test_parse_match_fat_arrow_inline_compound_assignment() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f() -> str:
  mut out = ""
  match 1:
    1 => out += "a"
    _ => out += "b"
  return out
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function declaration"),
        };
        assert_eq!(func.body.len(), 3);
        let match_expr = match &func.body[1].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        assert_eq!(arms.len(), 2);
        for arm in arms {
            match &arm.node.body {
                MatchBody::Block(stmts) => {
                    assert_eq!(stmts.len(), 1);
                    assert!(matches!(
                        stmts[0].node,
                        Statement::CompoundAssignment(_)
                    ));
                }
                MatchBody::Expr(_) => panic!("Expected inline compound assignment to parse as statement block"),
            }
        }
        Ok(())
    }

    #[test]
    fn test_parse_match_fat_arrow_block_allows_blank_before_body() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f() -> int:
  match Err("bad"):
    Ok(x) =>
      return x
    Err(err) =>

      return 0
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function declaration"),
        };
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[1].node.body, MatchBody::Block(ref stmts) if stmts.len() == 1));
        Ok(())
    }

    #[test]
    fn test_parse_match_arm_suite_does_not_inherit_outer_blank_line_intent() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(result: Result[int, str]) -> int:
  match result:
    Ok(value) => match value:
      Ready(x) => return x

      Failed(err) => return 0

    Err(err) =>
      return 1
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function declaration"),
        };
        let match_expr = match &func.body[0].node {
            Statement::Expr(expr) => expr,
            _ => panic!("Expected match expression statement"),
        };
        let arms = match &match_expr.node {
            Expr::Match(_, arms) => arms,
            _ => panic!("Expected match expression"),
        };
        let err_body = match &arms[1].node.body {
            MatchBody::Block(stmts) => stmts,
            _ => panic!("Expected block match body"),
        };
        assert_eq!(err_body.len(), 1);
        assert_eq!(
            err_body[0].leading_blank_lines, 0,
            "outer Err arm body should not inherit the preserved gap from the nested Ok arm"
        );
        Ok(())
    }

    #[test]
    fn test_parse_if_elif_else_allows_blank_before_suite_body() -> Result<(), Vec<CompileError>> {
        let source = r#"
def f(kind: str) -> int:
  if kind == "a":
    return 1
  elif kind == "b":

    return 2
  else:

    return 3
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function declaration"),
        };
        assert_eq!(func.body.len(), 1);
        assert!(matches!(func.body[0].node, Statement::If(_)));
        Ok(())
    }

    #[test]
    fn test_parse_const_decl() -> Result<(), Vec<CompileError>> {
        let source = r#"
const ANSWER: int = 42
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Const(c) => {
                assert_eq!(c.name, "ANSWER");
            }
            _ => panic!("Expected const"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_implicit_derives_metadata_decl() -> Result<(), Vec<CompileError>> {
        let source = r#"
__derives__ = [Serialize, Deserialize]
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        let Declaration::Const(c) = &program.declarations[0].node else {
            panic!("Expected __derives__ metadata as const declaration");
        };
        assert_eq!(c.name, "__derives__");
        assert!(c.ty.is_none());
        let Expr::List(entries) = &c.value.node else {
            panic!("Expected __derives__ value to parse as list literal");
        };
        assert_eq!(entries.len(), 2);
        Ok(())
    }

    #[test]
    fn test_parse_module_qualified_trait_bound() -> Result<(), Vec<CompileError>> {
        let source = r#"
def encode[T with json.Serialize](value: T) -> str:
  return value.to_json()
"#;
        let program = parse_str(source)?;
        let Declaration::Function(func) = &program.declarations[0].node else {
            panic!("Expected function declaration");
        };
        assert_eq!(func.type_params.len(), 1);
        assert_eq!(func.type_params[0].bounds.len(), 1);
        assert_eq!(func.type_params[0].bounds[0].name, "json.Serialize");
        Ok(())
    }

    #[test]
    fn test_parse_decimal_literal() -> Result<(), Vec<CompileError>> {
        let source = r#"
const PRICE = 19.99d
"#;
        let program = parse_str(source)?;
        let Declaration::Const(c) = &program.declarations[0].node else {
            panic!("Expected const");
        };
        let Expr::Literal(Literal::Decimal(value)) = &c.value.node else {
            panic!("Expected decimal literal");
        };
        assert_eq!(value.body, "19.99");
        assert_eq!(value.repr, "19.99d");
        Ok(())
    }

    #[test]
    fn test_parse_decimal_type_arguments() -> Result<(), Vec<CompileError>> {
        let source = r#"
const PRICE: decimal[10, 2] = 19.99d
"#;
        let program = parse_str(source)?;
        let Declaration::Const(c) = &program.declarations[0].node else {
            panic!("Expected const");
        };
        let Some(ty) = &c.ty else {
            panic!("Expected const type annotation");
        };
        let Type::Generic(name, args) = &ty.node else {
            panic!("Expected generic decimal type");
        };
        assert_eq!(name, "decimal");
        assert_eq!(args.len(), 2);
        assert!(matches!(&args[0].node, Type::IntLiteral(value) if value.value == 10));
        assert!(matches!(&args[1].node, Type::IntLiteral(value) if value.value == 2));
        Ok(())
    }

    #[test]
    fn test_parse_constrained_primitive_int_single_constraint() -> Result<(), Vec<CompileError>> {
        let source = "type NonNegativeInt = newtype int[ge=0]\n";
        let program = parse_str(source)?;
        let newtype = require_newtype_decl(&program.declarations[0])?;
        let Type::ConstrainedPrimitive(name, constraints) = &newtype.underlying.node else {
            panic!("Expected constrained primitive type, got: {:?}", newtype.underlying.node);
        };
        assert_eq!(name, "int");
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].node.key, TypeConstraintKey::Ge);
        assert_eq!(constraints[0].node.value.value, 0);
        assert_eq!(constraints[0].node.value.repr, "0");
        Ok(())
    }

    #[test]
    fn test_parse_constrained_primitive_multiple_constraints() -> Result<(), Vec<CompileError>> {
        let source = "type Digit = newtype int[gt=-1, lt=10]\n";
        let program = parse_str(source)?;
        let newtype = require_newtype_decl(&program.declarations[0])?;
        let Type::ConstrainedPrimitive(name, constraints) = &newtype.underlying.node else {
            panic!("Expected constrained primitive type, got: {:?}", newtype.underlying.node);
        };
        assert_eq!(name, "int");
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].node.key, TypeConstraintKey::Gt);
        assert_eq!(constraints[0].node.value.value, -1);
        assert_eq!(constraints[0].node.value.repr, "-1");
        assert_eq!(constraints[1].node.key, TypeConstraintKey::Lt);
        assert_eq!(constraints[1].node.value.value, 10);
        Ok(())
    }

    #[test]
    fn test_parse_constrained_primitive_float_accepts_integer_literal_constraint() -> Result<(), Vec<CompileError>> {
        let source = "type UnitRatio = newtype float[ge=0, le=1]\n";
        let program = parse_str(source)?;
        let newtype = require_newtype_decl(&program.declarations[0])?;
        let Type::ConstrainedPrimitive(name, constraints) = &newtype.underlying.node else {
            panic!("Expected constrained primitive type, got: {:?}", newtype.underlying.node);
        };
        assert_eq!(name, "float");
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].node.key, TypeConstraintKey::Ge);
        assert_eq!(constraints[1].node.key, TypeConstraintKey::Le);
        Ok(())
    }

    #[test]
    fn test_parse_constrained_primitive_rejects_duplicate_key() {
        let source = "type Bad = newtype int[ge=0, ge=1]\n";
        let errs = parse_str_err(source, "duplicate constrained primitive key should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Duplicate constrained primitive key `ge`")),
            "Expected duplicate constraint key error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_constrained_primitive_rejects_unsupported_key() {
        let source = "type Bad = newtype int[min=0]\n";
        let errs = parse_str_err(source, "unsupported constrained primitive key should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Unsupported constrained primitive key `min`")),
            "Expected unsupported constraint key error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_constrained_primitive_rejects_empty_constraint_block() {
        let source = "type Bad = newtype int[]\n";
        let errs = parse_str_err(source, "empty constrained primitive block should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("requires at least one constraint")),
            "Expected empty constraint block error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_constrained_primitive_rejects_second_constraint_block() {
        let source = "type Bad = newtype int[ge=0][lt=10]\n";
        let errs = parse_str_err(source, "second constrained primitive block should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Only one constraint block is allowed")),
            "Expected second constraint block error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_constrained_primitive_rejects_non_integer_literal_value() {
        let source = "type Bad = newtype int[ge=MIN_VALUE]\n";
        let errs = parse_str_err(source, "non-literal constrained primitive value should fail");
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Expected integer literal constraint value")),
            "Expected integer literal constraint value error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_static_decl() -> Result<(), Vec<CompileError>> {
        let source = r#"
pub static counter: int = 0
"#;
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Static(static_decl) => {
                assert_eq!(static_decl.name, "counter");
                assert_eq!(static_decl.visibility, Visibility::Public);
            }
            _ => panic!("Expected static"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_static_requires_type_annotation() {
        let source = "static counter = 0\n";
        let Err(errors) = parse_str(source) else {
            panic!("expected parse error");
        };
        assert!(errors.iter().any(|error| error.message.contains("requires an explicit type annotation")));
    }

    #[test]
    fn test_parse_static_requires_initializer() {
        let source = "static counter: int\n";
        let Err(errors) = parse_str(source) else {
            panic!("expected parse error");
        };
        assert!(errors.iter().any(|error| error.message.contains("requires an initializer")));
    }

    #[test]
    fn test_parse_static_rejected_in_function_body() {
        let source = r#"
def main() -> int:
  static counter: int = 0
  return counter
"#;
        let Err(errors) = parse_str(source) else {
            panic!("expected parse error");
        };
        assert!(errors.iter().any(|error| error.message.contains("only allowed at module scope")));
    }

    #[test]
    fn test_parse_fstring_expr_spans_multiple_interpolations() -> Result<(), Vec<CompileError>> {
        let source = "def greet(name: str, title: str) -> str:\n  return f\"Hello {title} {name}\"\n";
        let program = parse_str(source)?;

        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };

        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return with expression"),
        };

        let parts = match &return_expr.node {
            Expr::FString(parts) => parts,
            _ => panic!("Expected f-string expression"),
        };

        let first_expected_start = match source.find("{title}") {
            Some(start) => start,
            None => panic!("Could not find first interpolation in source"),
        };
        let second_expected_start = match source.find("{name}") {
            Some(start) => start,
            None => panic!("Could not find second interpolation in source"),
        };

        let first_expr = match &parts[1] {
            FStringPart::Expr(expr) => expr,
            _ => panic!("Expected first interpolation expression"),
        };
        assert_eq!(first_expr.span.start, first_expected_start);
        assert_eq!(first_expr.span.end, first_expected_start + "{title}".len());

        let second_expr = match &parts[3] {
            FStringPart::Expr(expr) => expr,
            _ => panic!("Expected second interpolation expression"),
        };
        assert_eq!(second_expr.span.start, second_expected_start);
        assert_eq!(second_expr.span.end, second_expected_start + "{name}".len());

        Ok(())
    }

    #[test]
    fn test_parse_fstring_expr_span_nested_expression() -> Result<(), Vec<CompileError>> {
        let source = "def calc(x: int, y: int, z: int) -> str:\n  return f\"value: {x + y * z}\"\n";
        let program = parse_str(source)?;

        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };

        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return with expression"),
        };

        let parts = match &return_expr.node {
            Expr::FString(parts) => parts,
            _ => panic!("Expected f-string expression"),
        };

        let expected_start = match source.find("{x + y * z}") {
            Some(start) => start,
            None => panic!("Could not find interpolation in source"),
        };

        let interpolation = match &parts[1] {
            FStringPart::Expr(expr) => expr,
            _ => panic!("Expected interpolation expression"),
        };

        assert_eq!(interpolation.span.start, expected_start);
        assert_eq!(interpolation.span.end, expected_start + "{x + y * z}".len());
        assert!(matches!(interpolation.node, Expr::Binary(_, _, _)));

        Ok(())
    }

    #[test]
    fn test_parse_fstring_expr_span_method_call_with_index() -> Result<(), Vec<CompileError>> {
        let source = "def render(users: List[str]) -> str:\n  return f\"user: {users[unknown_idx].upper()}\"\n";
        let program = parse_str(source)?;

        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };

        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return with expression"),
        };

        let parts = match &return_expr.node {
            Expr::FString(parts) => parts,
            _ => panic!("Expected f-string expression"),
        };

        let expected_start = match source.find("{users[unknown_idx].upper()}") {
            Some(start) => start,
            None => panic!("Could not find interpolation in source"),
        };

        let interpolation = match &parts[1] {
            FStringPart::Expr(expr) => expr,
            _ => panic!("Expected interpolation expression"),
        };
        assert_eq!(interpolation.span.start, expected_start);
        assert_eq!(
            interpolation.span.end,
            expected_start + "{users[unknown_idx].upper()}".len()
        );

        let (base, method, args) = match &interpolation.node {
            Expr::MethodCall(base, method, _, args) => (base, method, args),
            _ => panic!("Expected method call interpolation"),
        };
        assert_eq!(method, "upper");
        assert!(args.is_empty());

        let (_, index) = match &base.node {
            Expr::Index(collection, index) => {
                assert!(matches!(collection.node, Expr::Ident(ref name) if name == "users"));
                (collection, index)
            }
            _ => panic!("Expected index expression as method base"),
        };

        let expected_index_start = match source.find("unknown_idx") {
            Some(start) => start,
            None => panic!("Could not find unknown_idx in source"),
        };
        assert!(matches!(index.node, Expr::Ident(ref name) if name == "unknown_idx"));
        assert_eq!(index.span.start, expected_index_start);
        assert_eq!(index.span.end, expected_index_start + "unknown_idx".len());

        Ok(())
    }

    #[test]
    fn test_parse_function_call_with_explicit_type_args() -> Result<(), Vec<CompileError>> {
        let source = "def run() -> int:\n  return id[int](1)\n";
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return expression"),
        };
        match &return_expr.node {
            Expr::Call(callee, type_args, args) => {
                assert!(matches!(callee.node, Expr::Ident(ref name) if name == "id"));
                assert_eq!(type_args.len(), 1);
                assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
                assert_eq!(args.len(), 1);
            }
            other => panic!("Expected explicit-generic call, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_method_call_with_explicit_type_args() -> Result<(), Vec<CompileError>> {
        let source = "def run(box: Boxed[int]) -> int:\n  return box.get[int]()\n";
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return expression"),
        };
        match &return_expr.node {
            Expr::MethodCall(base, method, type_args, args) => {
                assert!(matches!(base.node, Expr::Ident(ref name) if name == "box"));
                assert_eq!(method, "get");
                assert_eq!(type_args.len(), 1);
                assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
                assert!(args.is_empty());
            }
            other => panic!("Expected explicit-generic method call, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_function_call_with_infer_type_arg_placeholder() -> Result<(), Vec<CompileError>> {
        let source = "def run() -> int:\n  return pair_map[int, _](1, 2)\n";
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return expression"),
        };
        match &return_expr.node {
            Expr::Call(callee, type_args, args) => {
                assert!(matches!(callee.node, Expr::Ident(ref name) if name == "pair_map"));
                assert_eq!(type_args.len(), 2);
                assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
                assert!(matches!(type_args[1].node, Type::Infer));
                assert_eq!(args.len(), 2);
            }
            other => panic!("Expected explicit-generic call with infer, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_method_call_with_infer_type_arg_placeholder() -> Result<(), Vec<CompileError>> {
        let source = "def run(box: Boxed[int]) -> int:\n  return box.unwrap[int, _](0)\n";
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return expression"),
        };
        match &return_expr.node {
            Expr::MethodCall(base, method, type_args, args) => {
                assert!(matches!(base.node, Expr::Ident(ref name) if name == "box"));
                assert_eq!(method, "unwrap");
                assert_eq!(type_args.len(), 2);
                assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
                assert!(matches!(type_args[1].node, Type::Infer));
                assert_eq!(args.len(), 1);
            }
            other => panic!("Expected explicit-generic method call with infer, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_fstring_expr_span_list_comp_filter_call() -> Result<(), Vec<CompileError>> {
        let source = "def render(items: List[int]) -> str:\n  return f\"values: {[x for x in items if unknown_pred(x)]}\"\n";
        let program = parse_str(source)?;

        let function = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };

        let return_expr = match &function.body[0].node {
            Statement::Return(Some(expr)) => expr,
            _ => panic!("Expected return with expression"),
        };

        let parts = match &return_expr.node {
            Expr::FString(parts) => parts,
            _ => panic!("Expected f-string expression"),
        };

        let expected_start = match source.find("{[x for x in items if unknown_pred(x)]}") {
            Some(start) => start,
            None => panic!("Could not find interpolation in source"),
        };

        let interpolation = match &parts[1] {
            FStringPart::Expr(expr) => expr,
            _ => panic!("Expected interpolation expression"),
        };
        assert_eq!(interpolation.span.start, expected_start);
        assert_eq!(
            interpolation.span.end,
            expected_start + "{[x for x in items if unknown_pred(x)]}".len()
        );

        let comp = match &interpolation.node {
            Expr::ListComp(comp) => comp,
            _ => panic!("Expected list comprehension interpolation"),
        };
        let filter = match &comp.filter {
            Some(filter) => filter,
            None => panic!("Expected list comprehension filter"),
        };
        let callee = match &filter.node {
            Expr::Call(callee, _, _args) => callee,
            _ => panic!("Expected filter call expression"),
        };

        let expected_callee_start = match source.find("unknown_pred") {
            Some(start) => start,
            None => panic!("Could not find unknown_pred in source"),
        };
        assert!(matches!(callee.node, Expr::Ident(ref name) if name == "unknown_pred"));
        assert_eq!(callee.span.start, expected_callee_start);
        assert_eq!(callee.span.end, expected_callee_start + "unknown_pred".len());

        Ok(())
    }

    #[test]
    fn test_parse_generator_expression_full_clause_shape() -> Result<(), Vec<CompileError>> {
        let source = "def run(xs: list[int], ys: list[int]) -> Generator[int]:\n  return (x * y for x in xs if x > 0 for y in ys if y > x)\n";
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let Statement::Return(Some(expr)) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return statement".to_string(),
                function.body[0].span,
            )]);
        };
        let Expr::Generator(generator) = &expr.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected generator expression".to_string(),
                expr.span,
            )]);
        };

        assert_eq!(generator.clauses.len(), 4);
        let ComprehensionClause::For { pattern, iter } = &generator.clauses[0] else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected first for clause".to_string(),
                expr.span,
            )]);
        };
        assert_eq!(pattern.node, Pattern::Binding("x".to_string()));
        assert!(matches!(iter.node, Expr::Ident(ref name) if name == "xs"));
        assert!(matches!(generator.clauses[1], ComprehensionClause::If(_)));
        let ComprehensionClause::For { pattern, iter } = &generator.clauses[2] else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected second for clause".to_string(),
                expr.span,
            )]);
        };
        assert_eq!(pattern.node, Pattern::Binding("y".to_string()));
        assert!(matches!(iter.node, Expr::Ident(ref name) if name == "ys"));
        assert!(matches!(generator.clauses[3], ComprehensionClause::If(_)));
        Ok(())
    }

    #[test]
    fn test_parse_generator_expression_tuple_unpack_binding() -> Result<(), Vec<CompileError>> {
        let source = "def names(xs: list[str]) -> Generator[str]:\n  return (name for idx, name in enumerate(xs))\n";
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let Statement::Return(Some(expr)) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return statement".to_string(),
                function.body[0].span,
            )]);
        };
        let Expr::Generator(generator) = &expr.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected generator expression".to_string(),
                expr.span,
            )]);
        };

        let ComprehensionClause::For { pattern, .. } = &generator.clauses[0] else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected for clause".to_string(),
                expr.span,
            )]);
        };
        let Pattern::Tuple(items) = &pattern.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected tuple binding pattern".to_string(),
                pattern.span,
            )]);
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
        assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
        Ok(())
    }

    #[test]
    fn test_parse_generator_function_yield_compatibility() -> Result<(), Vec<CompileError>> {
        let source = "def count() -> Generator[int]:\n  yield 1\n";
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        if !matches!(
            &function.body[0].node,
            Statement::Expr(expr) if matches!(expr.node, Expr::Yield(Some(_)))
        ) {
            return Err(vec![CompileError::new(
                "parser test internal error: expected yield expression statement".to_string(),
                function.body[0].span,
            )]);
        };
        Ok(())
    }

    // ========================================================================
    // Enum diagnostic tests (#113)
    // ========================================================================

    #[test]
    fn test_enum_fat_arrow_mapping_rejected_with_hint() {
        let source = "enum Categories:\n    GROCERIES => Category(\"Groceries\")\n";
        let Err(err) = parse_str(source) else {
            panic!("Fat arrow in enum body should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("mapped values"),
            "Expected hint about mapped values, got: {msg}"
        );
    }

    #[test]
    fn test_enum_dotted_variant_rejected_with_hint() {
        let source = "enum FlowType:\n    Cash.Inflow\n";
        let Err(err) = parse_str(source) else {
            panic!("Dotted variant in enum body should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("cannot contain dots"),
            "Expected hint about dots, got: {msg}"
        );
    }

    #[test]
    fn test_enum_assigned_value_rejected_with_hint() {
        let source = "enum Color:\n    Red = 1\n";
        let Err(err) = parse_str(source) else {
            panic!("Assigned value in enum body should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("assigned values"),
            "Expected hint about assigned values, got: {msg}"
        );
    }

    #[test]
    fn test_parse_value_enum_with_string_values() -> Result<(), Vec<CompileError>> {
        let source = "enum Color(str):\n    Red = \"red\"\n    Blue = \"blue\"\n    Cyan = alias Blue\n";
        let program = parse_str(source)?;
        let en = require_enum_decl(&program.declarations[0])?;

        assert!(matches!(en.value_type.as_ref().map(|ty| ty.node), Some(ValueEnumType::Str)));
        assert_eq!(en.variants.len(), 2);
        assert_eq!(en.variant_aliases.len(), 1);
        assert_eq!(en.variants[0].node.name, "Red");
        assert!(en.variants[0].node.fields.is_empty());
        assert!(matches!(
            en.variants[0].node.value.as_ref().map(|value| &value.node),
            Some(ValueEnumLiteral::Str(value)) if value == "red"
        ));
        assert!(matches!(
            en.variants[1].node.value.as_ref().map(|value| &value.node),
            Some(ValueEnumLiteral::Str(value)) if value == "blue"
        ));
        assert_eq!(en.variant_aliases[0].node.name, "Cyan");
        assert_eq!(en.variant_aliases[0].node.target, "Blue");
        Ok(())
    }

    #[test]
    fn test_parse_value_enum_with_integer_values() -> Result<(), Vec<CompileError>> {
        let source = "enum Status(int):\n    Pending = 1\n    Done = 2\n";
        let program = parse_str(source)?;
        let en = require_enum_decl(&program.declarations[0])?;

        assert!(matches!(en.value_type.as_ref().map(|ty| ty.node), Some(ValueEnumType::Int)));
        assert_eq!(en.variants.len(), 2);
        assert!(matches!(
            en.variants[0].node.value.as_ref().map(|value| &value.node),
            Some(ValueEnumLiteral::Int(value)) if value.value == 1
        ));
        assert!(matches!(
            en.variants[1].node.value.as_ref().map(|value| &value.node),
            Some(ValueEnumLiteral::Int(value)) if value.value == 2
        ));
        Ok(())
    }

    #[test]
    fn test_value_enum_variant_requires_explicit_value() {
        let source = "enum Color(str):\n    Red\n";
        let Err(err) = parse_str(source) else {
            panic!("Value enum variant without assigned value should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("explicit literal values"),
            "Expected hint about explicit literal values, got: {msg}"
        );
    }

    #[test]
    fn test_value_enum_variant_payload_rejected() {
        let source = "enum Color(str):\n    Red(str) = \"red\"\n";
        let Err(err) = parse_str(source) else {
            panic!("Value enum variant payload should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("cannot carry tuple or struct payloads"),
            "Expected hint about value enum payloads, got: {msg}"
        );
    }

    #[test]
    fn test_value_enum_variant_literal_type_must_match_header() {
        let source = "enum Color(str):\n    Red = 1\n";
        let Err(err) = parse_str(source) else {
            panic!("Value enum variant with wrong literal type should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("Expected string literal value"),
            "Expected hint about string literal values, got: {msg}"
        );
    }

    #[test]
    fn test_value_enum_header_type_must_be_str_or_int() {
        let source = "enum Color(float):\n    Red = 1\n";
        let Err(err) = parse_str(source) else {
            panic!("Value enum with unsupported backing type should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("must be 'str' or 'int'"),
            "Expected hint about value enum backing types, got: {msg}"
        );
    }

    #[test]
    fn test_enum_colon_annotation_rejected_with_hint() {
        let source = "enum Fields:\n    Name: str\n";
        let Err(err) = parse_str(source) else {
            panic!("Type annotation in enum body should be rejected");
        };
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("type annotations"),
            "Expected hint about type annotations, got: {msg}"
        );
    }

    #[test]
    fn test_parse_enum_with_trait_adoption_and_method() -> Result<(), Vec<CompileError>> {
        let source = r#"
enum Lookup with Index[str, int]:
    Mapping
    Empty

    def __getitem__(self, key: str) -> int:
        return 0
"#;
        let program = parse_str(source)?;
        let en = require_enum_decl(&program.declarations[0])?;

        assert_eq!(en.name, "Lookup");
        assert_eq!(en.traits.len(), 1);
        assert_eq!(en.traits[0].node.name, "Index");
        assert_eq!(en.traits[0].node.type_args.len(), 2);
        assert_eq!(require_simple_type(&en.traits[0].node.type_args[0])?, "str");
        assert_eq!(require_simple_type(&en.traits[0].node.type_args[1])?, "int");
        assert_eq!(en.variants.len(), 2);
        assert_eq!(en.methods.len(), 1);
        assert_eq!(en.methods[0].node.name, "__getitem__");
        assert_eq!(en.methods[0].node.receiver, Some(Receiver::Immutable));
        assert_eq!(en.methods[0].node.params.len(), 1);
        assert!(en.methods[0].node.body.is_some());
        Ok(())
    }

    #[test]
    fn test_parse_value_enum_with_trait_adoption_after_value_type() -> Result<(), Vec<CompileError>> {
        let source = r#"
enum Env(str) with From[str]:
    Dev = "development"
    Prod = "production"

    @classmethod
    def from(cls, value: str) -> Self:
        return Env.Dev
"#;
        let program = parse_str(source)?;
        let en = require_enum_decl(&program.declarations[0])?;

        assert!(matches!(en.value_type.as_ref().map(|ty| ty.node), Some(ValueEnumType::Str)));
        assert_eq!(en.traits.len(), 1);
        assert_eq!(en.traits[0].node.name, "From");
        assert_eq!(en.traits[0].node.type_args.len(), 1);
        assert_eq!(require_simple_type(&en.traits[0].node.type_args[0])?, "str");
        assert_eq!(en.variants.len(), 2);
        assert_eq!(en.methods.len(), 1);
        assert_eq!(en.methods[0].node.name, "from");
        assert_eq!(en.methods[0].node.receiver, None);
        assert_eq!(en.methods[0].node.params.len(), 1);
        assert_eq!(en.methods[0].node.decorators.len(), 1);
        Ok(())
    }

    #[test]
    fn test_valid_enum_still_parses() {
        let source = "enum Status:\n    Pending\n    Active\n    Done(str)\n";
        let Ok(program) = parse_str(source) else {
            panic!("Valid enum should parse");
        };
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Enum(e) => {
                assert!(e.value_type.is_none());
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0].node.name, "Pending");
                assert_eq!(e.variants[1].node.name, "Active");
                assert_eq!(e.variants[2].node.name, "Done");
                assert!(e.variants.iter().all(|variant| variant.node.value.is_none()));
                assert_eq!(e.variants[2].node.fields.len(), 1);
            }
            _ => panic!("Expected enum"),
        }
    }

    // ========================================
    // RFC 023: rust.module() directive parsing
    // ========================================

    #[test]
    fn test_rust_module_directive_basic() -> Result<(), Vec<CompileError>> {
        let source = "rust.module(\"incan_stdlib::testing\")\n\ndef foo() -> int:\n    return 1\n";
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 1);
        let rmp = program.rust_module_path.as_ref();
        assert!(rmp.is_some(), "rust_module_path should be set");
        assert_eq!(rmp.map(|s| s.node.as_str()), Some("incan_stdlib::testing"));
        Ok(())
    }

    #[test]
    fn test_rust_module_directive_with_docstring() -> Result<(), Vec<CompileError>> {
        let source = "\"Module docstring\"\nrust.module(\"my_crate::sub\")\n\ndef bar() -> str:\n    return \"hi\"\n";
        let program = parse_str(source)?;
        assert_eq!(program.declarations.len(), 2); // docstring + function
        assert_eq!(
            program.rust_module_path.as_ref().map(|s| s.node.as_str()),
            Some("my_crate::sub")
        );
        Ok(())
    }

    #[test]
    fn test_rust_module_directive_absent() -> Result<(), Vec<CompileError>> {
        let source = "def foo() -> int:\n    return 1\n";
        let program = parse_str(source)?;
        assert!(program.rust_module_path.is_none());
        Ok(())
    }

    #[test]
    fn test_rust_module_directive_duplicate_is_error() {
        let source = "rust.module(\"crate_a\")\nrust.module(\"crate_b\")\n\ndef foo() -> int:\n    return 1\n";
        let Err(err) = parse_str(source) else {
            panic!("Duplicate rust.module() should fail");
        };
        let has_duplicate_msg = err.iter().any(|e| e.message.contains("Duplicate"));
        assert!(has_duplicate_msg, "Should report duplicate rust.module(); errors: {:?}", err.iter().map(|e| &e.message).collect::<Vec<_>>());
    }

    #[test]
    fn test_rust_module_directive_not_at_top_is_error() {
        let source = "def foo() -> int:\n    return 1\n\nrust.module(\"incan_stdlib::testing\")\n";
        let Err(err) = parse_str(source) else {
            panic!("rust.module() after declarations should fail");
        };
        let has_msg = err.iter().any(|e| e.message.contains("must appear at the top"));
        assert!(
            has_msg,
            "Should report rust.module() placement error; errors: {:?}",
            err.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    // ---- rust.module() edge case tests ----

    #[test]
    fn test_rust_module_missing_parens() {
        // `rust.module "foo"` — missing parentheses should produce a parse error.
        let source = "rust.module \"foo\"\n\ndef bar() -> int:\n    return 1\n";
        let result = parse_str(source);
        assert!(result.is_err(), "rust.module without parens should be an error");
    }

    #[test]
    fn test_rust_module_non_string_arg() {
        // `rust.module(42)` — non-string argument should produce a parse error.
        let source = "rust.module(42)\n\ndef bar() -> int:\n    return 1\n";
        let result = parse_str(source);
        assert!(result.is_err(), "rust.module with non-string arg should be an error");
    }

    #[test]
    fn test_rust_module_empty_string() {
        // `rust.module("")` — empty string should parse fine (validated later by typechecker).
        let source = "rust.module(\"\")\n\n@rust.extern\ndef bar() -> int:\n    ...\n";
        let result = parse_str(source);
        // Should parse OK; the empty path is caught by the typechecker's path validation.
        assert!(result.is_ok(), "rust.module with empty string should parse; errors: {:?}", result.err());
    }

    // ---- Type alias tests ----

    #[test]
    fn test_type_alias_simple() {
        // `type Foo = Bar` should parse as Declaration::TypeAlias, not Declaration::Newtype.
        let source = "type Foo = Bar\n";
        let prog = match parse_str(source) {
            Ok(program) => program,
            Err(errs) => panic!("simple type alias should parse: {errs:?}"),
        };
        assert_eq!(prog.declarations.len(), 1);
        assert!(
            matches!(prog.declarations[0].node, Declaration::TypeAlias(_)),
            "Expected TypeAlias, got: {:?}",
            prog.declarations[0].node
        );
    }

    #[test]
    fn test_type_alias_generic() {
        // `pub type Query[T] = AxumQuery[T]` should parse as a public TypeAlias.
        let source = "pub type Query[T] = AxumQuery[T]\n";
        let prog = match parse_str(source) {
            Ok(program) => program,
            Err(errs) => panic!("generic type alias should parse: {errs:?}"),
        };
        assert_eq!(prog.declarations.len(), 1);
        let Declaration::TypeAlias(alias) = &prog.declarations[0].node else {
            panic!("Expected TypeAlias, got: {:?}", prog.declarations[0].node);
        };
        assert_eq!(alias.name, "Query");
        assert!(matches!(alias.visibility, Visibility::Public));
        assert_eq!(alias.type_params.len(), 1);
        assert_eq!(alias.type_params[0].name, "T");
    }

    // ---- RFC 035: Callable[...] parser desugaring ----

    #[test]
    fn test_callable_single_param_desugars_to_function_type() -> Result<(), Vec<CompileError>> {
        let source = r#"
def apply(f: Callable[int, int], x: int) -> int:
  return f(x)
"#;
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(function) => function,
            _ => panic!("Expected function declaration"),
        };
        let first_param = &function.params[0].node;
        match &first_param.ty.node {
            Type::Function(params, ret) => {
                assert_eq!(params.len(), 1, "Callable[int, int] should desugar to one-arg function type");
                assert!(matches!(params[0].node, Type::Simple(ref name) if name == "int"));
                assert!(matches!(ret.node, Type::Simple(ref name) if name == "int"));
            }
            other => panic!("Expected desugared function type, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_callable_zero_param_desugars_to_function_type() -> Result<(), Vec<CompileError>> {
        let source = r#"
def invoke(f: Callable[(), int]) -> int:
  return f()
"#;
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(function) => function,
            _ => panic!("Expected function declaration"),
        };
        let first_param = &function.params[0].node;
        match &first_param.ty.node {
            Type::Function(params, ret) => {
                assert!(params.is_empty(), "Callable[(), int] should desugar to zero-arg function type");
                assert!(matches!(ret.node, Type::Simple(ref name) if name == "int"));
            }
            other => panic!("Expected desugared function type, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_callable_multi_param_desugars_to_function_type() -> Result<(), Vec<CompileError>> {
        let source = r#"
def check(f: Callable[(int, str), bool]) -> None:
  pass
"#;
        let program = parse_str(source)?;
        let function = match &program.declarations[0].node {
            Declaration::Function(function) => function,
            _ => panic!("Expected function declaration"),
        };
        let first_param = &function.params[0].node;
        match &first_param.ty.node {
            Type::Function(params, ret) => {
                assert_eq!(params.len(), 2, "Callable[(int, str), bool] should desugar to two-arg function type");
                assert!(matches!(params[0].node, Type::Simple(ref name) if name == "int"));
                assert!(matches!(params[1].node, Type::Simple(ref name) if name == "str"));
                assert!(matches!(ret.node, Type::Simple(ref name) if name == "bool"));
            }
            other => panic!("Expected desugared function type, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_function_type_accepts_reference_params() -> Result<(), Vec<CompileError>> {
        let source = r#"
class Box:
  value: int

def decorate(f: (&Box, &mut Box) -> int) -> (&Box) -> int:
  return f
"#;
        let program = parse_str(source)?;
        let function = match &program.declarations[1].node {
            Declaration::Function(function) => function,
            _ => panic!("Expected function declaration"),
        };
        let first_param = &function.params[0].node;
        match &first_param.ty.node {
            Type::Function(params, _) => {
                assert!(matches!(params[0].node, Type::Ref(_)));
                assert!(matches!(params[1].node, Type::RefMut(_)));
            }
            other => panic!("Expected function type, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_callable_invalid_arity_is_parse_error() {
        let source = r#"
def bad(f: Callable[int]) -> None:
  pass
"#;
        let Err(errs) = parse_str(source) else {
            panic!("Callable with invalid arity should fail to parse");
        };
        assert!(
            errs.iter()
                .any(|err| err.message.contains("Callable[...] expects exactly 2 type arguments")),
            "Expected Callable arity error, got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        );
    }

    // ---- RFC 029: union type parser normalization ----

    #[test]
    fn test_union_pipe_return_type_desugars_to_canonical_generic() -> Result<(), Vec<CompileError>> {
        let source = r#"
def parse_value(raw: str) -> int | str:
  return raw
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        match &function.return_type.node {
            Type::Generic(name, args) => {
                assert_eq!(name, "Union");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0].node, Type::Simple(ref name) if name == "int"));
                assert!(matches!(args[1].node, Type::Simple(ref name) if name == "str"));
            }
            other => panic!("Expected canonical Union generic, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_union_pipe_inside_generic_binds_looser_than_type_args() -> Result<(), Vec<CompileError>> {
        let source = r#"
def load_values() -> List[int | str]:
  return []
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        match &function.return_type.node {
            Type::Generic(name, args) => {
                assert_eq!(name, "List");
                assert_eq!(args.len(), 1);
                match &args[0].node {
                    Type::Generic(name, members) => {
                        assert_eq!(name, "Union");
                        assert_eq!(members.len(), 2);
                        assert!(matches!(members[0].node, Type::Simple(ref name) if name == "int"));
                        assert!(matches!(members[1].node, Type::Simple(ref name) if name == "str"));
                    }
                    other => panic!("Expected nested canonical Union generic, got: {other:?}"),
                }
            }
            other => panic!("Expected List generic, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_union_pipe_accepts_none_member_in_annotation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def maybe_name() -> str | None:
  return None
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        match &function.return_type.node {
            Type::Generic(name, args) => {
                assert_eq!(name, "Union");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0].node, Type::Simple(ref name) if name == "str"));
                assert!(matches!(args[1].node, Type::Simple(ref name) if name == "None"));
            }
            other => panic!("Expected canonical Union generic containing None, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_union_pipe_flattens_nested_union_and_preserves_duplicates() -> Result<(), Vec<CompileError>> {
        let source = r#"
def parse_value() -> int | Union[str, int] | None | str:
  return 1
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        match &function.return_type.node {
            Type::Generic(name, args) => {
                assert_eq!(name, "Union");
                let names: Vec<_> = args
                    .iter()
                    .map(|arg| match &arg.node {
                        Type::Simple(name) => name.as_str(),
                        other => panic!("Expected simple union member, got: {other:?}"),
                    })
                    .collect();
                assert_eq!(names, vec!["int", "str", "int", "None", "str"]);
            }
            other => panic!("Expected flattened canonical Union generic, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_is_not_none_parses_as_identity_negation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def has_name(name: str | None) -> bool:
  if name is not None:
    return true
  return false
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let Statement::If(if_stmt) = &function.body[0].node else {
            panic!("Expected first statement to be if, got: {:?}", function.body[0].node);
        };
        let Condition::Expr(condition) = &if_stmt.condition else {
            panic!("Expected expression condition");
        };
        match &condition.node {
            Expr::Binary(left, BinaryOp::IsNot, right) => {
                assert!(matches!(left.node, Expr::Ident(ref name) if name == "name"));
                assert!(matches!(right.node, Expr::Literal(Literal::None)));
            }
            other => panic!("Expected `is not None` binary expression, got: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_newtype_still_parses_with_newtype_keyword() {
        // `type Foo = newtype Bar` must still produce a Newtype.
        let source = "type Foo = newtype Bar\n";
        let prog = match parse_str(source) {
            Ok(program) => program,
            Err(errs) => panic!("newtype should parse: {errs:?}"),
        };
        assert_eq!(prog.declarations.len(), 1);
        assert!(
            matches!(prog.declarations[0].node, Declaration::Newtype(_)),
            "Expected Newtype, got: {:?}",
            prog.declarations[0].node
        );
    }

    #[test]
    fn test_rust_module_missing_closing_paren() {
        // `rust.module("foo"` — missing closing paren should produce a parse error.
        let source = "rust.module(\"foo\"\n\ndef bar() -> int:\n    return 1\n";
        let result = parse_str(source);
        assert!(result.is_err(), "rust.module with missing closing paren should be an error");
    }

    #[test]
    fn test_library_import_activates_soft_keywords() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::mylib\n\nasync def my_func() -> None:\n    pass\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        // Without context, async should fail
        let result_no_context = crate::parser::parse(&tokens);
        assert!(result_no_context.is_err(), "Expected async function without soft keyword context to fail");

        // With imported vocab registrations mapping mylib -> async modifier, it should succeed.
        let mut map = std::collections::HashMap::new();
        map.insert(
            "mylib".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "mylib.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "async".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );

        let result_with_context = crate::parser::parse_with_context(&tokens, None, Some(&map));
        assert!(result_with_context.is_ok(), "Expected async function to parse with soft keyword context");
        Ok(())
    }

    #[test]
    fn test_parse_imported_vocab_block_statement() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route \"/health\":\n    pass\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: vec!["cached".to_string()],
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(
            function.body[0].node,
            crate::ast::Statement::VocabBlock(_)
        ));
        Ok(())
    }

    #[test]
    fn test_imported_vocab_keyword_can_still_parse_assignment_statement() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route = \"/health\"\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(
            function.body[0].node,
            crate::ast::Statement::Assignment(_)
        ));
        Ok(())
    }

    #[test]
    fn test_imported_vocab_block_accepts_scoped_leading_dot_path() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    .order.amount\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "analytics".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "analytics.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "query".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .in_declaration_body("query")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
        };
        let crate::ast::Expr::Surface(surface_expr) = &expr.node else {
            return Err(format!("expected surface expression, got {:?}", expr.node).into());
        };
        assert!(matches!(
            &surface_expr.key,
            incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                dependency_key,
                descriptor_key,
            } if dependency_key == "analytics" && descriptor_key == "query.field"
        ));
        assert!(matches!(
            &surface_expr.payload,
            crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
                if segments == &["order".to_string(), "amount".to_string()]
                    && owner.declaration == "query"
        ));
        Ok(())
    }

    #[test]
    fn test_scoped_leading_dot_path_still_rejects_outside_vocab_block() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\n\ndef configure() -> None:\n  .amount\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let keyword_map = std::collections::HashMap::new();
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .in_declaration_body("query")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration)
                            .with_misuse_scope(incan_vocab::ScopedSurfaceMisuseScope::ActivatingFile)
                            .with_diagnostic(
                                incan_vocab::ScopedSurfaceDiagnosticTemplate::new(
                                    "query-field-outside-scope",
                                    incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope,
                                    "query field shorthand is only valid inside query blocks",
                                )
                                .with_help("move this expression into a `query:` block"),
                            ),
                    ),
            ],
        );

        let result =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
        let errors = result.expect_err("leading-dot path should remain invalid outside the owning vocab block");
        let first_error = errors.first().expect("expected at least one leading-dot diagnostic");
        assert!(
            first_error
                .message
                .contains("query field shorthand is only valid inside query blocks"),
            "expected author-provided diagnostic, got {:?}",
            errors
        );
        assert!(
            first_error
                .hints
                .iter()
                .any(|hint| hint.contains("move this expression")),
            "expected author-provided help, got {:?}",
            errors
        );
        Ok(())
    }

    #[test]
    fn test_imported_vocab_block_prefers_scoped_glyph_over_core_binary() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::pipeline\n\ndef configure() -> None:\n  flow:\n    extract + normalize\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "pipeline".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "pipeline.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "flow".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "pipeline".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("pipeline.dsl")
                    .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::operator("flow.link", "+")
                            .in_declaration_body("flow")
                            .pairwise_chain(),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
        };
        assert!(matches!(
            &expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                        if glyph == "+" && owner.declaration == "flow"
                )
        ));
        Ok(())
    }

    #[test]
    fn test_imported_vocab_block_prefers_scoped_symbol_over_core_call() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "analytics".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "analytics.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "query".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                            .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                            .in_declaration_body("query"),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
        };
        assert!(matches!(
            &expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedSymbolCall { symbol, args, owner }
                        if symbol == "sum" && args.len() == 1 && owner.declaration == "query"
                )
        ));
        Ok(())
    }

    #[test]
    fn test_scoped_symbol_descriptor_does_not_change_call_outside_vocab_block()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\n\ndef configure() -> None:\n  sum(amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let keyword_map = std::collections::HashMap::new();
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                            .in_declaration_body("query"),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(
            &function.body[0].node,
            crate::ast::Statement::Expr(expr)
                if matches!(&expr.node, crate::ast::Expr::Call(callee, _, _)
                    if matches!(&callee.node, crate::ast::Expr::Ident(name) if name == "sum"))
        ));
        Ok(())
    }

    #[test]
    fn test_same_depth_scoped_symbol_ambiguity_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\nimport pub::metrics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "analytics".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "analytics.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("query")],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.analytics", "sum")
                            .in_declaration_body("query"),
                    ),
            ],
        );
        surface_map.insert(
            "metrics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("metrics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.metrics", "sum")
                            .in_declaration_body("query"),
                    ),
            ],
        );

        let result =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
        let errors = result.expect_err("same-depth scoped symbol collision should be ambiguous");
        assert!(
            errors
                .iter()
                .any(|err| err.message.contains("Ambiguous scoped symbol `sum`")),
            "expected ambiguous scoped symbol diagnostic, got {:?}",
            errors
        );
        Ok(())
    }

    #[test]
    fn test_nested_vocab_block_prefers_innermost_scoped_symbol() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "import pub::analytics\n\ndef configure() -> None:\n  query:\n    stage:\n      sum(amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "analytics".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "analytics.query".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::block("query"),
                    incan_vocab::KeywordSpec::sub_block("stage", "query"),
                ],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_declaration(incan_vocab::DeclarationSurface::named("stage").in_block("query"))
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                            .in_declaration_body("query"),
                    )
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("stage.sum", "sum")
                            .in_declaration_body("stage"),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(query_block) = &function.body[0].node else {
            return Err(format!("expected query block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::VocabBlock(stage_block) = &query_block.body[0].node else {
            return Err(format!("expected stage block, got {:?}", query_block.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &stage_block.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", stage_block.body[0].node).into());
        };
        assert!(matches!(
            &expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedSymbolCall { symbol, owner, .. }
                        if symbol == "sum" && owner.declaration == "stage"
                )
        ));
        Ok(())
    }

    #[test]
    fn test_active_scoped_symbol_misuse_uses_descriptor_diagnostic() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "analytics".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "analytics.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::block("query")],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "analytics".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("analytics.query")
                    .with_declaration(
                        incan_vocab::DeclarationSurface::named("query")
                            .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                    )
                    .with_scoped_symbol(
                        incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                            .in_clause_body("query", "SELECT")
                            .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                            .with_diagnostic(
                                incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                                    "query-sum-outside-select",
                                    incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                                    "query aggregate `sum` is only valid inside SELECT clauses",
                                )
                                .with_help("move `sum(...)` into a SELECT clause"),
                            ),
                    ),
            ],
        );

        let result =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
        let errors = result.expect_err("active scoped symbol misuse should use descriptor diagnostic");
        let first_error = errors.first().expect("expected at least one scoped-symbol diagnostic");
        assert!(
            first_error
                .message
                .contains("query aggregate `sum` is only valid inside SELECT clauses"),
            "expected author-provided scoped-symbol diagnostic, got {:?}",
            errors
        );
        assert!(
            first_error.hints.iter().any(|hint| hint.contains("move `sum(...)`")),
            "expected author-provided scoped-symbol help, got {:?}",
            errors
        );
        Ok(())
    }

    #[test]
    fn test_imported_vocab_block_accepts_multitoken_pipe_glyph() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::querykit\n\ndef configure() -> None:\n  query:\n    orders |> paid_orders\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "querykit".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "querykit".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "query".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "querykit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("querykit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                            .in_declaration_body("query")
                            .pairwise_chain(),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
        };
        assert!(matches!(
            &expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                        if glyph == "|>" && owner.declaration == "query"
                )
        ));
        Ok(())
    }

    #[test]
    fn test_scoped_glyph_descriptor_does_not_change_core_binary_outside_vocab_block()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::pipeline\n\ndef configure() -> None:\n  extract + normalize\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let keyword_map = std::collections::HashMap::new();
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "pipeline".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("pipeline.dsl")
                    .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::operator("flow.link", "+")
                            .in_declaration_body("flow")
                            .pairwise_chain(),
                    ),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(
            &function.body[0].node,
            crate::ast::Statement::Expr(expr)
                if matches!(&expr.node, crate::ast::Expr::Binary(_, crate::ast::BinaryOp::Add, _))
        ));
        Ok(())
    }

    #[test]
    fn rfc040_product_probe_query_method_arguments_accept_leading_dot_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::querykit\n\ndef configure(orders: Any) -> None:\n  orders.filter(.amount > 100).select(.customer_id)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let keyword_map = std::collections::HashMap::new();
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "querykit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("querykit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .with_eligibilities([
                                incan_vocab::ScopedSurfaceEligibility::call_argument("query", "filter"),
                                incan_vocab::ScopedSurfaceEligibility::call_argument("query", "select"),
                            ])
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                    ),
            ],
        );

        let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::Expr(expr) = &function.body[0].node else {
            return Err(format!("expected expression statement, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Expr::MethodCall(filter_call, select_name, _, select_args) = &expr.node else {
            return Err(format!("expected select method call, got {:?}", expr.node).into());
        };
        assert_eq!(select_name, "select");
        assert!(matches!(
            &select_args[0],
            crate::ast::CallArg::Positional(arg)
                if matches!(
                    &arg.node,
                    crate::ast::Expr::Surface(surface)
                        if matches!(
                            &surface.payload,
                            crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
                                if segments == &["customer_id".to_string()]
                                    && owner.declaration == "query"
                                    && owner.call.as_deref() == Some("select")
                        )
                )
        ));
        let crate::ast::Expr::MethodCall(_, filter_name, _, filter_args) = &filter_call.node else {
            return Err(format!("expected filter method call, got {:?}", filter_call.node).into());
        };
        assert_eq!(filter_name, "filter");
        assert!(matches!(
            &filter_args[0],
            crate::ast::CallArg::Positional(arg)
                if matches!(
                    &arg.node,
                    crate::ast::Expr::Binary(left, crate::ast::BinaryOp::Gt, _)
                        if matches!(
                            &left.node,
                            crate::ast::Expr::Surface(surface)
                                if matches!(
                                    &surface.payload,
                                    crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
                                        if segments == &["amount".to_string()]
                                            && owner.declaration == "query"
                                            && owner.call.as_deref() == Some("filter")
                                )
                        )
                )
        ));
        Ok(())
    }

    #[test]
    fn test_call_argument_scoped_leading_dot_rejects_unregistered_call() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::querykit\n\ndef configure(orders: Any) -> None:\n  orders.map(.amount)\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let keyword_map = std::collections::HashMap::new();
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "querykit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("querykit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .in_call_argument("query", "filter")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                    ),
            ],
        );

        let result =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
        let errors = result.expect_err("leading-dot path should remain invalid in unregistered calls");
        let first_error = errors.first().expect("expected at least one unregistered-call diagnostic");
        assert!(
            first_error
                .message
                .contains("Expected expression, found Punctuation(Dot)"),
            "expected ordinary leading-dot parse rejection, got {:?}",
            errors
        );
        Ok(())
    }

    #[test]
    fn rfc040_product_probe_route_head_accepts_verb_composition_and_mapping()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routekit\n\ndef configure() -> None:\n  route \"/users\":\n    get + post -> users.index\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "routekit".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routekit".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "routekit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("routekit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("route"))
                    .with_scoped_surfaces([
                        incan_vocab::ScopedSurfaceDescriptor::operator("route.verb", "+")
                            .in_declaration_body("route")
                            .pairwise_chain(),
                        incan_vocab::ScopedSurfaceDescriptor::operator("route.map", "->")
                            .in_declaration_body("route"),
                    ]),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
            return Err(format!("expected route expression statement, got {:?}", block.body[0].node).into());
        };
        let crate::ast::Expr::Surface(surface) = &expr.node else {
            return Err(format!("expected route mapping surface expression, got {:?}", expr.node).into());
        };
        let crate::ast::SurfaceExprPayload::ScopedGlyph {
            glyph,
            left,
            right,
            owner,
        } = &surface.payload
        else {
            return Err(format!("expected route mapping scoped glyph, got {:?}", surface.payload).into());
        };
        assert_eq!(glyph, "->");
        assert_eq!(owner.declaration, "route");
        assert!(matches!(
            &left.node,
            crate::ast::Expr::Surface(left_surface)
                if matches!(
                    &left_surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                        if glyph == "+" && owner.declaration == "route"
                )
        ));
        assert!(matches!(
            &right.node,
            crate::ast::Expr::Field(object, field)
                if matches!(&object.node, crate::ast::Expr::Ident(name) if name == "users")
                    && field == "index"
        ));
        Ok(())
    }

    #[test]
    fn rfc040_product_probe_workflow_accepts_pipeline_fallback_binding_and_shape_check()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::workflowkit\n\ndef configure() -> None:\n  flow:\n    extract >> normalize // fallback\n    result := check === expected\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut keyword_map = std::collections::HashMap::new();
        keyword_map.insert(
            "workflowkit".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "workflowkit".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "flow".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );
        let mut surface_map = std::collections::HashMap::new();
        surface_map.insert(
            "workflowkit".to_string(),
            vec![
                incan_vocab::DslSurface::on_import("workflowkit")
                    .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                    .with_scoped_surfaces([
                        incan_vocab::ScopedSurfaceDescriptor::operator("flow.pipe", ">>")
                            .in_declaration_body("flow")
                            .pairwise_chain(),
                        incan_vocab::ScopedSurfaceDescriptor::operator("flow.fallback", "//")
                            .in_declaration_body("flow"),
                        incan_vocab::ScopedSurfaceDescriptor::binding("flow.bind", ":=")
                            .in_declaration_body("flow"),
                        incan_vocab::ScopedSurfaceDescriptor::operator("flow.shape", "===")
                            .in_declaration_body("flow"),
                    ]),
            ],
        );

        let program =
            crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
                .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
            return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
        };
        let crate::ast::Statement::Expr(pipe_expr) = &block.body[0].node else {
            return Err(format!("expected workflow pipeline expression, got {:?}", block.body[0].node).into());
        };
        let crate::ast::Statement::Expr(bind_expr) = &block.body[1].node else {
            return Err(format!("expected workflow binding expression, got {:?}", block.body[1].node).into());
        };
        assert!(matches!(
            &pipe_expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, right, owner, .. }
                        if glyph == ">>"
                            && owner.declaration == "flow"
                            && matches!(
                                &right.node,
                                crate::ast::Expr::Surface(right_surface)
                                    if matches!(
                                        &right_surface.payload,
                                        crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                                            if glyph == "//" && owner.declaration == "flow"
                                    )
                            )
                )
        ));
        assert!(matches!(
            &bind_expr.node,
            crate::ast::Expr::Surface(surface)
                if matches!(
                    &surface.payload,
                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, right, owner, .. }
                        if glyph == ":="
                            && owner.declaration == "flow"
                            && matches!(
                                &right.node,
                                crate::ast::Expr::Surface(right_surface)
                                    if matches!(
                                        &right_surface.payload,
                                        crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                                            if glyph == "===" && owner.declaration == "flow"
                                    )
                            )
                )
        ));
        Ok(())
    }

    #[test]
    fn test_imported_vocab_keyword_can_still_parse_expression_statement() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route(\"/health\")\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(function.body[0].node, crate::ast::Statement::Expr(_)));
        Ok(())
    }

    #[test]
    fn test_imported_vocab_keyword_can_still_parse_typed_assignment_statement()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route: str = \"/health\"\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        assert!(matches!(
            function.body[0].node,
            crate::ast::Statement::Assignment(_)
        ));
        Ok(())
    }

    #[test]
    fn test_parse_nested_vocab_block_with_in_block_placement() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route \"/home\":\n    get:\n      pass\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec {
                        name: "route".to_string(),
                        surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                        compound_tokens: Vec::new(),
                        placement: incan_vocab::KeywordPlacement::TopLevel,
                    },
                    incan_vocab::KeywordSpec {
                        name: "get".to_string(),
                        surface_kind: incan_vocab::KeywordSurfaceKind::SubBlock,
                        compound_tokens: Vec::new(),
                        placement: incan_vocab::KeywordPlacement::InBlock(vec!["route".to_string()]),
                    },
                ],
                valid_decorators: Vec::new(),
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(route_block) = &function.body[0].node else {
            return Err("expected top-level vocab block in function body".into());
        };
        assert!(matches!(
            route_block.body[0].node,
            crate::ast::Statement::VocabBlock(_)
        ));
        Ok(())
    }

    #[test]
    fn test_parse_block_context_keyword_surface_as_vocab_block() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import pub::routes\n\ndef configure() -> None:\n  route \"/home\":\n    middleware auth:\n      pass\n";
        let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

        let mut map = std::collections::HashMap::new();
        map.insert(
            "routes".to_string(),
            vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "routes.dsl".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec {
                        name: "route".to_string(),
                        surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                        compound_tokens: Vec::new(),
                        placement: incan_vocab::KeywordPlacement::TopLevel,
                    },
                    incan_vocab::KeywordSpec {
                        name: "middleware".to_string(),
                        surface_kind: incan_vocab::KeywordSurfaceKind::BlockContextKeyword,
                        compound_tokens: Vec::new(),
                        placement: incan_vocab::KeywordPlacement::InBlock(vec!["route".to_string()]),
                    },
                ],
                valid_decorators: Vec::new(),
            }],
        );

        let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let function = match &program.declarations[1].node {
            crate::ast::Declaration::Function(function) => function,
            other => return Err(format!("expected function declaration, got {other:?}").into()),
        };
        let crate::ast::Statement::VocabBlock(route_block) = &function.body[0].node else {
            return Err("expected top-level vocab block in function body".into());
        };
        let crate::ast::Statement::VocabBlock(context_block) = &route_block.body[0].node else {
            return Err("expected nested context vocab block in route body".into());
        };
        assert_eq!(context_block.keyword, "middleware");
        Ok(())
    }

    #[test]
    fn test_parse_for_tuple_unpack_binding() {
        let source = "def bind(xs: list[str]) -> list[str]:\n  mut out: list[str] = []\n  for idx, name in enumerate(xs):\n    out.append(name)\n  return out\n";
        let program = match parse_str(source) {
            Ok(program) => program,
            Err(errs) => panic!("for tuple-unpack binding should parse: {errs:?}"),
        };
        let Declaration::Function(function) = &program.declarations[0].node else {
            panic!("expected function declaration");
        };
        let Statement::For(for_stmt) = &function.body[1].node else {
            panic!("expected for statement");
        };
        let Pattern::Tuple(items) = &for_stmt.pattern.node else {
            panic!("expected tuple binding pattern");
        };

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
        assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
    }

    #[test]
    fn test_parse_list_comprehension_tuple_unpack_binding() {
        let source =
            "def names(xs: list[str]) -> list[str]:\n  return [name for idx, name in enumerate(xs)]\n";
        let program = match parse_str(source) {
            Ok(program) => program,
            Err(errs) => panic!("list comprehension tuple-unpack binding should parse: {errs:?}"),
        };
        let Declaration::Function(function) = &program.declarations[0].node else {
            panic!("expected function declaration");
        };
        let Statement::Return(Some(expr)) = &function.body[0].node else {
            panic!("expected return statement");
        };
        let Expr::ListComp(comp) = &expr.node else {
            panic!("expected list comprehension");
        };
        let Pattern::Tuple(items) = &comp.pattern.node else {
            panic!("expected tuple binding pattern");
        };

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
        assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
    }

    #[test]
    fn test_parse_loop_expression_with_break_value() -> Result<(), Vec<CompileError>> {
        let source = r#"
def run() -> int:
  return loop:
    break 1
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let Statement::Return(Some(expr)) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "expected return statement with loop expression".to_string(),
                function.body[0].span,
            )]);
        };
        let Expr::Loop(loop_expr) = &expr.node else {
            return Err(vec![CompileError::new(
                "expected loop expression".to_string(),
                expr.span,
            )]);
        };
        let Statement::Break(Some(value)) = &loop_expr.body[0].node else {
            return Err(vec![CompileError::new(
                "expected break with value inside loop expression".to_string(),
                loop_expr.body[0].span,
            )]);
        };
        assert!(matches!(value.node, Expr::Literal(Literal::Int(_))));
        Ok(())
    }

    #[test]
    fn test_parse_race_for_expression_requires_std_async_activation() -> Result<(), Vec<CompileError>> {
        let source = r#"
def run() -> int:
  race = 1
  return race
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let Statement::Assignment(assign) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected assignment".to_string(),
                function.body[0].span,
            )]);
        };
        assert_eq!(assign.name, "race");
        let Statement::Return(Some(expr)) = &function.body[1].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return statement".to_string(),
                function.body[1].span,
            )]);
        };
        assert!(matches!(&expr.node, Expr::Ident(name) if name == "race"));
        Ok(())
    }

    #[test]
    fn test_parse_active_race_for_expression_surface_shape() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.async

async def run() -> int:
  return race for value:
    await fast() => value
    await slow() =>
      return value
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[1])?;
        let Statement::Return(Some(expr)) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return statement".to_string(),
                function.body[0].span,
            )]);
        };
        let Expr::Surface(surface) = &expr.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected surface expression".to_string(),
                expr.span,
            )]);
        };
        let SurfaceExprPayload::RaceFor(race) = &surface.payload else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected race-for payload".to_string(),
                expr.span,
            )]);
        };

        assert_eq!(race.binding, "value");
        assert_eq!(race.arms.len(), 2);
        assert!(matches!(&race.arms[0].awaitable.node, Expr::Call(callee, _, _) if matches!(&callee.node, Expr::Ident(name) if name == "fast")));
        assert!(matches!(&race.arms[0].body, RaceForBody::Expr(body) if matches!(&body.node, Expr::Ident(name) if name == "value")));
        assert!(matches!(&race.arms[1].awaitable.node, Expr::Call(callee, _, _) if matches!(&callee.node, Expr::Ident(name) if name == "slow")));
        assert!(matches!(&race.arms[1].body, RaceForBody::Block(stmts) if matches!(&stmts[0].node, Statement::Return(Some(value)) if matches!(&value.node, Expr::Ident(name) if name == "value"))));
        Ok(())
    }

    #[test]
    fn test_parse_race_for_rejects_pattern_binding_header() {
        let source = r#"
import std.async

async def run() -> int:
  return race for (value):
    await fast() => value
"#;
        let errors = parse_str(source).expect_err("pattern race header should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Pattern-binding race headers are not supported")),
            "expected pattern-binding diagnostic, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_race_for_rejects_default_arm() {
        let source = r#"
import std.async

async def run() -> int:
  return race for value:
    default => value
"#;
        let errors = parse_str(source).expect_err("default race arm should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Default race arms are not supported")),
            "expected default-arm diagnostic, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_race_for_rejects_guard_arm() {
        let source = r#"
import std.async

async def run() -> int:
  return race for value:
    await fast() if value > 0 => value
"#;
        let errors = parse_str(source).expect_err("guarded race arm should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Race arm guards are not supported")),
            "expected guard diagnostic, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_race_for_rejects_fairness_control() {
        let source = r#"
import std.async

async def run() -> int:
  return race for value:
    fair await fast() => value
"#;
        let errors = parse_str(source).expect_err("fairness-controlled race arm should fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Race fairness controls are not supported")),
            "expected fairness diagnostic, got: {errors:?}"
        );
    }

    #[test]
    fn test_parse_rfc028_operator_spellings() -> Result<(), Vec<CompileError>> {
        let source = r#"
def ops(a: Any, b: Any, c: Any) -> None:
  mat = a @ b
  piped = a |> b <| c
  bits = a & b | c ^ a << b >> c
  inv = ~a
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;

        let Statement::Assignment(mat) = &function.body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected mat assignment".to_string(),
                function.body[0].span,
            )]);
        };
        assert!(matches!(mat.value.node, Expr::Binary(_, BinaryOp::MatMul, _)));

        let Statement::Assignment(piped) = &function.body[1].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected piped assignment".to_string(),
                function.body[1].span,
            )]);
        };
        let Expr::Binary(left, BinaryOp::PipeBackward, _) = &piped.value.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected pipe-backward expression".to_string(),
                piped.value.span,
            )]);
        };
        assert!(matches!(left.node, Expr::Binary(_, BinaryOp::PipeForward, _)));

        let Statement::Assignment(bits) = &function.body[2].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected bits assignment".to_string(),
                function.body[2].span,
            )]);
        };
        let Expr::Binary(_, BinaryOp::BitOr, right) = &bits.value.node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected bit-or expression".to_string(),
                bits.value.span,
            )]);
        };
        assert!(matches!(right.node, Expr::Binary(_, BinaryOp::BitXor, _)));

        let Statement::Assignment(inv) = &function.body[3].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected inv assignment".to_string(),
                function.body[3].span,
            )]);
        };
        assert!(matches!(inv.value.node, Expr::Unary(UnaryOp::Invert, _)));
        Ok(())
    }

    #[test]
    fn test_parse_rfc028_compound_assignment_spellings() -> Result<(), Vec<CompileError>> {
        let source = r#"
def update(x: Any, y: Any) -> None:
  x @= y
  x &= y
  x |= y
  x ^= y
  x <<= y
  x >>= y
"#;
        let program = parse_str(source)?;
        let function = require_function_decl(&program.declarations[0])?;
        let expected = [
            CompoundOp::MatMul,
            CompoundOp::BitAnd,
            CompoundOp::BitOr,
            CompoundOp::BitXor,
            CompoundOp::Shl,
            CompoundOp::Shr,
        ];
        for (stmt, op) in function.body.iter().zip(expected) {
            assert!(
                matches!(&stmt.node, Statement::CompoundAssignment(assign) if assign.op == op),
                "expected compound assignment {op:?}, got {:?}",
                stmt.node
            );
        }
        Ok(())
    }

    #[test]
    fn test_parse_matmul_preserves_decorator_and_rust_import_at() -> Result<(), Vec<CompileError>> {
        let source = r#"
from rust::libm @ "0.2" import sqrt

@derive(Clone)
class Tensor:
  def apply(self, other: Tensor) -> Tensor:
    return self @ other
"#;
        let program = parse_str(source)?;
        let class = require_class_decl(&program.declarations[1])?;
        assert_eq!(class.decorators.len(), 1);
        let Some(body) = class.methods[0].node.body.as_ref() else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected concrete method body".to_string(),
                class.methods[0].span,
            )]);
        };
        let Statement::Return(Some(expr)) = &body[0].node else {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return statement".to_string(),
                body[0].span,
            )]);
        };
        assert!(matches!(expr.node, Expr::Binary(_, BinaryOp::MatMul, _)));
        Ok(())
    }

    #[test]
    fn test_parse_top_level_alias_declarations() -> Result<(), Vec<CompileError>> {
        let source = r#"
def avg(x: int) -> int:
  return x

mean = avg
pub average = alias avg
"#;
        let program = parse_str(source)?;
        let Declaration::Alias(mean) = &program.declarations[1].node else {
            panic!("expected bare alias, got {:?}", program.declarations[1].node);
        };
        assert_eq!(mean.name, "mean");
        assert_eq!(mean.target.segments, vec!["avg"]);
        assert!(!mean.explicit_marker);

        let Declaration::Alias(average) = &program.declarations[2].node else {
            panic!("expected explicit public alias, got {:?}", program.declarations[2].node);
        };
        assert_eq!(average.name, "average");
        assert_eq!(average.target.segments, vec!["avg"]);
        assert!(average.explicit_marker);
        assert_eq!(average.visibility, Visibility::Public);
        Ok(())
    }

    #[test]
    fn test_parse_method_alias_declarations() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Stats:
  value: int
  mean = alias avg

  def avg(self) -> int:
    return self.value

trait Named:
  display = name
  def name(self) -> str
"#;
        let program = parse_str(source)?;
        let model = require_model_decl(&program.declarations[0])?;
        assert_eq!(model.method_aliases.len(), 1);
        assert_eq!(model.method_aliases[0].node.name, "mean");
        assert_eq!(model.method_aliases[0].node.target, "avg");
        assert!(model.method_aliases[0].node.explicit_marker);

        let tr = require_trait_decl(&program.declarations[1])?;
        assert_eq!(tr.method_aliases.len(), 1);
        assert_eq!(tr.method_aliases[0].node.name, "display");
        assert_eq!(tr.method_aliases[0].node.target, "name");
        assert!(!tr.method_aliases[0].node.explicit_marker);
        Ok(())
    }

    #[test]
    fn test_parse_top_level_partial_declarations() -> Result<(), Vec<CompileError>> {
        let source = r#"
BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
pub JsonRoute = partial web::route(method="GET", content_type="json")
"#;
        let program = parse_str(source)?;
        let Declaration::Partial(bronze) = &program.declarations[0].node else {
            panic!("expected partial declaration, got {:?}", program.declarations[0].node);
        };
        assert_eq!(bronze.name, "BronzeReader");
        assert_eq!(bronze.target.segments, vec!["readers", "TableReader"]);
        assert_eq!(bronze.args.len(), 2);
        assert_eq!(bronze.args[0].name, "layer");
        assert!(matches!(bronze.args[0].value.node, Expr::Literal(Literal::String(ref value)) if value == "bronze"));

        let Declaration::Partial(route) = &program.declarations[1].node else {
            panic!("expected public partial declaration, got {:?}", program.declarations[1].node);
        };
        assert_eq!(route.visibility, Visibility::Public);
        assert_eq!(route.name, "JsonRoute");
        assert_eq!(route.target.segments, vec!["web", "route"]);
        assert_eq!(route.args[1].name, "content_type");
        Ok(())
    }

    #[test]
    fn test_parse_method_partial_declarations() -> Result<(), Vec<CompileError>> {
        let source = r#"
model Cell:
  alive: bool
  set_alive = partial set_state(state=true)

  def set_state(mut self, state: bool) -> None:
    self.alive = state

class Reader:
  json = partial open(format="json")
  def open(self, format: str) -> None:
    pass

type UserId = newtype int:
  one = partial from_underlying(value=1)

  def from_underlying(value: int) -> UserId:
    return UserId(value)

trait Named:
  display = partial name(prefix="name")
  def name(self, prefix: str) -> str
"#;
        let program = parse_str(source)?;
        let model = require_model_decl(&program.declarations[0])?;
        assert_eq!(model.method_partials.len(), 1);
        assert_eq!(model.method_partials[0].node.name, "set_alive");
        assert_eq!(model.method_partials[0].node.target, "set_state");
        assert_eq!(model.method_partials[0].node.args[0].name, "state");

        let class = require_class_decl(&program.declarations[1])?;
        assert_eq!(class.method_partials.len(), 1);
        assert_eq!(class.method_partials[0].node.name, "json");

        let newtype = require_newtype_decl(&program.declarations[2])?;
        assert_eq!(newtype.method_partials.len(), 1);
        assert_eq!(newtype.method_partials[0].node.target, "from_underlying");

        let tr = require_trait_decl(&program.declarations[3])?;
        assert_eq!(tr.method_partials.len(), 1);
        assert_eq!(tr.method_partials[0].node.name, "display");
        Ok(())
    }

    #[test]
    fn test_parse_local_partial_expression_preserves_callable_target() -> Result<(), Vec<CompileError>> {
        let source = r#"
def reader_for(layer: str) -> Reader:
  return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#;
        let program = parse_str(source)?;
        let func = require_function_decl(&program.declarations[0])?;
        let Statement::Return(Some(expr)) = &func.body[0].node else {
            panic!("expected return partial expression");
        };
        let Expr::Partial(partial) = &expr.node else {
            panic!("expected partial expression, got {:?}", expr.node);
        };
        assert_eq!(partial.args.len(), 2);
        assert_eq!(partial.args[0].name, "layer");
        assert!(matches!(partial.args[1].value.node, Expr::Dict(_)));
        assert!(matches!(partial.target.node, Expr::Field(_, ref name) if name == "reader"));
        Ok(())
    }

    #[test]
    fn test_parse_partial_rejects_positional_presets() {
        let err = parse_str_err(
            r#"
Bad = partial Target(1)
"#,
            "partial positional preset",
        );
        assert!(
            err.iter()
                .any(|err| err.message.contains("Partial presets only support keyword arguments")),
            "expected keyword-only partial preset diagnostic, got {err:?}"
        );
    }
}
