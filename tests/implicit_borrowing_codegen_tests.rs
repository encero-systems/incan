//! Regression coverage for reference shape and typed assignment ownership (#1455).

use incan::backend::ir::IrCodegen;
use incan::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lower source through the ordinary compiler and normalize only whitespace for ownership assertions.
fn compact_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    let generated = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))?;
    Ok(generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect())
}

#[test]
fn explicit_reference_local_borrows_a_field_without_cloning() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

pub class Holder:
    item: Item

    def observe(self) -> bool:
        current: &Item = self.item
        return current.is_table()
"#,
    )?;
    assert!(rust.contains("letcurrent:&Item=&self.item;"), "{rust}");
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    Ok(())
}

#[test]
fn owned_reassignment_materializes_a_borrowed_generic_and_infers_clone_bound() -> TestResult {
    let rust = compact_rust(
        r#"
pub def replace[T](first: T, second: &T) -> T:
    mut current = first
    current = second
    return current
"#,
    )?;
    assert!(rust.contains("<T:Clone,"), "{rust}");
    assert!(rust.contains("current=second.clone();"), "{rust}");
    Ok(())
}

#[test]
fn borrowed_reassignment_preserves_reference_shape_without_clone_bound() -> TestResult {
    let rust = compact_rust(
        r#"
pub def replace[T](first: &T, second: &T) -> None:
    mut current = first
    current = second
"#,
    )?;
    assert!(rust.contains("current=second;"), "{rust}");
    assert!(!rust.contains("T:Clone") && !rust.contains("second.clone()"), "{rust}");
    Ok(())
}

#[test]
fn owned_reassignment_preserves_a_field_source() -> TestResult {
    let rust = compact_rust(
        r#"
pub class Holder:
    name: str

    def copy_name(self) -> str:
        mut current = ""
        current = self.name
        return current
"#,
    )?;
    assert!(rust.contains("current=self.name.clone();"), "{rust}");
    Ok(())
}

#[test]
fn opaque_rust_child_reassignment_does_not_add_a_second_borrow() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

pub def traverse(item: &Item, keys: list[str]) -> bool:
    mut current = item
    for key in keys:
        match current.get(key):
            Some(child) => current = child
            None => return false
    return current.is_integer()
"#,
    )?;
    assert!(rust.contains("current=child;"), "{rust}");
    assert!(
        !rust.contains("current=&child") && !rust.contains("current=child.clone()"),
        "{rust}"
    );
    Ok(())
}
