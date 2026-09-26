//! CLI-driven refusals and boundaries: no legacy fallback and no receipt for unsupported source, operator
//! forms, siblings, callable defaults, range aggregates and unrepresentable elements; a shadow request stays
//! explicitly non-green and never alters execution; module and Rust interop boundaries refuse with primary
//! spans.

use super::*;

/// Reject the unimplemented legacy fallback spelling before it can create an artifact or receipt.
#[test]
fn replacement_cli_rejects_legacy_fallback_without_artifacts_or_receipts() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "legacy",
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "the unavailable legacy fallback spelling must be rejected"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("possible values: refuse"),
        "CLI must make the only supported fallback policy visible: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an invalid fallback policy must not produce legacy output or a replacement receipt"
    );
    Ok(())
}

/// Ensure an unsupported source shape fails before any legacy output can be generated.
#[test]
fn replacement_cli_refuses_unsupported_source_without_legacy_generation() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  values = {**{"first": 1}}
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
        ])
        .output()?;
    assert!(
        !output.status.success(),
        "unsupported source must refuse instead of falling back"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("dict aggregate"),
        "unsupported construct must be visible: {combined}"
    );
    assert!(
        combined.contains("original Incan source span"),
        "refusal must retain source authority: {combined}"
    );
    let expected_start = r#"def main() -> int:
  values = {**{"first": 1}}
  return 0
"#
    .find("{**{\"first\": 1}}")
    .ok_or("aggregate fixture must contain its dict literal")?;
    let expected_end = expected_start + "{**{\"first\": 1}}".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{expected_start}..{expected_end}",
            entrypoint.display()
        )),
        "CLI refusal must identify the aggregate-bearing source expression exactly: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project"),
        "refusal must not enter legacy generation: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "unsupported replacement input must not create legacy output or a replacement receipt"
    );
    Ok(())
}

/// New Body-IR forms stay visibly outside the deliberately bounded replacement executor until a later execution
/// slice admits them. A helper call and a primitive exercise its two distinct refusal paths.
#[test]
fn replacement_cli_refuses_new_operator_forms_without_artifacts_or_receipts() -> Result<(), Box<dyn std::error::Error>>
{
    let cases = [
        (
            "string-nonmembership",
            "def main() -> bool:\n  return \"a\" not in \"abc\"\n",
            "\"a\" not in \"abc\"",
            "call to runtime helper `str_not_contains`",
        ),
        (
            "power",
            "def main() -> int:\n  return 2 ** 3\n",
            "2 ** 3",
            "exponentiation",
        ),
    ];

    for (name, source, rejected_expression, expected_boundary) in cases {
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
            "{name} must refuse rather than widening the direct executor"
        );
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected_start = source
            .find(rejected_expression)
            .ok_or("fixture must contain the rejected operator expression")?;
        let expected_end = expected_start + rejected_expression.len();
        assert!(
            combined.contains(expected_boundary)
                && combined.contains("INCAN-R988-UNSUPPORTED")
                && combined.contains(&format!(
                    "primary Incan source location: {}:{expected_start}..{expected_end}",
                    entrypoint.display()
                )),
            "{name} refusal must name its profile boundary at the original operator span: {combined}"
        );
        assert!(
            !combined.contains("Generated Rust project")
                && !temporary.path().join("target/incan").exists()
                && !temporary.path().join(".incan/backend/receipt.json").exists(),
            "{name} refusal must not fall back, generate a legacy artifact, or publish a replacement receipt"
        );
    }
    Ok(())
}

/// A selected entrypoint must not publish a receipt when an invoked sibling fails the same direct profile gate.
#[test]
fn replacement_cli_refuses_an_unsupported_sibling_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def unsupported_sibling() -> int:
  unsafe:
    pass
  return 42

def main() -> int:
  return unsupported_sibling()
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
        .output()?;
    assert!(!output.status.success(), "unsupported sibling must visibly refuse");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("`unsafe:` acknowledgment region") && combined.contains("original Incan source span"),
        "the sibling refusal must preserve its direct profile boundary: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an unsupported sibling must not generate legacy output or publish a replacement receipt"
    );
    Ok(())
}

