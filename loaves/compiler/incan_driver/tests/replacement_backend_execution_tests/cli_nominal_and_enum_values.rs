//! CLI-driven nominal and enum programs: a direct named callable, source-local models, value and fieldless
//! enums, enum matching and same-error routing with a replacement receipt, and the profiles refused without
//! one.

use super::*;

/// A selected entrypoint may call an admitted sibling body directly, and its receipt must still name only the
/// replacement execution that actually happened.
#[test]
fn replacement_cli_executes_direct_named_callable_with_a_replacement_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def route(method: int, path: int, content_type: int = 3) -> int:
  return method * 100 + path * 10 + content_type

def main() -> int:
  method = 1
  get = partial route(method=method)
  return get(4)
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .args([
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an admitted direct sibling call must execute. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "143");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "successful direct execution must produce a verifiable replacement receipt: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct callable execution must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Execute a source-local plain model through Body IR and publish only the replacement selection receipt.
#[test]
fn replacement_cli_executes_a_source_local_nominal_model_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"model Pair:
  left: int
  right: int = 2

def score(pair: Pair) -> int:
  return pair.left + pair.right

def main() -> int:
  pair = Pair(right=2, left=40)
  return score(pair)
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an admitted source-local model must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert!(
        report["replacement_execution"]["result"] == "42",
        "the direct result must be source-observable in the replacement report: {report}"
    );
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| {
                snapshot.contains("executed nominal constructor name=Pair id=decl:main#decl.")
                    && snapshot.contains("fields=[left, right]")
            }),
        "the direct report must retain the exact nominal identity/layout execution evidence: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "a direct nominal report must not invent legacy generated-Rust or Oven evidence: {report}"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct nominal execution must bind an output identity: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a direct nominal execution must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Execute an exact source-local RFC 032 value-enum extraction and publish only replacement evidence.
#[test]
fn replacement_cli_executes_a_source_local_value_enum_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def status_code() -> int:
  return HttpStatus.NotFound.value()

def main() -> int:
  return status_code()
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an admitted source-local value enum must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "404");
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| {
                snapshot.contains("executed value-enum variant name=HttpStatus::NotFound enum_id=decl:main#decl.")
                    && snapshot.contains("raw=404")
                    && snapshot.contains("extracted value-enum scalar name=HttpStatus::NotFound")
            }),
        "the direct report must retain exact enum/member execution evidence: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "a direct value-enum report must not invent generated-Rust or Oven evidence: {report}"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct value-enum execution must bind an output identity: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a direct value-enum execution must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Execute source-local fieldless normal-enum equality and publish only replacement evidence.
#[test]
fn replacement_cli_executes_a_source_local_fieldless_enum_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"enum Signal:
  Ready
  Stop

def score(left: Signal, right: Signal) -> int:
  if left == Signal.Ready and right != Signal.Ready:
    return 42
  return 0

def main() -> int:
  return score(Signal.Ready, Signal.Stop)
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an admitted source-local fieldless enum must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| {
                snapshot.contains("executed fieldless-enum variant name=Signal::Ready enum_id=decl:main#decl.")
                    && snapshot.contains("executed fieldless-enum variant name=Signal::Stop enum_id=decl:main#decl.")
            }),
        "the direct report must retain exact normal-enum member execution evidence: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "a direct fieldless-enum report must not invent generated-Rust or Oven evidence: {report}"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct fieldless-enum execution must bind an output identity: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a direct fieldless-enum execution must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Execute fieldless-enum pattern dispatch directly and publish a replacement-only receipt.
#[test]
fn replacement_cli_executes_fieldless_enum_matching_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"enum Signal:
  Ready
  Stop

def main() -> int:
  return classify(Signal.Ready)

def classify(signal: Signal) -> int:
  match signal:
    case Signal.Ready:
      return 42
    case Signal.Stop:
      return 0
"#;
    fs::write(&entrypoint, source)?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an exact source-local fieldless-enum matcher must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| {
                snapshot.contains("fieldless fieldless_enum_variant(Signal::Ready")
                    && snapshot.contains("executed direct match arm")
            }),
        "the direct report must bind the selected fieldless pattern to its retained identity: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "a direct fieldless-enum matcher must not invent generated-Rust or Oven evidence: {report}"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert!(
        receipt["executed_backend"] == "replacement" && receipt["fallback_outcome"] == "not_needed",
        "the matcher must publish only an admitted replacement receipt: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a direct fieldless-enum matcher must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Execute intrinsic Result construction, exact same-error routing, and `Ok` matching through a replacement receipt.
#[test]
fn replacement_cli_executes_same_error_result_routing_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"enum Failure:
  Odd

