//! Collections, strings and output: list concatenation and membership, hashed dict membership, iteration over
//! representable elements, f-string interpolation, scalar JSON stringification (direct and through the CLI) and
//! `print` recorded rather than rendered.

use super::*;

#[test]
fn replacement_executes_list_concatenation_and_membership() -> Result<(), Box<dyn std::error::Error>> {
    // #1246 gave these operators a Body IR representation; folding in the runtime half is what keeps the executor
    // from refusing calls the compiler had just started emitting.
    let source = r#"
def main() -> int:
  xs = [1, 2]
  ys = [3, 4]
  joined = xs + ys
  if 3 in joined:
    if 9 not in joined:
      return joined[3]
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(4));
    for helper in ["list_concat", "list_contains", "list_not_contains"] {
        assert!(
            execution.body_snapshot.contains(&format!("call helper:{helper}(")),
            "{helper} must execute through its own helper: {}",
            execution.body_snapshot
        );
    }
    Ok(())
}

#[test]
fn replacement_concatenation_leaves_both_operands_usable() -> Result<(), Box<dyn std::error::Error>> {
    // `xs + ys` produces a new list in both Python and the Rust-emission backend, whose `list_concat` borrows both
    // sides. The executor must not consume either operand, and the result must not inherit either one's cursor.
    let source = r#"
def main() -> int:
  xs = [1, 2]
  ys = [30]
  joined = xs + ys
  return xs[0] + ys[0] + joined[2]
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(61));
    Ok(())
}

/// Dict membership now reaches its retained helper through an actual hashed carrier.
#[test]
fn replacement_executes_dict_membership_with_a_hashed_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> bool:
  d = {"a": 1}
  return "a" in d
"#;
    let module = lower_typed_body_ir(source)?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    assert!(
        module.render_snapshot().contains("call helper:dict_contains_key("),
        "Body IR must retain the exact membership helper that executes: {}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn replacement_concatenation_result_iterates_from_the_start() -> Result<(), Box<dyn std::error::Error>> {
    // Indexing the result does not read its cursor, so it cannot catch a concatenation that inherited a
    // partially-advanced iterator position from an operand. Iterating it does: a non-zero starting cursor drops the
    // leading elements and the sum comes out short.
    let source = r#"
def main() -> int:
  xs = [(1, 2)]
  ys = [(4, 8)]
  mut total = 0
  for left, right in xs + ys:
    total += left + right
  return total
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(15));
    Ok(())
}

#[test]
fn replacement_refuses_list_membership_it_cannot_compare() -> Result<(), Box<dyn std::error::Error>> {
    // The executor compares only scalars. An element outside that set must refuse rather than be skipped: skipping
    // would let `false` mean "not present" and "could not tell" at the same time, which is exactly the silent wrong
    // answer this operator's representation exists to prevent.
    let source = r#"
def main() -> bool:
  xs = [[1], [2]]
  return [1] in xs
"#;
    let module = lower_typed_body_ir(source)?;
    let Err(error) = execute_free_function(&module, "main", &[]) else {
        return Err("membership over non-scalar elements must refuse rather than answer".into());
    };

    assert!(
        format!("{error:?}").contains("non-scalar"),
        "the refusal must say it could not compare the elements: {error:?}"
    );
    Ok(())
}

#[test]
fn replacement_iterates_any_list_of_representable_elements() -> Result<(), Box<dyn std::error::Error>> {
    // `for value in [1, 2, 3]` — the most ordinary loop in the language — used to refuse, while `range` and
    // `list[tuple[scalar, scalar]]` both ran. Nothing in execution required the tuple shape: `poll_iterator` clones
    // the element at the cursor and `execute_builtin_next` assigns it, whatever it is. The restriction lived only in
    // the preflight gate, which asked for the one element type the first collection-loop vertical happened to need.
    for (label, source, expected) in [
        (
            "list of ints",
            "def main() -> int:\n  mut total = 0\n  for value in [1, 2, 3]:\n    total += value\n  return total\n",
            6,
        ),
        (
            "list of strings",
            "def main() -> int:\n  mut seen = 0\n  for value in [\"a\", \"b\"]:\n    seen += 1\n  return seen\n",
            2,
        ),
        (
            "list of bools",
            "def main() -> int:\n  mut seen = 0\n  for value in [true, false]:\n    seen += 1\n  return seen\n",
            2,
        ),
        (
            "nested lists",
            "def main() -> int:\n  mut seen = 0\n  for value in [[1], [2, 3]]:\n    seen += 1\n  return seen\n",
            2,
        ),
        (
            "scalar pairs, the shape that already worked",
            "def main() -> int:\n  mut total = 0\n  for left, right in [(1, 2)]:\n    total += left + right\n  return total\n",
            3,
        ),
    ] {
        let module = lower_typed_body_ir(source)?;
        let execution = execute_free_function(&module, "main", &[])?;
        assert_eq!(
            execution.value,
            ReplacementValue::Int(expected),
            "iterating a {label} must execute"
        );
    }
    Ok(())
}

