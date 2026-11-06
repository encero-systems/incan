//! Callable-name reads are an explicit checked operation, separate from source fields and type metadata.

use super::*;

/// Metadata on a generic callable value records its operation instead of an untyped field projection.
#[test]
fn generic_callable_name_is_an_explicit_operation() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def name[F](callback: F) -> str:\n  return callback.__name__\n",
        &["app", "callable_metadata"],
    )?;
    let body = body_named(&module, "name")?.render_snapshot();
    assert!(body.contains("call helper:callable_name("), "{body}");
    Ok(())
}

/// Parenthesized reads keep the checker-selected metadata operation and evaluate the same callback operand.
#[test]
fn parenthesized_callable_name_retains_the_operation() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def name[F](callback: F) -> str:\n  return (callback).__name__\n",
        &["app", "parenthesized_metadata"],
    )?;
    let body = body_named(&module, "name")?.render_snapshot();
    assert!(body.contains("call helper:callable_name("), "{body}");
    Ok(())
}

/// A user-authored field with this spelling stays an ordinary nominal projection.
#[test]
fn source_name_field_is_not_callable_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Label:\n  pub __name__: str\n\ndef label(value: Label) -> str:\n  return value.__name__\n",
        &["app", "source_field"],
    )?;
    let body = body_named(&module, "label")?.render_snapshot();
    assert!(!body.contains("call helper:callable_name("), "{body}");
    assert!(body.contains("__name__"), "{body}");
    Ok(())
}

/// Metadata on a type parameter itself remains the existing type reflection operation.
#[test]
fn type_parameter_name_is_not_callable_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def name[T]() -> str:\n  return T.__name__\n",
        &["app", "type_metadata"],
    )?;
    let body = body_named(&module, "name")?.render_snapshot();
    assert!(!body.contains("call helper:callable_name("), "{body}");
    Ok(())
}

/// Generated type-name calls retain their binder rather than an unresolved source function.
#[test]
fn generic_class_name_retains_checked_type_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def name[T]() -> str:\n  return T.__class_name__()\n",
        &["app", "class_metadata"],
    )?;
    let body = body_named(&module, "name")?.render_snapshot();
    assert!(body.contains("type-metadata:class_name<T>"), "{body}");
    Ok(())
}

/// Counting a generated type's field table retains the checked type and pure metadata operation.
#[test]
fn generic_field_count_retains_checked_type_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def count[T]() -> int:\n  return len(T.__fields__())\n",
        &["app", "field_count_metadata"],
    )?;
    let body = body_named(&module, "count")?.render_snapshot();
    assert!(body.contains("type-metadata:field_count<T>"), "{body}");
    Ok(())
}
