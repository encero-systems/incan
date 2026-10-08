//! Generated-Rust contract coverage for a checked `enumerate` list value.

use incan::backend::IrCodegen;
use incan::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lower source through the checked native codegen path and retain its Rust tokens for consumer-shape assertions.
fn generate_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))
}

/// A stored canonical `enumerate` result must be a checked list value, while direct loop consumption stays lazy.
#[test]
fn stored_enumerate_materializes_the_checked_list_without_changing_direct_for_laziness() -> TestResult {
    let source = r#"
pub def stored_then_direct() -> int:
  values = [4, 5]
  enumerated = enumerate(values)
  enumerated_alias = enumerated
  mut total = 0
  for index, value in enumerated_alias:
    total += index + value
  for direct_index, direct_value in enumerate(values):
    total += direct_index + direct_value
  return total
"#;
    let rust = generate_rust(source)?;
    let compact = rust
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let stored_start = compact.find("letenumerated").ok_or_else(|| {
        std::io::Error::other(format!("generated Rust must bind the stored enumerate value:\n{rust}"))
    })?;
    let alias_start = compact.find("letenumerated_alias").ok_or_else(|| {
        std::io::Error::other(format!(
            "generated Rust must retain the stored enumerate alias:\n{rust}"
        ))
    })?;
    let stored_binding = &compact[stored_start..alias_start];
    assert!(
        stored_binding.contains(".enumerate().map("),
        "stored canonical enumerate must retain the compiler-owned enumerate mapping: {rust}"
    );
    assert!(
        stored_binding.contains(".collect::<Vec<_>>();"),
        "checked list[tuple[int, T]] value must materialize before alias traversal: {rust}"
    );

    let direct_for_start = compact.find("for(direct_index,direct_value)in").ok_or_else(|| {
        std::io::Error::other(format!("generated Rust must retain the direct enumerate loop:\n{rust}"))
    })?;
    let direct_for = &compact[direct_for_start..];
    assert!(
        direct_for.contains(".enumerate().map("),
        "direct for-enumerate must keep the lazy compiler-selected iterator path: {rust}"
    );
    assert!(
        !direct_for.contains(".collect::<Vec<_>>()"),
        "direct for-enumerate must not materialize a value solely for iteration: {rust}"
    );
    Ok(())
}
