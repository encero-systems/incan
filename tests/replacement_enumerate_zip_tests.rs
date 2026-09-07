//! Contract tests for the selected, canonical `enumerate` and `zip` replacement profile.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use incan::backend::replacement::{ReplacementExecutionError, ReplacementValue, execute_free_function};
use incan::backend::selection::{BackendExecutionReceipt, BackendKind, FallbackOutcome};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::diagnostics::CompileError;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, StatementKind};
use incan_semantics_core::{HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Lower self-contained source through the checked Body IR consumed by direct execution.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_enumerate_zip".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Retain source diagnostics so malformed calls can refuse before or during direct-profile validation.
fn check_source(source: &str) -> Result<Vec<CompileError>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    match checker.check_program(&program) {
        Ok(()) => Ok(Vec::new()),
        Err(errors) => Ok(errors),
    }
}

/// Execute one source function and assert its complete direct observable contract.
fn assert_direct_execution(
    source: &str,
    function: &str,
    expected_value: ReplacementValue,
    expected_stdout: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, function, &[])?;
    assert_eq!(execution.value, expected_value);
    assert_eq!(execution.output.stdout(), expected_stdout);
    assert!(execution.output.stderr().is_empty());
    assert!(execution.output_identity.starts_with("sha256:"));
    Ok(())
}

/// Require one failed direct execution to retain its original source call span.
fn assert_direct_refusal_at_call(
    module: &BodyIrModule,
    function: &str,
    source: &str,
    call: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let error = match execute_free_function(module, function, &[]) {
        Ok(execution) => {
            return Err(format!(
                "{function} must refuse instead of completing with {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    if !matches!(&error, ReplacementExecutionError::Unsupported { .. }) {
        return Err(format!("{function} must report an unsupported-profile refusal, got {error}").into());
    }
    let start = source
        .find(call)
        .ok_or_else(|| format!("fixture must contain rejected call `{call}`"))?;
    let span = error
        .primary_span()
        .ok_or_else(|| format!("{function} refusal must retain an original source span"))?;
    assert_eq!(span.start, start);
    assert_eq!(span.end, start + call.len());
    Ok(())
}

/// Locate the Cargo-built compiler binary without consulting a shared target directory.
fn incan_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_incan")))
}

/// Build one isolated direct-replacement command with its own Incan home.
fn replacement_command(directory: &Path) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .current_dir(directory)
        .env("INCAN_HOME", directory.join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ]);
    command
}

/// A source-accepted Zip assignment preserves the original value and the alias in either consumption order.
#[test]
fn replacement_zip_alias_assignment_preserves_both_source_values() -> Result<(), Box<dyn std::error::Error>> {
    for (first, second) in [("pairs", "alias"), ("alias", "pairs")] {
        let source = format!(
            r#"def observe() -> int:
    pairs = zip([1], [2])
    alias = pairs
    mut total = 0
    for left, right in {first}:
        total += left + right
    for other_left, other_right in {second}:
        total += other_left + other_right
    return total
"#
        );
        assert_direct_execution(&source, "observe", ReplacementValue::Int(6), b"")?;
    }
    Ok(())
}

/// Repeating a consumed canonical Zip remains a source error rather than a replacement-profile success.
#[test]
fn replacement_zip_repeated_binding_retains_source_consumption_error() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def observe() -> int:
    pairs = zip([1], [2])
    mut total = 0
    for left, right in pairs:
        total += left + right
    for other_left, other_right in pairs:
        total += other_left + other_right
    return total
"#;
    let errors = check_source(source)?;
    let repeated = source
        .rfind("pairs:")
        .ok_or("fixture must contain repeated loop source")?;
    assert!(
        errors.iter().any(|error| {
            error.message.contains("iterator binding `pairs` was consumed")
                && error.span.start == repeated
                && error.span.end == repeated + "pairs".len()
        }),
        "{errors:?}"
    );
    Ok(())
}

/// Canonical enumerate pairs are zero-based, preserve tuple reads and patterns, and skip an empty list.
#[test]
fn replacement_enumerate_lists_are_zero_based_and_preserve_tuple_forms() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def enumerate_profile() -> int:
  values = [10, 20]
  mut total = 0
  for pair in enumerate(values):
    println(pair.0)
    println(pair.1)
    total += pair.0 + pair.1
  empty: list[int] = []
  for empty_index, empty_value in enumerate(empty):
    total += empty_index + empty_value
  for index, value in enumerate([3]):
    total += index + value
  return total
"#;
    assert_direct_execution(
        source,
        "enumerate_profile",
        ReplacementValue::Int(34),
        b"0\n10\n1\n20\n",
    )
}

/// Canonical Zip pairs are list-only, order-aligned, and stop after the shorter side.
#[test]
fn replacement_zip_lists_cover_equal_unequal_and_empty_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def zip_profile() -> int:
  mut total = 0
  for value, label in zip([1, 2], ["one", "two"]):
    println(value)
    println(label)
    total += value
  for pair in zip([7, 8], ["seven"]):
    println(pair.0)
    println(pair.1)
    total += pair.0
  empty_values: list[int] = []
  empty_labels: list[str] = []
  for empty_value, unused_number in zip(empty_values, [9]):
    total += empty_value
  for unused_value, empty_label in zip([4], empty_labels):
    total += unused_value
  return total
