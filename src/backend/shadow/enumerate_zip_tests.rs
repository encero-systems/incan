//! Native and checker evidence for the bounded global Enumerate/Zip packet.

use super::legacy_oven::{self, LegacyOvenCapability};
use super::{
    FunctionResultKind, GeneratedWrapperIdentifiers, PreparedShadowProfile, ShadowComparisonProfile, SourceObservable,
    TypedFunctionResult,
};
use crate::frontend::body_ir::build_body_ir_module_v0;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};
use crate::provider::FeatureSelection;

const EXTRA_ARGUMENT: &str = r#"def tail() -> int:
    println("tail")
    return 10

def observe() -> int:
    mut total = 0
    for index, value in enumerate([7], tail()):
        println(index)
        total += value
    return total
"#;

/// Require the documented unary signature before execution, including explicit builtin namespace syntax.
#[test]
fn enumerate_requires_exactly_one_checked_argument() -> Result<(), Box<dyn std::error::Error>> {
    for (source, call, count) in [
        ("def observe() -> None:\n    enumerate()\n", "enumerate()", 0),
        (EXTRA_ARGUMENT, "enumerate([7], tail())", 2),
        (
            "def observe() -> None:\n    std.builtins.enumerate([1], 10)\n",
            "std.builtins.enumerate([1], 10)",
            2,
        ),
    ] {
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_program(&program) {
            Ok(()) => return Err(format!("checker accepted invalid unary call {call}").into()),
            Err(errors) => errors,
        };
        let message = format!("enumerate() expects 1 argument(s), got {count}");
        assert!(
            errors
                .iter()
                .any(|error| error.message == message && source.get(error.span.start..error.span.end) == Some(call)),
            "{errors:?}"
        );
    }
    Ok(())
}

/// Observe unary enumeration independently of direct admission, preserving the reference route's own authority.
#[test]
fn native_enumerate_is_zero_based() -> Result<(), Box<dyn std::error::Error>> {
    let source = EXTRA_ARGUMENT.replace("enumerate([7], tail())", "enumerate([7])");
    assert_native_list_profile(source, "7", b"0\n")
}

/// A canonical Zip must include its source-owned stdlib dependencies in native comparison emission.
#[test]
fn native_global_zip_includes_implicit_stdlib() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def observe() -> int:\n    mut total = 0\n    for left, right in zip([1], [2]):\n        total += left + right\n    return total\n";
    assert_native_list_profile(source.to_string(), "3", b"")
}

/// Native compilation must infer the generic bound required by a backend-inserted alias clone.
#[test]
fn native_generic_alias_infers_required_clone_bound() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def duplicate[T](value: T) -> tuple[T, T]:
    alias = value
    return (value, alias)

def observe() -> int:
    duplicate("abc")
    return 6
"#;
    assert_native_list_profile(source.to_string(), "6", b"")
}

/// Consuming one named Zip twice must fail source checking before either comparison route executes.
#[test]
fn repeated_zip_binding_is_rejected_during_shadow_preflight() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def observe() -> int:
    pairs = zip([1], [2])
    mut total = 0
    for left, right in pairs:
        total += left + right
    for other_left, other_right in pairs:
        total += other_left + other_right
    return total
"#;
    let profile = ShadowComparisonProfile::new(source, "observe", Vec::new());
    let unavailable = match PreparedShadowProfile::new(&profile) {
        Ok(_) => return Err("repeated consumed iterator unexpectedly passed shadow preflight".into()),
        Err(unavailable) => unavailable,
    };
    assert!(unavailable.reason.contains("comparison source did not typecheck"));
    assert!(unavailable.reason.contains("iterator binding `pairs` was consumed"));
    Ok(())
}

/// Execute the native route without replacement preflight, using only an existing receipt-backed capability.
fn assert_native_list_profile(
    source: String,
    expected_result: &str,
    expected_stdout: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let capability = match LegacyOvenCapability::from_environment() {
        Ok(capability) => capability,
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping native enumerate observation: {}", unavailable.reason);
            return Ok(());
        }
    };
    let profile = ShadowComparisonProfile::new(source, "observe", Vec::new());
    let program = profile.program_after_input_contract()?;
    let module_path = vec!["enumerate_zip_native_intake".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    // Native reference evidence must not depend on replacement admission.
    let prepared = PreparedShadowProfile {
        body_ir: build_body_ir_module_v0(&program, &module_path, checker.type_info()),
        result_kind: FunctionResultKind::Int,
        wrapper_identifiers: GeneratedWrapperIdentifiers::fresh_from_checked_source(&checker)?,
    };
    let workspace = tempfile::tempdir()?;
    let source_path = workspace.path().join("native-enumerate-zip-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let route =
        legacy_oven::observe_legacy_route(&profile, &prepared, &materialization, &capability, workspace.path())?;
    assert!(route.authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(route.authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(route.authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(!route.authority.cargo_process_started);
    assert_eq!(route.process.exit_code, Some(0));
    assert_eq!(route.process.stdout, expected_stdout);
    assert!(route.process.stderr.is_empty());
    let observation = route.observation.ok_or("native result was not observed")?;
    assert_eq!(
        observation.observable,
        SourceObservable::Completed {
            result: TypedFunctionResult {
                kind: FunctionResultKind::Int,
                value: expected_result.to_string()
            }
        }
    );
    Ok(())
}
