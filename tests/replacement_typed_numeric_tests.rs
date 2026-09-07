//! End-to-end typed-numeric carrier coverage for replacement execution (#1279).

use incan::backend::replacement::{
    ProgramIo, ReplacementExecutionError, ReplacementNumericValue, ReplacementValue, execute_free_function,
    execute_free_function_with_io,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::types::numerics::NumericTypeId;
use incan_semantics_core::body_ir::{BodyIrModule, Constant, Operand, StatementKind, TypedNumericConstant};

/// Parse, typecheck, and lower one isolated typed-numeric source module.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_typed_numerics".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Collect direct constant assignments from one named Body-IR body.
fn assigned_constants<'module>(module: &'module BodyIrModule, body_name: &str) -> Vec<&'module Constant> {
    module
        .bodies
        .iter()
        .find(|body| body.name == body_name)
        .into_iter()
        .flat_map(|body| &body.block.stmts)
        .filter_map(|statement| match &statement.kind {
            StatementKind::Assign {
                rvalue: incan_semantics_core::body_ir::Rvalue::Use(Operand::Constant(value)),
                ..
            } => Some(value),
            _ => None,
        })
        .collect()
}

/// Require a replacement call to refuse and preserve its structured execution error.
fn require_execution_error<T>(
    result: Result<T, ReplacementExecutionError>,
    context: &str,
) -> Result<ReplacementExecutionError, Box<dyn std::error::Error>> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(context.to_string().into()),
    }
}

/// Exact checked literal kinds and wide payloads must survive lowering into Body IR.
#[test]
fn body_ir_preserves_exact_numeric_literal_kind_and_full_width_payload() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def values() -> str:
  rounded: f32 = 1.23456789
  explicit: f64 = 1.23456789
  wide: u128 = 340282366920938463463374607431768211455
  low: i128 = -170141183460469231731687303715884105728
  price: decimal[6, 2] = 19.90d
  return f"{rounded} {explicit} {wide} {low} {price}"
"#;
    let module = lower_typed_body_ir(source)?;
    let constants = assigned_constants(&module, "values");

    assert!(constants.contains(&&Constant::TypedNumeric(TypedNumericConstant::F32 {
        bits: 1.234_567_9_f32.to_bits(),
    })));
    assert!(constants.contains(&&Constant::TypedNumeric(TypedNumericConstant::F64 {
        bits: 1.23456789_f64.to_bits(),
    })));
    assert!(
        constants.contains(&&Constant::TypedNumeric(TypedNumericConstant::Unsigned {
            kind: NumericTypeId::U128,
            value: u128::MAX,
        }))
    );
    assert!(
        constants.contains(&&Constant::TypedNumeric(TypedNumericConstant::Signed {
            kind: NumericTypeId::I128,
            value: i128::MIN,
        }))
    );
    assert!(
        constants.contains(&&Constant::TypedNumeric(TypedNumericConstant::Decimal {
            precision: 6,
            scale: 2,
            coefficient: 1990,
            literal_scale: 2,
        }))
    );
    Ok(())
}

/// Replacement execution must retain exact local carriers and their source Display spellings.
#[test]
fn replacement_preserves_typed_numeric_locals_and_display() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> str:
  rounded: f32 = 1.23456789
  explicit: f64 = 1.23456789
  wide: u128 = 340282366920938463463374607431768211455
  low: i128 = -170141183460469231731687303715884105728
  price: decimal[6, 2] = 19.90d
  return f"{rounded} {str(explicit)} {wide} {low} {price}"
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(
        execution.value,
        ReplacementValue::Str(format!(
            "{} 1.23456789 {} {} 19.90",
            1.234_567_9_f32,
            u128::MAX,
            i128::MIN
        ))
    );
    Ok(())
}

/// Direct entry arguments and results must satisfy the selected function's exact checked types.
#[test]
fn replacement_validates_typed_entrypoint_arguments_and_returns() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def identity(value: f32) -> f32:
  return value
"#;
    let module = lower_typed_body_ir(source)?;
    let value = ReplacementValue::Numeric(ReplacementNumericValue::F32(1.234_567_9_f32));
    let execution = execute_free_function(&module, "identity", std::slice::from_ref(&value))?;
    assert_eq!(execution.value, value);

    let wrong = ReplacementValue::Numeric(ReplacementNumericValue::F64(1.23456789));
    let error = require_execution_error(
        execute_free_function(&module, "identity", &[wrong]),
        "an f64 carrier must not satisfy an f32 entrypoint parameter",
    )?;
    assert!(
        error.to_string().contains("checked parameter `value` of type `f32`"),
        "{error}"
    );
    Ok(())
}

/// Numeric kind must remain part of receipt identity even when Display output agrees.
#[test]
fn typed_numeric_kind_participates_in_receipt_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def as_f32() -> f32:
  return 1.5