/// Refuse an unsupported declaration default at the default's original source span before a selection can become an
/// execution receipt.
#[test]
fn replacement_cli_refuses_unsupported_callable_default_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"def keep(payload: bytes = b"x") -> bytes:
  return payload

def main() -> int:
  keep()
  return 1
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
        "a non-evaluable callable default must refuse instead of falling back"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let default_start = source.find("b\"x\"").ok_or("fixture must contain bytes default")?;
    let default_end = default_start + "b\"x\"".len();
    assert!(
        combined.contains("byte-string literal"),
        "refusal must name the unsupported default: {combined}"
    );
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{default_start}..{default_end}",
            entrypoint.display()
        )),
        "refusal must preserve the declaration-default span: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an unsupported default must not generate legacy output or publish a replacement receipt"
    );
    Ok(())
}

/// A source-selected range value is rejected at the aggregate before replacement can execute its bound call,
/// generate a legacy artifact, or publish a receipt.
#[test]
fn replacement_cli_refuses_a_range_aggregate_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"def bound() -> int:
  assert false
  return 4

def main() -> int:
  values = 0..bound()
  return 1
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
    assert!(!output.status.success(), "a range aggregate must visibly refuse");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let range_start = source
        .find("0..bound()")
        .ok_or("fixture must contain a range aggregate")?;
    let range_end = range_start + "0..bound()".len();
    assert!(
        combined.contains("range aggregate"),
        "refusal must name the unsupported aggregate: {combined}"
    );
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{range_start}..{range_end}",
            entrypoint.display()
        )),
        "refusal must retain the aggregate span: {combined}"
    );
    assert!(
        !combined.contains("assertion failed")
            && !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a range aggregate must not execute its bounds, fall back, or publish a receipt"
    );
    Ok(())
}

/// Refuse an empty default whose compiler-owned aggregate type lies outside the structural vocabulary.
#[test]
fn replacement_cli_refuses_typed_empty_non_structural_callable_default_without_a_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"def keep(items: list[float] = []) -> int:
  return 1

def main() -> int:
  return keep()
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
        "an empty non-structural default must visibly refuse instead of executing vacuously"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let aggregate_start = source.find("[]").ok_or("fixture must contain the empty aggregate")?;
    assert!(
        combined.contains("structural aggregate destination has unsupported Body-IR type `List[float]`"),
        "refusal must identify the unavailable aggregate type: {combined}"
    );
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{aggregate_start}..{}",
            entrypoint.display(),
            aggregate_start + "[]".len()
        )),
        "refusal must retain the empty default's original source span: {combined}"
    );
    assert!(
        !combined.contains("Generated Rust project")
            && !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an unsupported deferred aggregate must not generate legacy output or publish a replacement receipt"
    );
    Ok(())
}

/// Refuse a typed empty scalar list before replacement can publish a success receipt.
#[test]
fn replacement_cli_refuses_iteration_over_an_unrepresentable_element_without_a_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    // Renamed and re-aimed. This test was written when iterating *any* list of scalars refused, so an empty
    // `list[int]` was its subject. That restriction is gone — the executor always could iterate any element, and
    // only the preflight said otherwise — so the subject is now an element the runtime genuinely has no value for.
    // The refusal now lands on the list literal rather than the loop, because the runtime has no value for a model
    // instance and says so when the aggregate is built. The properties worth keeping are unchanged: the refusal
    // reaches the CLI naming the type it cannot hold, retains source authority, and publishes neither legacy output
    // nor a receipt.
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    let source = r#"model Point:
  x: int

