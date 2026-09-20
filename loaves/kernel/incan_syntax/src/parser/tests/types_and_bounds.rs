//! Type syntax: module-qualified trait bounds, decimal type arguments, constrained primitives, type aliases, RFC 035
//! `Callable[...]` desugaring and RFC 029 union pipe normalization.

use super::*;

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
        panic!(
            "Expected constrained primitive type, got: {:?}",
            newtype.underlying.node
        );
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
        panic!(
            "Expected constrained primitive type, got: {:?}",
            newtype.underlying.node
        );
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
        panic!(
            "Expected constrained primitive type, got: {:?}",
            newtype.underlying.node
        );
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
            assert_eq!(
                params.len(),
                1,
                "Callable[int, int] should desugar to one-arg function type"
            );
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
            assert!(
                params.is_empty(),
                "Callable[(), int] should desugar to zero-arg function type"
            );
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
            assert_eq!(
                params.len(),
                2,
                "Callable[(int, str), bool] should desugar to two-arg function type"
            );
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
