//! Direct source execution for the hashed set/dict membership boundary in #1247.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::{lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::body_ir::BodyIrModule;

/// Lower checked source through the same retained facts consumed by normal replacement execution.
fn lower(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["hashed_execution".to_string()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &path, checker.type_info()))
}

/// All four membership helpers execute against hashed carriers, and dict membership never searches values.
#[test]
fn set_and_dict_membership_execute_from_retained_helper_calls() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> int:\n  values = {1, 2, 3}\n  names = {\"key\": \"value\"}\n  if 2 in values and 4 not in values and \"key\" in names and \"value\" not in names:\n    return 42\n  return 0\n";
    let module = lower(source)?;
    let snapshot = module.render_snapshot();
    for helper in [
        "set_contains",
        "set_not_contains",
        "dict_contains_key",
        "dict_not_contains_key",
    ] {
        assert!(snapshot.contains(&format!("call helper:{helper}(")), "{snapshot}");
    }
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Int(42)
    );
    Ok(())
}

/// An empty dict still has a checked scalar key domain and answers absence without a value-kind refusal.
#[test]
fn typed_empty_dict_membership_executes() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower("def main() -> bool:\n  values: dict[str, int] = {}\n  return \"missing\" not in values\n")?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    Ok(())
}

/// A checked zero-argument Set constructor produces the same empty carrier as the membership runtime expects.
#[test]
fn typed_empty_set_membership_executes() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower("def main() -> bool:\n  values: set[int] = Set()\n  return 1 not in values\n")?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    Ok(())
}

/// The canonical zero-argument Dict constructor shares the typed-empty literal membership behavior.
#[test]
fn typed_empty_dict_constructor_membership_executes() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower("def main() -> bool:\n  values: dict[str, int] = Dict()\n  return \"missing\" not in values\n")?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    Ok(())
}

/// Concrete int, bool and string key kinds remain usable by source-local set membership expressions.
#[test]
fn scalar_set_key_kinds_execute() -> Result<(), Box<dyn std::error::Error>> {
    for expression in ["2 in {1, 2}", "true in {false, true}", "\"café\" in {\"café\"}"] {
        let module = lower(&format!("def main() -> bool:\n  return {expression}\n"))?;
        assert_eq!(
            execute_free_function(&module, "main", &[])?.value,
            ReplacementValue::Bool(true)
        );
    }
    Ok(())
}

/// Unit-returning source calls produce concrete unit keys, unlike an unconstrained Option-valued None literal.
#[test]
fn concrete_unit_set_keys_execute() -> Result<(), Box<dyn std::error::Error>> {
    let module = lower(
        "def unit_value() -> None:\n  pass\n\ndef main() -> bool:\n  values = {unit_value()}\n  return unit_value() in values\n",
    )?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    Ok(())
}

/// An empty container must not use vacuous membership to admit a key type outside the retained scalar profile.
#[test]
fn non_scalar_hashed_key_types_refuse_at_construction() -> Result<(), Box<dyn std::error::Error>> {
    for (source, aggregate) in [
        (
            "def main() -> bool:\n  values = {(1, 2)}\n  return (1, 2) in values\n",
            "{(1, 2)}",
        ),
        (
            "def main() -> bool:\n  values: dict[tuple[int, int], int] = {}\n  return (1, 2) not in values\n",
            "{}",
        ),
    ] {
        let module = lower(source)?;
        let error = execute_free_function(&module, "main", &[])
            .err()
            .ok_or("non-scalar hashed key type must refuse")?;
        assert!(error.to_string().contains("unsupported key type"), "{error}");
        let span = error
            .primary_span()
            .ok_or("construction refusal needs its original span")?;
        assert_eq!(source.get(span.start..span.end), Some(aggregate));
    }
    Ok(())
}