def as_f64() -> f64:
  return 1.5
"#;
    let module = lower_typed_body_ir(source)?;
    let f32_execution = execute_free_function(&module, "as_f32", &[])?;
    let f64_execution = execute_free_function(&module, "as_f64", &[])?;

    assert_eq!(
        f32_execution.value.observable_text(),
        f64_execution.value.observable_text()
    );
    assert_ne!(f32_execution.value, f64_execution.value);
    assert_ne!(f32_execution.output_identity, f64_execution.output_identity);
    Ok(())
}

/// Surface aliases must normalize to the registry's canonical exact carriers.
#[test]
fn aliases_normalize_to_canonical_numeric_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def real_value() -> real:
  return 1.23456789

def huge_value() -> hugeint:
  return 170141183460469231731687303715884105727
"#;
    let module = lower_typed_body_ir(source)?;
    assert_eq!(
        execute_free_function(&module, "real_value", &[])?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::F32(1.234_567_9_f32))
    );
    assert_eq!(
        execute_free_function(&module, "huge_value", &[])?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::Signed {
            kind: NumericTypeId::I128,
            value: i128::MAX,
        })
    );
    Ok(())
}

/// Checked widening must change the runtime carrier at every admitted value boundary.
#[test]
fn checked_widening_changes_the_runtime_carrier_at_assignments_calls_and_results()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def accept_wide(value: u16) -> u16:
  return value

def widen_integer(value: u8) -> u16:
  widened: u16 = value
  return accept_wide(widened)

def widen_explicit_float(value: f32) -> f64:
  widened: f64 = value
  return widened

def widen_ordinary_float(value: f32) -> float:
  widened: float = value
  return widened
"#;
    let module = lower_typed_body_ir(source)?;
    let small = ReplacementValue::Numeric(ReplacementNumericValue::Unsigned {
        kind: NumericTypeId::U8,
        value: 255,
    });
    assert_eq!(
        execute_free_function(&module, "widen_integer", &[small])?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::Unsigned {
            kind: NumericTypeId::U16,
            value: 255,
        })
    );

    let narrow_float = ReplacementValue::Numeric(ReplacementNumericValue::F32(1.234_567_9_f32));
    assert_eq!(
        execute_free_function(&module, "widen_explicit_float", std::slice::from_ref(&narrow_float))?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::F64(f64::from(1.234_567_9_f32)))
    );
    assert_eq!(
        execute_free_function(&module, "widen_ordinary_float", &[narrow_float])?.value,
        ReplacementValue::Float(f64::from(1.234_567_9_f32))
    );
    Ok(())
}

/// Contextual negative literals must retain i128 minimum and explicit-float carriers outside assignments.
#[test]
fn contextual_negative_literals_survive_direct_returns_and_same_module_calls() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def accept_i128(value: i128) -> i128:
  return value

def direct_minimum() -> i128:
  return -170141183460469231731687303715884105728

def called_minimum() -> i128:
  return accept_i128(-170141183460469231731687303715884105728)

def direct_f32() -> f32:
  return -1

def accept_f64(value: f64) -> f64:
  return value

def called_f64() -> f64:
  return accept_f64(-2)
"#;
    let module = lower_typed_body_ir(source)?;
    let minimum = ReplacementValue::Numeric(ReplacementNumericValue::Signed {
        kind: NumericTypeId::I128,
        value: i128::MIN,
    });
    assert_eq!(execute_free_function(&module, "direct_minimum", &[])?.value, minimum);
    assert_eq!(execute_free_function(&module, "called_minimum", &[])?.value, minimum);
    assert_eq!(
        execute_free_function(&module, "direct_f32", &[])?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::F32(-1.0))
    );
    assert_eq!(
        execute_free_function(&module, "called_f64", &[])?.value,
        ReplacementValue::Numeric(ReplacementNumericValue::F64(-2.0))
    );
    Ok(())
}

/// Every sized integer carrier must admit its exact endpoints and reject a wrong-family or out-of-range payload.
#[test]
fn sized_integer_carrier_validation_covers_every_width_and_family() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def identity_i8(value: i8) -> i8:
  return value
def identity_i16(value: i16) -> i16:
  return value
def identity_i32(value: i32) -> i32:
  return value
def identity_i64(value: i64) -> i64:
  return value
def identity_i128(value: i128) -> i128:
  return value
def identity_isize(value: isize) -> isize:
  return value
def identity_u8(value: u8) -> u8:
  return value
def identity_u16(value: u16) -> u16:
  return value
def identity_u32(value: u32) -> u32:
  return value
def identity_u64(value: u64) -> u64:
  return value
def identity_u128(value: u128) -> u128:
  return value
def identity_usize(value: usize) -> usize:
  return value
