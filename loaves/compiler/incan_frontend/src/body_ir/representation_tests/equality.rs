use super::*;

/// Verify own-body equality adaptation and its separate owner facet against the frozen module mutation, including
/// fields omitted by the publication codec. The standard slot is a selected native contract in this adapter test.
pub(super) fn assert_equality_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Compare:\n    def equal(self, other: Self) -> bool: ...\nmodel Shape with Compare:\n    value: int\n    def equal(self, other: Self) -> bool:\n        return self.value == other.value\n";
    let (_, _, mut candidate) = checked_seed(source)?;
    let implementation = candidate
        .trait_implementations
        .first_mut()
        .ok_or("missing comparison implementation")?;
    implementation.method.declaration_name = "__eq__".to_owned();
    implementation.method.origin = incan_semantics_core::SymbolOrigin::Module(vec![
        "std".to_owned(),
        "derives".to_owned(),
        "comparison".to_owned(),
    ]);
    let mut expected = candidate.clone();
    super::legacy_source_comparison::retain_equality_borrows(&mut expected);
    super::source_comparison::retain_equality_borrows(&mut candidate);
    assert_same_module(&expected, &candidate)?;
    assert!(candidate.nominal_declarations[0].comparison_equality.is_some());
    Ok(())
}
