//! Core body execution: the real replacement receipt, the single canonical entrypoint, refusals of unsupported
//! Body IR and structural return values, tuple and collection projections, shadowing and reassignment by local
//! identity, Python-signed division, and the selected string, ownership, control-flow and assertion cases.

use super::*;

/// Execute typed source through Body IR and bind the observed result to an explicit replacement receipt.
#[test]
fn replacement_executes_core_body_and_binds_a_real_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  left = 40
  right = 2
  if left > right:
    return left + right
  return right
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(execution.body_snapshot.contains("body main"));
    assert!(execution.body_snapshot.contains("span="));
    assert!(
        execution
            .ownership_reads
            .iter()
            .all(|read| !matches!(read.fact, OwnershipFact::Unknown)),
        "the selected core corpus must not erase ownership facts: {:?}",
        execution.ownership_reads
    );

    let selection = select_backend(
        BackendKind::Replacement,
        true,
        false,
        digest_output(&[source]),
        FallbackPolicy::Refuse,
    );
    let receipt = finalize_receipt(
        &selection,
        BackendKind::Replacement,
        execution.output_identity.clone(),
        ShadowComparisonState::NotRequested,
        incan_frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    receipt.verify_identity()?;
    assert_eq!(receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    assert_eq!(receipt.selection.source_identity, digest_output(&[source]));
    let ownership_evidence = execution.ownership_evidence();
    let runtime_evidence = execution.runtime_requirement_evidence();
    assert!(
        ownership_evidence
            .iter()
            .all(|read| !read.fact.is_empty() && read.span_end >= read.span_start),
        "canonical ownership evidence must retain stable facts and spans: {ownership_evidence:?}"
    );
    assert_eq!(
        serde_json::to_string(&runtime_evidence)?,
        serde_json::to_string(&execution.runtime_requirement_evidence())?,
        "runtime-requirement evidence must be deterministic across repeated projections"
    );
    let repeated = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.output_identity, repeated.output_identity);
    assert_eq!(execution.ownership_evidence(), repeated.ownership_evidence());
    assert_eq!(
        execution.runtime_requirement_evidence(),
        repeated.runtime_requirement_evidence()
    );
    Ok(())
}

/// Execute source-local structural values through an identity-selected sibling without widening the result profile.
#[test]
fn replacement_executes_source_local_tuple_list_index_and_mutation_through_a_direct_callable()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def score(mut values: list[int]) -> int:
  values[0] = 40
  pair = (values[0], 2)
  return pair.0 + pair.1

def main() -> int:
  values = [1, 2]
  return score(values)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("call fn:score("),
        "the result must come from direct Body-IR sibling execution: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Dispatch overloads using the declaration identity retained by Body IR, never a name scan at runtime.
#[test]
fn replacement_executes_the_exact_same_module_overload_selected_by_the_typechecker()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def pick(a: int, b: int) -> int:
  return a - b

def pick(b: str, a: str) -> str:
  return a

def main() -> int:
  return pick(a=42, b=1)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(41));
    assert!(
        execution.body_snapshot.contains("body pick"),
        "the selected declaration must execute as a direct Body-IR frame: {}",
        execution.body_snapshot
    );
    Ok(())
}

