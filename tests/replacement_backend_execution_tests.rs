//! End-to-end proof for the bounded #988 Body-IR replacement executor.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use incan::backend::replacement::{ReplacementValue, execute_free_function};
use incan::backend::selection::{
    BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState, digest_output, finalize_receipt,
    select_backend,
};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::{BodyIrModule, OwnershipFact};

/// Lower one self-contained, typechecked source module into the Body IR the replacement backend consumes.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_execution".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Locate the compiler binary built for this integration-test invocation.
fn incan_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/incan"))
}

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
        incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
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

/// Refuse an unimplemented dict aggregate at its original source span.
#[test]
fn replacement_refuses_unsupported_body_ir_with_the_original_source_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  values = {"first": 1}
  return 0
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "dict aggregates must remain outside the source-local structural profile but executed as {:?}",
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
        .find("{\"first\": 1}")
        .ok_or("aggregate fixture must contain its dict assignment")?;
    let expected_end = expected_start + "{\"first\": 1}".len();
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

/// Materialize a lazy generator expression in the replacement executor without evaluating its filter or element
/// while constructing the Body-IR value. The selected profile admits only the explicit `.collect()` consumer and
/// indexes its scalar result; it must not treat the generator rvalue itself as an eager list or use generated Rust.
#[test]
fn replacement_executes_a_lazy_generator_expression_only_when_collect_consumes_it()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  values = (value * 10 for value in range(1, 5) if value > 2).collect()
  return values[0] + values[1]
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(70));
    assert!(
        execution.body_snapshot.contains("generator(source="),
        "replacement execution must consume the explicit deferred generator rvalue"
    );
    assert!(
        execution.body_snapshot.contains("yield"),
        "the Body-IR proof must retain a yield rather than materializing an eager list"
    );
    Ok(())
}

/// Constructing a generator must not evaluate its deferred element body. The division would fail if the replacement
/// executor accidentally treated the rvalue as an eager collection; dropping the unconsumed value must still return
/// the surrounding scalar result.
#[test]
fn replacement_keeps_an_unconsumed_generator_expression_deferred() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  unused = (value // 0 for value in range(1, 2))
  return 7
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(7));
    assert!(
        execution.body_snapshot.contains("yield ") && execution.body_snapshot.contains("const(0)"),
        "the deferred body must remain represented even though it was never consumed"
    );
    Ok(())
}

/// A stored closure captures its lexical value at construction, executes in an isolated local frame, and contributes
/// direct Body-IR evidence rather than routing through generated Rust.
#[test]
fn replacement_executes_a_captured_stored_closure_in_an_isolated_frame() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  offset = 2
  add: (int) -> int = (value) => value + offset
  return add(40)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        execution.body_snapshot.contains("closure(params=[value:"),
        "direct evidence must retain the stored closure Body IR: {}",
        execution.body_snapshot
    );
    assert!(
        execution.body_snapshot.contains("executed stored callable frame"),
        "the receipt-bound evidence must distinguish an invoked closure from construction: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Partial presets stay construction-time captures, while an omitted source default runs at the call and a named
/// argument overrides the preset's declaration slot.
#[test]
fn replacement_executes_partial_presets_source_defaults_and_named_overrides() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def route(method: int, path: int, content_type: int = 3) -> int:
  return method * 100 + path * 10 + content_type

def main() -> int:
  mut method = 1
  get = partial route(method=method)
  normal = get(4)
  overridden = get(method=7, path=2, content_type=5)
  return normal + overridden
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(143 + 725));
    assert!(
        execution.body_snapshot.contains("body route"),
        "the forwarding target must be direct Body-IR evidence, not generated Rust: {}",
        execution.body_snapshot
    );
    assert!(
        execution.body_snapshot.contains("executed source default frame"),
        "the omitted declaration default must appear as an actually executed frame: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Every directly dispatched sibling body passes the same fail-closed profile gate as the selected entrypoint.
#[test]
fn replacement_refuses_an_unsupported_sibling_body_at_its_original_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def unsupported_sibling() -> int:
  values: list[int] = []
  for value in values:
    return value
  return 42

def main() -> int:
  return unsupported_sibling()
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "an unsupported sibling must refuse before it executes directly, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("for value in values")
        .ok_or("fixture must contain the unsupported scalar-list iteration")?;
    let span = error
        .primary_span()
        .ok_or("sibling refusal must retain a source span")?;
    assert_eq!(span.start, expected_start);
    assert!(error.to_string().contains("builtin collection iteration destination"));
    Ok(())
}