def main() -> int:
  mut seen = 0
  for value in [Point(x=1)]:
    seen += 1
  return seen
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
        "an unrepresentable iteration element must refuse instead of executing directly"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("unsupported Body-IR type `List[Point]`"),
        "refusal must name the element type the runtime has no value for: {combined}"
    );
    assert!(
        combined.contains("original Incan source span"),
        "refusal must retain source authority: {combined}"
    );
    let aggregate_start = source
        .find("[Point(x=1)]")
        .ok_or("fixture must contain the list literal")?;
    let aggregate_end = aggregate_start + "[Point(x=1)]".len();
    assert!(
        combined.contains(&format!(
            "primary Incan source location: {}:{aggregate_start}..{aggregate_end}",
            entrypoint.display()
        )),
        "CLI refusal must retain the exact aggregate source span: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "an iteration-profile refusal must not create legacy output or a replacement receipt"
    );
    Ok(())
}

/// Persist a requested shadow comparison as explicitly unavailable when no legacy runtime comparator exists.
#[test]
fn replacement_cli_shadow_receipt_is_explicitly_non_green() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--shadow",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "shadowed replacement build must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(receipt_path)?)?;
    assert_eq!(receipt["selection"]["shadow_requested"], true);
    let reason = receipt["shadow_comparison"]["unavailable"]["reason"]
        .as_str()
        .ok_or("requested shadow comparison must persist an unavailable reason")?;
    assert_eq!(
        reason,
        incan_driver::backend::shadow::PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON
    );

    // The CLI must report the same truth the corpus path does: this build observes the module's `main`, which the
    // bounded #1146 comparison profile deliberately excludes, so no comparison ran and none is claimed.
    assert!(
        receipt["shadow_comparison"].get("matched").is_none() && receipt["shadow_comparison"].get("diverged").is_none(),
        "a build-path shadow request must never persist a comparison outcome: {}",
        receipt["shadow_comparison"]
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "shadow comparison must not generate a legacy project as semantic evidence"
    );
    Ok(())
}

/// A shadow request must not change what the replacement build executes, records, or refuses.
///
/// The comparison axis is additive: asking for one may make a receipt non-green, but it may never turn a refusal
/// into a success, silently select the other backend, or alter the executed result.
#[test]
fn a_shadow_request_does_not_alter_replacement_execution() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = support::repo_command()
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--shadow",
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
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "non-printing programs must leave stdout empty"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(report["replacement_execution"]["result"], "42");

    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["selection"]["fallback_policy"], "refuse");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    Ok(())
}

