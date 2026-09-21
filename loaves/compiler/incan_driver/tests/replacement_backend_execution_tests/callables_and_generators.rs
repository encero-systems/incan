//! Callables and generators: named calls dispatched by canonical target, same-module overloads and range
//! declarations, stored closures, partials and callable defaults, lazy generator expressions, functions and
//! adapters, and the refusals for foreign, duplicate, noncanonical or idless call identities.

use super::*;

/// Find the canonical Generator.collect call in the fixture's `main` body for malformed-IR assertions.
fn canonical_collect_target_mut(module: &mut BodyIrModule) -> Option<&mut incan_semantics_core::body_ir::MethodTarget> {
    module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")?
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Call {
                callee: incan_semantics_core::body_ir::Callee::Method(target),
                ..
            } if target
                .canonical
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == "Generator.collect") =>
            {
                Some(target)
            }
            _ => None,
        })
}

/// Find one identity-selected named call in a body for malformed-IR and display-spelling assertions.
fn canonical_named_target_mut<'module>(
    module: &'module mut BodyIrModule,
    body_name: &str,
    declaration_name: &str,
) -> Option<&'module mut incan_semantics_core::body_ir::NamedCallableTarget> {
    module
        .bodies
        .iter_mut()
        .find(|body| body.name == body_name)?
        .block
        .stmts
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Call {
                callee:
                    incan_semantics_core::body_ir::Callee::Function(
                        incan_semantics_core::body_ir::CallableTarget::Named(target),
                    ),
                ..
            } if target
                .canonical
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == declaration_name) =>
            {
                Some(target)
            }
            _ => None,
        })
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

/// A checked method's retained canonical target, rather than its display spelling, is the replacement dispatch
/// authority. Removing that authority must fail closed before the deferred generator can be consumed.
#[test]
fn replacement_dispatches_generator_methods_by_canonical_target_and_refuses_absent_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> int:
  values = (value * 10 for value in range(1, 5) if value > 2).collect()
  return values[0] + values[1]
"#;
    let mut renamed = lower_typed_body_ir(source)?;
    canonical_collect_target_mut(&mut renamed)
        .ok_or("fixture must retain the canonical Generator.collect target")?
        .name = "not_collect".to_string();
    let execution = execute_free_function(&renamed, "main", &[])?;
    assert_eq!(execution.value, ReplacementValue::Int(70));

    let mut missing = lower_typed_body_ir(source)?;
    canonical_collect_target_mut(&mut missing)
        .ok_or("fixture must retain the canonical Generator.collect target")?
        .canonical = None;
    let error = match execute_free_function(&missing, "main", &[]) {
        Ok(_) => return Err("a source method without its canonical target must fail closed".into()),
        Err(error) => error,
    };
    assert!(
        error.primary_span().is_some() && error.to_string().contains("method `collect`"),
        "the missing method authority must refuse at the original call: {error}"
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
    // The stand-in is `unsafe:`, refused by design rather than pending work. This test previously used
    // scalar-list iteration, which stopped being unsupported once that gate was widened — the sixth time a
    // stand-in chosen from the "not yet" pile has decayed underneath a test that only needed *some* refusal.
    let source = r#"
def unsupported_sibling() -> int:
  unsafe:
    pass
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
        .find("unsafe:")
        .ok_or("fixture must contain the by-design refusal")?;
    let span = error
        .primary_span()
        .ok_or("sibling refusal must retain a source span")?;
    assert_eq!(span.start, expected_start);
    assert!(error.to_string().contains("`unsafe:` acknowledgement region"));
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

/// Refuse a coherent-looking named call/body pair whose declaration identity was corrupted to another module.
#[test]
fn replacement_refuses_a_foreign_named_call_identity_at_its_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def helper() -> int:
  return 42

def main() -> int:
  return helper()
"#;
    let mut module = lower_typed_body_ir(source)?;
    let foreign_direct_call_id = CompilerNodeId::declaration_span("foreign", 0, 23);
    let helper = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "helper")
        .ok_or("fixture must lower the helper body")?;
    helper.direct_call_id = foreign_direct_call_id.clone();
    let main = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("fixture must lower the main body")?;
    let target =
        main.block
            .stmts
            .iter_mut()
            .find_map(|statement| match &mut statement.kind {
                StatementKind::Call {
                    callee:
                        incan_semantics_core::body_ir::Callee::Function(
                            incan_semantics_core::body_ir::CallableTarget::Named(target),
                        ),
                    ..
                } if target.name == "helper" => Some(target),
                _ => None,
            })
            .ok_or("fixture must lower the helper call as a named Body-IR target")?;
    target.direct_call_id = Some(foreign_direct_call_id);

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a foreign named-call identity must refuse instead of dispatching, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let call_start = source
        .find("return helper()")
        .map(|start| start + "return ".len())
        .ok_or("fixture must contain the rejected helper call")?;
    let span = error
        .primary_span()
        .ok_or("foreign named-call identity refusal must retain a source span")?;
    assert_eq!(span.start, call_start);
    assert_eq!(span.end, call_start + "helper()".len());
    assert!(
        error
            .to_string()
            .contains("named callable declaration identity is not scoped to this Body-IR module"),
        "the refusal must name the foreign declaration identity: {error}"
    );
    Ok(())
}

/// Refuse duplicate same-module named-call identities instead of selecting the first malformed body as a decoy.
#[test]
fn replacement_refuses_duplicate_named_call_identities_at_the_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def helper() -> int:
  return 42

def main() -> int:
  return helper()
