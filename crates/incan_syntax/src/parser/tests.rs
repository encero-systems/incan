#[cfg(test)]
/// Parser unit tests.
///
/// These tests focus on correctness of specific syntactic forms and on the parser’s
/// error recovery behavior (avoiding cascaded errors).
mod tests {
    use super::*;
    use crate::lexer;

    fn parse_str(source: &str) -> Result<Program, Vec<CompileError>> {
        let tokens = lexer::lex(source).map_err(|_| vec![])?;
        parse(&tokens)
    }

    #[test]
    fn test_unexpected_indent_at_toplevel_is_single_clear_error() {
        // We intentionally allow the lexer to emit INDENT/DEDENT tokens at the top-level.
        // The parser should produce a single clear error and avoid cascading failures.
        let source = "  x = 1\n";
        let err = parse_str(source).expect_err("Top-level indentation should be rejected by the parser");
        assert_eq!(err.len(), 1, "Parser should return exactly one error (no cascade)");
        assert!(
            err[0].message.contains("Expected declaration") && err[0].message.contains("Indent"),
            "Error message should clearly indicate the unexpected INDENT token; got: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_model() {
        let source = r#"
model User:
  name: str
  age: int = 0
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.fields.len(), 2);
                assert!(m.traits.is_empty());
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_parse_class_docstring() {
        let source = r#"
class FieldInfo:
  """
  Compiler-provided field metadata returned by __fields__().
  Instances are immutable and read-only.
  """
  name: str
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.name, "FieldInfo");
                assert_eq!(c.fields.len(), 1);
                assert_eq!(c.fields[0].node.name, "name");
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_parse_model_field_metadata() {
        let source = r#"
model Account:
  type_ [alias="type", description="Account tier"]: str
  balance [description="Balance in cents"]: int
"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_model_field_alias_sugar() {
        let source = r#"
model Account:
  type_ as "type": str
"#;
        let program = parse_str(source).unwrap();
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            _ => panic!("Expected model"),
        };
        let field = &model.fields[0].node;
        assert_eq!(field.metadata.alias.as_deref(), Some("type"));
        assert_eq!(field.metadata.description, None);
    }

