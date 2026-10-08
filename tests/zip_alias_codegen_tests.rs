//! Generated-Rust regressions for source-accepted aliases of a canonical Zip value.

use incan::backend::ir::{
    conversions::{Conversion, ConversionContext, determine_conversion},
    expr::{IrExpr, IrExprKind, VarAccess, VarRefKind},
    ownership::{ValueUseSite, value_use_requires_clone_bound},
};
use incan::backend::{IrCodegen, ir::IrType};
use incan::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGINAL_THEN_ALIAS: &str = r#"
pub def original_then_alias() -> int:
  pairs = zip([1, 2], [10, 20])
  alias = pairs
  mut total = 0
  for original_left, original_right in pairs:
    total += original_left + original_right
  for alias_left, alias_right in alias:
    total += alias_left + alias_right
  return total
"#;

const ALIAS_THEN_ORIGINAL: &str = r#"
pub def alias_then_original() -> int:
  pairs = zip([1, 2], [10, 20])
  alias = pairs
  mut total = 0
  for alias_left, alias_right in alias:
    total += alias_left + alias_right
  for original_left, original_right in pairs:
    total += original_left + original_right
  return total
"#;

const GENERIC_ALIAS: &str = r#"
pub def preserve_generic_alias[T](value: T) -> tuple[T, T]:
  alias = value
  return (value, alias)
"#;

const GENERIC_LIST_INDEX_ASSIGNMENT: &str = r#"
pub def replace_first_then_preserve[T](mut items: list[T], value: T) -> T:
  items[0] = value
  return value
"#;

const GENERIC_ALIAS_WITH_UNRELATED_TYPE: &str = r#"
pub def preserve_generic_alias_without_unrelated_bound[T, U](value: T) -> tuple[T, T]:
  alias = value
  return (value, alias)
"#;

const GENERIC_CALLABLE_ALIAS: &str = r#"
pub def preserve_callable_alias[T, U](mapper: (T) -> U) -> tuple[(T) -> U, (T) -> U]:
  alias = mapper
  return (mapper, alias)
"#;

const GENERIC_CONCRETE_STRING_ALIAS: &str = r#"
pub def preserve_text_alias[T](text: str) -> str:
  alias = text
  return alias + text
"#;

const GENERIC_CALLABLE_LIST_ALIAS: &str = r#"
pub def preserve_callable_list_alias[T, U](mapper: (T) -> U) -> list[(T) -> U]:
  mappers = [mapper]
  alias = mappers
  marker = len(mappers)
  return alias
"#;

/// Lower a source-accepted Zip alias through the native pipeline and retain its ownership decisions.
fn generate_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))
}

/// Return one public generated-Rust function header without its body.
fn public_function_header<'a>(compact_rust: &'a str, name: &str) -> Result<&'a str, std::io::Error> {
    let declaration = format!("pubfn{name}");
    let start = compact_rust
        .find(&declaration)
        .ok_or_else(|| std::io::Error::other(format!("generated Rust did not retain public function `{name}`")))?;
    let suffix = &compact_rust[start..];
    let end = suffix
        .find('{')
        .ok_or_else(|| std::io::Error::other(format!("generated Rust function `{name}` did not have a body")))?;
    Ok(&suffix[..end])
}

/// Require the assignment ownership plan to preserve the source binding for the later loop.
fn assert_zip_alias_preserves_both_bindings(label: &str, source: &str) -> TestResult {
    let rust = generate_rust(source)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(
        compact.contains("letalias=pairs.clone();"),
        "{label} must clone the non-Copy Zip value at the source alias boundary so both bindings remain iterable:\n{rust}"
    );
    assert!(
        !compact.contains("letalias=pairs;"),
        "{label} must not move the source-accepted Zip value into its alias before the later loop:\n{rust}"
    );
    let public_function = format!("pubfn{label}");
    assert!(
        compact.contains(&public_function),
        "{label} must be retained by codegen so this fixture pins the alias assignment:\n{rust}"
    );
    Ok(())
}

/// Both source orders preserve independently traversable aliases through the centralized assignment plan.
#[test]
fn source_accepted_zip_aliases_clone_before_either_loop_order() -> TestResult {
    assert_zip_alias_preserves_both_bindings("original_then_alias", ORIGINAL_THEN_ALIAS)?;
    assert_zip_alias_preserves_both_bindings("alias_then_original", ALIAS_THEN_ORIGINAL)
}

/// A backend-inserted generic assignment clone must bring the matching `Clone` bound with it.
#[test]
fn generic_alias_assignment_infers_clone_bound_for_later_source_use() -> TestResult {
    let rust = generate_rust(GENERIC_ALIAS)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(
        compact.contains("pubfnpreserve_generic_alias<T:Clone>"),
        "a generic source alias that preserves its original binding must infer T: Clone:\n{rust}"
    );
    assert!(
        compact.contains("letalias=value.clone();"),
        "the generic source alias must use the centralized assignment clone plan:\n{rust}"
    );
    Ok(())
}

/// A list-index Assignment use must infer a generic clone bound when its source binding remains live.
#[test]
fn generic_list_index_assignment_infers_clone_bound_for_later_source_use() -> TestResult {
    let rust = generate_rust(GENERIC_LIST_INDEX_ASSIGNMENT)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(
        compact.contains("pubfnreplace_first_then_preserve<T:Clone>"),
        "a generic list-index assignment that preserves its source binding must infer T: Clone:\n{rust}"
    );
    assert!(
        compact.contains("*incan_stdlib::collections::list_get_mut(items,(0)asi64)=value.clone();"),
        "the list-index Assignment use must clone a non-Copy source binding that remains live:\n{rust}"
    );
    Ok(())
}