/// A same-module `range` declaration retains an exact direct-call identity instead of being confused with the builtin.
#[test]
fn replacement_executes_a_same_module_range_declaration_by_its_direct_call_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def range(start: int, end: int) -> int:
  return start + end

def main() -> int:
  return range(20, 22)
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(execution.body_snapshot.contains("body range"));
    Ok(())
}

/// Refuse an id-less `range` call unless lowering retained the explicit compiler-builtin target fact.
#[test]
fn replacement_refuses_an_idless_range_call_without_the_explicit_builtin_fact() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def count(values: list[int]) -> int:
  return 42

def main() -> int:
  return count(range(1, 3))
"#;
    let mut module = lower_typed_body_ir(source)?;
    let main = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("fixture must lower the main Body-IR body")?;
    let target =
        main.block
            .stmts
            .iter_mut()
            .find_map(|statement| match &mut statement.kind {
                incan_semantics_core::body_ir::StatementKind::Call {
                    callee:
                        incan_semantics_core::body_ir::Callee::Function(
                            incan_semantics_core::body_ir::CallableTarget::Named(target),
                        ),
                    ..
                } if target.name == "range" => Some(target),
                _ => None,
            })
            .ok_or("fixture must lower its range call as a named Body-IR target")?;
    assert!(
        target.builtin.is_some(),
        "an unshadowed range call must retain an explicit builtin target fact"
    );
    target.builtin = None;

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "an id-less range target without a builtin fact must refuse, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let range_start = source
        .find("range(1, 3)")
        .ok_or("fixture must contain the range call")?;
    let span = error
        .primary_span()
        .ok_or("an id-less range refusal must retain the original call span")?;
    assert_eq!(span.start, range_start);
    assert_eq!(span.end, range_start + "range(1, 3)".len());
    assert!(
        error.to_string().contains("call to function `range`"),
        "the refusal must name the unavailable call target: {error}"
    );
    Ok(())
}

/// A generator function resumes its retained loop cursor across yields without replaying its earlier elements.
/// `.collect()` is only the selected consumer; the underlying runtime polls one persisted frame at a time.
#[test]
fn replacement_resumes_a_generator_function_without_replaying_its_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def counter() -> Generator[int]:
  for value in range(1, 3):
    yield value
  yield 3

def main() -> int:
  values = counter().collect()
  return values[0] * 100 + values[1] * 10 + values[2]
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[]).map_err(|error| {
        format!(
            "generator-function execution failed: {error}\n{}",
            module.render_snapshot()
        )
    })?;

    assert_eq!(execution.value, ReplacementValue::Int(123));
    assert!(
        execution.body_snapshot.contains("body counter"),
        "the generator-function body must be bound into direct execution evidence: {}",
        execution.body_snapshot
    );
    assert!(
        execution.body_snapshot.contains("executed generator-function frame"),
        "the generator-function frame must be recorded only once polling begins: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Constructing then dropping a generator function keeps its body out of execution evidence until a consumer polls
/// the retained frame.
#[test]
fn replacement_keeps_an_unconsumed_generator_function_out_of_execution_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def counter() -> Generator[int]:
  yield 1

def main() -> int:
  unused = counter()
  return 42
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])?;

    assert_eq!(execution.value, ReplacementValue::Int(42));
    assert!(
        !execution.body_snapshot.contains("body counter"),
        "constructing a generator must not claim its body executed: {}",
        execution.body_snapshot
    );
    assert!(
        !execution.body_snapshot.contains("executed generator-function frame"),
        "constructing a generator must not claim its frame was polled: {}",
        execution.body_snapshot
    );
    Ok(())
}