"#;
    assert_direct_execution(
        source,
        "zip_profile",
        ReplacementValue::Int(10),
        b"1\none\n2\ntwo\n7\nseven\n",
    )
}

/// Builtin arguments execute their source-owned sibling calls in written order before Zip yields a pair.
#[test]
fn replacement_zip_evaluates_list_returning_sibling_arguments_in_written_order()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def left_values() -> list[int]:
  println("left")
  return [10]

def right_labels() -> list[str]:
  println("right")
  return ["ten"]

def zip_written_order() -> int:
  mut total = 0
  for value, label in zip(left_values(), right_labels()):
    println("pair")
    total += value
  return total
"#;
    assert_direct_execution(
        source,
        "zip_written_order",
        ReplacementValue::Int(10),
        b"left\nright\npair\n",
    )
}

/// Bare aliases retain only the compiler-recorded enumerate/Zip provenance, not a type-spelling guess.
#[test]
fn replacement_enumerate_and_zip_follow_bare_local_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def aliases() -> int:
  values = [5]
  labels = [6]
  values_alias = values
  labels_alias = labels
  enumerated = enumerate(values_alias)
  enumerated_alias = enumerated
  paired = zip(values_alias, labels_alias)
  paired_alias = paired
  mut total = 0
  for index, value in enumerated_alias:
    total += index + value
  for left, right in paired_alias:
    total += left + right
  return total
"#;
    assert_direct_execution(source, "aliases", ReplacementValue::Int(16), b"")
}

/// Source declarations retain their own direct-call identities when they reuse builtin spellings.
#[test]
fn replacement_preserves_same_spelled_source_enumerate_and_zip_functions() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def enumerate(values: list[int]) -> int:
  println("source enumerate")
  return 40

def zip(left: list[int], right: list[int]) -> int:
  println("source zip")
  return 2

def source_owned_spelling() -> int:
  return enumerate([1]) + zip([1], [2])
"#;
    assert_direct_execution(
        source,
        "source_owned_spelling",
        ReplacementValue::Int(42),
        b"source enumerate\nsource zip\n",
    )
}

/// The full call-span builtin fact and callee-span canonical fact agree for checker-recognized iterator builtins.
#[test]
fn replacement_retains_enumerate_and_zip_canonical_builtin_identities() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def retained_iterator_builtins(values: list[int], labels: list[str]) -> None:
  enumerated = enumerate(values)
  paired = zip(values, labels)
"#;
    let module = lower_typed_body_ir(source)?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "retained_iterator_builtins")
        .ok_or("fixture must lower the selected body")?;

    for expected in [BuiltinFnId::Enumerate, BuiltinFnId::Zip] {
        let target = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                StatementKind::Call {
                    callee: Callee::Function(CallableTarget::Named(target)),
                    ..
                } if target.builtin == Some(expected) => Some(target),
                _ => None,
            })
            .ok_or_else(|| {
                format!(
                    "fixture must retain the `{}` builtin target",
                    builtins::as_str(expected)
                )
            })?;
        let canonical = target.canonical.as_ref().ok_or_else(|| {
            format!(
                "`{}` must retain its canonical builtin identity",
                builtins::as_str(expected)
            )
        })?;
        assert_eq!(canonical.namespace, SymbolNamespace::OrdinaryLexical);
        assert_eq!(canonical.origin, SymbolOrigin::Builtin);
        assert_eq!(canonical.declaration_name, builtins::as_str(expected));
        assert_eq!(canonical.kind, SemanticSourceTargetKind::Builtin);
        assert_eq!(canonical.scope_discriminant, None);
        assert_eq!(canonical.declaration_span, HirSourceSpan::new(0, 0));
        assert!(target.direct_call_id.is_none());
    }
    Ok(())
}

/// A missing retained builtin identity cannot borrow the selected global enumerate execution rule.
#[test]
fn replacement_refuses_enumerate_without_its_checked_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def missing_enumerate_identity() -> int:
  values = [1]
  mut total = 0
  for pair in enumerate(values):
    total += pair.1
  return total
"#;
    let mut module = lower_typed_body_ir(source)?;
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "missing_enumerate_identity")
        .ok_or("fixture must lower the selected body")?;
    let target = body
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } if target.name == "enumerate" => Some(target),
            _ => None,
        })
        .ok_or("fixture must lower enumerate as a named Body-IR target")?;
    assert_eq!(target.builtin, Some(BuiltinFnId::Enumerate));
    assert!(target.direct_call_id.is_none());
    let canonical = target
        .canonical
        .as_ref()
        .ok_or("checked enumerate call must retain its canonical builtin identity")?;
    assert_eq!(canonical.namespace, SymbolNamespace::OrdinaryLexical);
    assert_eq!(canonical.origin, SymbolOrigin::Builtin);
    assert_eq!(canonical.declaration_name, "enumerate");
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Builtin);
    assert_eq!(canonical.scope_discriminant, None);
    assert_eq!(canonical.declaration_span, HirSourceSpan::new(0, 0));
    target.builtin = None;

    assert_direct_refusal_at_call(&module, "missing_enumerate_identity", source, "enumerate(values)")
}