#[test]
fn replacement_entrypoint_requires_one_canonical_free_function() -> Result<(), Box<dyn std::error::Error>> {
    let mut missing_identity = lower_typed_body_ir("def main() -> int:\n  return 42\n")?;
    missing_identity
        .bodies
        .first_mut()
        .ok_or("fixture must contain main")?
        .canonical = None;
    let error = match execute_free_function(&missing_identity, "main", &[]) {
        Ok(_) => return Err("an entrypoint without canonical authority must refuse".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("without an exact canonical free-function identity")
    );

    let mut mismatched_identity = lower_typed_body_ir("def main() -> int:\n  return 42\n")?;
    mismatched_identity
        .bodies
        .first_mut()
        .and_then(|body| body.canonical.as_mut())
        .ok_or("fixture must contain main's canonical identity")?
        .declaration_name = "not_main".to_string();
    let error = match execute_free_function(&mismatched_identity, "main", &[]) {
        Ok(_) => return Err("an entrypoint whose body and requested target disagree must refuse".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("without an exact canonical free-function identity")
    );

    let method_only = lower_typed_body_ir("class App:\n  def main(self) -> int:\n    return 42\n")?;
    let error = match execute_free_function(&method_only, "main", &[]) {
        Ok(_) => return Err("a same-named method must not become a free-function entrypoint".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("without an exact canonical free-function identity")
    );

    let overloaded = lower_typed_body_ir(
        "def main(value: int) -> int:\n  return value\n\ndef main(value: str) -> int:\n  return 0\n",
    )?;
    let error = match execute_free_function(&overloaded, "main", &[ReplacementValue::Int(1)]) {
        Ok(_) => return Err("an overloaded entrypoint must not select the first body".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("ambiguous overloaded free-function entrypoint")
    );
    Ok(())
}

/// Refuse a dict spread outside the membership profile at its original source span.
#[test]
fn replacement_refuses_unsupported_body_ir_with_the_original_source_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  values = {**{"first": 1}}
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "dict spreads must remain outside the hashed membership profile but executed as {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };

    assert!(
        error.primary_span().is_some(),
        "unsupported execution must retain an Incan source span: {error}"
    );
    let expected_start = source
        .find("{**{\"first\": 1}}")
        .ok_or("aggregate fixture must contain its dict assignment")?;
    let expected_end = expected_start + "{**{\"first\": 1}}".len();
    let span = error
        .primary_span()
        .ok_or("aggregate refusal must retain its original source span")?;
    assert_eq!(span.start, expected_start);
    assert_eq!(span.end, expected_end);
    assert!(
        error.to_string().contains("dict aggregate"),
        "unsupported operation must be named visibly: {error}"
    );
    Ok(())
}

/// Keep aggregate return values outside the scalar source-observable result profile.
#[test]
fn replacement_refuses_structural_aggregate_return_values() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> list[int]:
  return [1]
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "an aggregate return is outside the scalar result profile but executed as {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };

    assert!(
        error.primary_span().is_some(),
        "aggregate-return refusal must retain an Incan source span: {error}"
    );
    assert!(
        error.to_string().contains("returning list"),
        "aggregate return must be named visibly: {error}"
    );
    Ok(())
}

/// Execute the selected plain builtin-collection loop over scalar tuple values.
#[test]
fn replacement_executes_plain_scalar_tuple_collection_loop() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  pairs = [(1, 2), (3, 4)]
  for pair in pairs:
    if false:
      return 0
  return 7
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(7));
    assert!(execution.body_snapshot.contains("iter_next("));
    Ok(())
}

/// Execute `for a, b in pairs` directly and bind the actual result to a replacement receipt.
#[test]
fn replacement_executes_scalar_tuple_collection_destructuring_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  pairs = [(1, 2), (4, 5)]
  for a, b in pairs:
    if a == 4:
      return a * 10 + b
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(45));
    assert!(execution.body_snapshot.contains(".0"));
    assert!(execution.body_snapshot.contains(".1"));

    let selection = select_backend(
        BackendKind::Replacement,
        true,
        false,
        digest_output(&[source]),
        FallbackPolicy::Refuse,
    );
    let receipt = finalize_receipt(
        &selection,
        BackendKind::Replacement,
        execution.output_identity,
        ShadowComparisonState::NotRequested,
        incan_frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    receipt.verify_identity()?;
    assert_eq!(receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    Ok(())
}

/// Refuse an index projection outside the selected tuple-field loop profile at its original source span.
#[test]
fn replacement_refuses_collection_index_projection_with_the_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  pairs = [(1, 2)]
  return pairs[0][0]
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => return Err(format!("index projection executed as {:?}", execution.value).into()),
        Err(error) => error,
    };
    let expected_start = source
        .find("return pairs[0][0]")
        .ok_or("index-projection fixture must contain its source statement")?;
    let span = error
        .primary_span()
        .ok_or("collection index-projection refusal must retain its source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("nested place projection"),
        "collection index projection must remain visible: {error}"
    );
    Ok(())
}

/// Execute a one-level source-local list index and numeric tuple projection directly.
#[test]
fn replacement_executes_plain_collection_index_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  pairs = [(1, 2)]
  pair = pairs[0]
  return pair.0
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    Ok(())
}

