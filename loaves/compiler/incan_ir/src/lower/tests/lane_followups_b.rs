//! Focused lowering regressions for the dev.6 followups-b lane: builtin source-trait adoption paths, `Option`
//! identity tests, and the Rust `Ord` bound used by generic sorting.

use super::*;

/// Implementations of source-owned comparison traits must name their generated stdlib declarations, not Rust's
/// prelude traits whose ABI slots do not use Incan dunder names.
#[test]
fn followups_b_comparison_trait_adoptions_keep_source_trait_paths() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.derives.comparison import Eq, Hash, Ord

model Equal with Eq:
    value: int

    def __eq__(self, other: Self) -> bool:
        return self.value == other.value

model Ordered with Ord:
    value: int

    def __eq__(self, other: Self) -> bool:
        return self.value == other.value

    def __lt__(self, other: Self) -> bool:
        return self.value < other.value

model Hashed with Hash:
    value: int

    def __hash__(self) -> int:
        return self.value
"#,
    )?;
    let impls = ir
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            IrDeclKind::Impl(implementation) => Some((
                implementation.target_type.as_str(),
                implementation.trait_name.as_deref(),
                implementation
                    .methods
                    .iter()
                    .map(|method| method.name.as_str())
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (owner, source_trait, method) in [
        ("Equal", "Eq", "__eq__"),
        ("Ordered", "Ord", "__lt__"),
        ("Hashed", "Hash", "__hash__"),
    ] {
        let expected = format!("crate::__incan_std::derives::comparison::{source_trait}");
        assert!(
            impls.iter().any(|(target, trait_name, methods)| {
                *target == owner && *trait_name == Some(expected.as_str()) && methods.contains(&method)
            }),
            "{owner} must implement the source trait under `{expected}` with `{method}`: {impls:?}"
        );
    }
    Ok(())
}

/// `Option` identity tests lower to the native presence predicates and never compare payloads.
#[test]
fn followups_b_option_none_identity_lowers_to_presence_methods() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def absent[T](value: Option[T]) -> bool:
    return value is None

def present[T](value: Option[T]) -> bool:
    return value is not None
"#,
    )?;
    for (function_name, expected_method) in [("absent", "is_none"), ("present", "is_some")] {
        let function = ir
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.kind {
                IrDeclKind::Function(function) if function.name == function_name => Some(function),
                _ => None,
            })
            .ok_or_else(|| format!("missing function `{function_name}`"))?;
        let Some(IrStmtKind::Return(Some(returned))) = function.body.first().map(|statement| &statement.kind) else {
            return Err(format!("`{function_name}` has no returned expression"));
        };
        let IrExprKind::MethodCall { method, args, .. } = &returned.kind else {
            return Err(format!(
                "`{function_name}` did not lower to a method call: {returned:?}"
            ));
        };
        assert_eq!(method, expected_method);
        assert!(args.is_empty(), "the presence predicate takes no arguments: {args:?}");
    }
    Ok(())
}

/// Incan's total-order bound must lower to Rust's `Ord`, which is what `slice::sort` requires.
#[test]
fn followups_b_ord_type_parameter_bound_lowers_to_rust_ord() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def ordered[T with Ord](items: list[T]) -> list[T]:
    return sorted(items)
"#,
    )?;
    let function = ir
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.kind {
            IrDeclKind::Function(function) if function.name == "ordered" => Some(function),
            _ => None,
        })
        .ok_or("missing function `ordered`")?;
    let parameter = function
        .type_params
        .iter()
        .find(|parameter| parameter.name == "T")
        .ok_or("missing type parameter `T`")?;
    assert!(
        parameter.bounds.iter().any(|bound| bound.trait_path == "Ord"),
        "`T with Ord` must carry Rust's total-order bound: {:?}",
        parameter.bounds
    );
    assert!(
        !parameter.bounds.iter().any(|bound| bound.trait_path == "PartialOrd"),
        "the total-order bound must not degrade to PartialOrd: {:?}",
        parameter.bounds
    );
    Ok(())
}