/// A spread remains outside the fixed-arity builtin profile even when the parser and checker retain it.
#[test]
fn replacement_refuses_enumerate_spread_at_the_original_call_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def enumerate_spread() -> int:
  values = [[1]]
  for pair in enumerate(*values):
    println(pair.0)
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    assert_direct_refusal_at_call(&module, "enumerate_spread", source, "enumerate(*values)")
}

/// A malformed enumerate call must stop at its source span whether the frontend or executor owns the refusal.
#[test]
fn replacement_refuses_malformed_enumerate_arity_without_a_successful_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def malformed_enumerate_arity() -> int:
  values = [1]
  for pair in enumerate(values, 10):
    println(pair.0)
  return 0
"#;
    let call = "enumerate(values, 10)";
    let start = source
        .find(call)
        .ok_or("fixture must contain the malformed enumerate call")?;
    let end = start + call.len();
    let errors = check_source(source)?;
    if errors.is_empty() {
        let module = lower_typed_body_ir(source)?;
        return assert_direct_refusal_at_call(&module, "malformed_enumerate_arity", source, call);
    }
    let diagnostic = errors
        .iter()
        .find(|error| error.span.start <= start && error.span.end >= end)
        .ok_or_else(|| format!("malformed enumerate arity must diagnose its call span, got {errors:?}"))?;
    assert!(diagnostic.span.start <= start && diagnostic.span.end >= end);
    Ok(())
}

/// A checker-accepted non-list enumerate input remains outside this first direct iterator profile.
#[test]
fn replacement_refuses_nonlist_enumerate_at_the_original_call_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def enumerate_nonlist() -> int:
  mut total = 0
  for pair in enumerate("x"):
    total += pair.0
  return total
"#;
    let module = lower_typed_body_ir(source)?;
    assert_direct_refusal_at_call(&module, "enumerate_nonlist", source, "enumerate(\"x\")")
}

/// The explicit builtin namespace carries the same checked builtin identity as the ambient spelling.
#[test]
fn replacement_executes_explicit_builtin_namespace_enumerate() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def explicit_namespace_enumerate() -> int:
  mut total = 0
  for pair in std.builtins.enumerate([1]):
    total += pair.0
  return total
"#;
    assert_direct_execution(source, "explicit_namespace_enumerate", ReplacementValue::Int(0), b"")
}

/// Normal main execution preserves program output, receipt identity, and explicit refusal fallback.
#[test]
fn replacement_cli_executes_enumerate_and_zip_without_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source = r#"
def main() -> int:
  values = [10, 20]
  labels = ["ten"]
  mut total = 0
  for index, value in enumerate(values):
    println(index)
    println(value)
    total += index + value
  for paired_value, label in zip(values, labels):
    println(paired_value)
    println(label)
    total += paired_value
  return total
"#;
    fs::write(temporary.path().join("main.incn"), source)?;
    let output = replacement_command(temporary.path())
        .args(["--report", "json", "--report-output", "enumerate-zip-report.json"])
        .output()?;
    assert!(
        output.status.success(),
        "selected enumerate/Zip main must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"0\n10\n1\n20\n10\nten\n");
    assert!(output.stderr.is_empty());

    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("enumerate-zip-report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["backend"]["fallback_outcome"], "not_needed");
    assert_eq!(report["replacement_execution"]["result"], "41");
    assert_eq!(
        report["replacement_execution"]["stdout_bytes"],
        serde_json::json!(output.stdout)
    );
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));

    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: BackendExecutionReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    receipt.verify_identity()?;
    assert_eq!(receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(receipt.selection.selected_backend, BackendKind::Replacement);
    assert!(
        report["replacement_execution"]["output_identity"]
            .as_str()
            .is_some_and(|identity| identity == receipt.output_identity),
        "the report must retain the receipt-bound direct output identity: {report}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "replacement execution must not generate a legacy target directory"
    );
    Ok(())
}

/// A non-list selected builtin must refuse before output or successful receipt publication.
#[test]
fn replacement_cli_refuses_nonlist_enumerate_without_a_successful_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source = r#"
def main() -> int:
  println("must not run")
  mut total = 0
  for pair in enumerate("x"):
    total += pair.0
  return total
"#;
    fs::write(temporary.path().join("main.incn"), source)?;
    let output = replacement_command(temporary.path()).output()?;
    assert!(!output.status.success(), "a non-list enumerate call must refuse");
    assert!(
        output.stdout.is_empty(),
        "profile validation must refuse before the preceding program print"
    );
    let call = "enumerate(\"x\")";
    let start = source.find(call).ok_or("fixture must contain non-list enumerate")?;
    let end = start + call.len();
    let source_path = fs::canonicalize(temporary.path().join("main.incn"))?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains(&format!(
            "primary Incan source location: {}:{start}..{end}",
            source_path.display()
        )),
        "CLI refusal must retain the exact original call span: {stderr}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a refused profile must not generate legacy output or publish a success receipt"
    );
    Ok(())
}