#[test]
fn replacement_still_refuses_iteration_over_an_unrepresentable_element() -> Result<(), Box<dyn std::error::Error>> {
    // Widening the gate to "any representable element" is not the same as removing it. A list this runtime has no
    // value for must still refuse at the source span rather than reaching the poll and failing deeper.
    let source = r#"
model Point:
  x: int

def main() -> int:
  mut seen = 0
  for value in [Point(x=1)]:
    seen += 1
  return seen
"#;
    let module = lower_typed_body_ir(source)?;
    let Err(error) = execute_free_function(&module, "main", &[]) else {
        return Err("iterating a list of model instances must refuse".into());
    };

    let aggregate_start = source
        .find("[Point(x=1)]")
        .ok_or("fixture must contain the list literal")?;
    let span = error
        .primary_span()
        .ok_or("an unrepresentable aggregate refusal must retain its source span")?;
    assert_eq!(span.start, aggregate_start);
    assert_eq!(span.end, aggregate_start + "[Point(x=1)]".len());

    assert!(
        format!("{error:?}").contains("unsupported Body-IR type")
            || format!("{error:?}").contains("not a list of representable elements"),
        "the refusal must name the element type it cannot hold: {error:?}"
    );
    Ok(())
}

#[test]
fn replacement_executes_f_string_interpolation() -> Result<(), Box<dyn std::error::Error>> {
    // Body IR gives an f-string its own structured node rather than a desugared concatenation, and the executor
    // simply never evaluated it -- the same "represented but not executed" shape as list iteration. Interpolation
    // is restricted to the scalars whose rendering provably matches the Rust-emission backend's `{}` / `{:?}`.
    let source = r#"
def main() -> str:
  count = 3
  name = "rows"
  ok = true
  return f"{count} {name} ok={ok} quoted={name:?}"
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(
        execution.value,
        ReplacementValue::Str("3 rows ok=true quoted=\"rows\"".to_string())
    );
    Ok(())
}

#[test]
fn replacement_executes_json_stringify_for_the_admitted_scalar_domain() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def negative() -> str:
  return json_stringify(-9223372036854775807)

def truth() -> str:
  return json_stringify(true)

def escaped() -> str:
  return json_stringify("line\né\t\\\"")

def absent() -> str:
  return json_stringify(None)
"#;
    let module = lower_typed_body_ir(source)?;

    assert_eq!(
        execute_free_function(&module, "negative", &[])?.value,
        ReplacementValue::Str("-9223372036854775807".to_string())
    );
    assert_eq!(
        execute_free_function(&module, "truth", &[])?.value,
        ReplacementValue::Str("true".to_string())
    );
    assert_eq!(
        execute_free_function(&module, "escaped", &[])?.value,
        ReplacementValue::Str("\"line\\né\\t\\\\\\\"\"".to_string())
    );
    assert_eq!(
        execute_free_function(&module, "absent", &[])?.value,
        ReplacementValue::Str("null".to_string())
    );
    Ok(())
}

#[test]
fn replacement_json_stringify_evaluates_its_operand_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def observed_operand() -> int:
  println("direct JSON operand")
  return 7

def main() -> str:
  return json_stringify(observed_operand())
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Str("7".to_string()));
    assert_eq!(execution.emitted_output(), ["direct JSON operand"]);
    Ok(())
}

#[test]
fn replacement_cli_reports_scalar_json_without_legacy_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> str:
  maximum = json_stringify(9223372036854775807)
  escaped = json_stringify("line\né\t\\\"")
  absent = json_stringify(None)
  return f"{maximum}|{escaped}|{absent}"
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
        "scalar JSON must execute through the ordinary replacement CLI path. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(temporary.path().join("replacement-report.json"))?)?;
    assert_eq!(
        report["replacement_execution"]["result"],
        r#"9223372036854775807|"line\né\t\\\""|null"#
    );
    assert_eq!(report["replacement_execution"]["stdout_bytes"], serde_json::json!([]));
    assert_eq!(report["replacement_execution"]["stderr_bytes"], serde_json::json!([]));
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        temporary.path().join(".incan/backend/receipt.json"),
    )?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], "not_needed");
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:"))
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct scalar JSON must not create a legacy generated-project directory"
    );
    Ok(())
}

#[test]
fn replacement_refuses_json_stringify_outside_the_scalar_domain_at_the_call_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> str:\n  return json_stringify([1, 2])\n";
    let module = lower_typed_body_ir(source)?;
    let error = execute_free_function(&module, "main", &[])
        .err()
        .ok_or("direct list JSON must remain outside the scalar profile")?;
    let call = "json_stringify([1, 2])";
    let start = source.find(call).ok_or("fixture must contain the JSON call")?;
    let span = error
        .primary_span()
        .ok_or("direct JSON refusal must retain a source span")?;

    assert_eq!((span.start, span.end), (start, start + call.len()));
    assert!(
        error.to_string().contains("`json_stringify` of list"),
        "the refusal must name the builtin and unsupported value kind: {error}"
    );
    Ok(())
}

