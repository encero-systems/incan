//! Direct execution and malformed-evidence coverage for checked `isinstance` targets.

use incan::backend::replacement::{
    ProgramIo, ReplacementExecutionError, ReplacementValue, execute_free_function, execute_free_function_with_io,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::{Body, BodyIrModule, IsInstanceTarget, Rvalue, StatementKind};
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind};

const UNION_SOURCE: &str = r#"def is_int(value: int | str) -> bool:
  if isinstance(value, int):
    return true
  return false

def is_str(value: int | str) -> bool:
  if isinstance(value, str):
    return true
  return false

def is_bool(value: bool | str) -> bool:
  if isinstance(value, bool):
    return true
  return false

def float_member() -> float | str:
  return 1.5

def is_float() -> bool:
  value = float_member()
  if isinstance(value, float):
    return true
  return false
"#;
const FIXED_CASE_SOURCE: &str = include_str!("fixtures/replacement/isinstance_targets.incn");

/// Lower self-contained checked source into the Body IR consumed by direct execution.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_isinstance".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Find the one direct typed test in a deliberately simple fixture body.
fn isinstance_test_mut(
    body: &mut Body,
) -> Result<(&mut IncanType, &mut IsInstanceTarget, HirSourceSpan), Box<dyn std::error::Error>> {
    body.block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Assign {
                rvalue: Rvalue::IsInstance { value_ty, target, .. },
                ..
            } => Some((value_ty, target, statement.span)),
            _ => None,
        })
        .ok_or("fixture must lower one typed isinstance operation".into())
}

/// Require one direct check to return a boolean with no program stream effects.
fn assert_check(
    module: &BodyIrModule,
    function: &str,
    argument: ReplacementValue,
    expected: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let execution = execute_free_function(module, function, &[argument])?;
    assert_eq!(execution.value, ReplacementValue::Bool(expected));
    assert!(execution.output.stdout().is_empty());
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

#[test]
fn replacement_executes_true_and_false_checks_for_the_bounded_primitive_target_set()
-> Result<(), Box<dyn std::error::Error>> {
    let module = lower(UNION_SOURCE)?;
    for (function, positive, negative) in [
        (
            "is_int",
            ReplacementValue::Int(7),
            ReplacementValue::Str("x".to_string()),
        ),
        (
            "is_str",
            ReplacementValue::Str("x".to_string()),
            ReplacementValue::Int(7),
        ),
        (
            "is_bool",
            ReplacementValue::Bool(true),
            ReplacementValue::Str("x".to_string()),
        ),
    ] {
        assert_check(&module, function, positive, true)?;
        assert_check(&module, function, negative, false)?;
    }
    let float_execution = execute_free_function(&module, "is_float", &[])?;
    assert_eq!(float_execution.value, ReplacementValue::Bool(true));
    assert!(float_execution.output.stdout().is_empty());
    assert!(float_execution.output.stderr().is_empty());
    Ok(())
}

#[test]
fn fixed_case_executes_every_admitted_target_with_exact_streams() -> Result<(), Box<dyn std::error::Error>> {
    let execution = execute_free_function(&lower(FIXED_CASE_SOURCE)?, "isinstance_targets", &[])?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(execution.output.stdout(), b"isinstance targets\n");
    assert!(execution.output.stderr().is_empty());
    Ok(())
}

#[test]
fn source_defined_isinstance_keeps_its_direct_callable_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def isinstance(value: int, target: int) -> bool:
  return value == target

def probe() -> bool:
  return isinstance(1, 1)

def explicit_probe(value: int | str) -> bool:
  return std.builtins.isinstance(value, int)
"#;
    let module = lower(source)?;
    let probe = module
        .bodies
        .iter()
        .find(|body| body.name == "probe")
        .ok_or("fixture must lower probe")?;
    let target =
        probe
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                StatementKind::Call {
                    callee:
                        incan_semantics_core::body_ir::Callee::Function(
                            incan_semantics_core::body_ir::CallableTarget::Named(target),
                        ),
                    ..
                } => Some(target),
                _ => None,
            })
            .ok_or("source-defined isinstance must remain an ordinary direct call")?;
    assert!(target.direct_call_id.is_some());
    assert!(target.builtin.is_none());
    assert_eq!(
        execute_free_function(&module, "probe", &[])?.value,
        ReplacementValue::Bool(true)
    );
    assert_check(&module, "explicit_probe", ReplacementValue::Int(1), true)?;
    assert_check(
        &module,
        "explicit_probe",
        ReplacementValue::Str("text".to_string()),
        false,
    )?;
    Ok(())
}