/// Map and filter adapters retain an unpolled source and invoke local closures through the same callable-frame
/// binder used by ordinary local calls.
#[test]
fn replacement_executes_lazy_generator_adapters_with_local_callbacks() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  offset = 1
  increment: (int) -> int = (value) => value + offset
  accepted: (int) -> bool = (value) => value > 2
  values = (value for value in range(1, 5)).map(increment).filter(accepted).collect()
  return values[0] * 10 + values[1]
"#;
    let module = lower_typed_body_ir(source)?;
    let execution = execute_free_function(&module, "main", &[])
        .map_err(|error| format!("adapter execution failed: {error}\n{}", module.render_snapshot()))?;

    assert_eq!(execution.value, ReplacementValue::Int(34));
    assert!(execution.body_snapshot.contains("method:map"));
    assert!(execution.body_snapshot.contains("method:filter"));
    assert!(execution.body_snapshot.contains("executed generator-expression frame"));
    assert!(
        execution
            .body_snapshot
            .contains("executed generator-adapter callback frame")
    );
    Ok(())
}

/// An omitted non-evaluable source default must refuse at the default expression rather than execute a legacy path
/// or publish an untruthful receipt.
#[test]
fn replacement_refuses_an_unsupported_callable_default_at_its_original_span() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
def keep(payload: bytes = b"x") -> bytes:
  return payload

def main() -> int:
  keep()
  return 1
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("bytes default must refuse directly, got {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let default_start = source
        .find("b\"x\"")
        .ok_or("default fixture must contain bytes literal")?;
    let span = error.primary_span().ok_or("default refusal must retain source span")?;
    assert_eq!(span.start, default_start);
    assert_eq!(span.end, default_start + "b\"x\"".len());
    assert!(error.to_string().contains("bytes literal"));
    Ok(())
}

/// A missing direct-entry argument refuses at the original declaration body, whose call is the selected entrypoint.
#[test]
fn replacement_refuses_a_missing_required_callable_argument_at_the_declaration_body_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def needs(value: int) -> int:
  return value
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "needs", &[]) {
        Ok(execution) => {
            return Err(format!(
                "the required callable parameter must refuse when omitted, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let expected_start = source.find("def needs").ok_or("fixture must contain the declaration")?;
    let span = error
        .primary_span()
        .ok_or("missing parameter refusal must retain a source span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error
            .to_string()
            .contains("missing required callable parameter `value`"),
        "unexpected missing-parameter refusal: {error}"
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
        incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    receipt.verify_identity()?;
    assert_eq!(receipt.executed_backend, BackendKind::Replacement);
    assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
    Ok(())
}

/// Refuse an async declaration even without an explicit suspension; it creates a task rather than a scalar value.
#[test]
fn replacement_refuses_an_async_body_without_an_await_at_its_declaration_span() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
import std.async

async def main() -> int:
  return 42
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("async body executed synchronously as {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("async def main")
        .ok_or("async fixture must contain its declaration")?;
    let span = error
        .primary_span()
        .ok_or("async body refusal must retain its declaration span")?;
    assert_eq!(span.start, expected_start);
    assert!(
        error.to_string().contains("async task body"),
        "async declarations must refuse as task bodies before #1155: {error}"
    );
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

/// Refuse lexical shadowing until Body IR carries an explicit binding-equivalence fact direct frames can honor.
#[test]
fn replacement_refuses_lexically_shadowed_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  x = 1
  if true:
    let x = 2
  return x
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("shadowing must refuse directly, got {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .find("let x = 2")
        .ok_or("shadowing fixture must contain the inner binding")?;
    let span = error
        .primary_span()
        .ok_or("shadowing refusal must retain the inner binding span")?;
    assert_eq!(span.start, expected_start);
    assert!(error.to_string().contains("repeated user binding"));
    Ok(())
}

/// Refuse same-scope reassignment until Body IR carries an explicit binding-equivalence fact.
#[test]
fn replacement_refuses_reassignment_with_a_repeated_user_binding_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  mut x = 1
  x = 2
  return x
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!("reassignment must refuse directly, got {:?}", execution.value).into());
        }
        Err(error) => error,
    };
    let expected_start = source
        .rfind("x = 2")
        .ok_or("reassignment fixture must contain the second binding")?;
    let span = error
        .primary_span()
        .ok_or("reassignment refusal must retain the second binding span")?;
    assert_eq!(span.start, expected_start);
    assert!(error.to_string().contains("repeated user binding"));
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

/// Run the explicit replacement CLI path without allowing a legacy fallback.
#[test]
fn replacement_cli_executes_typed_body_ir_and_persists_a_replacement_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  left = 40
  right = 2
  if left > right:
    return left + right
  return right
"#,
    )?;

    let output = Command::new(incan_binary())
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
        output.status.success(),
        "replacement build must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("replacement backend executed `main`: 42"),
        "unexpected replacement output: {stdout}"
    );
    assert!(
        !stdout.contains("Generated Rust project"),
        "replacement path must not enter Rust generation: {stdout}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create a legacy generated-project directory"
    );

    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(receipt_path)?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["selection"]["selected_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    assert!(
        receipt["replacement_execution"].is_null(),
        "persisted #986 receipts must remain receipt-only; execution evidence belongs to the report payload"
    );
    assert!(
        receipt["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:"))
    );
    Ok(())
}

