//! Function declarations and parameter lists, decorator paths, arguments and namespaces, `const` / `static` /
//! `__derives__` bindings, alias and partial declarations, and RFC 104 capability declarations.

use super::*;

#[test]
fn test_parse_describe_decorator_allows_trailing_comma() -> Result<(), Vec<CompileError>> {
    let source = r#"
@describe(
  functions,
  FunctionId("normalize"),
  FunctionSpec(summary="Normalize text"),
)
def normalize(value: str) -> str:
  return value
"#;

    let program = parse_str(source)?;
    let decorators = program
        .declarations
        .first()
        .and_then(|declaration| match &declaration.node {
            Declaration::Function(function) => Some(&function.decorators),
            _ => None,
        })
        .ok_or_else(|| {
            vec![CompileError::new(
                "expected decorated function".to_string(),
                Span::default(),
            )]
        })?;
    assert_eq!(decorators.len(), 1);
    assert_eq!(decorators[0].node.args.len(), 3);
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
        .ok_or_else(|| {
            vec![CompileError::new(
                "expected class declaration".to_string(),
                Span::default(),
            )]
        })?;
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
fn test_parse_decorator_factory_with_explicit_type_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
@registered[(str) -> ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
  pass
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };
    let dec = &func.decorators[0].node;
    assert_eq!(dec.path.segments, vec!["registered"]);
    assert_eq!(dec.name, "registered");
    assert!(dec.is_call);
    assert_eq!(dec.type_args.len(), 1);
    assert!(matches!(&dec.type_args[0].node, Type::Function(_, _)));
    assert_eq!(dec.args.len(), 1);
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
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("requires an explicit type annotation"))
    );
}

#[test]
fn test_parse_static_requires_initializer() {
    let source = "static counter: int\n";
    let Err(errors) = parse_str(source) else {
        panic!("expected parse error");
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("requires an initializer"))
    );
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
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("only allowed at module scope"))
    );
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
        panic!(
            "expected public partial declaration, got {:?}",
            program.declarations[1].node
        );
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

/// The full RFC 104 shape: docstring, description, a typed scope block, and checked `requires` references.
#[test]
fn parses_a_capability_declaration_with_scope_and_requires() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "capability refund:\n",
        "    \"\"\"Issue a refund for a captured charge.\"\"\"\n",
        "    description = \"Issue a refund for a captured charge\"\n",
        "    scope:\n",
        "        tenant: str\n",
        "        region: str\n",
        "    requires = [host.http.request]\n",
    );
    let program = parse_str(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let Declaration::Capability(cap) = &program.declarations[0].node else {
        return Err(Box::from("expected a capability declaration".to_string()));
    };
    assert_eq!(cap.name, "refund");
    assert!(cap.docstring.is_some());
    assert!(cap.description.is_some());
    assert_eq!(cap.scope.len(), 2);
    assert_eq!(cap.scope[0].node.name, "tenant");
    assert_eq!(cap.scope[1].node.name, "region");
    assert_eq!(cap.requires.len(), 1);
    Ok(())
}

#[test]
fn parses_a_capability_declaration_without_scope_or_requires() -> Result<(), Box<dyn std::error::Error>> {
    let source = "capability ping:\n    description = \"Reach a host\"\n";
    let program = parse_str(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let Declaration::Capability(cap) = &program.declarations[0].node else {
        return Err(Box::from("expected a capability declaration".to_string()));
    };
    assert_eq!(cap.name, "ping");
    assert!(cap.scope.is_empty());
    assert!(cap.requires.is_empty());
    Ok(())
}

#[test]
fn an_unknown_capability_clause_is_a_syntax_error() -> Result<(), Box<dyn std::error::Error>> {
    let source = "capability ping:\n    describe = \"typo\"\n";
    let errors = parse_str_err(source, "an unknown clause must not parse");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Unknown capability clause")),
        "expected a clause-specific error, got {errors:?}"
    );
    Ok(())
}