#[test]
fn replacement_dispatches_a_lexical_json_stringify_function_by_declaration_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def json_stringify(value: int) -> int:
  return value + 1

def main() -> int:
  return json_stringify(41)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("body json_stringify"),
        "the direct call must execute the retained source declaration: {}",
        execution.body_snapshot
    );
    Ok(())
}

#[test]
fn replacement_refuses_f_string_interpolation_it_cannot_render_identically() -> Result<(), Box<dyn std::error::Error>> {
    // Interpolation is deliberately narrow: structural list Display remains outside the shared rendering profile.
    // Ordinary float Display now uses the same normalized f64 value as native emission; Float Debug still refuses.
    let source = r#"
def main() -> str:
  values = [1, 2]
  return f"{values}"
"#;
    let module = lower_typed_body_ir(source)?;
    let Err(error) = execute_free_function(&module, "main", &[]) else {
        return Err("list interpolation must refuse until both backends render it the same".into());
    };

    assert!(
        format!("{error:?}").contains("f-string interpolation of list"),
        "the refusal must name the value kind it will not render: {error:?}"
    );
    Ok(())
}

#[test]
fn replacement_executes_print_by_recording_its_output() -> Result<(), Box<dyn std::error::Error>> {
    // `println` was the single largest blocker in the example corpus: 25 of 68 examples reached Body IR and stopped
    // at their first call. It is now resolved through `incan_lang`'s builtin registry rather than by name, and the
    // line is *recorded* rather than written -- output a caller can read back is output a comparison can check.
    let source = r#"
def main() -> None:
  println("hello")
  println("count", 3, true)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.emitted_output(), ["hello", "count 3 true"]);
    assert_eq!(execution.value, ReplacementValue::Unit);
    Ok(())
}

#[test]
fn both_backends_render_a_multi_argument_print_the_same_way() -> Result<(), Box<dyn std::error::Error>> {
    // Both backends used to emit `args.first()` and discard the rest, so `println("count", 3, true)` printed
    // `count`. Nothing reported the loss: `check_expr::calls::builtins` gives `Print` no arity check, unlike `Len`
    // beside it, so the dropped arguments were invisible from source, from diagnostics, and from the generated Rust
    // unless read line by line.
    //
    // This asserts the two renderings together rather than separately. Either one alone could drift back to a
    // single argument while still passing its own test; what matters is that they agree.
    let source = "def main() -> None:\n  println(\"count\", 3, true)\n";

    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(
        execution.emitted_output(),
        ["count 3 true"],
        "the replacement executor must render every argument, space-separated"
    );

    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let rust = incan_driver::backend::IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let printed = rust
        .lines()
        .find(|line| line.contains("println!(\"{}"))
        .ok_or("generated Rust must contain the print call")?;
    assert!(
        printed.contains("{} {} {}"),
        "generated Rust must carry one placeholder per argument, got: {printed}"
    );
    Ok(())
}

/// Protected output bindings must be rejected before a replacement Body-IR module can be constructed.
#[test]
fn replacement_rejects_a_source_declaration_that_redefines_immutable_print() -> Result<(), Box<dyn std::error::Error>> {
    // `print` is an immutable language function. Replacement lowering only receives programs that have already
    // passed the shared frontend contract, so redefining it must fail before Body IR can assign it another meaning.
    let source = r#"
def print(value: int) -> int:
  return value + 1
"#;
    let Err(error) = lower_typed_body_ir(source) else {
        return Err("a source declaration named `print` must fail type checking".into());
    };
    assert!(
        error
            .to_string()
            .contains("Cannot redefine immutable built-in function 'print'"),
        "the frontend must report the immutable builtin contract, got: {error}"
    );
    Ok(())
}

#[test]
fn replacement_refuses_to_print_a_value_it_cannot_render_identically() -> Result<(), Box<dyn std::error::Error>> {
    // Printing a list directly is refused at check time (INCAN-T0103), so the value reaches the printer through
    // the f-string that renders it, and that interpolation shares the same boundary: a value whose spelling this
    // runtime and the Rust-emission backend do not provably agree on refuses instead of guessing.
    let source = r#"
def main() -> None:
  items = [1, 2]
  println(f"{items}")
"#;
    let module = lower_typed_body_ir(source)?;
    let Err(error) = execute_free_function(&module, "main", &[]) else {
        return Err("printing a list must refuse until both backends render it the same".into());
    };

    assert!(
        format!("{error:?}").contains("f-string interpolation of list"),
        "the refusal must name the value kind it will not render: {error:?}"
    );
    Ok(())
}
