//! Contextual Result callback input and result channels retained for direct execution.

use super::*;

/// Detect an unresolved component even when a callable or Result carrier contains it.
fn has_unresolved_component(ty: &IncanType) -> bool {
    match ty {
        IncanType::Unknown | IncanType::Infer => true,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().any(has_unresolved_component),
        IncanType::Ref(inner) | IncanType::RefMut(inner) => has_unresolved_component(inner),
        IncanType::Function { params, return_type } => {
            params.iter().any(|parameter| has_unresolved_component(&parameter.ty))
                || has_unresolved_component(return_type)
        }
        _ => false,
    }
}

/// Require contextual callback channels to reach Body IR without unresolved nested carriers.
fn check_callback_facts(source: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let module = build(source, &["m", name])?;
    for body in &module.bodies {
        assert!(
            body.locals.iter().all(|local| !has_unresolved_component(&local.ty)),
            "{}",
            module.render_snapshot()
        );
    }
    Ok(())
}

/// All six Result combinators retain captured callback parameters and closed result channels.
#[test]
fn capturing_result_callbacks_retain_checked_channels() -> Result<(), Box<dyn std::error::Error>> {
    check_callback_facts(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/capturing_closures_as_result_combinator_callbacks.incn"
        ),
        "capturing_result_callbacks",
    )
}

/// A callback constructor's omitted Result channel takes the checked call's context.
#[test]
fn result_constructor_callbacks_retain_checked_channels() -> Result<(), Box<dyn std::error::Error>> {
    check_callback_facts(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/result_combinator_closures_return_constructors.incn"
        ),
        "constructor_result_callbacks",
    )
}

/// Generic, projected and repeated Result receivers retain the callback's contextual signature.
#[test]
fn reused_result_callbacks_retain_checked_channels() -> Result<(), Box<dyn std::error::Error>> {
    check_callback_facts(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/ownership_parameters_and_receivers/result_methods_leave_their_receiver_usable.incn"
        ),
        "reused_result_callbacks",
    )
}