/// Execute a typed empty scalar-pair list directly and publish only replacement receipt evidence.
#[test]
fn replacement_cli_executes_typed_empty_scalar_tuple_list_with_a_replacement_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  values: list[tuple[int, int]] = []
  for a, b in values:
    return a + b
  return 42
"#,
    )?;

    let output = Command::new(incan_binary())
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
        output.status.success(),
        "typed empty scalar-pair list must execute directly. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("replacement backend executed `main`: 42"),
        "unexpected typed empty pair-list output: {stdout}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct replacement execution must not create a legacy generated-project directory"
    );

    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    let receipt: serde_json::Value = serde_json::from_str(&fs::read_to_string(receipt_path)?)?;
    assert_eq!(receipt["executed_backend"], "replacement");
    assert_eq!(receipt["fallback_outcome"], serde_json::json!("not_needed"));
    Ok(())
}

/// Emit stable direct-execution evidence in the distinct, non-Oven replacement JSON report.
#[test]
fn replacement_cli_json_report_projects_canonical_execution_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = Command::new(incan_binary())
        .args([
            "build",
            entrypoint.to_string_lossy().as_ref(),
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
            "--report",
            "json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "replacement JSON report must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["schema_version"], "incan.replacement_execution.v0");
    assert_eq!(report["status"], "success");
    assert_eq!(report["mode"], "executable");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(
        report["replacement_execution"]["output_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "direct report must project the receipt-bound output identity: {report}"
    );
    assert!(
        report["replacement_execution"]["ownership_reads"].is_array()
            && report["replacement_execution"]["runtime_requirements"].is_array(),
        "direct report must project canonical ownership and runtime evidence: {report}"
    );
    assert!(
        report.get("generated").is_none() && report.get("oven").is_none(),
        "direct replacement report must not invent generated-Rust or Oven evidence: {report}"
    );
    Ok(())
}

/// Reject the unimplemented legacy fallback spelling before it can create an artifact or receipt.
#[test]
fn replacement_cli_rejects_legacy_fallback_without_artifacts_or_receipts() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = Command::new(incan_binary())
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
  values = {"first": 1}
  return 0
"#,
    )?;

    let output = Command::new(incan_binary())
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
  values = {"first": 1}
  return 0
"#
    .find("{\"first\": 1}")
    .ok_or("aggregate fixture must contain its dict literal")?;
    let expected_end = expected_start + "{\"first\": 1}".len();
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

/// A selected entrypoint must not publish a receipt when an invoked sibling fails the same direct profile gate.
#[test]
fn replacement_cli_refuses_an_unsupported_sibling_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def unsupported_sibling() -> int:
  values: list[int] = []
  for value in values:
    return value
  return 42

def main() -> int:
  return unsupported_sibling()