#[test]
fn replacement_refuses_unsupported_or_malformed_targets_at_a_trusted_span_before_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def probe(value: int | str) -> bool:\n  println(\"must not run\")\n  return isinstance(value, int)\n";
    let target_start = source.find("int)").ok_or("fixture must contain the target")?;

    for corruption in ["value_type", "unsupported", "identity", "span"] {
        let mut module = lower(source)?;
        let probe = module
            .bodies
            .iter_mut()
            .find(|body| body.name == "probe")
            .ok_or("fixture must lower probe")?;
        let (value_ty, target, call_span) = isinstance_test_mut(probe)?;
        let expected_span = match corruption {
            "value_type" => {
                *value_ty = IncanType::Generic {
                    base: "List".to_string(),
                    args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
                };
                call_span
            }
            "unsupported" => {
                target.ty = IncanType::Named("Unrepresented".to_string());
                HirSourceSpan::new(target_start, target_start + 3)
            }
            "identity" => {
                target.canonical = Some(CanonicalSymbolId::module_declaration(
                    vec!["replacement_isinstance".to_string()],
                    "Unrepresented",
                    SemanticSourceTargetKind::Model,
                    target.span,
                ));
                HirSourceSpan::new(target_start, target_start + 3)
            }
            "span" => {
                target.span = HirSourceSpan::new(0, 1);
                call_span
            }
            _ => return Err("unreachable corruption case".into()),
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = execute_free_function_with_io(
            &module,
            "probe",
            &[ReplacementValue::Int(1)],
            &mut ProgramIo::new(&mut stdout, &mut stderr),
        )
        .err()
        .ok_or("corrupted target evidence must refuse")?;
        assert!(
            matches!(error, ReplacementExecutionError::Unsupported { .. }),
            "{error}"
        );
        assert_eq!(error.primary_span(), Some(expected_span));
        assert!(stdout.is_empty(), "profile validation must run before println");
        assert!(stderr.is_empty());
    }
    Ok(())
}

#[test]
fn primitive_targets_with_different_runtime_tags_do_not_coerce() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def probe(value: int | bool) -> bool:\n  return isinstance(value, int)\n";
    let module = lower(source)?;
    assert_check(&module, "probe", ReplacementValue::Int(1), true)?;
    assert_check(&module, "probe", ReplacementValue::Bool(true), false)?;
    assert_eq!(
        module
            .bodies
            .iter()
            .find(|body| body.name == "probe")
            .and_then(|body| body.locals.last())
            .map(|local| &local.ty),
        Some(&IncanType::Primitive(IncanPrimitiveType::Bool))
    );
    Ok(())
}

#[test]
fn non_scalar_checked_value_types_refuse_before_effects() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def probe(value: int | List[int]) -> bool:\n  println(\"must not run\")\n  return isinstance(value, int)\n";
    let module = lower(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = execute_free_function_with_io(
        &module,
        "probe",
        &[ReplacementValue::Int(1)],
        &mut ProgramIo::new(&mut stdout, &mut stderr),
    )
    .err()
    .ok_or("a non-scalar checked value type must refuse before execution")?;
    let span = error.primary_span().ok_or("the refusal must retain the call span")?;

    assert_eq!(source.get(span.start..span.end), Some("isinstance(value, int)"));
    assert!(error.to_string().contains("value type"), "{error}");
    assert!(stdout.is_empty(), "profile validation must run before println");
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn malformed_isinstance_in_a_reachable_callee_refuses_before_entrypoint_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def broken(value: int | str) -> bool:
  return isinstance(value, int)

def probe() -> bool:
  println("must not run")
  return broken(1)
"#;
    let mut module = lower(source)?;
    let broken = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "broken")
        .ok_or("fixture must lower the reachable sibling")?;
    let (_, target, _) = isinstance_test_mut(broken)?;
    target.ty = IncanType::Named("Corrupted".to_string());
    let target_start = source
        .find("int)\n\n")
        .ok_or("fixture must contain the broken target")?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = execute_free_function_with_io(&module, "probe", &[], &mut ProgramIo::new(&mut stdout, &mut stderr))
        .err()
        .ok_or("reachable malformed target evidence must refuse during preparation")?;

    assert_eq!(
        error.primary_span(),
        Some(HirSourceSpan::new(target_start, target_start + 3))
    );
    assert!(error.to_string().contains("Corrupted"), "{error}");
    assert!(
        stdout.is_empty(),
        "the entrypoint must not print before reachable-body preflight"
    );
    assert!(stderr.is_empty());
    Ok(())
}

#[test]
fn checked_nominal_target_refuses_at_its_retained_reference_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"model Marker:
  value: int

def probe(value: Marker | str) -> bool:
  return isinstance(value, Marker)
"#;
    let module = lower(source)?;
    let error = execute_free_function(&module, "probe", &[ReplacementValue::Str("text".to_string())])
        .err()
        .ok_or("a nominal isinstance target is outside the bounded primitive profile")?;
    let span = error.primary_span().ok_or("the refusal must retain a target span")?;
    assert_eq!(source.get(span.start..span.end), Some("Marker"));
    assert!(error.to_string().contains("declaration identity"), "{error}");
    Ok(())
}