/// Container value clones share immutable tables, keeping a membership operand read independent of table size.
#[test]
fn hashed_value_clones_share_their_tables() -> Result<(), Box<dyn std::error::Error>> {
    use incan::backend::replacement::hashed::{ReplacementDict, ReplacementSet};
    use std::rc::Rc;

    let set = Rc::new(ReplacementSet::from_elements([ReplacementValue::Int(1)])?);
    let dict = Rc::new(ReplacementDict::from_entries([(
        ReplacementValue::Int(1),
        ReplacementValue::Int(2),
    )])?);
    let ReplacementValue::Set(set_clone) = ReplacementValue::Set(Rc::clone(&set)).clone() else {
        return Err("set clone must preserve its carrier".into());
    };
    let ReplacementValue::Dict(dict_clone) = ReplacementValue::Dict(Rc::clone(&dict)).clone() else {
        return Err("dict clone must preserve its carrier".into());
    };
    assert!(Rc::ptr_eq(&set, &set_clone));
    assert!(Rc::ptr_eq(&dict, &dict_clone));
    Ok(())
}

/// Hash construction and membership keep source evaluation order, including duplicate dict values.
#[test]
fn hashed_construction_preserves_source_side_effect_order() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def mark(label: str, value: int) -> int:
  println(label)
  return value

def main() -> bool:
  values = {mark("set first", 1), mark("set second", 2)}
  mapping = {mark("key first", 1): mark("value first", 10), mark("key duplicate", 1): mark("value duplicate", 20)}
  return mark("needle", 1) in values and 1 in mapping
"#;
    let module = lower(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Bool(true));
    assert_eq!(
        execution.emitted_output(),
        [
            "set first",
            "set second",
            "key first",
            "value first",
            "key duplicate",
            "value duplicate",
            "needle"
        ]
    );
    Ok(())
}

/// Carrier-level equality must not silently widen the source execution profile.
#[test]
fn hashed_container_equality_remains_an_original_span_refusal() -> Result<(), Box<dyn std::error::Error>> {
    for expression in ["{1} == {1}", "{1: 2} == {1: 2}"] {
        let source = format!("def main() -> bool:\n  return {expression}\n");
        let module = lower(&source)?;
        let error = execute_free_function(&module, "main", &[])
            .err()
            .ok_or("hashed container equality must remain outside this profile")?;
        let span = error.primary_span().ok_or("equality refusal needs its source span")?;
        assert_eq!(source.get(span.start..span.end), Some(expression));
        assert!(error.to_string().contains("comparison"), "{error}");
    }
    Ok(())
}

/// Construction and membership must not accidentally admit unrelated operations on the new carriers.
#[test]
fn hashed_non_membership_operations_remain_refused() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def main() -> int:\n  values = {1: 2}\n  return values[1]\n",
        "def main() -> bool:\n  mut values = {1: 2}\n  values[1] = 3\n  return 1 in values\n",
        "def main() -> None:\n  values = {1, 2}\n  for value in values:\n    pass\n",
        "def main() -> None:\n  values = {1: 2}\n  for value in values:\n    pass\n",
        "def main() -> None:\n  println({1})\n",
        "def main() -> None:\n  println({1: 2})\n",
    ] {
        let module = lower(source)?;
        let error = execute_free_function(&module, "main", &[])
            .err()
            .ok_or_else(|| format!("non-membership container operation unexpectedly executed: {source}"))?;
        assert!(
            matches!(error, ReplacementExecutionError::Unsupported { .. }),
            "{error}"
        );
        let span = error
            .primary_span()
            .ok_or("profile refusal must preserve its source span")?;
        assert!(
            span.end > span.start && source.get(span.start..span.end).is_some(),
            "{error}"
        );
    }
    Ok(())
}

/// Ordinary CLI execution publishes replacement evidence for a source that exercises all four hashed helpers.
#[test]
fn hashed_membership_publishes_a_replacement_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> bool:\n  values = {1, 2}\n  mapping = {1: 20}\n  return 1 in values and 3 not in values and 1 in mapping and 20 not in mapping\n",
    )?;
    let binary = std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_incan")));
    let output = Command::new(binary)
        .current_dir(temporary.path())
        .env("INCAN_HOME", temporary.path().join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            "report.json",
        ])
        .output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "true");
    assert!(temporary.path().join(".incan/backend/receipt.json").is_file());
    Ok(())
}