/// Refuse imports and Rust-module directives with typed, file-addressable source diagnostics.
#[test]
fn replacement_cli_refuses_module_boundaries_with_primary_spans() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "import",
            "import std.io\n\ndef main() -> int:\n  return 42\n",
            "import declaration",
        ),
        (
            "aliased-async-activation",
            "import std.async as async_runtime\n\ndef main() -> int:\n  return 42\n",
            "import declaration",
        ),
        (
            "from-async-service",
            "from std.async.time import sleep\n\ndef main() -> int:\n  return 42\n",
            "import declaration",
        ),
        (
            "rust-module",
            "rust.module(\"incan_std_testing\")\n\ndef main() -> int:\n  return 42\n",
            "Rust interop `rust.module` directive",
        ),
        (
            "generic-model",
            "model Box[T]:\n  value: T\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
        (
            "class",
            "class Pair:\n  left: int\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
        (
            "model-method",
            "model Pair:\n  left: int\n  def score(self) -> int:\n    return self.left\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
        (
            "model-property",
            "model Pair:\n  left: int\n  property score -> int:\n    return self.left\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
        (
            "model-field-alias",
            "model Pair:\n  left [alias=\"wire_left\"]: int\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
        (
            "payload-enum",
            "enum Flag:\n  On(int)\n\ndef main() -> int:\n  return 42\n",
            "non-function top-level declaration",
        ),
    ];
    for (name, source, expected_boundary) in cases {
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
            "{name} must be refused by the source-only profile"
        );
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains("INCAN-R988-UNSUPPORTED"),
            "missing typed diagnostic: {combined}"
        );
        assert!(
            combined.contains(expected_boundary),
            "missing refusal boundary: {combined}"
        );
        assert!(
            combined.contains(&format!(
                "primary Incan source location: {}:{}..",
                entrypoint.display(),
                0
            )),
            "refusal must retain its actual unsupported source span: {combined}"
        );
        assert!(
            !temporary.path().join("target/incan").exists()
                && !temporary.path().join(".incan/backend/receipt.json").exists(),
            "{name} refusal must not create legacy output or a replacement receipt"
        );
    }
    Ok(())
}

/// A Rust or Python interop boundary must refuse as a missing host, not as a construct the profile has not reached.
///
/// #1262 requires public diagnostics to distinguish host-capability failures from the other reasons a program is
/// refused. Before this, `import rust::serde_json` and `import std.io` both reported "import declaration", which told
/// a reader that the replacement route had not got round to imports yet. The truth is narrower and more useful: the
/// route reached this import and found that executing it needs a Rust interop host it does not have.
///
/// The crate is named because it is the actionable identity -- it is what an eventual interop plan is selected
/// against -- and the refusal is addressed to the file that crosses the boundary, which is not always the entry
/// module.
#[test]
fn a_rust_interop_boundary_refuses_as_a_missing_host() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "rust-crate-import",
            "import rust::serde_json\n\ndef main() -> int:\n  return 42\n",
            "Rust interop module import of crate `serde_json`",
        ),
        (
            "rust-item-import",
            "from rust::incan_std_core::text import normalize\n\ndef main() -> int:\n  return 42\n",
            "Rust interop item import of crate `incan_std_core`",
        ),
        (
            "python-import",
            "import python \"os\"\n\ndef main() -> int:\n  return 42\n",
            "Python interop import of `os`",
        ),
    ];
    for (name, source, expected_boundary) in cases {
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
            "{name} must be refused by the source-only profile"
        );
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains("INCAN-R988-UNSUPPORTED"),
            "{name} must be refused through the typed profile diagnostic: {combined}"
        );
        assert!(
            combined.contains(expected_boundary),
            "{name} must name the interop boundary it crosses: {combined}"
        );
        assert!(
            !combined.contains("does not support import declaration"),
            "{name} must not report an interop boundary as an unreached construct: {combined}"
        );
    }

    // The pair that makes the distinction real: an ordinary import is still refused as a construct this profile has
    // not reached, and never claims a missing interop host.
    let temporary = tempfile::tempdir()?;
    let ordinary = temporary.path().join("ordinary.incn");
    fs::write(&ordinary, "import std.io\n\ndef main() -> int:\n  return 42\n")?;
    let output = support::repo_command()
        .args([
            "build",
            ordinary.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ])
        .output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("does not support import declaration"),
        "a standard-library import must still refuse as an unreached construct: {combined}"
    );
    assert!(
        !combined.contains("interop host"),
        "a standard-library import must not claim a missing interop host: {combined}"
    );
    Ok(())
}

/// An interop refusal is addressed to the module that crosses the boundary, not to the entry module.
///
/// #1318 admitted local module imports into the profile, so the first Rust boundary a program reaches is now often in
/// a module the entry file merely imported. A refusal that pointed at the entry module would send a reader to a file
/// containing nothing to fix.
#[test]
fn a_rust_interop_refusal_names_the_module_that_crosses_the_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let helpers = temporary.path().join("helpers.incn");
    fs::write(
        &helpers,
        "from rust::incan_std_core::text import normalize\n\npub def helper() -> int:\n  return 1\n",
    )?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        "from helpers import helper\n\ndef main() -> int:\n  return helper()\n",
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
        .output()?;
    assert!(
        !output.status.success(),
        "a Rust interop boundary must be refused wherever it is declared"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Rust interop item import of crate `incan_std_core`"),
        "an imported module's interop boundary must be named: {combined}"
    );
    assert!(
        combined.contains(&format!("primary Incan source location: {}", helpers.display()))
            || combined.contains("helpers.incn:"),
        "the refusal must address the module that crosses the boundary: {combined}"
    );
    assert!(
        !combined.contains("main.incn:0.."),
        "the refusal must not be addressed to the entry module: {combined}"
    );
    Ok(())
}