/// Execute a standalone numeric tuple projection directly.
#[test]
fn replacement_executes_standalone_tuple_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  pair = (1, 2)
  return pair.0
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(1));
    Ok(())
}

/// Execute explicit `let` shadowing from the canonical locals selected by Body IR, without recovering by name.
#[test]
fn replacement_executes_let_shadowing_by_local_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  mut total = 0
  x = 1
  if true:
    let x = 2
    total += x
  return total + x
"#;
    let module = lower_typed_body_ir(source)?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("shadowing fixture must lower `main`")?;
    let x_locals = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .collect::<Vec<_>>();
    assert_eq!(x_locals.len(), 2, "the outer and inner declarations must both survive");
    assert_ne!(x_locals[0].id, x_locals[1].id);
    assert_ne!(x_locals[0].identity, x_locals[1].identity);

    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(3));
    Ok(())
}

/// `mut` is also a declaration form: its inner value must not overwrite the same-spelled outer local.
#[test]
fn replacement_executes_mut_shadowing_by_local_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  mut total = 0
  mut x = 4
  if true:
    mut x = 7
    total += x
  return total + x
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(11));
    Ok(())
}

/// Execute same-scope reassignment through the original local rather than declaring a duplicate binding.
#[test]
fn replacement_executes_reassignment_without_a_repeated_user_binding_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  mut x = 1
  x = 2
  return x
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(2));
    Ok(())
}

/// Match Incan's Python-style signed integer division and modulo contract through the replacement executor.
#[test]
fn replacement_uses_python_signed_floor_division_and_modulo() -> Result<(), Box<dyn std::error::Error>> {
    let division = lower_typed_body_ir(
        r#"def main() -> int:
  return 7 // -3
"#,
    )?;
    let modulo = lower_typed_body_ir(
        r#"def main() -> int:
  return 7 % -3
"#,
    )?;

    assert_eq!(
        execute_free_function(&division, "main", &[])?.value,
        ReplacementValue::Int(-3)
    );
    assert_eq!(
        execute_free_function(&modulo, "main", &[])?.value,
        ReplacementValue::Int(-2)
    );
    Ok(())
}

/// Execute the remaining source-only cases selected for the first #988 proof corpus.
#[test]
fn replacement_executes_the_selected_string_ownership_control_flow_and_assertion_cases()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "string concatenation",
            r#"def main() -> str:
  name = "Ada"
  return "hi " + name
"#,
            ReplacementValue::Str("hi Ada".to_string()),
            "helper:str_concat",
        ),
        (
            "non-copy return",
            r#"def main() -> str:
  value = "owned"
  return value
"#,
            ReplacementValue::Str("owned".to_string()),
            "move(_",
        ),
        (
            "normalized range and while control flow",
            r#"def main() -> int:
  for value in range(1, 5):
    if value % 2 == 0:
      continue
  while false:
    return 0
  return 10
"#,
            ReplacementValue::Int(10),
            "loop:",
        ),
        (
            "assertion and floor division",
            r#"def main() -> int:
  a = 84
  b = 2
  assert b != 0
  return a // b
"#,
            ReplacementValue::Int(42),
            "assert",
        ),
    ];
    for (name, source, expected, snapshot_evidence) in cases {
        let module = lower_typed_body_ir(source)?;
        let execution = execute_free_function(&module, "main", &[]).map_err(|error| {
            format!(
                "replacement execution failed for {name}: {error}\n{}",
                module.render_snapshot()
            )
        })?;
        assert_eq!(execution.value, expected, "replacement result diverged for {name}");
        assert!(
            execution.body_snapshot.contains(snapshot_evidence),
            "Body IR proof for {name} omitted `{snapshot_evidence}`:\n{}",
            execution.body_snapshot
        );
    }

    let failing_assertion = lower_typed_body_ir(
        r#"def main() -> int:
  a = 84
  b = 0
  assert b != 0
  return a // b
"#,
    )?;
    let error = match execute_free_function(&failing_assertion, "main", &[]) {
        Ok(execution) => return Err(format!("failing assertion executed as {:?}", execution.value).into()),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("assertion failed"),
        "unexpected assertion outcome: {error}"
    );
    assert!(
        error.primary_span().is_some(),
        "assertion failure lost its source span: {error}"
    );
    Ok(())
}