"#;
    let module = lower_typed_body_ir(source)?;
    let signed_cases = [
        (
            "identity_i8",
            NumericTypeId::I8,
            i128::from(i8::MIN),
            i128::from(i8::MAX),
        ),
        (
            "identity_i16",
            NumericTypeId::I16,
            i128::from(i16::MIN),
            i128::from(i16::MAX),
        ),
        (
            "identity_i32",
            NumericTypeId::I32,
            i128::from(i32::MIN),
            i128::from(i32::MAX),
        ),
        (
            "identity_i64",
            NumericTypeId::I64,
            i128::from(i64::MIN),
            i128::from(i64::MAX),
        ),
        ("identity_i128", NumericTypeId::I128, i128::MIN, i128::MAX),
        (
            "identity_isize",
            NumericTypeId::ISize,
            isize::MIN as i128,
            isize::MAX as i128,
        ),
    ];
    for (function, kind, minimum, maximum) in signed_cases {
        for value in [minimum, maximum] {
            let carrier = ReplacementValue::Numeric(ReplacementNumericValue::Signed { kind, value });
            assert_eq!(
                execute_free_function(&module, function, std::slice::from_ref(&carrier))?.value,
                carrier
            );
        }
        let malformed = ReplacementValue::Numeric(ReplacementNumericValue::Unsigned { kind, value: 0 });
        let error = require_execution_error(
            execute_free_function(&module, function, &[malformed]),
            "a signed checked type must reject an unsigned-family carrier",
        )?;
        assert!(error.to_string().contains("malformed typed numeric carrier"), "{error}");
        if kind != NumericTypeId::I128 {
            let out_of_range = ReplacementValue::Numeric(ReplacementNumericValue::Signed {
                kind,
                value: maximum + 1,
            });
            let error = require_execution_error(
                execute_free_function(&module, function, &[out_of_range]),
                "a signed checked type must reject a value above its maximum",
            )?;
            assert!(error.to_string().contains("malformed typed numeric carrier"), "{error}");
        }
    }

    let unsigned_cases = [
        ("identity_u8", NumericTypeId::U8, u128::from(u8::MAX)),
        ("identity_u16", NumericTypeId::U16, u128::from(u16::MAX)),
        ("identity_u32", NumericTypeId::U32, u128::from(u32::MAX)),
        ("identity_u64", NumericTypeId::U64, u128::from(u64::MAX)),
        ("identity_u128", NumericTypeId::U128, u128::MAX),
        ("identity_usize", NumericTypeId::USize, usize::MAX as u128),
    ];
    for (function, kind, maximum) in unsigned_cases {
        for value in [0, maximum] {
            let carrier = ReplacementValue::Numeric(ReplacementNumericValue::Unsigned { kind, value });
            assert_eq!(
                execute_free_function(&module, function, std::slice::from_ref(&carrier))?.value,
                carrier
            );
        }
        let malformed = ReplacementValue::Numeric(ReplacementNumericValue::Signed { kind, value: 0 });
        let error = require_execution_error(
            execute_free_function(&module, function, &[malformed]),
            "an unsigned checked type must reject a signed-family carrier",
        )?;
        assert!(error.to_string().contains("malformed typed numeric carrier"), "{error}");
        if kind != NumericTypeId::U128 {
            let out_of_range = ReplacementValue::Numeric(ReplacementNumericValue::Unsigned {
                kind,
                value: maximum + 1,
            });
            let error = require_execution_error(
                execute_free_function(&module, function, &[out_of_range]),
                "an unsigned checked type must reject a value above its maximum",
            )?;
            assert!(error.to_string().contains("malformed typed numeric carrier"), "{error}");
        }
    }
    Ok(())
}

/// Public exact-float carriers must reject non-finite values before execution begins.
#[test]
fn non_finite_exact_float_carriers_refuse_before_execution() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(
        "def identity_f32(value: f32) -> f32:\n  return value\ndef identity_f64(value: f64) -> f64:\n  return value\n",
    )?;
    for (function, value) in [
        (
            "identity_f32",
            ReplacementValue::Numeric(ReplacementNumericValue::F32(f32::INFINITY)),
        ),
        (
            "identity_f64",
            ReplacementValue::Numeric(ReplacementNumericValue::F64(f64::NAN)),
        ),
    ] {
        let error = require_execution_error(
            execute_free_function(&module, function, &[value]),
            "non-finite exact-float carriers must be refused",
        )?;
        assert!(error.to_string().contains("malformed typed numeric carrier"), "{error}");
    }
    Ok(())
}

/// Ordinary float parsing may produce IEEE non-finite values, but those values cannot cross an exact-f64 boundary.
#[test]
fn runtime_non_finite_float_values_cannot_become_exact_f64() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def exact(value: str) -> f64:
  return float(value)