def half(value: int) -> Result[int, Failure]:
  if value % 2 != 0:
    return Err(Failure.Odd)
  return Ok(value // 2)

def quarter(value: int) -> Result[int, Failure]:
  half_value = half(value)?
  return half(half_value)

def main() -> int:
  match quarter(8):
    case Ok(value):
      return value
    case Err(_):
      return 0
  return 0
"#,
    )?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
            "--report-output",
            temporary
                .path()
                .join("replacement-report.json")
                .to_string_lossy()
                .as_ref(),
        ])
        .output()?;
    assert!(
        output.status.success(),
        "an exact same-error Result path must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "2");
    assert!(
        report["replacement_execution"]["body_snapshot"]
            .as_str()
            .is_some_and(|snapshot| {
                snapshot.contains("result_ok(")
                    && snapshot.contains("same_error_type=Failure")
                    && snapshot.contains("executed Result try route=ok")
            }),
        "the direct report must retain intrinsic Result construction and exact routing evidence: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "a direct Result report must not invent generated-Rust or Oven evidence: {report}"
    );
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert!(
        receipt["executed_backend"] == "replacement"
            && receipt["fallback_outcome"] == "not_needed"
            && receipt["identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct Result execution must bind a replacement receipt identity: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "a direct Result execution must not create a legacy generated-project directory"
    );
    Ok(())
}

/// Refuse generated value-enum lookup helpers that this profile has not represented as direct Body-IR behavior.
#[test]
fn replacement_cli_refuses_value_enum_from_value_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def main() -> int:
  parsed = HttpStatus.from_value(404)
  return 42
"#;
    fs::write(&entrypoint, source)?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "an unrepresented value-enum lookup helper must visibly refuse"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let source_expression = "HttpStatus.from_value(404)";
    let start = source
        .find(source_expression)
        .ok_or("fixture must contain the refused value-enum lookup")?;
    let end = start + source_expression.len();
    assert!(
        combined.contains("from_value"),
        "the refusal must name the unrepresented lookup surface: {combined}"
    );
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{start}..{end}",
            entrypoint.display()
        )),
        "the refusal must retain the lookup expression source span: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an unrepresented value-enum lookup must not fall back or publish a receipt"
    );
    Ok(())
}

/// Refuse non-structural nominal fields and nested nominal projections without creating fallback artifacts or receipts.
#[test]
fn replacement_cli_refuses_nonstructural_and_nested_nominal_profiles_without_a_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "non-structural-field",
            r#"model FloatBox:
  value: float

def main() -> int:
  boxed = FloatBox(value=1.5)
  return 42
"#,
            "constructor `FloatBox` with a non-structural field value",
            "FloatBox(value=1.5)",
        ),
        (
            "nested-index",
            r#"model Bucket:
  values: list[int]

def main() -> int:
  bucket = Bucket(values=[40, 2])
  return bucket.values[0]
"#,
            "nested place projection",
            "return bucket.values[0]",
        ),
        (
            "nested-slice",
            r#"model Bucket:
  values: list[int]

def main() -> list[int]:
  bucket = Bucket(values=[40, 2])
  return bucket.values[0:1]
"#,
            "nested place projection",
            "return bucket.values[0:1]",
        ),
        (
            "plain-slice",
            r#"def main() -> list[int]:
  values = [40, 2]
  return values[0:1]
"#,
            "slice projection",
            "return values[0:1]",
        ),
    ];
    for (name, source, expected_boundary, source_expression) in cases {
        let temporary = tempfile::tempdir()?;
        let entrypoint = temporary.path().join(format!("{name}.incn"));
        fs::write(&entrypoint, source)?;
        let output = support::repo_command()
            .args([
                "build",
                entrypoint.to_string_lossy().as_ref(),
                "--backend",
                "replacement",
                "--backend-fallback",
                "refuse",
            ])
            .output()?;
        assert!(
            !output.status.success(),
            "{name} must visibly refuse outside the nominal profile"
        );
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let start = source
            .find(source_expression)
            .ok_or("fixture must contain its rejected source expression")?;
        let end = start + source_expression.len();
        assert!(
            combined.contains(expected_boundary),
            "{name} must name its direct-profile boundary: {combined}"
        );
        assert!(
            combined.contains(&format!(
                "primary Incan source location: {}:{start}..{end}",
                entrypoint.display()
            )),
            "{name} must retain the exact rejected source span: {combined}"
        );
        assert!(
            !combined.contains("Generated Rust project")
                && !temporary.path().join("target/incan").exists()
                && !temporary.path().join(".incan/backend/receipt.json").exists(),
            "{name} must not fall back or publish a replacement receipt"
        );
    }
    Ok(())
}

/// Refuse an omitted nominal field default before a replacement execution receipt can be written.
#[test]
fn replacement_cli_refuses_an_omitted_nominal_field_default_without_a_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"model Pair:
  left: int
  right: int = 2

def main() -> int:
  pair = Pair(left=40)
  return pair.left
"#;
    fs::write(&entrypoint, source)?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "a model default without retained declaration computation must visibly refuse"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let constructor_start = source
        .find("Pair(left=40)")
        .ok_or("fixture must contain its defaulted constructor")?;
    let constructor_end = constructor_start + "Pair(left=40)".len();
    assert!(
        combined.contains("constructor `Pair` with an omitted field default"),
        "the refusal must identify the unavailable nominal default: {combined}"
    );
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{constructor_start}..{constructor_end}",
            entrypoint.display()
        )),
        "the refusal must retain the constructor source span: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an unavailable nominal default must not fall back or publish a receipt"
    );
    Ok(())
}
