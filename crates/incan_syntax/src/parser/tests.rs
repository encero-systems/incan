#[cfg(test)]
/// Parser unit tests.
///
/// These tests focus on correctness of specific syntactic forms and on the parser’s error recovery behavior
/// (avoiding cascaded errors).
mod tests {
    use super::*;
    use crate::lexer;

    fn parse_str(source: &str) -> Result<Program, Vec<CompileError>> {
        let tokens = lexer::lex(source).map_err(|_| vec![])?;
        parse(&tokens)
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
        match &program.declarations[0].node {
            Declaration::Model(m) => {
                assert_eq!(m.name, "User");
                assert_eq!(m.fields.len(), 2);
                assert!(m.traits.is_empty());
            }
            _ => panic!("Expected model"),
        }
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
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.name, "FieldInfo");
                assert_eq!(c.fields.len(), 1);
                assert_eq!(c.fields[0].node.name, "name");
            }
            _ => panic!("Expected class"),
        }
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
        Ok(())
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
    fn test_parse_assert_requires_std_testing_import() {
        let source = r#"
def f(x: int) -> None:
  assert x > 0
"#;
        let Err(err) = parse_str(source) else {
            panic!("Expected assert statement without std.testing import to fail");
        };
        assert!(
            err[0].message.contains("only available after importing `std.testing`"),
            "Unexpected error: {}",
            err[0].message
        );
    }

    #[test]
    fn test_parse_assert_with_std_testing_import_ok() -> Result<(), Vec<CompileError>> {
        let source = r#"
import std.testing

def f(x: int) -> None:
  assert x > 0, "x must be positive"
"#;
        let program = parse_str(source)?;
        let func = match &program.declarations[1].node {
            Declaration::Function(f) => f,
            _ => panic!("Expected function declaration"),
        };
        assert!(matches!(
            &func.body[0].node,
            Statement::Surface(surface)
                if matches!(
                    surface.payload,
                    SurfaceStmtPayload::KeywordArgs(_)
                )
        ));
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
        let tr = match &program.declarations[0].node {
            Declaration::Trait(t) => t,
            _ => panic!("Expected trait declaration"),
        };
        assert_eq!(tr.name, "Debug");
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
        let nt = match &program.declarations[0].node {
            Declaration::Newtype(nt) => nt,
            _ => panic!("Expected newtype declaration"),
        };
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
        let nt = match &program.declarations[0].node {
            Declaration::Newtype(nt) => nt,
            _ => panic!("Expected rusttype/newtype declaration"),
        };
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
                assert_eq!(m.traits[0].node, "Describable");
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
                assert_eq!(m.traits[0].node, "A");
                assert_eq!(m.traits[1].node, "B");
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
    fn test_parse_pub_from_outside_src_lib_is_error() {
        let source = "pub from widgets import Widget\n";
        let result = parse_str_with_module_path(source, Some("project/src/main.incn"));
        assert!(result.is_err(), "Expected parser to reject `pub from` outside src/lib.incn");
        let err = result.err().unwrap_or_default();
        assert!(
            err[0].message.contains("only valid in `src/lib.incn`"),
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
        let result = parse_str(source);
        let err = result.expect_err("Unclosed import list should produce a parse error");
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
        let result = parse_str(source);
        let err = result.expect_err("Empty import list should produce a parse error");
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
            Expr::MethodCall(base, method, args) => (base, method, args),
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
            Expr::Call(callee, _args) => callee,
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
    fn test_valid_enum_still_parses() {
        let source = "enum Status:\n    Pending\n    Active\n    Done(str)\n";
        let Ok(program) = parse_str(source) else {
            panic!("Valid enum should parse");
        };
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
}