"#,
    )?;

    let output = Command::new(incan_binary())
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
        combined.contains("builtin collection iteration destination")
            && combined.contains("original Incan source span"),
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

    let output = Command::new(incan_binary())
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
        combined.contains("bytes literal"),
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

    let output = Command::new(incan_binary())
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

/// Apply the same aggregate type gate to a partial's synthesized deferred closure frame.
#[test]
fn replacement_refuses_typed_empty_non_structural_default_inside_a_partial_closure()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def keep(prefix: int, items: list[float] = []) -> int:
  return prefix

def main() -> int:
  deferred = partial keep(prefix=1)
  return deferred()
"#;
    let module = lower_typed_body_ir(source)?;
    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a partial closure must not execute a typed-empty non-structural default, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let aggregate_start = source.find("[]").ok_or("fixture must contain the empty aggregate")?;
    let span = error
        .primary_span()
        .ok_or("a partial-closure aggregate refusal must retain the default source span")?;
    assert_eq!(span.start, aggregate_start);
    assert_eq!(span.end, aggregate_start + "[]".len());
    assert!(
        error
            .to_string()
            .contains("structural aggregate destination has unsupported Body-IR type `List[float]`"),
        "the deferred closure refusal must name the unavailable aggregate type: {error}"
    );
    Ok(())
}

/// Refuse a typed empty scalar list before replacement can publish a success receipt.
#[test]
fn replacement_cli_refuses_typed_empty_scalar_list_without_a_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(
        &entrypoint,
        r#"def main() -> int:
  values: list[int] = []
  for value in values:
    return value
  return 42
"#,
    )?;

    let output = Command::new(incan_binary())
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
        "typed empty scalar list must refuse instead of executing directly"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("builtin collection iteration destination"),
        "refusal must name the unsupported scalar-list iteration profile: {combined}"
    );
    assert!(
        combined.contains("original Incan source span"),
        "refusal must retain source authority: {combined}"
    );
    assert!(
        !temporary.path().join("target/incan").exists()
            && !temporary.path().join(".incan/backend/receipt.json").exists(),
        "typed empty scalar-list refusal must not create legacy output or a replacement receipt"
    );
    Ok(())
}

/// Persist a requested shadow comparison as explicitly unavailable when no legacy runtime comparator exists.
#[test]
fn replacement_cli_shadow_receipt_is_explicitly_non_green() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let entrypoint = temporary.path().join("main.incn");
    fs::write(&entrypoint, "def main() -> int:\n  return 42\n")?;

    let output = Command::new(incan_binary())
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
    assert_eq!(reason, incan::backend::shadow::PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON);

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

    let output = Command::new(incan_binary())
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
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("replacement backend executed `main`: 42"),
        "unexpected shadowed replacement output: {stdout}"
    );

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
            "rust-module",
            "rust.module(\"incan_stdlib::testing\")\n\ndef main() -> int:\n  return 42\n",
            "Rust interop `rust.module` directive",
        ),
    ];
    for (name, source, expected_boundary) in cases {
        let temporary = tempfile::tempdir()?;
        let entrypoint = temporary.path().join(format!("{name}.incn"));
        fs::write(&entrypoint, source)?;
        let output = Command::new(incan_binary())
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
            combined.contains(&format!("primary Incan source location: {}:0..", entrypoint.display())),
            "refusal must retain the first unsupported source span: {combined}"
        );
        assert!(
            !temporary.path().join("target/incan").exists()
                && !temporary.path().join(".incan/backend/receipt.json").exists(),
            "{name} refusal must not create legacy output or a replacement receipt"
        );
    }
    Ok(())
}

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

    let output = Command::new(incan_binary())
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
        output.status.success(),
        "an admitted direct sibling call must execute. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("replacement backend executed `main`: 143"),
        "the receipt-producing direct path must observe the callable result"
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
        "successful direct execution must produce a verifiable replacement receipt: {receipt}"
    );
    assert!(
        !temporary.path().join("target/incan").exists(),
        "direct callable execution must not create a legacy generated-project directory"
    );
    Ok(())
}