/// A required clone bound remains local to the generic value that the assignment actually preserves.
#[test]
fn generic_alias_assignment_does_not_bind_an_unrelated_type_parameter() -> TestResult {
    let rust = generate_rust(GENERIC_ALIAS_WITH_UNRELATED_TYPE)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let header = public_function_header(&compact, "preserve_generic_alias_without_unrelated_bound")?;

    assert!(
        header.contains("T:Clone"),
        "the preserved generic value must retain its required Clone bound:\n{rust}"
    );
    assert!(
        !header.contains("U:Clone"),
        "an unrelated generic parameter must not inherit the assignment clone bound:\n{rust}"
    );
    assert!(
        compact.contains("letalias=value.clone();"),
        "the source-preserved generic alias must retain its assignment clone:\n{rust}"
    );
    Ok(())
}

/// Function pointers remain cloneable without requiring bounds on their argument or return types.
#[test]
fn callable_alias_clone_does_not_bind_callable_type_parameters() -> TestResult {
    let rust = generate_rust(GENERIC_CALLABLE_ALIAS)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let header = public_function_header(&compact, "preserve_callable_alias")?;

    assert!(
        !header.contains("Clone"),
        "a cloned fn(T) -> U value must not infer Clone for T or U:\n{rust}"
    );
    assert!(
        compact.contains("letalias=mapper.clone();"),
        "the source-preserved callable alias must retain the backend assignment clone:\n{rust}"
    );
    Ok(())
}

/// A concrete string clone must not turn an otherwise generic function into a Clone-constrained API.
#[test]
fn concrete_string_alias_clone_does_not_bind_enclosing_generic_parameter() -> TestResult {
    let rust = generate_rust(GENERIC_CONCRETE_STRING_ALIAS)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let header = public_function_header(&compact, "preserve_text_alias")?;

    assert!(
        !header.contains("Clone"),
        "a cloned concrete string must not infer an unrelated T: Clone bound:\n{rust}"
    );
    assert!(
        compact.contains("letalias=text.clone();"),
        "the source-preserved string alias must retain the backend assignment clone:\n{rust}"
    );
    Ok(())
}

/// A list clone depends on its element's Clone implementation, not on a function pointer's generic signature.
#[test]
fn callable_list_alias_clone_does_not_bind_callable_type_parameters() -> TestResult {
    let rust = generate_rust(GENERIC_CALLABLE_LIST_ALIAS)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let header = public_function_header(&compact, "preserve_callable_list_alias")?;

    assert!(
        !header.contains("Clone"),
        "a cloned list of fn(T) -> U values must not infer Clone for T or U:\n{rust}"
    );
    assert!(
        compact.contains("letalias=mappers.clone();"),
        "the source-preserved callable-list alias must retain the backend assignment clone:\n{rust}"
    );
    Ok(())
}

/// Create an IR value-binding expression with the requested lowered ownership access.
fn variable(name: &str, access: VarAccess, ty: IrType) -> IrExpr {
    IrExpr::new(
        IrExprKind::Var {
            name: name.to_string(),
            access,
            ref_kind: VarRefKind::Value,
        },
        ty,
    )
}

/// The Assignment policy clones a later-used non-Copy local and exposes that decision to bound inference.
#[test]
fn assignment_plans_non_copy_read_as_clone_and_clone_bound() {
    let ty = IrType::List(Box::new(IrType::Int));
    let expr = variable("values", VarAccess::Read, ty.clone());

    assert_eq!(
        determine_conversion(&expr, Some(&ty), ConversionContext::Assignment),
        Conversion::Clone
    );
    assert!(value_use_requires_clone_bound(
        &expr,
        ValueUseSite::Assignment { target_ty: Some(&ty) }
    ));
}

/// The Assignment policy preserves a final non-Copy move and ordinary Copy read.
#[test]
fn assignment_preserves_move_and_copy_without_clone() {
    let non_copy_ty = IrType::List(Box::new(IrType::Int));
    let move_expr = variable("values", VarAccess::Move, non_copy_ty.clone());
    assert_eq!(
        determine_conversion(&move_expr, Some(&non_copy_ty), ConversionContext::Assignment),
        Conversion::None
    );
    assert!(!value_use_requires_clone_bound(
        &move_expr,
        ValueUseSite::Assignment {
            target_ty: Some(&non_copy_ty)
        }
    ));

    let copy_expr = variable("count", VarAccess::Read, IrType::Int);
    assert_eq!(
        determine_conversion(&copy_expr, Some(&IrType::Int), ConversionContext::Assignment),
        Conversion::None
    );
}

/// Existing string and borrowed-value materialization must win before the generic variable rule.
#[test]
fn assignment_preserves_string_and_borrowed_materialization_precedence() {
    let string_expr = IrExpr::new(IrExprKind::String("text".to_string()), IrType::String);
    assert_eq!(
        determine_conversion(&string_expr, Some(&IrType::String), ConversionContext::Assignment),
        Conversion::ToString
    );

    let list_ty = IrType::List(Box::new(IrType::Int));
    let borrowed_expr = variable(
        "borrowed_values",
        VarAccess::Read,
        IrType::Ref(Box::new(list_ty.clone())),
    );
    assert_eq!(
        determine_conversion(&borrowed_expr, Some(&list_ty), ConversionContext::Assignment),
        Conversion::Clone
    );
}
