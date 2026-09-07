//! Native reference evidence for Unicode-scalar string length.

use super::legacy_oven::{self, LegacyOvenCapability};
use super::{
    FunctionResultKind, GeneratedWrapperIdentifiers, PreparedShadowProfile, ShadowComparisonProfile, SourceObservable,
    TypedFunctionResult,
};
use crate::frontend::body_ir::build_body_ir_module_v0;
use crate::frontend::typechecker::TypeChecker;
use crate::provider::FeatureSelection;

/// Observe one string-length expression independently of replacement admission.
fn assert_native_string_length(expression: &str) -> Result<(), Box<dyn std::error::Error>> {
    let capability = match LegacyOvenCapability::from_environment() {
        Ok(capability) => capability,
        Err(unavailable)
            if std::env::var_os("INCAN_SHADOW_REQUIRE_LEGACY_ROUTE")
                .is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            return Err(unavailable.into());
        }
        Err(unavailable) => {
            eprintln!("skipping native string-length observation: {}", unavailable.reason);
            return Ok(());
        }
    };
    let profile = ShadowComparisonProfile::new(
        format!("def observe() -> int:\n    println(\"native len\")\n    return {expression}\n"),
        "observe",
        Vec::new(),
    );
    let program = profile.program_after_input_contract()?;
    let module_path = vec!["len_string_native_intake".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let prepared = PreparedShadowProfile {
        body_ir: build_body_ir_module_v0(&program, &module_path, checker.type_info()),
        result_kind: FunctionResultKind::Int,
        wrapper_identifiers: GeneratedWrapperIdentifiers::fresh_from_checked_source(&checker)?,
    };
    let workspace = tempfile::tempdir()?;
    let source_path = workspace.path().join("native-string-len-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let route =
        legacy_oven::observe_legacy_route(&profile, &prepared, &materialization, &capability, workspace.path())?;
    assert!(!route.authority.cargo_process_started);
    assert_eq!(route.process.exit_code, Some(0));
    assert_eq!(route.process.stdout, b"native len\n");
    assert!(route.process.stderr.is_empty());
    let observation = route.observation.ok_or("native result was not observed")?;
    assert_eq!(
        observation.observable,
        SourceObservable::Completed {
            result: TypedFunctionResult {
                kind: FunctionResultKind::Int,
                value: "7".to_string(),
            },
        }
    );
    Ok(())
}

/// Observe the global builtin independently of replacement admission.
///
/// This is intentionally a native-only intake test: before the repair, direct execution refuses `len(str)`, but
/// that refusal must not hide what the existing Rust-emission route actually does.
#[test]
fn native_global_len_str_counts_unicode_scalars() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_string_length(r#"len("") + len("abc") + len("é") + len("😀") + len("é")"#)
}

/// The canonical method spelling observes the same five Unicode-scalar rows on the native route.
#[test]
fn native_method_len_str_counts_unicode_scalars() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_string_length(r#""".len() + "abc".len() + "é".len() + "😀".len() + "é".len()"#)
}
