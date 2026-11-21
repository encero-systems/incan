//! Rust constants read from Incan source.

use super::*;

/// A Rust `&'static str` constant reads as Incan text, while Body IR keeps its native borrow; other constants keep
/// their native type.
#[test]
fn static_text_constants_read_as_incan_text() {
    let mut checker = TypeChecker::new();
    let text = Span::new(3, 9);
    let ty = checker.checked_rust_constant(text, "dep::GREETING".into(), "&'static str", "dep::GREETING");
    assert_eq!(ty, ResolvedType::Str);
    assert_eq!(
        checker.type_info.rust.checked_constants.get(&text),
        Some(&"dep::GREETING".to_owned())
    );
    assert_eq!(
        checker.type_info.expressions.body_ir_expr_types.get(&text),
        Some(&ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into()))))
    );
    let number = Span::new(10, 15);
    let ty = checker.checked_rust_constant(number, "dep::LIMIT".into(), "i64", "dep::LIMIT");
    assert_eq!(ty, ResolvedType::Int);
    assert!(!checker.type_info.expressions.body_ir_expr_types.contains_key(&number));
}