"#;
    let mut module = lower_typed_body_ir(source)?;
    let decoy = module
        .bodies
        .iter()
        .find(|body| body.name == "helper")
        .cloned()
        .ok_or("fixture must lower the helper body")?;
    module.bodies.insert(0, decoy);

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "duplicate named-call identities must refuse instead of selecting a body, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let call_start = source
        .find("return helper()")
        .map(|start| start + "return ".len())
        .ok_or("fixture must contain the rejected helper call")?;
    let span = error
        .primary_span()
        .ok_or("duplicate named-call identity refusal must retain a source span")?;
    assert_eq!(span.start, call_start);
    assert_eq!(span.end, call_start + "helper()".len());
    assert!(
        error
            .to_string()
            .contains("declaration identity selects multiple Body-IR bodies"),
        "the refusal must identify the duplicate direct-call target: {error}"
    );
    Ok(())
}

/// Refuse a same-module identity whose retained helper body does not match its own declaration span.
#[test]
fn replacement_refuses_a_noncanonical_same_module_named_call_identity_at_the_original_source_span()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def helper() -> int:
  return 42

def main() -> int:
  return helper()
"#;
    let mut module = lower_typed_body_ir(source)?;
    let noncanonical_id = CompilerNodeId::declaration_span(module.module_id.path(), 0, 0);
    let helper = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "helper")
        .ok_or("fixture must lower the helper body")?;
    helper.direct_call_id = noncanonical_id.clone();
    let main = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("fixture must lower the main body")?;
    let target =
        main.block
            .stmts
            .iter_mut()
            .find_map(|statement| match &mut statement.kind {
                StatementKind::Call {
                    callee:
                        incan_semantics_core::body_ir::Callee::Function(
                            incan_semantics_core::body_ir::CallableTarget::Named(target),
                        ),
                    ..
                } if target.name == "helper" => Some(target),
                _ => None,
            })
            .ok_or("fixture must lower the helper call as a named Body-IR target")?;
    target.direct_call_id = Some(noncanonical_id);

    let error = match execute_free_function(&module, "main", &[]) {
        Ok(execution) => {
            return Err(format!(
                "a noncanonical same-module identity must refuse instead of dispatching, got {:?}",
                execution.value
            )
            .into());
        }
        Err(error) => error,
    };
    let call_start = source
        .find("return helper()")
        .map(|start| start + "return ".len())
        .ok_or("fixture must contain the rejected helper call")?;
    let span = error
        .primary_span()
        .ok_or("noncanonical named-call identity refusal must retain a source span")?;
    assert_eq!(span.start, call_start);
    assert_eq!(span.end, call_start + "helper()".len());
    assert!(
        error
            .to_string()
            .contains("canonical target disagrees with its physical Body-IR declaration"),
        "the refusal must identify the mismatched semantic and physical declaration identities: {error}"
    );
    Ok(())
}

/// Named-call dispatch follows the retained declaration identity, never the source/display spelling. Removing the
/// canonical half of that handoff must refuse even when the physical same-module id remains present.
#[test]
fn replacement_dispatches_named_calls_by_canonical_target_and_refuses_absent_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def helper() -> int:
  return 42

def main() -> int:
  return helper()
"#;
    let mut renamed = lower_typed_body_ir(source)?;
    canonical_named_target_mut(&mut renamed, "main", "helper")
        .ok_or("fixture must lower the canonical helper call")?
        .name = "not_helper".to_string();
    assert_eq!(
        execute_free_function(&renamed, "main", &[])?.value,
        ReplacementValue::Int(42),
        "the retained canonical target must select helper independently of its display spelling"
    );

    let mut missing = lower_typed_body_ir(source)?;
    canonical_named_target_mut(&mut missing, "main", "helper")
        .ok_or("fixture must lower the canonical helper call")?
        .canonical = None;
    let error = match execute_free_function(&missing, "main", &[]) {
        Ok(_) => return Err("a physical direct-call id without its canonical target must fail closed".into()),
        Err(error) => error,
    };
    assert!(
        error.primary_span().is_some() && error.to_string().contains("without a canonical declaration target"),
        "the missing named-call authority must refuse at the original call: {error}"
    );
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
        .ok_or("default fixture must contain a byte-string literal")?;
    let span = error.primary_span().ok_or("default refusal must retain source span")?;
    assert_eq!(span.start, default_start);
    assert_eq!(span.end, default_start + "b\"x\"".len());
    // The refusal moved but did not weaken. Before #1165 a byte-string literal had no `bir::Constant`, so
    // *lowering* refused it; now it lowers and the executor refuses the value, because this profile carries no
    // `bytes` runtime representation. The property under test is unchanged -- refused at the default's own span,
    // with no receipt -- so only the construct's name in the message moves.
    assert!(error.to_string().contains("byte-string literal"));
    Ok(())
}

/// A materialized range aggregate remains outside the direct runtime profile. Preflight must reject the aggregate
/// before evaluating either bound, so an observable bound cannot turn a refusal into a partial execution.
#[test]
fn replacement_refuses_a_range_aggregate_before_evaluating_its_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def bound() -> int:
  assert false
  return 4

def main() -> int:
  values = 0..bound()
  return 1
"#;
    let module = lower_typed_body_ir(source)?;
    let error = execute_free_function(&module, "main", &[])
        .err()
        .ok_or("a range aggregate must refuse before replacement execution")?;
    let range_start = source
        .find("0..bound()")
        .ok_or("fixture must contain a range aggregate")?;
    let span = error
        .primary_span()
        .ok_or("range aggregate refusal must retain its source span")?;

    assert!(error.to_string().contains("range aggregate"));
    assert_eq!(span.start, range_start);
    assert_eq!(span.end, range_start + "0..bound()".len());
    assert!(
        !error.to_string().contains("assertion failed"),
        "the range boundary must reject before it can execute the bound call: {error}"
    );
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
