//! Normalize RFC 041 capability shorthand at the direct-route declaration and call boundaries.

use incan_lang::interop::is_rust_capability_bound;

/// Keep declared binders while folding bare capability continuations into the preceding capability-bounded binder.
///
/// This is the frozen legacy `lower_type_params` convention: a bare capability following a capability-bound
/// parameter extends its bounds rather than introducing another parameter. A leading capability name or one after
/// an ordinary parameter remains a binder. The caller supplies checked bound classification or checked AST bounds;
/// this helper never changes the legacy header or symbol tables.
pub(crate) fn parameter_names<'a>(parameters: impl Iterator<Item = (&'a str, bool, bool)>) -> Vec<String> {
    let mut names = Vec::new();
    let mut preceding_capability = false;
    for (name, has_bounds, capability_bound) in parameters {
        if !has_bounds && preceding_capability && is_rust_capability_bound(name) {
            continue;
        }
        names.push(name.to_owned());
        preceding_capability = capability_bound;
    }
    names
}

/// Normalize one already checked source header by the same registry-owned capability classification.
pub(crate) fn source_parameter_names(parameters: &[crate::ast::TypeParam]) -> Vec<String> {
    parameter_names(parameters.iter().map(|parameter| {
        (
            parameter.name.as_str(),
            !parameter.bounds.is_empty(),
            parameter
                .bounds
                .iter()
                .any(|bound| is_rust_capability_bound(&bound.name)),
        )
    }))
}