    #[test]
    fn test_parse_model_field_alias_and_as_error() {
        let source = r#"
model Account:
  type_ [alias="type"] as "type": str
"#;
        let err = parse_str(source).expect_err("Expected alias + as sugar to be rejected");
        assert!(
            err[0]
                .message
                .contains("Cannot combine 'alias=\"...\"' with 'as \"...\"'"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_keyword_named_args_and_member_access() {
        let source = r#"
def f(a: Foo) -> int:
  let x = Foo(type=1, class=2)
  return a.type
"#;
        let program = parse_str(source).unwrap();
        let func = match &program.declarations[0].node {
            Declaration::Function(func) => func,
            _ => panic!("Expected function"),
        };
        let call_expr = match &func.body[0].node {
            Statement::Assignment(stmt) => match &stmt.value.node {
                Expr::Call(_, args) => args,
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
    }

    #[test]
    fn test_parse_pattern_named_key_keyword() {
        let source = r#"
def f(a: Foo) -> int:
  match a:
    Foo(type=x) => return x
"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_decorator_paths() {
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
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_namespaced_decorator_with_named_args() {
        // RFC 022: Namespaced decorators with positional + named arguments
        let source = r#"
from std.web import POST

@std.web.route("/things", methods=[POST])
async def create() -> None:
  pass
"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_decorator_with_rust_namespace() {
        // RFC 023: @rust.extern decorator must parse correctly (rust is a keyword)
        let source = r#"
@rust.extern
def foo() -> None:
  pass
"#;
        let program = parse_str(source).unwrap();
        let func = match &program.declarations[0].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function"),
        };
        assert_eq!(func.decorators.len(), 1);
        let dec = &func.decorators[0].node;
        assert_eq!(dec.path.segments, vec!["rust", "extern"]);
        assert_eq!(dec.name, "extern");
    }

    #[test]
    fn test_parse_import_path_with_async_segment() {
        let source = r#"
from std.async.time import sleep
"#;
        let program = parse_str(source).unwrap();
        let decl = match &program.declarations[0].node {
            Declaration::Import(import) => import,
            _ => panic!("Expected import declaration"),
        };
        let ImportKind::From { module, .. } = &decl.kind else {
            panic!("Expected from-import");
        };
        assert_eq!(module.segments, vec!["std", "async", "time"]);
    }

    #[test]
    fn test_parse_trait_with_docstring() {
        let source = r#"
trait Debug:
    """Debug representation."""
    def __repr__(self) -> str: ...
"#;
        let program = parse_str(source).unwrap();
        let tr = match &program.declarations[0].node {
            Declaration::Trait(t) => t,
            _ => panic!("Expected trait declaration"),
        };
        assert_eq!(tr.name, "Debug");
        assert_eq!(tr.methods.len(), 1);
        assert_eq!(tr.methods[0].node.name, "__repr__");
    }

    #[test]
    fn test_parse_non_identifier_alias() {
        let source = r#"
model Weird:
  one_ [alias="1"]: int
"#;
        let program = parse_str(source).unwrap();
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            _ => panic!("Expected model"),
        };
        let field = &model.fields[0].node;
        assert_eq!(field.metadata.alias.as_deref(), Some("1"));
    }

    #[test]
    fn test_parse_duplicate_metadata_key_error() {
        // RFC 021: Duplicate metadata keys are compile-time errors
        let source = r#"
model Account:
  type_ [alias="a", alias="b"]: str
"#;
        let err = parse_str(source).expect_err("Expected duplicate alias key error");
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
        let err = parse_str(source).expect_err("Expected duplicate description key error");
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
        let err = parse_str(source).expect_err("Expected unknown metadata key error");
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
        let err = parse_str(source).expect_err("Expected non-string metadata value error");
        // Parser should fail because it expects a string literal
        assert!(
            err[0].message.contains("string") || err[0].message.contains("Expected"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_model_with_traits() {
        let source = r#"
trait Describable:
  def describe(self) -> str: ...

model User with Describable:
  name: str
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 2);
        match &program.declarations[1].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.traits.len(), 1);
                assert_eq!(m.traits[0].node, "Describable");
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_parse_model_with_multiple_traits() {
        let source = r#"
trait A:
  def a(self) -> int: ...

trait B:
  def b(self) -> int: ...

model User with A, B:
  x: int
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 3);
        match &program.declarations[2].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.traits.len(), 2);
                assert_eq!(m.traits[0].node, "A");
                assert_eq!(m.traits[1].node, "B");
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_parse_function() {
        let source = r#"
def add(a: int, b: int) -> int:
  return a + b
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_import() {
        let source = "import polars::prelude as pl";
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_rust_import_with_version_and_features() {
        let source = r#"import rust::tokio @ "1.0" with ["full", "macros"] as rt"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_rust_from_with_version_and_features() {
        let source = r#"from rust::time @ "0.3" with ["formatting"] import Instant"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_rust_import_with_features_requires_version() {
        let source = r#"import rust::tokio with ["full"]"#;
        let err = parse_str(source).expect_err("Expected rust import features to require version");
        assert!(
            err[0].message.contains("features require a version"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_match() {
        let source = r#"
def handle(opt: Option[int]) -> int:
  match opt:
    case Some(x):
      return x
    case None:
      return 0
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 1);
    }

    #[test]
    fn test_parse_match_fat_arrow_inline_return() {
        let source = r#"
def f() -> int:
  match Ok(1):
    Ok(x) => return x
    Err(_) => return 0
"#;
        let program = parse_str(source).unwrap();
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
    }

    #[test]
    fn test_parse_const_decl() {
        let source = r#"
const ANSWER: int = 42
"#;
        let program = parse_str(source).unwrap();
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Const(c) => {
                assert_eq!(c.name, "ANSWER");
            }
            _ => panic!("Expected const"),
        }
    }

    // ========================================================================
    // Enum diagnostic tests (#113)
    // ========================================================================

    #[test]
    fn test_enum_fat_arrow_mapping_rejected_with_hint() {
        let source = "enum Categories:\n    GROCERIES => Category(\"Groceries\")\n";
        let err = parse_str(source).expect_err("Fat arrow in enum body should be rejected");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("mapped values"),
            "Expected hint about mapped values, got: {msg}"
        );
    }

    #[test]
    fn test_enum_dotted_variant_rejected_with_hint() {
        let source = "enum FlowType:\n    Cash.Inflow\n";
        let err = parse_str(source).expect_err("Dotted variant in enum body should be rejected");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("cannot contain dots"),
            "Expected hint about dots, got: {msg}"
        );
    }

    #[test]
    fn test_enum_assigned_value_rejected_with_hint() {
        let source = "enum Color:\n    Red = 1\n";
        let err = parse_str(source).expect_err("Assigned value in enum body should be rejected");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("assigned values"),
            "Expected hint about assigned values, got: {msg}"
        );
    }

    #[test]
    fn test_enum_colon_annotation_rejected_with_hint() {
        let source = "enum Fields:\n    Name: str\n";
        let err = parse_str(source).expect_err("Type annotation in enum body should be rejected");
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("type annotations"),
            "Expected hint about type annotations, got: {msg}"
        );
    }

    #[test]
    fn test_valid_enum_still_parses() {
        let source = "enum Status:\n    Pending\n    Active\n    Done(str)\n";
        let program = parse_str(source).expect("Valid enum should parse");
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0].node {
            Declaration::Enum(e) => {
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0].node.name, "Pending");
                assert_eq!(e.variants[1].node.name, "Active");
                assert_eq!(e.variants[2].node.name, "Done");
                assert_eq!(e.variants[2].node.fields.len(), 1);
            }
            _ => panic!("Expected enum"),
        }
    }
}