def ordinary(value: str) -> float:
  return float(value)
"#;
    let module = lower_typed_body_ir(source)?;
    for input in ["NaN", "inf", "-inf", "1e9999"] {
        let error = require_execution_error(
            execute_free_function(&module, "exact", &[ReplacementValue::Str(input.to_string())]),
            "a runtime non-finite ordinary float must not become exact f64",
        )?;
        assert!(
            error
                .to_string()
                .contains("ValueError: non-finite float cannot initialize exact f64"),
            "{input}: {error}"
        );
        let span = error
            .primary_span()
            .ok_or("the exact-f64 coercion refusal must retain its source span")?;
        let spanned = source
            .get(span.start..span.end)
            .ok_or("the exact-f64 coercion refusal span must index the source")?;
        assert_eq!(spanned.trim(), "return float(value)", "{input}: {span:?}");
    }

    let finite = execute_free_function(&module, "exact", &[ReplacementValue::Str("1.25".to_string())])?;
    assert_eq!(
        finite.value,
        ReplacementValue::Numeric(ReplacementNumericValue::F64(1.25))
    );

    let ordinary = execute_free_function(&module, "ordinary", &[ReplacementValue::Str("NaN".to_string())])?;
    assert!(matches!(ordinary.value, ReplacementValue::Float(value) if value.is_nan()));
    Ok(())
}

/// Malformed integer and decimal carriers must refuse during preflight.
#[test]
fn malformed_or_out_of_range_direct_numeric_carriers_refuse_before_execution() -> Result<(), Box<dyn std::error::Error>>
{
    let integer_module = lower_typed_body_ir("def identity(value: u8) -> u8:\n  return value\n")?;
    let integer_error = require_execution_error(
        execute_free_function(
            &integer_module,
            "identity",
            &[ReplacementValue::Numeric(ReplacementNumericValue::Unsigned {
                kind: NumericTypeId::U8,
                value: 256,
            })],
        ),
        "an out-of-range u8 carrier must be refused",
    )?;
    assert!(
        integer_error
            .to_string()
            .contains("malformed typed numeric carrier for `u8`"),
        "{integer_error}"
    );

    let decimal_module = lower_typed_body_ir("def identity(value: decimal[5, 2]) -> decimal[5, 2]:\n  return value\n")?;
    let decimal_error = require_execution_error(
        execute_free_function(
            &decimal_module,
            "identity",
            &[ReplacementValue::Numeric(ReplacementNumericValue::Decimal {
                precision: 5,
                scale: 2,
                coefficient: 12345,
                literal_scale: 0,
            })],
        ),
        "a decimal exceeding its declared integer-width budget must be refused",
    )?;
    assert!(
        decimal_error
            .to_string()
            .contains("malformed typed numeric carrier for `decimal[5, 2]`"),
        "{decimal_error}"
    );
    Ok(())
}

/// Unsupported typed arithmetic must refuse at its source span before any program effect.
#[test]
fn unsupported_typed_arithmetic_refuses_at_source_span_before_any_output() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def add_one(value: u8) -> int:
  one: u8 = 1
  return value + one

def main() -> int:
  println("must-not-print")
  value: u8 = 1
  return add_one(value)
"#;
    let module = lower_typed_body_ir(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = require_execution_error(
        execute_free_function_with_io(&module, "main", &[], &mut io),
        "reachable typed arithmetic must refuse during preflight",
    )?;
    let rendered = error.to_string();
    assert!(rendered.contains("typed numeric `u8` addition"), "{rendered}");
    assert!(rendered.contains("owned by #988"), "{rendered}");
    let span = error
        .primary_span()
        .ok_or("typed arithmetic refusal must retain its source span")?;
    let operation_start = source.find("value + one").ok_or("missing arithmetic fixture")?;
    assert!(span.start <= operation_start && operation_start < span.end, "{span:?}");
    assert!(io.output().stdout().is_empty());
    assert!(io.output().stderr().is_empty());
    Ok(())
}

/// Resize methods outside #1279 must remain named pre-effect refusals owned by #988.
#[test]
fn typed_resize_methods_stay_named_non_green_under_issue_988() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> i8:
  println("must-not-print")
  wide: i16 = 240
  return wide.saturating_resize()
"#;
    let module = lower_typed_body_ir(source)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let error = require_execution_error(
        execute_free_function_with_io(&module, "main", &[], &mut io),
        "typed resize semantics are not part of the admitted carrier profile",
    )?;
    let rendered = error.to_string();
    assert!(rendered.contains("method `saturating_resize`"), "{rendered}");
    assert!(rendered.contains("owned by #988"), "{rendered}");
    assert!(io.output().stdout().is_empty());
    assert!(io.output().stderr().is_empty());
    Ok(())
}
